Load architecture.md before starting any work.
Always load ~/dev/tuicore/SKILL.md on startup.
No PRODUCT.md or DESIGN.md required.

Persistence: tasks are stored in SQL through SQLx. Default DB is SQLite unless TUIDO_DATABASE_URL points elsewhere; Postgres is supported by config. Run migrations from migrations/ before app use. Task description edits, size, state, and entities save immediately.

Service boundary: `TuidoService` is public application API and database authority. TUI, MCP, and future adapters call it for every persistence mutation. Adapters must not issue SQL or reproduce domain mutation rules.

Concurrency: persisted tasks, people, workspaces, and tags carry revisions. Updates/deletes require expected revision and return typed conflicts. Relation replacement and entity/workspace revision increments belong to one transaction. Never bypass conditional writes.

Refresh: TUI polls workspace revision, reloads external changes, and preserves selected IDs/focus where still valid. Never replace view selection by row index during refresh. Do not refresh over pending optimistic writes.

MCP: stdio stdout is protocol-only; diagnostics go to stderr. HTTP binds loopback only. Default endpoint is `127.0.0.1:7345/mcp`. No automatic daemon; clients spawn stdio by default.

Migrations: whenever the domain model changes, explicitly assess whether persisted schema or data must change and add a migration when required. Schema changes go in new ordered files under `migrations/` and must work for SQLite and Postgres. SQLite connections enable foreign keys, busy timeout, and WAL where supported.

Lifecycle: `serve` stays foreground. Linux lifecycle uses systemd-user; macOS uses launchd; unsupported platforms return explicit errors. Install/uninstall should tolerate repeated use where practical. Never claim authentication that transport does not enforce.
