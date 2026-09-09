use crate::ast::*;
use std::collections::HashMap;

pub fn parse_env_file(input: &str) -> Result<EnvFile, String> {
    let mut root_section = EnvSection {
        extends: vec![],
        variables: HashMap::new(),
    };
    let mut sections = HashMap::new();
    let mut current_section = "".to_string(); // Root section
    let mut current_extends = Vec::new();
    let mut current_vars: HashMap<String, EnvVariable> = HashMap::new();

    // Join lines ending with `\` (shell-style continuation)
    let joined: Vec<String> = {
        let mut out = Vec::new();
        let mut buf = String::new();
        for line in input.lines() {
            if line.ends_with('\\') {
                // trim() strips indent from continuation lines (the `\` is for readability)
                buf.push_str(line[..line.len() - 1].trim());
            } else {
                buf.push_str(line.trim_start_matches(|c: char| c == ' ' || c == '\t'));
                out.push(buf.clone());
                buf.clear();
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
        out
    };

    for line in &joined {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse section header
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous section if any
            flush_section(
                &current_section,
                &current_extends,
                &current_vars,
                &mut root_section,
                &mut sections,
            );

            // Parse new section header
            let header = &line[1..line.len() - 1]; // Remove [ ]
            current_section = parse_section_header(header)?;
            current_extends = extract_extends(header)?;
            // Carry forward existing vars from a previously seen section with the same name
            current_vars = sections
                .get(&current_section)
                .map(|s| s.variables.clone())
                .unwrap_or_default();
            continue;
        }

        // Parse variable or alias
        if let Ok((key, var)) = parse_env_line(line) {
            current_vars.insert(key, var);
        }
    }

    // Save last section
    flush_section(
        &current_section,
        &current_extends,
        &current_vars,
        &mut root_section,
        &mut sections,
    );

    Ok(EnvFile {
        root_section,
        sections,
    })
}

/// Commit the current accumulation into root_section or sections (merging if duplicate).
fn flush_section(
    name: &str,
    extends: &[crate::ast::EnvSource],
    vars: &std::collections::HashMap<String, EnvVariable>,
    root: &mut crate::ast::EnvSection,
    sections: &mut std::collections::HashMap<String, crate::ast::EnvSection>,
) {
    if name.is_empty() {
        if !vars.is_empty() {
            root.extends = extends.to_vec();
            root.variables.extend(vars.clone());
        }
    } else if !vars.is_empty() || !extends.is_empty() {
        let entry = sections.entry(name.to_string()).or_insert_with(|| crate::ast::EnvSection {
            extends: vec![],
            variables: std::collections::HashMap::new(),
        });
        // Merge: later declarations override earlier ones
        entry.extends = extends.to_vec();
        entry.variables.extend(vars.clone());
    }
}

fn parse_section_header(header: &str) -> Result<String, String> {
    let parts: Vec<&str> = header.split_whitespace().collect();
    if parts.is_empty() {
        return Err("Empty section header".to_string());
    }
    Ok(parts[0].to_string())
}

fn extract_extends(header: &str) -> Result<Vec<EnvSource>, String> {
    if !header.contains("extends") {
        return Ok(Vec::new());
    }

    let parts: Vec<&str> = header.split_whitespace().collect();
    let mut extends = Vec::new();
    let mut in_extends = false;

    for part in parts {
        if part == "extends" {
            in_extends = true;
            continue;
        }

        if !in_extends {
            continue;
        }

        // Remove trailing comma
        let part = part.trim_end_matches(',');

        // Parse extends source
        if part.contains('@') {
            let (section, file) = part.split_once('@').unwrap_or(("", part));
            let section = if section.is_empty() { None } else { Some(section.to_string()) };
            extends.push(EnvSource::File {
                file: file.to_string(),
                section,
            });
        } else {
            // Local section reference
            extends.push(EnvSource::File {
                file: String::new(), // Empty file = local reference
                section: Some(part.to_string()),
            });
        }
    }

    Ok(extends)
}

fn parse_env_line(line: &str) -> Result<(String, EnvVariable), String> {
    // Case 1: "KEY as ALIAS" (no value — rename from parent via extends)
    if !line.contains('=') && !line.contains(':') {
        if let Some(as_pos) = line.find(" as ") {
            let key = line[..as_pos].trim().trim_start_matches('*').to_string();
            let secret = line.trim_start().starts_with('*');
            let aliases: Vec<String> = line[as_pos + 4..]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !key.is_empty() && !aliases.is_empty() {
                return Ok((key, EnvVariable { value: String::new(), secret, aliases }));
            }
        }
        return Err(format!("Invalid env line: {}", line));
    }

    // Split off optional " as ALIAS1, ALIAS2" suffix (only after '=')
    let (main_part, aliases) = if let Some(as_pos) = find_as_boundary(line) {
        let aliases: Vec<String> = line[as_pos + 4..]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (&line[..as_pos], aliases)
    } else {
        (line, vec![])
    };

    // Try '=' first, then ':' as value separator
    let sep_pos = main_part.find('=').or_else(|| main_part.find(':'));
    if let Some(pos) = sep_pos {
        let key_part = main_part[..pos].trim();
        let value = &main_part[pos + 1..];
        let secret = key_part.starts_with('*');
        let key = key_part.trim_start_matches('*').to_string();
        let value = strip_quotes(value);
        return Ok((key, EnvVariable { value, secret, aliases }));
    }

    Err(format!("Invalid env line: {}", line))
}

/// Finds the byte position of " as " that separates value from alias list.
/// Only looks after the first '=' to avoid false positives.
fn find_as_boundary(line: &str) -> Option<usize> {
    if let Some(eq) = line.find('=') {
        let after_eq = &line[eq..];
        if let Some(pos) = after_eq.find(" as ") {
            return Some(eq + pos);
        }
    }
    None
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"'))
        || (s.starts_with('\'') && s.ends_with('\''))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_env() {
        let input = r#"
[common]
DB_HOST=localhost
DB_PORT=5432

[app extends common]
APP_DEBUG=true
"#;

        let result = parse_env_file(input).unwrap();
        assert!(result.sections.contains_key("common"));
        assert!(result.sections.contains_key("app"));

        let common = &result.sections["common"];
        assert_eq!(common.variables.get("DB_HOST").unwrap().value, "localhost");
    }

    #[test]
    fn test_parse_secrets() {
        let input = r#"
[db]
*PASSWORD=secretpass
USER=admin
"#;

        let result = parse_env_file(input).unwrap();
        let db = &result.sections["db"];

        let password = db.variables.get("PASSWORD").unwrap();
        assert!(password.secret);
        assert_eq!(password.value, "secretpass");

        let user = db.variables.get("USER").unwrap();
        assert!(!user.secret);
    }
}
