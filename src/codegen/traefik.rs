use crate::ast::{RouteAdd, ServiceBlock, ServeConfig, TraefikDefaults};
use serde_yaml::{Mapping, Value};

pub struct TraefikGenerator;

impl TraefikGenerator {
    /// Generate dynamic Traefik config (conf.d/http.yml).
    /// Handles multiple `serve` directives per service with named routes.
    pub fn generate_dynamic(
        services: &[ServiceBlock],
        router_container: &str,
        domain: &str,
        defaults: &TraefikDefaults,
    ) -> Value {
        let mut http = Mapping::new();
        let mut routers = Mapping::new();
        let mut services_map = Mapping::new();

        for service in services {
            let is_router = service.name == router_container;

            for serve in &service.serve {
                // Router container's non-api_internal serves are skipped (it has no backend).
                if is_router && !serve.api_internal {
                    continue;
                }
                let router_name = Self::router_name(&service.name, serve);
                // api@internal is a built-in Traefik service; for normal serves use a lb service.
                let svc_name = if serve.api_internal {
                    "api@internal".to_string()
                } else {
                    format!("{}-service", router_name)
                };
                let base_rule = Self::base_rule(serve, domain);

                let mut router = Self::generate_router(serve, &base_rule, defaults, &svc_name);

                // Apply route_add overrides that target this serve
                let serve_key = serve.name.as_deref().unwrap_or(&serve.subdomain);
                for ra in &service.route_add {
                    if ra.route_name.as_deref() == Some(serve_key) {
                        Self::apply_route_add(&mut router, ra, &base_rule);
                    }
                }

                routers.insert(router_name.clone().into(), Value::Mapping(router));
                if !serve.api_internal {
                    services_map.insert(
                        svc_name.into(),
                        Self::generate_lb_service(service, serve),
                    );
                }
            }
        }

        if !routers.is_empty() {
            http.insert("routers".into(), Value::Mapping(routers));
        }
        if !services_map.is_empty() {
            http.insert("services".into(), Value::Mapping(services_map));
        }

        let mut root = Mapping::new();
        root.insert("http".into(), Value::Mapping(http));
        Value::Mapping(root)
    }

    /// Router name: `{service_name}-{route_name}` if named,
    /// service name for root domain ("."), otherwise the subdomain.
    fn router_name(service_name: &str, serve: &ServeConfig) -> String {
        if let Some(ref n) = serve.name {
            format!("{}-{}", service_name, n)
        } else if serve.subdomain == "." {
            service_name.to_string()
        } else {
            serve.subdomain.clone()
        }
    }

    /// Build the base Host rule for a serve directive.
    /// "." subdomain means the root domain (no prefix).
    fn base_rule(serve: &ServeConfig, domain: &str) -> String {
        if serve.subdomain == "." {
            format!("Host(`{}`)", domain)
        } else {
            format!("Host(`{}.{}`)", serve.subdomain, domain)
        }
    }

    fn generate_router(
        serve: &ServeConfig,
        base_rule: &str,
        defaults: &TraefikDefaults,
        svc_name: &str,
    ) -> Mapping {
        let mut router = Mapping::new();

        router.insert("rule".into(), base_rule.to_string().into());

        let entry_points: Vec<Value> = defaults
            .entry_points
            .iter()
            .map(|ep| Value::String(ep.clone()))
            .collect();
        if !entry_points.is_empty() {
            router.insert("entryPoints".into(), Value::Sequence(entry_points));
        }

        router.insert("service".into(), svc_name.to_string().into());

        // Strip optional @file suffix from input, then append @file for Traefik's file provider
        let middlewares: Vec<Value> = serve
            .middlewares
            .iter()
            .map(|mw| {
                let name = mw.find('@').map(|i| &mw[..i]).unwrap_or(mw.as_str());
                Value::String(format!("{}@file", name))
            })
            .collect();
        if !middlewares.is_empty() {
            router.insert("middlewares".into(), Value::Sequence(middlewares));
        }

        // TLS
        let mut tls = Mapping::new();
        if let Some(ref resolver) = defaults.cert_resolver {
            tls.insert("certResolver".into(), resolver.clone().into());
        }
        if !tls.is_empty() {
            router.insert("tls".into(), Value::Mapping(tls));
        }

        router
    }

