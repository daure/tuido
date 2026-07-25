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

## Build and install

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
