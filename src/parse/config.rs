use crate::ast::*;
use std::collections::HashMap;

pub fn parse_config_file(input: &str) -> Result<ConfigFile, String> {
    let mut input_paths = InputPaths {
        containers: "containers/".to_string(),
        network: "network/".to_string(),
        env: "env/".to_string(),
    };

    let mut output_paths = OutputPaths {
        compose: "build/docker-compose.yml".to_string(),
        traefik: "build/traefik".to_string(),
        env: "build/env".to_string(),
    };

    let mut router_container = "traefik".to_string();
    let mut variables: HashMap<String, ConfigValue> = HashMap::new();
    let mut traefik_entry_points = vec!["websecure".to_string()];
    let mut traefik_cert_resolver = Some("letsencrypt".to_string());
    let mut traefik_tls_options = None;
    let mut defaults_restart = "unless-stopped".to_string();

    let mut current_section = String::new();
    // For multi-line list values (e.g. "entryPoints:\n  - websecure")
    let mut current_list_key: Option<String> = None;

    for raw_line in input.lines() {
        let trimmed = raw_line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // YAML list item continuation: "  - value"
        if trimmed.starts_with("- ") {
            if let Some(ref lk) = current_list_key {
                let item = trimmed[2..].trim().to_string();
                if current_section == "traefik" && lk == "entrypoints" {
                    traefik_entry_points.push(item);
                }
            }
            continue;
        } else {
            current_list_key = None;
        }

        let indent = raw_line.len() - raw_line.trim_start().len();

        // Section header: zero indent AND ends with ':' AND no value after ':'
        if indent == 0 && trimmed.ends_with(':') && !trimmed[..trimmed.len()-1].contains(':') {
            current_section = trimmed.trim_end_matches(':').to_lowercase();
            continue;
        }

        // Parse key-value pairs based on current section
        match current_section.as_str() {
            "input" => {
                if let Ok((key, val)) = parse_kv_line(trimmed) {
                    match key.as_str() {
                        "containers" => input_paths.containers = val,
                        "network" => input_paths.network = val,
                        "env" => input_paths.env = val,
                        _ => {}
                    }
                }
            }
            "output" => {
                if let Ok((key, val)) = parse_kv_line(trimmed) {
                    match key.as_str() {
                        "compose" => output_paths.compose = val,
                        "traefik" => output_paths.traefik = val,
                        "env" => output_paths.env = val,
                        _ => {}
                    }
                }
            }
            "traefik" => {
                if let Ok((key, val)) = parse_kv_line(trimmed) {
                    match key.as_str() {
                        "entryPoints" | "entrypoints" => {
                            if val.is_empty() {
                                // Multi-line list follows
                                traefik_entry_points.clear();
                                current_list_key = Some("entrypoints".to_string());
                            } else {
                                traefik_entry_points = parse_list(&val);
                            }
                        }
                        "certResolver" | "cert_resolver" => {
                            traefik_cert_resolver = if val.is_empty() { None } else { Some(val) };
                        }
                        "tlsOptions" | "tls_options" => {
                            traefik_tls_options = Some(val);
                        }
                        _ => {}
                    }
                }
            }
            "defaults" => {
                if let Ok((key, val)) = parse_kv_line(trimmed) {
                    if key == "restart" {
                        defaults_restart = val;
                    }
                }
            }
            "" => {
                // Root level - global variables
                if trimmed.contains(':') || trimmed.contains('=') {
                    if let Ok((key, val)) = parse_kv_line(trimmed) {
                        let secret = trimmed.starts_with('*');
                        let key = key.trim_start_matches('*').to_string();

                        if key == "router_container" {
                            router_container = val;
                        } else {
                            variables.insert(
                                key,
                                if secret {
                                    ConfigValue::Secret(val)
                                } else {
                                    ConfigValue::String(val)
                                },
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(ConfigFile {
        input: input_paths,
        output: output_paths,
        router_container,
        variables,
        traefik_defaults: TraefikDefaults {
            entry_points: traefik_entry_points,
            cert_resolver: traefik_cert_resolver,
            tls_options: traefik_tls_options,
        },
        defaults: Defaults {
            restart: defaults_restart,
        },
    })
}

fn parse_kv_line(line: &str) -> Result<(String, String), String> {
    // Try ':' first (YAML-style), then '=' (shell-style)
    if let Some((key, value)) = line.split_once(':') {
        Ok((key.trim().to_string(), value.trim().to_string()))
    } else if let Some((key, value)) = line.split_once('=') {
        Ok((key.trim().to_string(), value.trim().to_string()))
    } else {
        Err(format!("Invalid config line: {}", line))
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let input = r#"
DOMAIN: example.com
DATA_PATH: /var/lib/dyard

input:
  containers: containers/
  network: network/

output:
  compose: build/docker-compose.yml
  traefik: build/traefik

traefik:
  entryPoints: [https]
  certResolver: letsencrypt

defaults:
  restart: unless-stopped
"#;

        let config = parse_config_file(input).unwrap();
        assert_eq!(config.variables.get("DOMAIN").unwrap(), &ConfigValue::String("example.com".to_string()));
        assert_eq!(config.input.containers, "containers/");
        assert_eq!(config.output.compose, "build/docker-compose.yml");
    }
}
