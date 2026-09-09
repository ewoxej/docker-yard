# dyard Infrastructure DSL Specification

Document describes the format and translation rules for **dyard** — a language for describing server infrastructure based on Docker + Traefik.

Components:
- **Service-DSL** (`.dyard` files in `containers/`) → docker-compose services
- **Env-DSL** (`*.env` files) → environment variables and secrets
- **Config-DSL** (`config.dyard` in root) → global parameters, generation paths, defaults
- **Output** (generated `build/`) → docker-compose.yml, Traefik configs, .env files

Status: **v1.0 (complete and tested)**. All features listed below are implemented and working.

---

## 1. Lexical Rules

Applied to all DSL variants.

- **UTF-8 encoding**. One logical instruction per line (no line continuations).
- **Comments**: `#` to end of line. `#` inside quotes (`"..."` or `'...'`) is literal.
- **String values**: can be single or double quoted. Quotes required if value contains space, comma, `#`, or `@`.
- **Lists**: comma-separated. Whitespace around commas ignored.
- **Empty lines** ignored.
- **Keyword case**: lowercase required. Values are case-sensitive.

### 1.1 Variable Substitution — Stages and Sources

Variable reference is always marked with `@` (keyword `from` not used).

| Form | Resolved by | Result |
|---|---|---|
| `${VAR}` | **docker-compose** at runtime | remains `${VAR}` in output (value from root `.env`) |
| `${VAR@file.env}` | **parser** at generation time | value from implicit (root) section of file |
| `${VAR@section@file.env}` | **parser** at generation time | value from specific section |

**Rationale**: `${DATA_PATH}`, `${DOMAIN}` should reach docker-compose (runtime substitution), while cross-file references that Docker doesn't understand are expanded by parser at generation time.

**Secrets modify substitution rules** (§3.5). If `VAR` is marked as secret, parser does not write value directly to output even at generation time. Instead, it passes the value through `.env` file. Details in §3.5.

**Generation error** if `${...@...}` points to non-existent file/section/variable. Parser builds a reference graph and exits with clear message (`file.env:LINE: unknown var X`), not broken output.

---

## 2. Service-DSL (`.dyard` files)

File = list of **blocks**. Block starts with `container ...` line and continues until next `container` or EOF. One block = one compose service.

### 2.1 Service Declaration

```
container image <image-ref> as <name>
```

- `image` — optional noise word (can be omitted)
- `<image-ref>` — full image reference (`ghcr.io/...:tag`)
- `<name>` — service key and container_name
- Trailing `:` after name allowed and ignored (`as immich-ml:`)

→ generates:
```yaml
services:
  <name>:
    image: <image-ref>
    container_name: <name>
```

### 2.2 Block Directives

| DSL | compose | Notes |
|---|---|---|
| `limit to 512m, 1cpu` | `mem_limit: 512m` / `cpus: 1` | order not fixed, can be reversed |
| `network <name>` | adds `<name>` to service `networks:` | optional if only one network (auto-added); one line per network |
| `map <container_path> as <host_path>[:mode]` | `volumes: - <container_path>:<host_path>[:mode]` | see 2.3 |
| `expose: N` | `expose: - "N"` | |
| `user: "1000:1000"` | `user: "1000:1000"` | passthrough |
| `depends_on <svc> [healthy]` | see 2.6 | |
| `healthcheck with "..."` + timing line | see 2.5 | uses `with` instead of `:` |
| `env from ...` | see 2.7 | |
| `serve [name] as <subdomain>:<port>` | Traefik route, see 2.4 | named routes for multi-serve; see §2.4 |
| `route_add [name] <path>: <value>` | Traefik dynamic config override | see §2.4.4; `@` = base rule |
| `raw <field>: <value>` | YAML passthrough | see 2.8 |
| `end` | ends service block | optional raw Docker YAML follows until next `container` |

### 2.3 Volumes (`map`)

Intentional "what maps to where" order:

```
map <container_path>[:mode] as <host_path>
```

- Left side = **container path**, right side = **host path** (docker-like order)
- `mode` (`ro`/`rw`) specified at container path but moved to end in docker output:
  `map /config:rw as ${DATA_PATH}/config` → `- /config:rw:${DATA_PATH}/config`
- `${...}` in paths follow substitution rules (§1.1)

### 2.4 Traefik Routing (`serve`)

