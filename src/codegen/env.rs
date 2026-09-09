use crate::ast::{EnvFile, EnvVariable};

pub struct EnvGenerator;

impl EnvGenerator {
    /// Generate .env file content from EnvFile AST
    /// Flattens all sections and returns key=value pairs
    pub fn generate_env_file(env_file: &EnvFile, section_name: Option<&str>) -> String {
        let mut lines = Vec::new();

        let section = if let Some(name) = section_name {
            env_file.sections.get(name)
        } else {
            Some(&env_file.root_section)
        };

        if let Some(section) = section {
            for (key, var) in &section.variables {
                lines.push(format!("{}={}", key, var.value));

                // Add aliases
                for alias in &var.aliases {
                    lines.push(format!("{}={}", alias, var.value));
                }
            }
        }

        lines.sort();
        lines.join("\n")
    }

    /// Generate .env file for a specific service
    /// Includes all variables from env_from sources
    pub fn generate_service_env(
        key: &str,
        var: &EnvVariable,
    ) -> String {
        let mut lines = vec![format!("{}={}", key, var.value)];

        for alias in &var.aliases {
            lines.push(format!("{}={}", alias, var.value));
        }

        lines.join("\n")
    }
}

pub fn generate_env_content(env_file: &EnvFile, section: Option<&str>) -> String {
    EnvGenerator::generate_env_file(env_file, section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_env_generation() {
        let mut vars = HashMap::new();
        vars.insert(
            "DB_HOST".to_string(),
            EnvVariable {
                value: "localhost".to_string(),
                secret: false,
                aliases: vec![],
            },
        );
        vars.insert(
            "DB_PORT".to_string(),
            EnvVariable {
                value: "5432".to_string(),
                secret: false,
                aliases: vec![],
            },
        );

        let env = crate::ast::EnvSection {
            extends: vec![],
            variables: vars,
        };

        let env_file = EnvFile {
            root_section: env,
            sections: HashMap::new(),
        };

        let content = generate_env_content(&env_file, None);
        assert!(content.contains("DB_HOST=localhost"));
        assert!(content.contains("DB_PORT=5432"));
    }
}