    /// Apply a `route_add` entry to a router mapping.
    /// If key is "rule" and value contains "@", replace "@" with the base rule.
    fn apply_route_add(router: &mut Mapping, ra: &RouteAdd, base_rule: &str) {
        let value = if ra.key == "rule" {
            if let Value::String(ref s) = ra.value {
                Value::String(s.replace('@', base_rule))
            } else {
                ra.value.clone()
            }
        } else {
            ra.value.clone()
        };

        // Use dotted-path setter to support nested keys (e.g. "tls.certResolver")
        let mut root = Value::Mapping(std::mem::take(router));
        let _ = set_nested_value(&mut root, &ra.key, value);
        if let Value::Mapping(m) = root {
            *router = m;
        }
    }

    fn generate_lb_service(service: &ServiceBlock, serve: &ServeConfig) -> Value {
        let mut svc = Mapping::new();
        let mut lb = Mapping::new();
        let mut servers = Vec::new();

        let url = format!("http://{}:{}", service.name, serve.port);
        let mut server = Mapping::new();
        server.insert("url".into(), url.into());
        servers.push(Value::Mapping(server));

        lb.insert("servers".into(), Value::Sequence(servers));
        svc.insert("loadBalancer".into(), Value::Mapping(lb));
        Value::Mapping(svc)
    }

    /// Generate static Traefik config (traefik.yml).
    /// If `base` is provided, it is used as the starting config (from network-config file);
    /// otherwise a default config with api.dashboard is generated.
    pub fn generate_static(router_service: &ServiceBlock, base: Option<Value>) -> Value {
        let mut root = base.unwrap_or_else(|| {
            let mut config = Mapping::new();
            let mut api = Mapping::new();
            api.insert("dashboard".into(), true.into());
            config.insert("api".into(), Value::Mapping(api));
            Value::Mapping(config)
        });

        // Apply route_add directives (those without a route_name go to static config)
        for ra in &router_service.route_add {
            if ra.route_name.is_none() {
                let _ = set_nested_value(&mut root, &ra.key, ra.value.clone());
            }
        }

        root
    }
}

/// Set a value at a dotted path, creating intermediate mappings as needed.
pub fn set_nested_value(config: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let parts: Vec<&str> = path.splitn(2, '.').collect();

    if let Value::Mapping(ref mut map) = config {
        if parts.len() == 1 {
            map.insert(parts[0].to_string().into(), value);
        } else {
            let key: Value = parts[0].to_string().into();
            let entry = map
                .entry(key)
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            set_nested_value(entry, parts[1], value)?;
        }
        Ok(())
    } else {
        Err("Expected mapping at config node".to_string())
    }
}

pub fn generate_traefik_dynamic(
    services: &[ServiceBlock],
    router_container: &str,
    domain: &str,
    defaults: &TraefikDefaults,
) -> String {
    let config = TraefikGenerator::generate_dynamic(services, router_container, domain, defaults);
    let yaml = serde_yaml::to_string(&config).unwrap_or_else(|_| String::new());
    // Add blank lines between routers and between lb services (child_indent=4 inside http.routers)
    let yaml = separate_http_entries(&yaml, "routers");
    separate_http_entries(&yaml, "services")
}

/// Insert blank lines between entries of `block_name` inside the http: mapping.
/// These entries sit at 4-space indent in the serde_yaml output.
fn separate_http_entries(yaml: &str, block_name: &str) -> String {
    let block_header = format!("  {}:", block_name); // http.routers is at indent=2
    let mut out = String::with_capacity(yaml.len() + 256);
    let mut in_block = false;
    let mut first = true;

    for line in yaml.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if line.trim_end() == block_header.trim_end() || line.trim_end() == block_header {
            in_block = true;
            first = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_block {
            if indent <= 2 && !line.trim().is_empty() {
                in_block = false;
                out.push('\n');
            } else if indent == 4 && !trimmed.is_empty() && !trimmed.starts_with('-') {
                if !first {
                    out.push('\n');
                }
                first = false;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

pub fn generate_traefik_static(router_service: &ServiceBlock, base: Option<Value>) -> String {
    let config = TraefikGenerator::generate_static(router_service, base);
    serde_yaml::to_string(&config).unwrap_or_else(|_| String::new())
}
