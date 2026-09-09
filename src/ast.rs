use std::collections::HashMap;

/// Service-DSL AST
#[derive(Debug, Clone)]
pub struct ServiceBlock {
    /// Source file name and 1-based line where `container` was declared.
    pub source_file: String,
    pub source_line: usize,
    pub name: String,
    pub image: String,
    pub limit: Option<ResourceLimit>,
    pub networks: Vec<String>,
    pub volumes: Vec<Volume>,
    pub expose: Vec<u16>,
    pub user: Option<String>,
    pub depends_on: Vec<Dependency>,
    pub healthcheck: Option<HealthCheck>,
    pub env_from: Vec<EnvSource>,
    /// Multiple serve directives allowed; if more than one, all must have a name.
    pub serve: Vec<ServeConfig>,
    pub route_add: Vec<RouteAdd>,
    pub network_config: Option<String>,
    pub raw: HashMap<String, serde_yaml::Value>,
    /// Raw YAML block from `end` directive — deep-merged into top-level compose output.
    pub raw_compose_block: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResourceLimit {
    pub memory: String,
    pub cpus: String,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub container_path: String,
    pub host_path: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub service: String,
    pub condition: DependencyCondition,
}

#[derive(Debug, Clone)]
pub enum DependencyCondition {
    Started,
    Healthy,
}

#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub test: String,
    pub interval: Option<String>,
    pub timeout: Option<String>,
    pub retries: Option<u32>,
    pub start_period: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EnvSource {
    Inline { key: String, value: String, secret: bool },
    /// Include all variables from `section` of `file`.
    File { file: String, section: Option<String> },
    /// Reference a single variable: `${KEY@section@file}`.
    /// Resolved to Inline during codegen.
    VarRef { key: String, file: String, section: Option<String> },
}

#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Optional route name for multi-route containers (mandatory when multiple serves exist).
    /// In output: router is named `{container_name}-{route_name}`.
    pub name: Option<String>,
    /// Subdomain for the route, or "." to host at the root domain.
    pub subdomain: String,
    pub port: u16,
    /// When true, the Traefik service is `api@internal` (no port; valid only for the router container).
    pub api_internal: bool,
    pub middlewares: Vec<String>,
    pub use_service: Option<String>,
}

/// A `route_add` directive that targets a specific named route or the current serve context.
#[derive(Debug, Clone)]
pub struct RouteAdd {
    /// Which named serve this applies to. None = resolved to last-seen serve at parse time,
    /// or to the static traefik config if the service is the router container.
    pub route_name: Option<String>,
    /// Dotted path key (e.g. "priority", "rule", "entryPoints.web.address").
    pub key: String,
    pub value: serde_yaml::Value,
}

/// Env-DSL AST
#[derive(Debug, Clone)]
pub struct EnvFile {
    pub root_section: EnvSection,
    pub sections: HashMap<String, EnvSection>,
}

#[derive(Debug, Clone)]
pub struct EnvSection {
    pub extends: Vec<EnvSource>,
    pub variables: HashMap<String, EnvVariable>,
}

#[derive(Debug, Clone)]
pub struct EnvVariable {
    pub value: String,
    pub secret: bool,
    pub aliases: Vec<String>,
}

/// Config-DSL AST
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub input: InputPaths,
    pub output: OutputPaths,
    pub router_container: String,
    pub variables: HashMap<String, ConfigValue>,
    pub traefik_defaults: TraefikDefaults,
    pub defaults: Defaults,
}

#[derive(Debug, Clone)]
pub struct InputPaths {
    pub containers: String,
    pub network: String,
    pub env: String,
}

#[derive(Debug, Clone)]
pub struct OutputPaths {
    pub compose: String,
    pub traefik: String,
    pub env: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    String(String),
    Secret(String),
}

#[derive(Debug, Clone)]
pub struct TraefikDefaults {
    pub entry_points: Vec<String>,
    pub cert_resolver: Option<String>,
    pub tls_options: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Defaults {
    pub restart: String,
}

/// Parsed project
#[derive(Debug)]
pub struct Project {
    pub config: ConfigFile,
    pub services: Vec<ServiceBlock>,
    pub env_files: HashMap<String, EnvFile>,
}
