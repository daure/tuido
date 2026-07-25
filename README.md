# Tuido

Keyboard-first task manager with optional MCP access.

## Development

```bash
# TUI only
cargo run

# TUI + HTTP MCP at http://127.0.0.1:7345/mcp
cargo run -- dev

# stdio MCP only
cargo run -- mcp

# foreground HTTP MCP only
cargo run -- serve
```

MCP client configuration:

```json
{"command":"cargo","args":["run","--","mcp"]}
```

Set `TUIDO_DATABASE_URL` to use another SQLite database or Postgres. Otherwise Tuido uses its default local SQLite database.
Default data locations follow each platform: `$XDG_DATA_HOME/tuido` (or
`~/.local/share/tuido`) on Linux, `~/Library/Application Support/tuido` on macOS,
and the local application-data directory on Windows. Path overrides
`XDG_DATA_HOME`, `TUIDO_CONFIG_DIR`, and `TUIDO_MIGRATIONS_DIR` must be absolute.
UI config files are optional and load from the platform config directory under
`tuido`; set `TUIDO_CONFIG_DIR` to select another absolute directory. Existing
`TUICORE_CONFIG_DIR` or `~/.tuicore` files take precedence for compatibility.

## Install

On another machine:

```bash
cargo install tuitodo --locked
```

Update later:

```bash
cargo install tuitodo --locked --force
```

## Build from source

```bash
cargo test
cargo build --release
cargo install --path .
```

After installation:

```bash
tuido             # TUI
tuido mcp         # stdio MCP
tuido dev         # TUI + HTTP MCP
tuido serve       # foreground HTTP MCP
```

Installed MCP client configuration:

```json
{"command":"tuido","args":["mcp"]}
```

## Persistent MCP service

```bash
tuido service install
tuido service start
tuido service stop
tuido service uninstall
```

Service lifecycle supports Linux systemd-user and macOS launchd. HTTP stays loopback-only. Run `tuido --help` for details.
`service install` snapshots current `TUIDO_DATABASE_URL` into owner-readable service definition (mode `0600`) so background service and interactive clients keep using same configured workspace. Re-run install after changing database URL. If variable is unset during install, service uses normal default local SQLite path.

Postgres compatibility test is intentionally ignored by default. Run it explicitly against disposable database:

```bash
TUIDO_TEST_POSTGRES_URL=postgres://... cargo test --test postgres_service -- --ignored
```

## Release

Requires a clean Git tree and crates.io credentials from `cargo login`.
Run `cargo patch`, `cargo minor`, or `cargo major`. Release workflow checks crates.io
for latest stable Tuicore and release-version availability, updates dependency and
lockfile when needed, then runs tests, package validation, and publish dry-run.
After validation it shows exact versions and asks once before commit, tag, and live
publish. It never pushes; follow printed push commands after successful publication.
