use crate::ast::{Project, ServiceBlock, EnvSource, ConfigFile};
use std::collections::{HashSet, HashMap};

#[derive(Debug, Clone)]
pub enum ValidationError {
    MissingService(String),
    MissingMiddleware(String),
    MissingEnvFile(String),
    MissingVariable(String, String),
    MissingEnvSection(String, String),
    InvalidEnvSource(String),
    MissingRouterContainer(String),
    DuplicateService(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::MissingService(msg) => write!(f, "{}", msg),
            ValidationError::MissingMiddleware(mw) => {
                write!(f, "Middleware '{}' referenced but not found", mw)
            }
            ValidationError::MissingEnvFile(file) => {
                write!(f, "Environment file '{}' not found", file)
            }
            ValidationError::MissingVariable(file, var) => {
                write!(f, "Variable '{}' not found in '{}'", var, file)
            }
            ValidationError::MissingEnvSection(file, section) => {
                write!(f, "Section '{}' not found in '{}'", section, file)
            }
            ValidationError::InvalidEnvSource(src) => {
                write!(f, "Invalid env source: {}", src)
            }
            ValidationError::MissingRouterContainer(name) => {
                write!(f, "Router container '{}' not found in services", name)
            }
            ValidationError::DuplicateService(msg) => write!(f, "{}", msg),
        }
    }
}

#[derive(Default)]
pub struct Validator {
    errors: Vec<ValidationError>,
}

impl Validator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn validate_project(&mut self, project: &Project) -> Result<(), Vec<ValidationError>> {
        // Validate router container exists
        self.validate_router_container(&project.config, &project.services);

        // Detect duplicate service names
        let mut seen: HashMap<&str, &ServiceBlock> = HashMap::new();
        for service in &project.services {
            if let Some(first) = seen.get(service.name.as_str()) {
                self.errors.push(ValidationError::DuplicateService(format!(
                    "{}:{}: service '{}' is already defined at {}:{}",
                    service.source_file, service.source_line,
                    service.name,
                    first.source_file, first.source_line,
                )));
            } else {
                seen.insert(&service.name, service);
            }
        }

        // Validate services
        let service_names: HashSet<String> =
            project.services.iter().map(|s| s.name.clone()).collect();

        for service in &project.services {
            self.validate_service(service, &service_names, &project.config);
        }

        // Validate env files
        for (file_name, env_file) in &project.env_files {
            self.validate_env_file(file_name, env_file);
        }

        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.drain(..).collect())
        }
    }

    fn validate_router_container(
        &mut self,
        config: &ConfigFile,
        services: &[ServiceBlock],
    ) {
        let service_names: HashSet<&str> = services.iter().map(|s| s.name.as_str()).collect();

        if !service_names.contains(config.router_container.as_str()) {
            self.errors.push(ValidationError::MissingRouterContainer(
                config.router_container.clone(),
            ));
        }
    }

    fn validate_service(
        &mut self,
        service: &ServiceBlock,
        service_names: &HashSet<String>,
        _config: &ConfigFile,
    ) {
        let loc = format!("{}:{}", service.source_file, service.source_line);

        // Validate depends_on references
        for dep in &service.depends_on {
            if !service_names.contains(&dep.service) {
                self.errors.push(ValidationError::MissingService(
                    format!("{}: Service '{}' referenced in '{}' but not defined", loc, dep.service, service.name)
                ));
            }
        }

        // Validate serve directives
        if service.serve.len() > 1 {
            for serve in &service.serve {
                if serve.name.is_none() {
                    self.errors.push(ValidationError::InvalidEnvSource(format!(
                        "{}: '{}': all `serve` directives must have a name when there are multiple (e.g. `serve web as ...`)",
                        loc, service.name
                    )));
                }
            }
        }
        for serve in &service.serve {
            if serve.port == 0 && !serve.api_internal {
                self.errors.push(ValidationError::InvalidEnvSource(
                    format!("{}: invalid port 0 in serve for '{}'", loc, service.name),
                ));
            }
        }

        // Validate env_from references
        for env_src in &service.env_from {
            match env_src {
                EnvSource::File { file, section } => {
                    if !file.is_empty() {
                        if !file.ends_with(".env") {
                            self.errors.push(ValidationError::InvalidEnvSource(format!(
                                "Invalid env file format: {}",
                                file
                            )));
                        }

                        if let Some(sec) = section {
                            if sec.is_empty() {
                                self.errors.push(ValidationError::InvalidEnvSource(
                                    "Empty section name".to_string(),
                                ));
                            }
                        }
                    }
                }
                EnvSource::Inline { key, .. } => {
                    if key.is_empty() {
                        self.errors.push(ValidationError::InvalidEnvSource(
                            "Empty variable name".to_string(),
                        ));
                    }
                }
                EnvSource::VarRef { key, file, .. } => {
                    if key.is_empty() || file.is_empty() {
                        self.errors.push(ValidationError::InvalidEnvSource(
                            "Empty key or file in var reference".to_string(),
                        ));
                    }
                }
            }
        }
    }

    fn validate_env_file(&mut self, file_name: &str, env_file: &crate::ast::EnvFile) {
        // Validate that extends references are valid
        for (section_name, section) in &env_file.sections {
            self.validate_section_extends(file_name, section_name, section, env_file);
        }

        // Validate root section
        self.validate_section_extends(file_name, "root", &env_file.root_section, env_file);
    }

    fn validate_section_extends(
        &mut self,
        file_name: &str,
        _section_name: &str,
        section: &crate::ast::EnvSection,
        env_file: &crate::ast::EnvFile,
    ) {
        for extends_src in &section.extends {
            if let EnvSource::File { file, section: ext_section } = extends_src {
                if file.is_empty() {
                    // Local reference
                    if let Some(ext_sec) = ext_section {
                        if !env_file.sections.contains_key(ext_sec) {
                            self.errors.push(ValidationError::MissingEnvSection(
                                file_name.to_string(),
                                ext_sec.clone(),
                            ));
                        }
                    }
                } else {
                    // External file reference
                    if !file.ends_with(".env") {
                        self.errors.push(ValidationError::InvalidEnvSource(format!(
                            "Invalid env file in extends: {}",
                            file
                        )));
                    }
                }
            }
        }
    }
}

pub fn validate(project: &Project) -> Result<(), Vec<ValidationError>> {
    let mut validator = Validator::new();
    validator.validate_project(project)
}
