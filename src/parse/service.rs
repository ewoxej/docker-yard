use crate::ast::*;
use super::{ws, identifier, quoted_string};
use nom::{
    bytes::complete::{tag, take_until},
    character::complete::line_ending,
    combinator::opt,
    IResult,
};
use std::collections::HashMap;

type HealthCheckTiming = (Option<String>, Option<String>, Option<u32>, Option<String>);

pub fn parse_service_file(input: &str, filename: &str) -> Result<Vec<ServiceBlock>, String> {
    // Ensure input ends with a newline so all `take_until("\n")` parsers see a terminator.
    let owned;
    let input: &str = if input.ends_with('\n') {
        input
    } else {
        owned = format!("{}\n", input);
        &owned
    };

    let mut services = Vec::new();
    let mut remaining = input;

    while !remaining.is_empty() {
        remaining = skip_empty_lines(remaining);
        if remaining.is_empty() {
            break;
        }

        let block_line = line_of(input, remaining);
        match parse_service_block(remaining) {
            Ok((rest, mut service)) => {
                service.source_file = filename.to_string();
                service.source_line = block_line;
                services.push(service);
                remaining = rest;
            }
            Err(e) => {
                let (fail_input, code) = match &e {
                    nom::Err::Error(err) | nom::Err::Failure(err) => {
                        (err.input, format!("{:?}", err.code))
                    }
                    nom::Err::Incomplete(_) => (remaining, "Incomplete".to_string()),
                };
                let line = line_of(input, fail_input);
                let snippet = fail_input.lines().next().unwrap_or("").trim();
                return Err(format!("{}:{}: parse error ({}): {:?}", filename, line, code, snippet));
            }
        }
    }

    Ok(services)
}

/// Returns the 1-based line number of `remaining` within `original`.
/// Works because nom slices are always sub-slices of the original input.
fn line_of(original: &str, remaining: &str) -> usize {
    let offset = (remaining.as_ptr() as usize).saturating_sub(original.as_ptr() as usize);
    let offset = offset.min(original.len());
    original[..offset].chars().filter(|&c| c == '\n').count() + 1
}

fn skip_empty_lines(input: &str) -> &str {
    let mut s = input;
    while s.starts_with('\n') || s.starts_with('\r') {
        if s.starts_with("\r\n") {
            s = &s[2..];
        } else {
            s = &s[1..];
        }
    }
    s
}

