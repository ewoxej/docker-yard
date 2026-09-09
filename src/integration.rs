use crate::ast::{EnvFile, EnvSource, EnvVariable, Project};
use crate::parse::{parse_config_file, parse_service_file, parse_env_file};
use crate::validate;
use crate::codegen::{generate_compose_yaml, generate_traefik_dynamic, generate_traefik_static};
use serde_yaml::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::collections::HashMap;

#[derive(Debug)]
pub struct BuildContext {
    pub project: Project,
    pub project_root: PathBuf,
    pub output_dir: PathBuf,
}

pub struct ProjectBuilder;

impl ProjectBuilder {
    pub fn build(config_path: &Path) -> Result<BuildContext, String> {
        let config_content = fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config: {}", e))?;

        let config = parse_config_file(&config_content)?;

        let project_root = config_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let containers_dir = project_root.join(&config.input.containers);
        let env_dir = project_root.join(&config.input.env);

        // Find and parse service files
        let mut services = Vec::new();
        if containers_dir.exists() {
            let mut entries: Vec<_> = fs::read_dir(&containers_dir)
                .map_err(|e| format!("Failed to read containers dir: {}", e))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("dyard"))
                .collect();
            // Sort for deterministic order
            entries.sort_by_key(|e| e.file_name());

            for entry in entries {
                let path = entry.path();
                let content = fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
                let mut parsed = parse_service_file(&content, filename)?;
                services.append(&mut parsed);
            }
        }

        // Find and parse env files
        let mut env_files = HashMap::new();
        if env_dir.exists() {
            for entry in fs::read_dir(&env_dir)
                .map_err(|e| format!("Failed to read env dir: {}", e))?
            {
                let entry = entry.map_err(|e| format!("Directory error: {}", e))?;
                let path = entry.path();

                if path.extension().and_then(|s| s.to_str()) == Some("env") {
                    let content = fs::read_to_string(&path)
                        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

                    let env_file = parse_env_file(&content)?;
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown.env")
                        .to_string();

                    env_files.insert(file_name, env_file);
                }
            }
        }

        let project = Project { config, services, env_files };

        validate(&project).map_err(|errors| {
            errors.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })?;

