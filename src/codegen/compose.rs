use crate::ast::{ServiceBlock, EnvSource};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;

pub struct ComposeGenerator;

impl ComposeGenerator {
    pub fn generate(services: &[ServiceBlock], router_container: &str) -> Value {
        let mut compose = Mapping::new();

        // Auto-network per serve-enabled non-router service: "{name}-net"
        // Traefik joins all of them; each service always gets its own auto-net.
        let auto_nets: Vec<String> = services
            .iter()
            .filter(|s| !s.serve.is_empty() && s.name != router_container)
            .map(|s| format!("{}-net", s.name))
            .collect();

        let mut services_map = Mapping::new();
        for service in services {
            let extra_networks: Vec<String> = if service.name == router_container {
                // Router joins every auto-net
                auto_nets.clone()
            } else if !service.serve.is_empty() {
                // Serve-enabled service gets its own dedicated auto-net (always, even if explicit nets exist)
                vec![format!("{}-net", service.name)]
            } else {
                vec![]
            };
            services_map.insert(
                service.name.clone().into(),
                Self::generate_service(service, &extra_networks),
            );
        }
        compose.insert("services".into(), Value::Mapping(services_map));

        // Top-level networks: all explicit + all auto-nets
        let all_extra: BTreeSet<String> = auto_nets.into_iter().collect();
        let networks = Self::collect_networks(services, &all_extra);
        if !networks.is_empty() {
            let mut networks_map = Mapping::new();
            for net in networks {
                networks_map.insert(net.into(), Mapping::new().into());
            }
            compose.insert("networks".into(), Value::Mapping(networks_map));
        }

        let mut result = Value::Mapping(compose);

        // Deep-merge raw_compose_block from each service into the top-level output
        for service in services {
            if let Some(ref raw) = service.raw_compose_block {
                if let Ok(raw_value) = serde_yaml::from_str::<Value>(raw) {
                    deep_merge_yaml(&mut result, raw_value);
                }
            }
        }

        result
    }