fn parse_service_block(input: &str) -> IResult<&str, ServiceBlock> {
    let (input, _) = ws(input)?;
    let (input, (name, image)) = parse_container_line(input)?;
    let (input, _) = line_ending(input)?;

    let mut limit = None;
    let mut networks = Vec::new();
    let mut volumes = Vec::new();
    let mut expose = Vec::new();
    let mut user = None;
    let mut depends_on = Vec::new();
    let mut healthcheck = None;
    let mut env_from = Vec::new();
    let mut serve: Vec<ServeConfig> = Vec::new();
    let mut route_add: Vec<RouteAdd> = Vec::new();
    let mut network_config = None;
    let mut raw = HashMap::new();
    let mut raw_compose_block: Option<String> = None;

    // Track the name of the most recently declared serve (for nameless route_add resolution)
    let mut last_serve_name: Option<String> = None;

    let mut remaining = input;
    loop {
        remaining = skip_empty_lines(remaining);

        // End of file or next container block
        if remaining.starts_with("container ") || remaining.is_empty() {
            break;
        }

        // `end` directive — terminates this block and captures raw YAML that follows
        let trimmed = remaining.trim_start_matches([' ', '\t']);
        if trimmed.starts_with("end\n") || trimmed.starts_with("end\r\n") || trimmed == "end" {
            // Consume the `end` line
            if let Some(nl) = remaining.find('\n') {
                remaining = &remaining[nl + 1..];
            } else {
                remaining = "";
            }
            // Collect everything until the next `container` block (or EOF)
            let (raw_block, rest) = collect_raw_block(remaining);
            let raw_block = raw_block.trim();
            if !raw_block.is_empty() {
                raw_compose_block = Some(raw_block.to_string());
            }
            remaining = rest;
            break;
        }

        if let Ok((rest, l)) = parse_limit_line(remaining) {
            limit = Some(l);
            remaining = rest;
        } else if let Ok((rest, n)) = parse_network_line(remaining) {
            networks.push(n);
            remaining = rest;
        } else if let Ok((rest, v)) = parse_volume_line(remaining) {
            volumes.push(v);
            remaining = rest;
        } else if let Ok((rest, e)) = parse_expose_line(remaining) {
            expose.extend(e);
            remaining = rest;
        } else if let Ok((rest, u)) = parse_user_line(remaining) {
            user = Some(u);
            remaining = rest;
        } else if let Ok((rest, d)) = parse_depends_on_line(remaining) {
            depends_on.extend(d);
            remaining = rest;
        } else if let Ok((rest, hc)) = parse_healthcheck_line(remaining) {
            healthcheck = Some(hc);
            remaining = rest;
        } else if let Ok((rest, env)) = parse_env_from_line(remaining) {
            env_from.extend(env);
            remaining = rest;
        } else if let Ok((rest, s)) = parse_serve_line(remaining) {
            // Track last serve name for nameless route_add resolution
            last_serve_name = s.name.clone();
            serve.push(s);
            remaining = rest;
        } else if let Ok((rest, mut ra)) = parse_route_add_line(remaining) {
            // Resolve nameless route_add to the most recently seen serve
            if ra.route_name.is_none() {
                ra.route_name = last_serve_name.clone();
            }
            route_add.push(ra);
            remaining = rest;
        } else if let Ok((rest, nc)) = parse_network_config_line(remaining) {
            network_config = Some(nc);
            remaining = rest;
        } else if let Ok((rest, (key, val))) = parse_raw_line(remaining) {
            raw.insert(key, val);
            remaining = rest;
        } else {
            // Skip unknown line
            if let Ok((rest, _)) = take_until::<_, _, nom::error::Error<_>>("\n")(remaining) {
                remaining = rest.strip_prefix('\n').unwrap_or(rest);
            } else {
                break;
            }
        }
    }

    Ok((
        remaining,
        ServiceBlock {
            source_file: String::new(), // filled in by parse_service_file
            source_line: 0,
            name,
            image,
            limit,
            networks,
            volumes,
            expose,
            user,
            depends_on,
            healthcheck,
            env_from,
            serve,
            route_add,
            network_config,
            raw,
            raw_compose_block,
        },
    ))
}

/// Collect lines into a raw string until the next `container` block or EOF.
fn collect_raw_block(input: &str) -> (&str, &str) {
    let mut pos = 0;
    while pos < input.len() {
        let remaining = &input[pos..];
        let line_start = remaining.trim_start_matches([' ', '\t', '\n', '\r']);
        if line_start.starts_with("container ") {
            break;
        }
        if let Some(nl) = remaining.find('\n') {
            pos += nl + 1;
        } else {
            pos = input.len();
        }
    }
    (&input[..pos], &input[pos..])
}

fn parse_container_line(input: &str) -> IResult<&str, (String, String)> {
    let (input, _) = tag("container ")(input)?;
    let (input, _) = ws(input)?;
    let (input, _) = opt(tag("image"))(input)?;
    let (input, _) = ws(input)?;
    let (input, image) = take_until(" as ")(input)?;
    let (input, _) = tag(" as ")(input)?;
    let (input, _) = ws(input)?;
    let (input, name) = identifier(input)?;
    let (input, _) = opt(tag(":"))(input)?;

    Ok((input, (name, image.trim().to_string())))
}

fn parse_limit_line(input: &str) -> IResult<&str, ResourceLimit> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("limit to ")(input)?;
    let (input, memory) = take_until(",")(input)?;
    let (input, _) = tag(",")(input)?;
    let (input, _) = ws(input)?;
    let (input, cpus) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    Ok((
        input,
        ResourceLimit {
            memory: memory.trim().to_string(),
            cpus: cpus.trim().to_string(),
        },
    ))
}

fn parse_network_line(input: &str) -> IResult<&str, String> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("network ")(input)?;
    let (input, network) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    Ok((input, network.trim().to_string()))
}