        // Derive output_dir from the compose output path (its parent directory)
        let compose_path = project_root.join(&project.config.output.compose);
        let output_dir = compose_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        Ok(BuildContext {
            output_dir,
            project,
            project_root,
        })
    }

    pub fn generate_outputs(ctx: &BuildContext, regenerate: bool) -> Result<(), String> {
        let output_dir = &ctx.output_dir;
        let project_root = &ctx.project_root;
        let network_dir = project_root.join(&ctx.project.config.input.network);

        // Paths from config
        let compose_path = project_root.join(&ctx.project.config.output.compose);
        let traefik_dir = project_root.join(&ctx.project.config.output.traefik);
        let conf_d_dir = traefik_dir.join("conf.d");
        // If output.env ends with ".env" (old format), treat it as the root .env path
        // and put service env files in a sibling "env/" folder.
        // Otherwise treat output.env as the env files folder.
        let configured_env = project_root.join(&ctx.project.config.output.env);
        let (env_output_dir, root_env_path) = if ctx.project.config.output.env.ends_with(".env") {
            let folder = configured_env.parent().unwrap_or(&configured_env).join("env");
            (folder, configured_env)
        } else {
            (configured_env, output_dir.join(".env"))
        };

        // --- Incremental regeneration check (per-file hashes) ---
        let cache_path = project_root.join(".dyard-cache");
        let current_hashes = Self::compute_file_hashes(ctx)?;

        if !regenerate && cache_path.exists() {
            if let Ok(cached) = fs::read_to_string(&cache_path) {
                let old_hashes: HashMap<String, u64> = cached.lines()
                    .filter_map(|line| {
                        let mut parts = line.splitn(2, '\t');
                        let path = parts.next()?.to_string();
                        let hash: u64 = parts.next()?.parse().ok()?;
                        Some((path, hash))
                    })
                    .collect();

                let mut changed: Vec<&str> = Vec::new();
                let mut added: Vec<&str> = Vec::new();
                for (path, hash) in &current_hashes {
                    match old_hashes.get(path.as_str()) {
                        Some(old) if old == hash => {}
                        Some(_) => changed.push(path),
                        None => added.push(path),
                    }
                }
                let removed: Vec<&str> = old_hashes.keys()
                    .filter(|p| !current_hashes.contains_key(p.as_str()))
                    .map(|s| s.as_str())
                    .collect();

                if changed.is_empty() && added.is_empty() && removed.is_empty() {
                    println!("✓ No changes detected, skipping regeneration (use --regenerate to force)");
                    return Ok(());
                }
                for f in &changed { println!("  ~ changed: {}", f); }
                for f in &added   { println!("  + added:   {}", f); }
                for f in &removed { println!("  - removed: {}", f); }
            }
        }

        // Create output directories
        fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output dir: {}", e))?;
        fs::create_dir_all(&traefik_dir)
            .map_err(|e| format!("Failed to create traefik dir: {}", e))?;
        fs::create_dir_all(&conf_d_dir)
            .map_err(|e| format!("Failed to create conf.d dir: {}", e))?;
        fs::create_dir_all(&env_output_dir)
            .map_err(|e| format!("Failed to create env output dir: {}", e))?;

        // --- Pre-process: resolve VarRefs and template strings ---
        let mut resolution_errors: Vec<String> = Vec::new();
        let resolved_services: Vec<_> = ctx.project.services.iter().map(|svc| {
            let mut svc = svc.clone();
            let loc = format!("{}:{}", svc.source_file, svc.source_line);

            // Resolve VarRef env sources → Inline
            svc.env_from = svc.env_from.into_iter().filter_map(|src| match src {
                EnvSource::VarRef { ref key, ref file, ref section } => {
                    match Self::env_file_lookup(&ctx.project.env_files, file) {
                        None => {
                            resolution_errors.push(format!(
                                "{}: env file '{}' not found (referenced in env from for '{}')",
                                loc, file, svc.name
                            ));
                            None
                        }
                        Some(ef) => {
                            let vars = Self::resolve_env_section(ef, section.as_deref(), 0);
                            match vars.get(key.as_str()) {
                                None => {
                                    let sec = section.as_deref().unwrap_or("root");
                                    resolution_errors.push(format!(
                                        "{}: variable '{}' not found in section '{}' of '{}' (referenced in env from for '{}')",
                                        loc, key, sec, file, svc.name
                                    ));
                                    None
                                }
                                Some(var) => Some(EnvSource::Inline {
                                    key: key.clone(),
                                    value: var.value.clone(),
                                    secret: var.secret,
                                }),
                            }
                        }
                    }
                }
                other => Some(other),
            }).collect();

            // Inject Inline { secret: true } sentinels for any secrets found in File sources.
            // This ensures compose.rs sees has_secret_inline=true and adds the .secrets.env
            // env_file reference, even though the actual value comes from the File source.
            let file_secret_inlines: Vec<EnvSource> = svc.env_from.iter()
                .filter_map(|src| if let EnvSource::File { file, section } = src {
                    Some((file.clone(), section.clone()))
                } else { None })
                .flat_map(|(file, section)| {
                    Self::env_file_lookup(&ctx.project.env_files, &file)
                        .map(|ef| {
                            let vars = Self::resolve_env_section(ef, section.as_deref(), 0);
                            vars.into_iter()
                                .filter(|(_, v)| v.secret)
                                .map(|(k, v)| EnvSource::Inline { key: k, value: v.value, secret: true })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();
            svc.env_from.extend(file_secret_inlines);

            // Resolve ${KEY@section@file} in volume host paths
            svc.volumes = svc.volumes.into_iter().map(|mut v| {
                let resolved = Self::resolve_template_string(
                    &v.host_path, &ctx.project.env_files, &ctx.project.config.variables,
                );
                Self::check_unresolved(&resolved, &format!("{} volume '{}'", loc, v.host_path), &mut resolution_errors);
                v.host_path = resolved;
                v
            }).collect();

            // Resolve ${KEY@section@file} in raw values (strings only).
            // Secrets are kept as ${KEY} references so they are not inlined in compose.
            let raw_keys: Vec<String> = svc.raw.keys().cloned().collect();
            for k in raw_keys {
                if let Some(serde_yaml::Value::String(s)) = svc.raw.get(&k) {
                    let resolved = Self::resolve_template_string_compose(
                        s, &ctx.project.env_files, &ctx.project.config.variables,
                    );
                    Self::check_unresolved(&resolved, &format!("{} raw '{}'", loc, k), &mut resolution_errors);
                    svc.raw.insert(k, serde_yaml::Value::String(resolved));
                }
            }

            svc
        }).collect();

        if !resolution_errors.is_empty() {
            return Err(resolution_errors.join("\n"));
        }

        // --- Generate docker-compose.yml ---
        let compose_yaml = generate_compose_yaml(
            &resolved_services,
            &ctx.project.config.router_container,
        );
        fs::write(&compose_path, &compose_yaml)
            .map_err(|e| format!("Failed to write compose: {}", e))?;
        println!("✓ Generated {}", compose_path.display());

        // --- Generate Traefik dynamic config ---
        let domain = ctx.project.config.variables
            .get("DOMAIN")
            .and_then(|v| match v {
                crate::ast::ConfigValue::String(s) => Some(s.clone()),
                crate::ast::ConfigValue::Secret(_) => None,
            })
            .unwrap_or_else(|| "example.com".to_string());

        let traefik_dynamic = generate_traefik_dynamic(
            &resolved_services,
            &ctx.project.config.router_container,
            &domain,
            &ctx.project.config.traefik_defaults,
        );

        let traefik_http_path = conf_d_dir.join("http.yml");
        fs::write(&traefik_http_path, traefik_dynamic)
            .map_err(|e| format!("Failed to write traefik config: {}", e))?;
        println!("✓ Generated {}", traefik_http_path.display());

        let router_container = &ctx.project.config.router_container;

        // --- Copy non-router services' network-config to conf.d/ (with var substitution + dot-key expansion) ---
        for service in &resolved_services {
            if service.name == *router_container { continue; }
            if let Some(ref nc_path) = service.network_config {
                let src = project_root.join(nc_path);
                if src.exists() {
                    let filename = src.file_name()
                        .ok_or_else(|| format!("Invalid network-config path: {}", nc_path))?;
                    let dest = conf_d_dir.join(filename);
                    let raw = fs::read_to_string(&src)
                        .map_err(|e| format!("Failed to read network-config {}: {}", src.display(), e))?;
                    let processed = Self::process_network_yaml(
                        &raw, nc_path, &ctx.project.config.variables, &ctx.project.env_files,
                    )?;
                    fs::write(&dest, processed)
                        .map_err(|e| format!("Failed to write network-config {}: {}", dest.display(), e))?;
                    println!("✓ Copied network-config {} → {}", src.display(), dest.display());
                } else {
                    return Err(format!(
                        "network-config file not found: {} (referenced in service '{}')",
                        src.display(), service.name
                    ));
                }
            }
        }

        // --- Copy _*.yml template files from network/ to conf.d/ (with var substitution + dot-key expansion) ---
        if network_dir.exists() {
            for entry in fs::read_dir(&network_dir)
                .map_err(|e| format!("Failed to read network dir: {}", e))?
            {
                let entry = entry.map_err(|e| format!("Directory error: {}", e))?;
                let path = entry.path();
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if filename.starts_with('_') && filename.ends_with(".yml") {
                    let dest = conf_d_dir.join(filename);
                    let raw = fs::read_to_string(&path)
                        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
                    let processed = Self::process_network_yaml(
                        &raw, filename, &ctx.project.config.variables, &ctx.project.env_files,
                    )?;
                    fs::write(&dest, processed)
                        .map_err(|e| format!("Failed to write {}: {}", dest.display(), e))?;
                    println!("✓ Copied network template {} → {}", path.display(), dest.display());
                }
            }
        }

        // --- Generate Traefik static config ---
        // If the router has a network-config, process + deep-merge it as the base; otherwise use defaults.
        let router_service = resolved_services.iter()
            .find(|s| s.name == ctx.project.config.router_container)
            .ok_or_else(|| format!("Router container '{}' not found", ctx.project.config.router_container))?;

        let static_base: Option<Value> = router_service.network_config.as_ref().and_then(|nc_path| {
            let src = project_root.join(nc_path);
            let raw = fs::read_to_string(&src).ok()?;
            let processed = Self::process_network_yaml(
                &raw, nc_path, &ctx.project.config.variables, &ctx.project.env_files,
            ).ok()?;
            serde_yaml::from_str::<Value>(&processed).ok()
        });

        let traefik_static = generate_traefik_static(router_service, static_base);
        let traefik_path = traefik_dir.join("traefik.yml");
        fs::write(&traefik_path, traefik_static)
            .map_err(|e| format!("Failed to write traefik.yml: {}", e))?;
        println!("✓ Generated {}", traefik_path.display());

        // --- Generate root .env ---
        // Start with config.dyard variables
        let mut root_env: HashMap<String, String> = ctx.project.config.variables.iter()
            .map(|(k, v)| match v {
                crate::ast::ConfigValue::String(s) => (k.clone(), s.clone()),
                crate::ast::ConfigValue::Secret(s) => (k.clone(), s.clone()),
            })
            .collect();

        // Collect all non-secret vars from every env file referenced via `env from`.
        // We collect all sections (not just the referenced one) because volume paths
        // may reference vars from sibling sections of the same file.
        // Only vars that appear as ${VAR} (no @) in compose are actually written to root .env.
        let mut candidate_pool: HashMap<String, String> = HashMap::new();
        let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();
        for service in &resolved_services {
            for src in &service.env_from {
                if let EnvSource::File { file, .. } = src {
                    if file.is_empty() || !seen_files.insert(file.clone()) { continue; }
                    if let Some(ef) = Self::env_file_lookup(&ctx.project.env_files, file) {
                        // Root section
                        let root_vars = Self::resolve_env_section(ef, None, 0);
                        for (k, v) in root_vars {
                            if !v.secret { candidate_pool.entry(k).or_insert(v.value); }
                        }
                        // All named sections
                        for section_name in ef.sections.keys() {
                            let vars = Self::resolve_env_section(ef, Some(section_name), 0);
                            for (k, v) in vars {
                                if !v.secret { candidate_pool.entry(k).or_insert(v.value); }
                            }
                        }
                    }
                }
            }
        }

        // Collect secret vars from env files so they can be written to root .env
        // when referenced as ${KEY} in raw compose fields.
        let mut secret_pool: HashMap<String, String> = HashMap::new();
        let mut seen_files_s: std::collections::HashSet<String> = std::collections::HashSet::new();
        for service in &resolved_services {
            for src in &service.env_from {
                if let EnvSource::File { file, .. } = src {
                    if file.is_empty() || !seen_files_s.insert(file.clone()) { continue; }
                    if let Some(ef) = Self::env_file_lookup(&ctx.project.env_files, file) {
                        let root_vars = Self::resolve_env_section(ef, None, 0);
                        for (k, v) in root_vars {
                            if v.secret { secret_pool.entry(k).or_insert(v.value); }
                        }
                        for section_name in ef.sections.keys() {
                            let vars = Self::resolve_env_section(ef, Some(section_name), 0);
                            for (k, v) in vars {
                                if v.secret { secret_pool.entry(k).or_insert(v.value); }
                            }
                        }
                    }
                }
            }
        }

        // Build full flat map (config vars + candidates), then iteratively expand
        // ${VAR} refs within values (handles chained: A=${B}/x, B=${C}/y, C=val).
        // config.dyard vars (root_env) take priority over env file candidates.
        let mut flat: HashMap<String, String> = candidate_pool.clone();
        flat.extend(secret_pool.clone());
        flat.extend(root_env.clone());
        for _ in 0..10 {
            let snapshot = flat.clone();
            let mut changed = false;
            for val in flat.values_mut() {
                for (k, v) in &snapshot {
                    let placeholder = format!("${{{}}}", k);
                    if val.contains(&placeholder) {
                        *val = val.replace(&placeholder, v);
                        changed = true;
                    }
                }
            }
            if !changed { break; }
        }

        // Find all ${VAR} (no @) referenced in compose, add to root_env if in flat map
        let mut i = 0;
        while i < compose_yaml.len() {
            let Some(start) = compose_yaml[i..].find("${").map(|p| i + p) else { break; };
            let Some(end) = compose_yaml[start..].find('}').map(|p| start + p) else { break; };
            let expr = &compose_yaml[start + 2..end];
            if !expr.contains('@') && !root_env.contains_key(expr) {
                if let Some(val) = flat.get(expr) {
                    root_env.insert(expr.to_string(), val.clone());
                }
            }
            i = end + 1;
        }

        let mut root_env_lines: Vec<String> = root_env.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        root_env_lines.sort();
        fs::write(&root_env_path, root_env_lines.join("\n"))
            .map_err(|e| format!("Failed to write .env: {}", e))?;
        println!("✓ Generated {}", root_env_path.display());

        // --- Generate service env files in env/ ---
        for service in &resolved_services {
            // Collect all secret lines: from Inline VarRefs AND from File sources.
            let mut all_secret_lines: Vec<String> = service.env_from.iter()
                .filter_map(|src| match src {
                    EnvSource::Inline { key, value, secret: true } => Some(format!("{}={}", key, value)),
                    _ => None,
                })
                .collect();

            for source in &service.env_from {
                if let EnvSource::File { file, section } = source {
                    if file.is_empty() { continue; }

                    let filename = format!(
                        "{}.{}env",
                        service.name,
                        section.as_ref().map(|s| format!("{}.", s)).unwrap_or_default()
                    );
                    let env_path = env_output_dir.join(&filename);

                    let content = if let Some(ef) = Self::env_file_lookup(&ctx.project.env_files, file.as_str()) {
                        let vars = Self::resolve_env_section(ef, section.as_deref(), 0);
                        let mut lines: Vec<String> = Vec::new();
                        for (k, v) in &vars {
                            let mut resolved_value = Self::resolve_template_string(
                                &v.value, &ctx.project.env_files, &ctx.project.config.variables,
                            );
                            for (fk, fv) in &flat {
                                resolved_value = resolved_value.replace(&format!("${{{}}}", fk), fv);
                            }
                            let loc = format!("env file '{}' key '{}'", file, k);
                            let mut errs = Vec::new();
                            Self::check_unresolved(&resolved_value, &loc, &mut errs);
                            for e in errs { eprintln!("✗ {}", e); }

                            let entries: Vec<String> = if v.aliases.is_empty() {
                                vec![format!("{}={}", k, resolved_value)]
                            } else {
                                v.aliases.iter().map(|alias| format!("{}={}", alias, resolved_value)).collect()
                            };
                            if v.secret {
                                all_secret_lines.extend(entries);
                            } else {
                                lines.extend(entries);
                            }
                        }
                        lines.sort();
                        lines.join("\n")
                    } else {
                        String::new()
                    };

                    fs::write(&env_path, content)
                        .map_err(|e| format!("Failed to write {}: {}", env_path.display(), e))?;
                    println!("✓ Generated {}", env_path.display());
                }
            }

            if !all_secret_lines.is_empty() {
                let secrets_path = env_output_dir.join(format!("{}.secrets.env", service.name));
                all_secret_lines.sort();
                all_secret_lines.dedup();
                fs::write(&secrets_path, all_secret_lines.join("\n"))
                    .map_err(|e| format!("Failed to write {}: {}", secrets_path.display(), e))?;
                println!("✓ Generated {}", secrets_path.display());
            }
        }

        // --- Save per-file hashes to cache ---
        let mut cache_lines: Vec<String> = current_hashes.iter()
            .map(|(path, hash)| format!("{}\t{}", path, hash))
            .collect();
        cache_lines.sort();
        fs::write(&cache_path, cache_lines.join("\n"))
            .map_err(|e| format!("Failed to write cache: {}", e))?;

        println!("\n✓ All configs generated successfully!");
        Ok(())
    }

    /// Resolve section variables with extends chain support (same-file, up to 10 levels).
    /// Child variables override parent variables. Aliases are preserved.
    /// For "KEY as ALIAS" entries (empty value), fills value from parent.
    fn resolve_env_section(
        ef: &EnvFile,
        section_name: Option<&str>,
        depth: usize,
    ) -> HashMap<String, EnvVariable> {
        if depth > 10 { return HashMap::new(); }

        let section = match section_name {
            Some(name) => ef.sections.get(name),
            None => Some(&ef.root_section),
        };
        let Some(section) = section else {
            return HashMap::new();
        };

        // Start with parent variables (lower priority)
        let mut merged: HashMap<String, EnvVariable> = HashMap::new();
        for parent_src in &section.extends {
            if let EnvSource::File { file, section: parent_sec } = parent_src {
                if file.is_empty() {
                    let parent_vars = Self::resolve_env_section(ef, parent_sec.as_deref(), depth + 1);
                    merged.extend(parent_vars);
                }
            }
        }

        // Override with this section's own variables (child has higher priority)
        for (k, v) in &section.variables {
            if v.value.is_empty() && !v.aliases.is_empty() {
                // Syntax: "SOURCE_KEY as OUTPUT_KEY"
                // k         = source key (exists in parent section)
                // aliases   = [output name (what the service sees)]
                let output_key = &v.aliases[0];
                if let Some(parent_var) = merged.remove(k.as_str()) {
                    // Remove the original source key, expose value under the new output key
                    merged.insert(output_key.clone(), EnvVariable {
                        value: parent_var.value.clone(),
                        secret: parent_var.secret,
                        aliases: vec![],
                    });
                }
                // If source key not in parent, skip
            } else {
                merged.insert(k.clone(), v.clone());
            }
        }

        merged
    }

    /// Report any `${KEY@...}` patterns left unresolved in `s` as errors.
    fn check_unresolved(s: &str, location: &str, errors: &mut Vec<String>) {
        let mut i = 0;
        while i < s.len() {
            let Some(start) = s[i..].find("${").map(|p| i + p) else { break; };
            let Some(end) = s[start..].find('}').map(|p| start + p) else { break; };
            let expr = &s[start + 2..end];
            if expr.contains('@') {
                errors.push(format!("{}: unresolved variable '${{{}}}'", location, expr));
            }
            i = end + 1;
        }
    }

    /// Look up an env file by name, trying both "name" and "name.env".
    fn env_file_lookup<'a>(env_files: &'a HashMap<String, EnvFile>, file: &str) -> Option<&'a EnvFile> {
        env_files.get(file).or_else(|| {
            if !file.ends_with(".env") {
                env_files.get(&format!("{}.env", file))
            } else {
                None
            }
        })
    }

    /// Expand `${KEY@section@file}` references in a string value.
    /// Falls back to `${KEY}` if the file/section is not loaded.
    fn resolve_template_string(
        s: &str,
        env_files: &HashMap<String, EnvFile>,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
    ) -> String {
        let mut result = s.to_string();

        // Replace ${KEY@section@file} or ${KEY@file} patterns (must have ≥1 '@')
        let mut i = 0;
        while i < result.len() {
            let Some(start) = result[i..].find("${").map(|p| i + p) else { break; };
            let Some(end) = result[start..].find('}').map(|p| start + p) else { break; };
            let expr = &result[start + 2..end];

            let at_count = expr.chars().filter(|&c| c == '@').count();
            let replacement = if at_count >= 2 {
                let last_at = expr.rfind('@').unwrap();
                let second_at = expr[..last_at].rfind('@').unwrap();
                let key = expr[..second_at].trim();
                let section = expr[second_at + 1..last_at].trim();
                let file = expr[last_at + 1..].trim();

                if let Some(ef) = Self::env_file_lookup(env_files, file) {
                    let vars = Self::resolve_env_section(ef, if section.is_empty() { None } else { Some(section) }, 0);
                    vars.get(key).map(|v| v.value.clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(val) = replacement {
                result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
                // Don't advance i — re-check from start in case replacement contains ${...}
            } else {
                i = end + 1;
            }
        }

        // Expand remaining ${VAR} from config variables
        for (k, v) in config_vars {
            let placeholder = format!("${{{}}}", k);
            let val = match v {
                crate::ast::ConfigValue::String(s) | crate::ast::ConfigValue::Secret(s) => s.clone(),
            };
            result = result.replace(&placeholder, &val);
        }

        result
    }

    /// Like `resolve_template_string` but replaces secret `${KEY@section@file}` with
    /// `${KEY}` so secrets are not inlined in compose — Docker reads them from .env.
    fn resolve_template_string_compose(
        s: &str,
        env_files: &HashMap<String, EnvFile>,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
    ) -> String {
        let mut result = s.to_string();
        let mut i = 0;
        while i < result.len() {
            let Some(start) = result[i..].find("${").map(|p| i + p) else { break; };
            let Some(end) = result[start..].find('}').map(|p| start + p) else { break; };
            let expr = result[start + 2..end].to_string();
            let at_count = expr.chars().filter(|&c| c == '@').count();
            let replacement: Option<String> = if at_count >= 2 {
                let last_at = expr.rfind('@').unwrap();
                let second_at = expr[..last_at].rfind('@').unwrap();
                let key = expr[..second_at].trim().to_string();
                let section = expr[second_at + 1..last_at].trim();
                let file = expr[last_at + 1..].trim();
                Self::env_file_lookup(env_files, file).and_then(|ef| {
                    let vars = Self::resolve_env_section(ef, if section.is_empty() { None } else { Some(section) }, 0);
                    vars.get(key.as_str()).map(|v| {
                        if v.secret { format!("${{{}}}", key) } else { v.value.clone() }
                    })
                })
            } else {
                None
            };
            if let Some(val) = replacement {
                let is_ref = val.starts_with("${") && val.ends_with('}');
                result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
                i = start + if is_ref { val.len() } else { 0 };
            } else {
                i = end + 1;
            }
        }
        for (k, v) in config_vars {
            let placeholder = format!("${{{}}}", k);
            if let crate::ast::ConfigValue::String(val) = v {
                result = result.replace(&placeholder, val);
            }
        }
        result
    }

    /// Process a network YAML file:
    /// 1. Resolve `${VAR}` and `${KEY@section@file}` references (non-secrets only).
    /// 2. Temporarily replace `{{ Go templates }}` with placeholders so the file
    ///    can be parsed as standard YAML.
    /// 3. Parse YAML, expand `key.sub.leaf` dot-notation keys into nested mappings.
    /// 4. Serialize back and restore Go-template placeholders.
    fn process_network_yaml(
        content: &str,
        file_hint: &str,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
        env_files: &HashMap<String, EnvFile>,
    ) -> Result<String, String> {
        // Step 1: expand bare `${VAR@section@file}` lines into YAML list items.
        //   trustedIPs:
        //     ${CLOUDFLARE_IPS@traefik@common.env}   ← spliced as `- ip` entries
        // Must run before inline substitution so the bare-line detection still works.
        let text = Self::splice_list_varrefs(content, env_files, config_vars);

        // Step 2: resolve inline ${VAR} and ${KEY@section@file} references
        let text = Self::resolve_template_string_nonsecret(&text, env_files, config_vars);

        // Step 3: extract {{ Go templates }} so they don't confuse the YAML parser
        let (yaml_text, go_templates) = Self::extract_go_templates(&text);

        // Step 4: parse YAML and expand dot-notation keys
        let parsed: Value = serde_yaml::from_str(&yaml_text)
            .map_err(|e| format!("{}: failed to parse YAML: {}", file_hint, e))?;
        let expanded = Self::expand_dot_keys(parsed);

        // Step 5: serialize and restore Go templates
        let serialized = serde_yaml::to_string(&expanded)
            .map_err(|e| format!("{}: failed to serialize YAML: {}", file_hint, e))?;
        Ok(Self::restore_go_templates(serialized, &go_templates))
    }

    /// Expand lines whose sole content is a `${VAR@section@file}` reference into
    /// YAML list items. The variable value is split on commas (or newlines) and
    /// each element is emitted as `<indent>- <item>`. Secrets are silently skipped.
    fn splice_list_varrefs(
        text: &str,
        env_files: &HashMap<String, EnvFile>,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
    ) -> String {
        let mut result = String::with_capacity(text.len());
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("${") && trimmed.ends_with('}') {
                let inner = &trimmed[2..trimmed.len() - 1];
                if inner.contains('@') {
                    if let Some(val) = Self::resolve_varref_value(inner, env_files, config_vars) {
                        let indent = line.len() - line.trim_start().len();
                        let pad = " ".repeat(indent);
                        for item in Self::split_list_value(&val) {
                            result.push_str(&pad);
                            result.push_str("- ");
                            result.push_str(&item);
                            result.push('\n');
                        }
                        continue;
                    }
                }
            }
            result.push_str(line);
            result.push('\n');
        }
        result
    }

    /// Split a list variable value into individual items.
    /// Priority: comma → newline → single item.
    /// Strips leading `- ` YAML list markers (so values can be stored in either
    /// comma-separated or YAML-block format).
    fn split_list_value(val: &str) -> Vec<String> {
        let val = val.trim();
        if val.is_empty() { return vec![]; }

        // Try YAML parse first — handles "[item1, item2, ...]" inline sequence format
        if let Ok(Value::Sequence(seq)) = serde_yaml::from_str::<Value>(val) {
            return seq.into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect();
        }

        // Fall back: newline → single item
        // Strip leading "- " YAML block markers so both formats are accepted
        let raw: Vec<&str> = if val.contains('\n') {
            val.split('\n').collect()
        } else {
            vec![val]
        };

        raw.into_iter()
            .map(|s| s.trim().trim_start_matches('-').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Resolve a single `KEY@section@file` or `KEY@file` varref expression to its
    /// string value. Returns None if the variable is secret or not found.
    fn resolve_varref_value(
        inner: &str,
        env_files: &HashMap<String, EnvFile>,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
    ) -> Option<String> {
        let at_count = inner.chars().filter(|&c| c == '@').count();
        if at_count >= 2 {
            let last_at = inner.rfind('@')?;
            let second_at = inner[..last_at].rfind('@')?;
            let key = inner[..second_at].trim();
            let section = inner[second_at + 1..last_at].trim();
            let file = inner[last_at + 1..].trim();
            let vars = Self::resolve_env_section(Self::env_file_lookup(env_files, file)?, if section.is_empty() { None } else { Some(section) }, 0);
            let var = vars.get(key)?;
            if var.secret { return None; }
            Some(var.value.clone())
        } else {
            let at = inner.find('@')?;
            let key = inner[..at].trim();
            let file = inner[at + 1..].trim();
            if file.is_empty() {
                match config_vars.get(key)? {
                    crate::ast::ConfigValue::String(s) => Some(s.clone()),
                    crate::ast::ConfigValue::Secret(_) => None,
                }
            } else {
                let vars = Self::resolve_env_section(Self::env_file_lookup(env_files, file)?, None, 0);
                let var = vars.get(key)?;
                if var.secret { return None; }
                Some(var.value.clone())
            }
        }
    }

    /// Like `resolve_template_string` but skips secret variables so they are not
    /// written in plaintext into network config files.
    fn resolve_template_string_nonsecret(
        s: &str,
        env_files: &HashMap<String, EnvFile>,
        config_vars: &HashMap<String, crate::ast::ConfigValue>,
    ) -> String {
        // First resolve cross-file ${KEY@section@file} refs (same logic as the full resolver)
        let mut result = s.to_string();
        let mut i = 0;
        while i < result.len() {
            let Some(start) = result[i..].find("${").map(|p| i + p) else { break };
            let Some(end) = result[start..].find('}').map(|p| start + p) else { break };
            let expr = &result[start + 2..end];
            let at_count = expr.chars().filter(|&c| c == '@').count();
            let replacement = if at_count >= 2 {
                let last_at = expr.rfind('@').unwrap();
                let second_at = expr[..last_at].rfind('@').unwrap();
                let key = expr[..second_at].trim();
                let section = expr[second_at + 1..last_at].trim();
                let file = expr[last_at + 1..].trim();
                Self::env_file_lookup(env_files, file).and_then(|ef| {
                    let vars = Self::resolve_env_section(ef, if section.is_empty() { None } else { Some(section) }, 0);
                    vars.get(key).filter(|v| !v.secret).map(|v| v.value.clone())
                })
            } else if at_count == 1 {
                let at = expr.find('@').unwrap();
                let key = expr[..at].trim();
                let file = expr[at + 1..].trim();
                Self::env_file_lookup(env_files, file).and_then(|ef| {
                    let vars = Self::resolve_env_section(ef, None, 0);
                    vars.get(key).filter(|v| !v.secret).map(|v| v.value.clone())
                })
            } else {
                None
            };
            if let Some(val) = replacement {
                result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
            } else {
                i = end + 1;
            }
        }
        // Then substitute simple ${VAR} from config (non-secret only)
        for (k, v) in config_vars {
            if let crate::ast::ConfigValue::String(val) = v {
                result = result.replace(&format!("${{{}}}", k), val);
            }
        }
        result
    }

    /// Replace `{{ ... }}` Go-template expressions with unique plain-string placeholders
    /// so the file parses as standard YAML. Returns modified text + extracted templates.
    fn extract_go_templates(text: &str) -> (String, Vec<String>) {
        let mut out = String::with_capacity(text.len());
        let mut templates: Vec<String> = Vec::new();
        let mut remaining = text;
        while let Some(start) = remaining.find("{{") {
            out.push_str(&remaining[..start]);
            remaining = &remaining[start..];
            if let Some(end_rel) = remaining.find("}}") {
                let end = end_rel + 2;
                templates.push(remaining[..end].to_string());
                out.push_str(&format!("__GOTEMPL{}__", templates.len() - 1));
                remaining = &remaining[end..];
            } else {
                out.push_str(remaining);
                remaining = "";
            }
        }
        out.push_str(remaining);
        (out, templates)
    }

    /// Restore Go-template placeholders back to their original expressions.
    fn restore_go_templates(text: String, templates: &[String]) -> String {
        let mut result = text;
        for (i, tmpl) in templates.iter().enumerate() {
            result = result.replace(&format!("__GOTEMPL{}__", i), tmpl);
        }
        result
    }

    /// Recursively expand `"key.sub.leaf"` string keys in YAML mappings into nested mappings.
    fn expand_dot_keys(value: Value) -> Value {
        match value {
            Value::Mapping(map) => {
                let mut result = serde_yaml::Mapping::new();
                for (k, v) in map {
                    // Recurse into values first so nested mappings are also expanded
                    let v = Self::expand_dot_keys(v);
                    if let Value::String(ref key_str) = k {
                        if key_str.contains('.') {
                            // Use set_nested_value to insert into the growing result mapping
                            let mut root = Value::Mapping(result);
                            let _ = crate::codegen::traefik::set_nested_value(&mut root, key_str, v);
                            result = match root {
                                Value::Mapping(m) => m,
                                _ => serde_yaml::Mapping::new(),
                            };
                            continue;
                        }
                    }
                    result.insert(k, v);
                }
                Value::Mapping(result)
            }
            Value::Sequence(seq) => {
                Value::Sequence(seq.into_iter().map(Self::expand_dot_keys).collect())
            }
            other => other,
        }
    }

    /// Hash all input files to detect changes for incremental regeneration.
    fn hash_file(path: &Path) -> u64 {
        let mut hasher = DefaultHasher::new();
        fs::read_to_string(path).unwrap_or_default().hash(&mut hasher);
        hasher.finish()
    }

    /// Compute a per-file hash map for all tracked input files.
    /// Keys are paths relative to project_root for stable cache keys.
    fn compute_file_hashes(ctx: &BuildContext) -> Result<HashMap<String, u64>, String> {
        let mut hashes: HashMap<String, u64> = HashMap::new();
        let root = &ctx.project_root;

        let add = |hashes: &mut HashMap<String, u64>, path: &Path| {
            if let Ok(rel) = path.strip_prefix(root) {
                hashes.insert(rel.to_string_lossy().into_owned(), Self::hash_file(path));
            }
        };

        // config.dyard
        let config_path = root.join("config.dyard");
        if config_path.exists() { add(&mut hashes, &config_path); }

        // container .dyard files
        let containers_dir = root.join(&ctx.project.config.input.containers);
        if containers_dir.exists() {
            let mut paths: Vec<PathBuf> = fs::read_dir(&containers_dir)
                .map_err(|e| format!("Failed to read containers dir: {}", e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("dyard"))
                .collect();
            paths.sort();
            for path in paths { add(&mut hashes, &path); }
        }

        // env files
        let env_dir = root.join(&ctx.project.config.input.env);
        if env_dir.exists() {
            let mut paths: Vec<PathBuf> = fs::read_dir(&env_dir)
                .map_err(|e| format!("Failed to read env dir: {}", e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("env"))
                .collect();
            paths.sort();
            for path in paths { add(&mut hashes, &path); }
        }

        // network template files (_*.yml) and service-specific network configs
        let network_dir = root.join(&ctx.project.config.input.network);
        if network_dir.exists() {
            let mut paths: Vec<PathBuf> = fs::read_dir(&network_dir)
                .map_err(|e| format!("Failed to read network dir: {}", e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yml"))
                .collect();
            paths.sort();
            for path in paths { add(&mut hashes, &path); }
        }

        // service-specific network config files referenced via `network config:`
        for service in &ctx.project.services {
            if let Some(ref nc_path) = service.network_config {
                let abs = root.join(nc_path);
                if abs.exists() { add(&mut hashes, &abs); }
            }
        }

        Ok(hashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_context_creation() {
        let _: Result<BuildContext, String> = Err("Mock test".to_string());
    }
}