    fn generate_service(service: &ServiceBlock, extra_networks: &[String]) -> Value {
        let mut svc = Mapping::new();

        // Image
        svc.insert("image".into(), service.image.clone().into());

        // Container name
        svc.insert("container_name".into(), service.name.clone().into());

        // Restart policy
        svc.insert("restart".into(), "unless-stopped".into());

        // Resource limits
        if let Some(ref limit) = service.limit {
            svc.insert("mem_limit".into(), limit.memory.clone().into());
            // Strip optional "cpu"/"cpus" suffix (e.g. "2cpu" → "2")
            let cpus = limit.cpus.trim_end_matches("cpus").trim_end_matches("cpu").trim();
            svc.insert("cpus".into(), cpus.into());
        }

        // Networks (include auto-assigned extras for router container)
        let all_networks: Vec<&str> = service
            .networks
            .iter()
            .map(|s| s.as_str())
            .chain(extra_networks.iter().map(|s| s.as_str()))
            .collect();
        if !all_networks.is_empty() {
            let networks: Vec<Value> = all_networks
                .iter()
                .map(|n| Value::String(n.to_string()))
                .collect();
            svc.insert("networks".into(), Value::Sequence(networks));
        }

        // Volumes
        if !service.volumes.is_empty() {
            let volumes: Vec<Value> = service
                .volumes
                .iter()
                .map(|v| {
                    let vol_str = if let Some(ref mode) = v.mode {
                        format!("{}:{}:{}", v.host_path, v.container_path, mode)
                    } else {
                        format!("{}:{}", v.host_path, v.container_path)
                    };
                    Value::String(vol_str)
                })
                .collect();
            svc.insert("volumes".into(), Value::Sequence(volumes));
        }

        // Expose
        if !service.expose.is_empty() {
            let expose: Vec<Value> = service
                .expose
                .iter()
                .map(|p| Value::String(p.to_string()))
                .collect();
            svc.insert("expose".into(), Value::Sequence(expose));
        }

        // User
        if let Some(ref user) = service.user {
            svc.insert("user".into(), user.clone().into());
        }

        // Depends on
        if !service.depends_on.is_empty() {
            let mut depends = Mapping::new();
            for dep in &service.depends_on {
                let mut dep_map = Mapping::new();
                let condition = match dep.condition {
                    crate::ast::DependencyCondition::Started => "service_started",
                    crate::ast::DependencyCondition::Healthy => "service_healthy",
                };
                dep_map.insert("condition".into(), condition.into());
                depends.insert(dep.service.clone().into(), Value::Mapping(dep_map));
            }
            svc.insert("depends_on".into(), Value::Mapping(depends));
        }

        // Healthcheck
        if let Some(ref hc) = service.healthcheck {
            let mut healthcheck = Mapping::new();
            healthcheck.insert(
                "test".into(),
                Value::Sequence(vec![
                    "CMD-SHELL".into(),
                    hc.test.clone().into(),
                ]),
            );

            if let Some(ref interval) = hc.interval {
                healthcheck.insert("interval".into(), interval.clone().into());
            }
            if let Some(ref timeout) = hc.timeout {
                healthcheck.insert("timeout".into(), timeout.clone().into());
            }
            if let Some(retries) = hc.retries {
                healthcheck.insert("retries".into(), retries.into());
            }
            if let Some(ref start_period) = hc.start_period {
                healthcheck.insert("start_period".into(), start_period.clone().into());
            }

            svc.insert("healthcheck".into(), Value::Mapping(healthcheck));
        }

        // Environment
        if !service.env_from.is_empty() || !service.raw.is_empty() {
            let mut env_map = Mapping::new();

            for source in &service.env_from {
                match source {
                    EnvSource::Inline { key, value, .. } => {
                        env_map.insert(key.clone().into(), value.clone().into());
                    }
                    EnvSource::File { .. } | EnvSource::VarRef { .. } => {
                        // File → handled by env_file below; VarRef → resolved before codegen
                    }
                }
            }

            if !env_map.is_empty() {
                svc.insert("environment".into(), Value::Mapping(env_map));
            }

            // env_file for file sources (all vars — secret and non-secret — in one file)
            let file_sources: Vec<String> = service
                .env_from
                .iter()
                .filter_map(|s| {
                    if let EnvSource::File { file, section } = s {
                        if !file.is_empty() {
                            Some(format!(
                                "env/{}.{}env",
                                service.name,
                                section.as_ref().map(|s| format!("{}.", s)).unwrap_or_default()
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if !file_sources.is_empty() {
                svc.insert(
                    "env_file".into(),
                    Value::Sequence(
                        file_sources.into_iter().map(|s| s.into()).collect(),
                    ),
                );
            }
        }

        // Passthrough raw fields
        for (key, value) in &service.raw {
            svc.insert(key.clone().into(), value.clone());
        }

        Value::Mapping(svc)
    }

    fn collect_networks(services: &[ServiceBlock], extra: &BTreeSet<String>) -> Vec<String> {
        let mut networks = BTreeSet::new();
        for service in services {
            for net in &service.networks {
                networks.insert(net.clone());
            }
        }
        networks.extend(extra.iter().cloned());
        networks.into_iter().collect()
    }
}

/// Recursively deep-merge `overlay` into `base`. Mappings are merged; other types are replaced.
fn deep_merge_yaml(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Mapping(base_map), Value::Mapping(overlay_map)) => {
            for (k, v) in overlay_map {
                let entry = base_map
                    .entry(k)
                    .or_insert(Value::Null);
                deep_merge_yaml(entry, v);
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
    }
}

pub fn generate_compose_yaml(services: &[ServiceBlock], router_container: &str) -> String {
    let compose = ComposeGenerator::generate(services, router_container);
    let yaml = serde_yaml::to_string(&compose).unwrap_or_else(|_| String::new());
    separate_block_entries(&yaml, "services", 2)
}

/// Insert a blank line between direct children of `parent_key` at `child_indent` spaces.
fn separate_block_entries(yaml: &str, parent_key: &str, child_indent: usize) -> String {
    let section_header = format!("{}:", parent_key);
    let mut out = String::with_capacity(yaml.len() + 512);
    let mut in_section = false;
    let mut first_child = true;

    for line in yaml.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if line.trim() == section_header {
            in_section = true;
            first_child = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_section {
            if indent == 0 && !line.trim().is_empty() {
                // Exited the block
                in_section = false;
                out.push('\n');
            } else if indent == child_indent
                && !trimmed.is_empty()
                && !trimmed.starts_with('-')
            {
                // New child entry
                if !first_child {
                    out.push('\n');
                }
                first_child = false;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ResourceLimit, ServiceBlock};

    #[test]
    fn test_simple_service_generation() {
        let service = ServiceBlock {
            source_file: "test".to_string(),
            source_line: 1,
            name: "web".to_string(),
            image: "nginx:latest".to_string(),
            limit: Some(ResourceLimit {
                memory: "512m".to_string(),
                cpus: "1".to_string(),
            }),
            networks: vec!["web-net".to_string()],
            volumes: vec![],
            expose: vec![80],
            user: None,
            depends_on: vec![],
            healthcheck: None,
            env_from: vec![],
            serve: vec![],
            route_add: vec![],
            network_config: None,
            raw: Default::default(),
            raw_compose_block: None,
        };

        let yaml = generate_compose_yaml(&[service], "traefik");
        assert!(yaml.contains("nginx:latest"));
        assert!(yaml.contains("512m"));
    }
}