fn parse_volume_line(input: &str) -> IResult<&str, Volume> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("map ")(input)?;
    let (input, host_path) = take_until(" as ")(input)?;
    let (input, _) = tag(" as ")(input)?;
    let (input, container_path) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    let container = container_path.trim();
    let (container_path, mode) = if let Some(pos) = container.rfind(':') {
        (container[..pos].to_string(), Some(container[pos + 1..].to_string()))
    } else {
        (container.to_string(), None)
    };

    Ok((
        input,
        Volume {
            container_path,
            host_path: host_path.trim().to_string(),
            mode,
        },
    ))
}

fn parse_expose_line(input: &str) -> IResult<&str, Vec<u16>> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("expose:")(input)?;
    let (input, _) = ws(input)?;
    let (input, ports_str) = take_until("\n")(input)?;
    let (input, _) = opt(line_ending)(input)?;

    let ports = ports_str
        .split(',')
        .filter_map(|p| p.trim().parse::<u16>().ok())
        .collect();

    Ok((input, ports))
}

fn parse_user_line(input: &str) -> IResult<&str, String> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("user: ")(input)?;
    let (input, user) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    Ok((input, user.trim().to_string()))
}

fn parse_depends_on_line(input: &str) -> IResult<&str, Vec<Dependency>> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("depends_on ")(input)?;
    let (input, deps_str) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    let deps = deps_str
        .split(',')
        .map(|d| {
            let d = d.trim();
            if d.ends_with(" healthy") {
                Dependency {
                    service: d.trim_end_matches(" healthy").to_string(),
                    condition: DependencyCondition::Healthy,
                }
            } else if d.ends_with(" started") {
                Dependency {
                    service: d.trim_end_matches(" started").to_string(),
                    condition: DependencyCondition::Started,
                }
            } else {
                Dependency {
                    service: d.to_string(),
                    condition: DependencyCondition::Started,
                }
            }
        })
        .collect();

    Ok((input, deps))
}

fn parse_healthcheck_line(input: &str) -> IResult<&str, HealthCheck> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("healthcheck with ")(input)?;
    let (input, test) = quoted_string(input)?;
    let (input, _) = line_ending(input)?;

    // Try to parse optional timing line (healthcheck after 10s every 30s repeat 3 wait 5s)
    let (input, timing) = match parse_healthcheck_timing_line(input) {
        Ok((rest, t)) => (rest, Some(t)),
        Err(_) => (input, None),
    };

    let (interval, timeout, retries, start_period) = timing.unwrap_or((None, None, None, None));

    Ok((
        input,
        HealthCheck {
            test,
            interval,
            timeout,
            retries,
            start_period,
        },
    ))
}

fn parse_healthcheck_timing_line(input: &str) -> IResult<&str, HealthCheckTiming> {
    // Lookahead: check if the next non-empty line has timing keywords
    let trimmed = input.trim_start_matches([' ', '\t']);
    let line_end = trimmed.find('\n').unwrap_or(trimmed.len());
    let next_line = &trimmed[..line_end];
    let timing_part = next_line
        .strip_prefix("healthcheck")
        .map(|s| s.trim())
        .unwrap_or(next_line);

    let has_timing = timing_part.contains("every ")
        || timing_part.contains("wait ")
        || timing_part.contains("repeat ")
        || timing_part.contains("after ");

    if !has_timing {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }

    let (input, _) = ws(input)?;
    let (input, _) = opt(tag("healthcheck "))(input)?;
    let (input, timing_str) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    let mut interval = None;
    let mut timeout = None;
    let mut retries: Option<u32> = None;
    let mut start_period = None;

    let tokens: Vec<&str> = timing_str.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "every" if i + 1 < tokens.len() => {
                interval = Some(tokens[i + 1].to_string());
                i += 2;
            }
            "wait" if i + 1 < tokens.len() => {
                timeout = Some(tokens[i + 1].to_string());
                i += 2;
            }
            "repeat" if i + 1 < tokens.len() => {
                retries = tokens[i + 1].parse().ok();
                i += 2;
            }
            "after" if i + 1 < tokens.len() => {
                start_period = Some(tokens[i + 1].to_string());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok((input, (interval, timeout, retries, start_period)))
}

fn parse_env_from_line(input: &str) -> IResult<&str, Vec<EnvSource>> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("env from ")(input)?;
    let (input, sources_str) = take_until("\n")(input)?;
    let (input, _) = opt(line_ending)(input)?;

    let mut sources = Vec::new();
    for s in sources_str.split(',') {
        if let Ok((_, source)) = parse_env_source(s.trim()) {
            sources.push(source);
        }
    }

    Ok((input, sources))
}