#### 2.4.1 Syntax

```
serve as <subdomain>:<port>
serve as <subdomain>:<port> through <mw1>, <mw2>
serve <name> as <subdomain>:<port> through ...
serve as traefik
```

- `<subdomain>` — subdomain prefix; route rule becomes `Host(<subdomain>.<DOMAIN>)`
- `<subdomain>` = `.` — root domain; rule becomes `Host(<DOMAIN>)` with no prefix
- `<port>` — container's internal port for the Traefik load balancer
- `<name>` — optional route name; **required** when a container has multiple `serve` directives
  - Output router name: `{container_name}-{name}` (e.g. `health-api`)
  - Without name: router named after subdomain (or service name for root domain)
- `through` — optional comma-separated list of middleware names
  - Middleware name is used directly as the Traefik reference (e.g. `cloudflare-hsts`)
  - Middleware definitions live in the `network/` directory and are copied to `conf.d/` automatically (§6.1)
- `serve as traefik` — special form without `:port`; Traefik service is set to `api@internal` (exposes the Traefik API/dashboard). Only meaningful in the router container.

**Single serve (unnamed):**
```
serve as app:3000 through cors
```

**Multiple serves (names required):**
```
serve web as health:8080 through cloudflare-hsts, auth-flow
route_add web priority:1
serve api as health:8080 through cloudflare-hsts
route_add rule: '@ && HeaderRegexp(`Authorization`, `^Bearer `)'
route_add api priority:100
```

**Root domain:**
```
serve as .:443
```

**Traefik API/dashboard:**
```
container traefik:latest as traefik
  serve as traefik through cloudflare-hsts
  route_add rule: 'Host(`traefik.example.com`)'
```

#### 2.4.2 Automatic Traefik Network Assignment

When a service declares `serve as ...`, the router container (`router_container` from config.dyard) is automatically added to all networks that service belongs to. This ensures Traefik can reach the service without manual network configuration on the traefik container.

Example: if `app` belongs to networks `proxy` and `internal`, and `app` has `serve as app:3000`, then `traefik` is automatically added to `proxy` and `internal` in the generated docker-compose.yml.

#### 2.4.3 Generation

Parser generates Traefik dynamic config (path from config.dyard):

```yaml
http:
  routers:
    app:  # name = <subdomain>
      rule: Host(`app.${DOMAIN}`)
      entryPoints: [https]  # from config.dyard defaults
      service: app-service  # auto-generated
      middlewares: [cors]   # from through clause, name only
      tls:
        certResolver: letsencrypt  # from config.dyard
  services:
    app-service:  # auto-generated
      loadBalancer:
        servers:
          - url: http://<name>:<port>
```

**Key points:**
- Router name = `<subdomain>`
- Service name = `<subdomain>-service` (auto-generated)
- Entry points and cert resolver from config.dyard defaults (§4)
- Parser uses file-provider (file-based YAML), not docker labels
- Middlewares referenced by bare name — definitions come from `_*.yml` files in `network/` (§6.1)
- `serve as traefik` (no port): generates `service: api@internal` instead of a load balancer entry

#### 2.4.4 `route_add` — Dynamic Config Overrides

`route_add` modifies generated Traefik configuration using dotted-path keys.

**For the router container** (e.g. `traefik`) — adds to **static config** (`traefik.yml`):
```
route_add entryPoints.web.address: "0.0.0.0:80"
route_add entryPoints.websecure.proxyProtocol.trustedIPs: ["10.0.0.1/32"]
```

**For regular services** — adds to the router entry in **dynamic config** (`http.yml`):
```
route_add [name] <key>: <value>
```

- `[name]` — optional route name (matches `serve <name> as ...`). If omitted, applies to the most recently declared `serve` in the block.
- Key `rule` with `@` placeholder: `@` is replaced by the base `Host(...)` rule at generation time:
  ```
  route_add rule: '@ && HeaderRegexp(`Authorization`, `^Bearer `)'
  # → rule: "Host(`health.domain`) && HeaderRegexp(`Authorization`, `^Bearer `)"
  ```
- Supports dotted paths for nested fields: `tls.certResolver`, `priority`, etc.

### 2.5 Healthcheck

Two lines: command and timing.

```
healthcheck with "<test-command>"
healthcheck after 10s every 30s repeat 3 wait 5s
```

