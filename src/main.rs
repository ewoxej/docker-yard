use dyard::ProjectBuilder;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::{Command, exit};

#[derive(Parser)]
#[command(name = "dyard")]
#[command(about = "Infrastructure DSL for Docker + Traefik", long_about = None)]
struct Cli {
    /// Path to config.dyard (auto-discovers ./config.dyard if not specified)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate configuration files
    Generate {
        /// Force regeneration even if no changes
        #[arg(long)]
        regenerate: bool,
    },
    /// Start services (docker-compose up -d)
    Up {
        /// Detached mode
        #[arg(short)]
        d: bool,
        /// Force regeneration
        #[arg(long)]
        regenerate: bool,
    },
    /// Stop services (docker-compose down)
    Down {},
    /// Restart services (down + up)
    Restart {
        /// Detached mode
        #[arg(short)]
        d: bool,
    },
    /// View logs
    Logs {
        /// Service name (optional)
        service: Option<String>,
        /// Follow log output
        #[arg(short)]
        f: bool,
        /// Number of lines to show
        #[arg(long)]
        tail: Option<String>,
    },
    /// Show container status
    Ps {},
}

fn find_config(config: &Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = config {
        return Ok(path.clone());
    }

    // Look for config.dyard in current directory
    let default = PathBuf::from("config.dyard");
    if default.exists() {
        return Ok(default);
    }

    Err("config.dyard not found. Provide with --config flag or create config.dyard in current directory".to_string())
}

fn main() {
    let cli = Cli::parse();

    let config_path = match find_config(&cli.config) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("✗ {}", e);
            exit(1);
        }
    };

    match cli.command {
        Some(Commands::Generate { regenerate }) => {
            handle_generate(&config_path, regenerate);
        }
        Some(Commands::Up { d, regenerate }) => {
            handle_up(&config_path, d, regenerate);
        }
        Some(Commands::Down {}) => {
            handle_down(&config_path);
        }
        Some(Commands::Restart { d }) => {
            handle_restart(&config_path, d);
        }
        Some(Commands::Logs { service, f, tail }) => {
            handle_logs(&config_path, service, f, tail);
        }
        Some(Commands::Ps {}) => {
            handle_ps(&config_path);
        }
        None => {
            // Passthrough unknown commands to docker-compose
            passthrough_to_docker_compose();
        }
    }
}

fn handle_generate(config_path: &Path, regenerate: bool) {
    println!("📦 Building project from {}", config_path.display());

    match ProjectBuilder::build(config_path) {
        Ok(ctx) => {
            println!("✓ Project loaded: {} services, {} env files",
                ctx.project.services.len(),
                ctx.project.env_files.len());

            match ProjectBuilder::generate_outputs(&ctx, regenerate) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("✗ Generation failed: {}", e);
                    exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("✗ Build failed: {}", e);
            exit(1);
        }
    }
}

fn handle_up(config_path: &Path, detached: bool, regenerate: bool) {
    handle_generate(config_path, regenerate);
    println!("🚀 Starting services...");
    compose_with_file(config_path, &["up"], detached);
}

fn handle_down(config_path: &Path) {
    println!("Stopping services from {}", config_path.display());
    compose_with_file(config_path, &["down"], false);
}

fn handle_restart(config_path: &Path, detached: bool) {
    println!("Restarting services from {}", config_path.display());
    compose_with_file(config_path, &["down"], false);
    compose_with_file(config_path, &["up"], detached);
}

fn compose_with_file(config_path: &Path, cmd: &[&str], detached: bool) {
    let compose_file = match ProjectBuilder::build(config_path) {
        Ok(ctx) => {
            let root = config_path.parent().unwrap_or_else(|| std::path::Path::new("."));
            root.join(&ctx.project.config.output.compose)
        }
        Err(_) => {
            let build_dir = config_path.parent().unwrap_or_else(|| std::path::Path::new("."));
            build_dir.join("build/docker-compose.yml")
        }
    };
    let file_path = compose_file.to_str().unwrap_or("build/docker-compose.yml");

    let mut args = vec!["compose", "-f", file_path];
    args.extend_from_slice(cmd);
    if detached {
        args.push("-d");
    }
    run_docker_command(&args);
}

fn handle_logs(_config_path: &PathBuf, service: Option<String>, follow: bool, tail: Option<String>) {
    let mut args = vec!["compose", "logs"];

    if follow {
        args.push("-f");
    }

    if let Some(t) = &tail {
        args.push("--tail");
        args.push(t.as_str());
    }

    if let Some(svc) = &service {
        args.push(svc.as_str());
    }

    run_docker_command(&args);
}

fn handle_ps(config_path: &Path) {
    println!("Status from {}", config_path.display());

    run_docker_command(&["compose", "ps"]);
}

fn passthrough_to_docker_compose() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = Command::new("docker");
    cmd.arg("compose").args(&args);
    execute_command(cmd);
}

fn run_docker_command(args: &[&str]) {
    let mut cmd = Command::new("docker");
    for arg in args {
        cmd.arg(arg);
    }
    execute_command(cmd);
}

fn execute_command(mut cmd: Command) {
    let status = cmd.status().expect("Failed to run command");
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}
