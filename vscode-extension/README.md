# dyard DSL Syntax Highlighting

Visual Studio Code syntax highlighting extension for the dyard infrastructure DSL.

## Features

✨ **Syntax Highlighting** for:
- `.dyard` files (Service-DSL)
- `config.dyard` (Configuration files)
- `.env` files with sections

🎯 **Language Support**:
- Keywords highlighting (container, network, limit, expose, serve, etc.)
- Variable references (`${VAR}`, `${VAR@file.env}`)
- Comments (`# comment`)
- String literals
- Numbers and memory units (512m, 1g, 1cpu)
- Secret markers (*)

📝 **Code Snippets** for quick service definitions:
- `container` - Basic container definition
- `service-net` - Service with network
- `service-health` - Service with healthcheck
- `depends` - Service with dependencies
- `volumes` - Service with volume mapping
- `env-var` - Service with environment variables
- `traefik-route` - Service with Traefik routing

## Installation

### From VSIX (recommended)

1. Download `dyard-syntax-*.vsix` from the [Releases page](https://github.com/ewoxej/docker-yard/releases).
2. Install:

   **VS Code UI** — Extensions sidebar (`Ctrl+Shift+X`) → `···` menu → *Install from VSIX…* → pick the file.

   **Command line:**
   ```bash
   code --install-extension dyard-syntax-*.vsix
   ```

3. Reload VS Code.

### Uninstall

```bash
code --uninstall-extension dyard.dyard-syntax
```

## Usage

### Service Definition (.dyard)

```dyard
# Web service
container image nginx:latest as web
limit to 512m, 1cpu
network: app-net
expose: 8080
healthcheck interval 30s timeout 10s retries 3 start_period 40s
serve as app:8080
```

### Configuration (config.dyard)

```dyard
DOMAIN: example.local
EMAIL: admin@example.local

input:
  containers: ./containers
  env: ./env

output:
  compose: ./build

router_container: traefik

traefik:
  entryPoints:
    - http
    - https
  certResolver: letsencrypt
```

### Environment File (.env)

```env
[database]
DB_HOST: localhost
DB_PORT: 5432
*DB_PASSWORD: secret123

[app]
DEBUG: true
LOG_LEVEL: info
```

## Snippets

Use snippets to quickly create service definitions:

- Type `container` + Enter for basic container
- Type `service-health` + Enter for healthcheck
- Type `traefik-route` + Enter for Traefik routing

## Customization

To customize colors, add this to your `settings.json`:

```json
"editor.tokenColorCustomizations": {
  "[Your Theme]": {
    "variable.other.constant.dyard": "#FF6B6B",
    "keyword.dyard": "#4ECDC4",
    "string.dyard": "#95E1D3"
  }
}
```

## License

MIT

## Support

For issues or feature requests, visit the [dyard repository](https://github.com/yourusername/dyard)