Fixed keyword mapping:

| Keyword | compose field |
|---|---|
| (quoted string) | `test` (wrapped in `CMD-SHELL`) |
| `every <d>` | `interval` |
| `wait <d>` | `timeout` |
| `repeat <n>` | `retries` |
| `after <d>` | `start_period` |

- Word order in timing line is flexible; each keyword optional
- Leading `healthcheck` in timing line allowed and ignored

### 2.6 Dependencies

```
depends_on <svc> [healthy]
depends_on <svc1> healthy, <svc2> started
```

- Without `healthy`/`started` → `condition: service_started`
- With `healthy` → `condition: service_healthy`
- `<svc>` **must** match `<name>` of some service (§2.1). Otherwise: generation error.

### 2.7 Environment (`env from`)

Comma-separated list. Each element is one of:

1. **Inline variable**: `TEST=1`, `TZ='Europe/Budapest'` → normal variable. `*NAME=value` — inline secret (§3.5)
2. **File whole**: `@filename.env` → includes implicit (root) section
3. **File section**: `sectionname@filename.env` → includes specific section (§3)

Rules:
- File reference always contains `@`. Form `name@file.env` = section; form `@file.env` = root section. Bare token without `=` and without `@` is error.
- List resolution: left to right, last value wins

#### 2.7.1 Materialization Model

Where resolved variables go depends on secrets presence:

- **No secrets** → written as literals in service `environment:` block (`KEY: value`). Runtime reference `${VAR}` in value stays as `${VAR}` (§1.1)
- **Has secrets** → parser generates `env/<service>.env` and connects via `env_file:`. **All** variables (normal and secrets) go there under real names. In compose, instead of literal:
  ```yaml
  environment:
    ORDINARY: value
    SECRET_VAR: ${SECRET_VAR}  # value from env_file
    NESTED: prefix/${SECRET_VAR}/suffix
  ```

### 2.8 Passthrough Keys (`raw`)

Raw YAML lines passed without transformation, marked with `raw`:

```
raw security_opt: [apparmor:docker-default, label:type:container_t]
raw pid: host
raw tmpfs: ["/tmp:rw,size=50m"]
raw cap_drop: [ALL]
```

Parser copies content after `raw` directly into service YAML. Escape hatch for unsupported compose options.

### 2.9 Service Defaults

- `restart: unless-stopped` added to **every** service automatically. Can be overridden by global config (§4)
- Top-level `networks:` collected by parser from all `network` directives across all blocks

### 2.10 Raw Compose Blocks (`end`)

`end` is an optional explicit block terminator. Any YAML after `end` — until the next `container` or EOF — is **deep-merged** into the top-level docker-compose output. This allows declaring volumes, networks, or additional compose keys without modifying the generator.

```
container app:latest as api
  network proxy
  serve as api:3000
end
volumes:
  api-uploads:
    driver: local
networks:
  proxy:
    external: true

container redis:7 as cache
  ...
```

**Merge behaviour:**
- Mapping keys are merged recursively (generated values and raw values coexist)
- Non-mapping values from the raw block overwrite generated values
- `services:`, `volumes:`, `networks:` sections are deep-merged — adding entries rather than replacing

---

## 3. Env-DSL (`*.env` files)

### 3.1 Sections

```
[common]
KEY=value
*SECRET_NAME=12345
[db extends common]
...
[app extends common, redis@redis.env]
...
```

- Lines before first `[...]` — implicit "root" section
- `[name]` — section declaration
- Inside section: `KEY=value` lines, secrets `*KEY=value` (§3.5)

### 3.2 Inheritance (`extends`)

```
[<name> extends <src1>, <src2>, ...]
```

Each `<src>`:
- same-file section: `common`
- other-file section: `section@filename.env`
- whole other file (root section): `@filename.env`

- Resolution: all `extends` left-to-right (later overrides earlier), then own section lines (override inherited)
- Cycles in `extends` — generation error

### 3.3 Aliases (`as`)

```
DB_HOSTNAME as POSTGRES_HOST
```

Duplicates **value** under new key. Works after `extends` resolution. Requires source key to exist. **Secrecy inherited**: secret alias is also secret (§3.5).

### 3.4 Cross-file References

`${VAR@file.env}` and `${VAR@section@file.env}` in values — resolved by parser at generation time (§1.1).

