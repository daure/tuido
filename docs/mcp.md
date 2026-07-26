# Tuido MCP

Tuido exposes full create/read/update/delete tools for tasks, people, projects, and tags/labels. Task tools also set state, complete, reject, snooze, and unsnooze. Create, update, and action mutation responses and list/get responses include latest entity revisions. Deletion confirmations include only the deleted entity and ID; callers should refresh the workspace after cascading deletes. Pass latest `expected_revision` to every update, action, or delete; stale writes return revision conflicts.

Task `state` is the user-facing **Status** (`todo`, `in_progress`, `snoozed`, `done`, or `rejected`). `people_ids` links people involved in the task besides the workspace owner; it does not represent assignment or ownership. Entity and workspace revisions are internal synchronization and optimistic-concurrency tokens. Clients should use them for mutations and refresh detection, but should not show them as ordinary task metadata unless the user requests them.

Task create, update, and read payloads use `description` for free-form task context.

`get_workspace` filters its task collection while always returning the complete people, project, and tag catalogs. This gives clients the available relation IDs needed to create or update tasks without separate catalog requests.

## Stdio (recommended)

Configure MCP client to spawn Tuido:

```json
{
  "command": "tuido",
  "args": ["mcp"]
}
```

Stdout carries MCP protocol only. `TUIDO_DATABASE_URL` selects database; otherwise Tuido uses SQLite under user data directory.

## Development HTTP

`tuido dev` runs TUI and Streamable HTTP MCP at `http://127.0.0.1:7346/mcp`. Both share database-backed `TuidoService`; TUI observes MCP writes by workspace-revision polling. The separate development port allows the installed service to keep running on `127.0.0.1:7345`; override the development address with `--bind` when needed.

## Foreground and installed service

`tuido serve` runs loopback HTTP in foreground until signal/session ends. Override loopback address with `--bind`.

```text
tuido service install
tuido service start
tuido service stop
tuido service uninstall
```

Linux uses systemd user services. macOS uses launchd. Windows lifecycle is unsupported; run `tuido serve` manually.

At install time Tuido copies current `TUIDO_DATABASE_URL` into generated systemd or launchd definition and restricts definition to owner (`0600`). This preserves selected workspace across terminal sessions and prevents database credentials in URL from becoming group/world-readable. Re-run `tuido service install` after changing URL. When variable is unset during install, service resolves same default local SQLite path as other Tuido processes.

Current `rmcp` HTTP integration has no clean bearer middleware in this package. HTTP therefore stays loopback-only and installed service relies on local OS account isolation. No bearer authentication is claimed or accepted.