fn parse_env_source(input: &str) -> IResult<&str, EnvSource> {
    let raw = input.trim();

    // Strip ${...} wrapper if present → VarRef or File with multiple @
    let (inner, is_varref_syntax) = if raw.starts_with("${") && raw.ends_with('}') {
        (&raw[2..raw.len() - 1], true)
    } else {
        (raw, false)
    };

    if !is_varref_syntax && inner.contains('=') {
        // Inline: KEY=VALUE or *KEY=VALUE
        let (before, after) = inner.split_once('=').unwrap();
        let secret = before.trim().starts_with('*');
        let key = before.trim().trim_start_matches('*').to_string();
        return Ok(("", EnvSource::Inline {
            key,
            value: after.trim().to_string(),
            secret,
        }));
    }

    let at_count = inner.chars().filter(|&c| c == '@').count();

    if at_count >= 2 {
        // KEY@section@file  →  VarRef
        let last_at = inner.rfind('@').unwrap();
        let second_at = inner[..last_at].rfind('@').unwrap();
        let key = inner[..second_at].trim().to_string();
        let section = inner[second_at + 1..last_at].trim().to_string();
        let file = inner[last_at + 1..].trim().to_string();
        return Ok(("", EnvSource::VarRef {
            key,
            file,
            section: if section.is_empty() { None } else { Some(section) },
        }));
    }

    if at_count == 1 {
        // section@file  →  File
        let (name, file) = inner.split_once('@').unwrap();
        let section = if !name.trim().is_empty() { Some(name.trim().to_string()) } else { None };
        return Ok(("", EnvSource::File { file: file.trim().to_string(), section }));
    }

    Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
}

fn parse_serve_line(input: &str) -> IResult<&str, ServeConfig> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("serve ")(input)?;

    // Parse optional route name: "serve NAME as ..." vs "serve as ..."
    let (input, name) = if let Ok((rest, _)) =
        tag::<_, _, nom::error::Error<&str>>("as ")(input)
    {
        // No name — starts directly with "as "
        (rest, None)
    } else {
        let (rest, n) = identifier(input)?;
        let (rest, _) = tag(" as ")(rest)?;
        (rest, Some(n))
    };

    let (input, rest) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    // Split on optional " through " to separate "subdomain:port" from middlewares
    let (addr_str, mw_str) = if let Some(idx) = rest.find(" through ") {
        (&rest[..idx], Some(&rest[idx + " through ".len()..]))
    } else {
        (rest, None)
    };

    // Parse "subdomain:port" — split on last ':' to allow "." as subdomain.
    // If no ':' is present, treat as api@internal (Traefik dashboard, no port needed).
    let (subdomain, port, api_internal) = if let Some(colon) = addr_str.rfind(':') {
        let sub = addr_str[..colon].trim().to_string();
        let p: u16 = addr_str[colon + 1..].trim().parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
        })?;
        (sub, p, false)
    } else {
        // No port — api@internal serve (e.g. `serve as traefik`)
        (addr_str.trim().to_string(), 0u16, true)
    };

    let (middlewares, use_service) = if let Some(mw_rest) = mw_str {
        if let Some((mw_part, use_part)) = mw_rest.split_once(" use ") {
            let mws = mw_part.split(',').map(|m| m.trim().to_string()).collect();
            (mws, Some(use_part.trim().to_string()))
        } else {
            let mws = mw_rest.split(',').map(|m| m.trim().to_string()).collect();
            (mws, None)
        }
    } else {
        (vec![], None)
    };

    Ok((
        input,
        ServeConfig {
            name,
            subdomain,
            port,
            api_internal,
            middlewares,
            use_service,
        },
    ))
}