```
OVERWRITEHOST=${NEXTCLOUD_URL@config.dyard}
```

If `VAR` is secret, substitution follows secret rules (§3.5), not literal.

### 3.5 Secrets

Variable declared with leading `*` — **secret**. The `*` is stripped from key name (`*SECRET_NAME=12345` → key `SECRET_NAME`).

**Secrecy = data flow taint.** Spreads to all derivatives:
- value inherited via `extends`
- alias `as` of secret
- any variable/field where secret was substituted into

Marked-as-secret value **never** written as literal to output files. Single mechanism:

1. Parser generates `env/<service>.env` file for service (if any secret exists) and adds `env_file: - env/<service>.env`
2. Secrets written **under real names**: `KEY=<value>`. Applies to service containers and traefik
3. Everywhere in output (compose `environment`, Traefik dynamic config, etc.) secret substituted as `${KEY}` — docker/Traefik take value from environment (from `env_file`)

Thus:
- compose file contains no secret plaintext
- Traefik dynamic configs — only `${KEY}`
- Secret values live only in generated `env/<service>.env` (and root `.env` for system vars)

**Protection boundary**: secrets absent from compose/configs (git-safe), but in `env/` files and container runtime visible plaintext.

Generated `env/<service>.env` and root `.env` — most sensitive files: require `.gitignore` and `chmod 600`.

---

## 4. Config-DSL (`config.dyard`)

Single file in root, defines:
- Input paths (`input`) — where to read `.dyard` files, env
- Output paths (`output`) — where to write compose, traefik configs, env-files
- Router container (`router_container`) — which container handles Traefik (by name)
- Global variables (`DOMAIN`, `DATA_PATH`, etc.) — available everywhere via `${VAR}`
- Traefik defaults (`traefik`) and service defaults (`defaults`) — applied to all routes and containers

### 4.1 Structure

```
input:
  containers: containers/
  env: env/

output:
  compose: ./build
  traefik: ./build/traefik
  env: ./build

router_container: traefik

DOMAIN: example.local
DATA_PATH: /data
EMAIL: admin@example.local

traefik:
  entryPoints:
    - https
  certResolver: letsencrypt

defaults:
  restart: unless-stopped
```

### 4.2 Variables and Defaults

- **Global variables**: `DOMAIN: value` → available everywhere via `${DOMAIN}`
- **Traefik defaults** (`traefik` block) — `entryPoints`, `certResolver`, etc. applied to all `serve` directives unless overridden
- **Service defaults** (`defaults` block) — `restart` and other options for all containers

### 4.3 Traefik Configuration

Traefik uses two types:

**Static config** (`traefik.yml`):
- Generated by parser from defaults and `route_add` directives
- Contains entry points, API settings, global options
- Mounted into container at fixed path

**Dynamic configs** (`conf.d/http.yml`):
- Generated from `serve` directives in service blocks
- Router definitions with rules, services, middlewares
- Auto-loaded by Traefik file provider

---

## 5. Validation Rules (parser must check)

1. `${VAR@[section@]file.env}` — file, section, variable exist
2. `${VAR@config.dyard}` — variable exists in config
3. `depends_on <svc>` — service with that name exists
4. `serve as <subdomain>` — port specified, rule generates correctly
5. `env from` — each element valid format, files exist
6. `extends` — sources exist, no cycles
7. `as` alias — source key exists after `extends` resolution
8. **If secret in `env from`, parser must generate `env/<service>.env`** and connect via `env_file:` — §3.5
9. Router container specified in config exists in services

---

## 6. Project Structure

```
project/
├── config.dyard              # Global config (§4)
├── containers/               # Service-DSL files (.dyard)
│   ├── traefik.dyard
│   ├── app.dyard
│   └── ...
├── env/                      # Env files (optional, user-defined)
│   ├── app.env
│   └── ...
├── network/                      # Traefik network config templates (optional)
│   ├── _redirect.yml             # Copied to conf.d/ if starts with _
│   ├── cors.yml                  # Middleware definitions (referenced in serve as ... through)
│   └── ...
└── build/ (or path from config.dyard)
    ├── docker-compose.yml        # Generated compose
    ├── .env                      # Root .env (secrets and globals)
    ├── .dyard-cache              # Hash of input files (incremental regen)
    └── traefik/
        ├── conf.d/
        │   ├── http.yml          # Generated routes
        │   ├── cors.yml          # Copied from network/ via `through`
        │   └── _redirect.yml     # Copied from network/ (template pattern)
        └── traefik.yml           # Generated static config
```

