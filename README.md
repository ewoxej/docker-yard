# dyard

Infrastructure DSL for Docker Compose + Traefik. Generates `docker-compose.yml`, Traefik configs, and `.env` files from a concise declarative syntax.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/ewoxej/dyard/main/install.sh | sh
```

Or download the binary from [Releases](https://github.com/ewoxej/dyard/releases) and put it on your `PATH` manually.

## Usage

```bash
dyard generate              # generate configs (skip if unchanged)
dyard generate --regenerate # force full regeneration

dyard up -d                 # generate + docker compose up -d
dyard up -d traefik         # start one service
dyard up -d web api db      # start several services

dyard down                  # docker compose down
dyard down traefik authelia # stop specific services

dyard restart traefik       # down + up for a service

dyard logs -f nextcloud     # follow logs
dyard ps                    # container status
```

`dyard` looks for `config.dyard` in the current directory. Use `--config <path>` to point elsewhere.

## VS Code Extension

Syntax highlighting and snippets for `.dyard` and `config.dyard` files.

### Install from VSIX (Releases)

1. Download `dyard-syntax-*.vsix` from [Releases](https://github.com/ewoxej/dyard/releases).
2. Install it:

   **VS Code UI** — Extensions sidebar (`Ctrl+Shift+X`) → `···` menu → *Install from VSIX…* → pick the file.

   **Command line:**
   ```bash
   code --install-extension dyard-syntax-*.vsix
   ```

3. Reload VS Code. `.dyard` files will have syntax highlighting.

### Uninstall

```bash
code --uninstall-extension dyard.dyard-syntax
```

## Build from source

Requires Rust and the `x86_64-unknown-linux-musl` target.

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Binary: `target/x86_64-unknown-linux-musl/release/dyard`