fn parse_route_add_line(input: &str) -> IResult<&str, RouteAdd> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("route_add ")(input)?;
    let (input, line) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    // Parse optional route_name prefix: "NAME key:value" vs "key:value"
    // If there's a space before the first ':', the first word is the route name.
    if let Some((before_colon, after_colon)) = line.split_once(':') {
        let before_colon = before_colon.trim();
        let value_str = after_colon.trim();

        let (route_name, key) = if let Some(space_pos) = before_colon.find(' ') {
            (
                Some(before_colon[..space_pos].to_string()),
                before_colon[space_pos + 1..].trim().to_string(),
            )
        } else {
            (None, before_colon.to_string())
        };

        let value = serde_yaml::from_str(value_str)
            .unwrap_or_else(|_| serde_yaml::Value::String(value_str.to_string()));

        Ok((input, RouteAdd { route_name, key, value }))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    }
}

fn parse_network_config_line(input: &str) -> IResult<&str, String> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("network-config from ")(input)?;
    let (input, file) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    Ok((input, file.trim().to_string()))
}

fn parse_raw_line(input: &str) -> IResult<&str, (String, serde_yaml::Value)> {
    let (input, _) = ws(input)?;
    let (input, _) = tag("raw ")(input)?;
    let (input, line) = take_until("\n")(input)?;
    let (input, _) = line_ending(input)?;

    if let Some((key, value)) = line.split_once(':') {
        let key = key.trim().to_string();
        let value_str = value.trim();
        let value = serde_yaml::from_str(value_str)
            .unwrap_or_else(|_| serde_yaml::Value::String(value_str.to_string()));
        Ok((input, (key, value)))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_service() {
        let input = "container image nginx:latest as web\n";
        let result = parse_service_file(input, "test").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "web");
        assert_eq!(result[0].image, "nginx:latest");
    }

    #[test]
    fn test_parse_serve_named() {
        let input = "container nginx:latest as app\n  serve web as health:8080\n";
        let result = parse_service_file(input, "test").unwrap();
        let serve = &result[0].serve[0];
        assert_eq!(serve.name, Some("web".to_string()));
        assert_eq!(serve.subdomain, "health");
        assert_eq!(serve.port, 8080);
    }

    #[test]
    fn test_parse_serve_unnamed() {
        let input = "container nginx:latest as app\n  serve as app:3000\n";
        let result = parse_service_file(input, "test").unwrap();
        let serve = &result[0].serve[0];
        assert_eq!(serve.name, None);
        assert_eq!(serve.subdomain, "app");
        assert_eq!(serve.port, 3000);
    }

    #[test]
    fn test_parse_serve_root_domain() {
        let input = "container nginx:latest as app\n  serve as .:80\n";
        let result = parse_service_file(input, "test").unwrap();
        assert_eq!(result[0].serve[0].subdomain, ".");
    }

    #[test]
    fn test_parse_route_add_named() {
        let input = "container nginx:latest as app\n  serve web as health:80\n  route_add web priority:1\n";
        let result = parse_service_file(input, "test").unwrap();
        let ra = &result[0].route_add[0];
        assert_eq!(ra.route_name, Some("web".to_string()));
        assert_eq!(ra.key, "priority");
    }

    #[test]
    fn test_parse_route_add_resolved_to_last_serve() {
        let input = "container nginx:latest as app\n  serve api as health:80\n  route_add rule: 'Host(`x`)'\n";
        let result = parse_service_file(input, "test").unwrap();
        // route_name should be resolved to "api" (last seen serve name)
        assert_eq!(result[0].route_add[0].route_name, Some("api".to_string()));
    }

    #[test]
    fn test_parse_end_raw_block() {
        let input = "container nginx:latest as app\n  network proxy\nend\nvolumes:\n  uploads: {}\n";
        let result = parse_service_file(input, "test").unwrap();
        assert!(result[0].raw_compose_block.is_some());
        assert!(result[0].raw_compose_block.as_ref().unwrap().contains("volumes:"));
    }
}