---

## 6.1 Network Configuration Templates

The `network/` directory holds user-authored YAML files for Traefik configuration.

### Automatic copying

All files matching `_*.yml` pattern in `network/` are automatically copied to `build/traefik/conf.d/` during generation. This is the recommended way to add custom Traefik configurations (middlewares, services, routers) that are not generated by dyard.

```
network/
├── _redirect.yml          → copied to conf.d/_redirect.yml
├── _auth.yml              → copied to conf.d/_auth.yml
└── cors.yml               # not auto-copied (no _ prefix)
```

### Per-service network config

A service can specify an additional file to copy using `network-config from`:

```
container nginx:latest as app
  network-config from network/app-extra.yml
```

For a **regular service**, the file is copied to `conf.d/` (dynamic config).

For the **router container** (traefik), `network-config from` designates the **static config file** — it is copied to `build/traefik/` (not `conf.d/`) and acts as `traefik.yml` supplement:

```
container traefik:latest as traefik
  network-config from static.yml
```

---

## 7. CLI Commands

```bash
# Generate configs (if needed) and start containers
dyard up -d

# Same, but force regenerate
dyard up -d --regenerate

# Stop containers
dyard down

# Regenerate configs only
dyard generate

# View logs (with follow mode)
dyard logs [service]
dyard logs -f [service]
dyard logs --tail 100 [service]

# Container status
dyard ps

# Restart containers
dyard restart -d
```

If `config.dyard` not found, command searches in current directory (auto-discovery like docker-compose).

### Incremental Regeneration

dyard tracks a hash of all input files (containers, env files, network templates) in `.dyard-cache`. On the next `generate` or `up`, if no input files changed, generation is skipped:

```
✓ No changes detected, skipping regeneration (use --regenerate to force)
```

To force a full regeneration regardless of cache:

```bash
dyard generate --regenerate
dyard up -d --regenerate
```

---

## 8. Examples

### 8.1 Full Service-DSL Block

**containers/app.dyard:**
```
container image nginx:latest as app
limit to 512m, 1cpu
network app-net
expose: 8080
serve as app:8080
route_add entryPoints.websecure: true
depends_on db healthy
healthcheck with "curl -f http://localhost/health || exit 1"
healthcheck after 10s every 30s repeat 3 wait 5s
env from APP_ENV=production, *DB_PASSWORD=secret123
map /app/config as /data/app-config:ro
```

Generated:
- Docker-compose service `app` with volume, env, healthcheck
- Traefik router `app` with service `app-service`, port 8080
- Env-file `env/app.env` with variables (if secrets present)

### 8.2 Config.dyard Example

```
DOMAIN: example.com
EMAIL: admin@example.com
DATA_PATH: /var/lib/dyard

input:
  containers: containers/
  env: env/

output:
  compose: ./build
  traefik: ./build/traefik

router_container: traefik

traefik:
  entryPoints:
    - https
  certResolver: letsencrypt

defaults:
  restart: unless-stopped
```

### 8.3 Env File with Sections and Secrets

**env/app.env:**
```
[common]
DB_HOST: localhost
DB_USER: appuser

[app extends common]
DEBUG: true
*DB_PASSWORD: secretpass123
```

Usage: `env from app.env@app`
- Variables from section `app` (including inherited from `common`)
- Secrets automatically go to `env/<service>.env`

---

## 9. Implementation Notes

**Current version (v1.0):**
- ✅ Service-DSL parsing with all directives
- ✅ Env-DSL with sections, inheritance, secrets
- ✅ Config-DSL with global variables and defaults
- ✅ Docker-compose generation
- ✅ Traefik dynamic config generation
- ✅ Environment variable substitution (two-stage)
- ✅ Secret handling via env_file
- ✅ CLI with auto-discovery of config.dyard
- ✅ `serve as ... through mw@file` middleware references
- ✅ Automatic Traefik network assignment for serve-enabled services
- ✅ Network config templates (`_*.yml` auto-copy from `network/`)
- ✅ Per-service `network-config from <file>` directive
- ✅ Incremental regeneration with `.dyard-cache` hash
- ✅ Healthcheck two-line syntax (command + timing)
