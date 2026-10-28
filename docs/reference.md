# Tuido reference

[Overview](../README.md) · [Development](#development) · [Installation and updates](#install) · [MCP service](#persistent-mcp-service) · [Releasing](#release)

Keyboard-first task manager with optional MCP access.

## Development

```bash
# TUI only
cargo run

# TUI + HTTP MCP at http://127.0.0.1:7346/mcp
cargo run -- dev

# stdio MCP only
cargo run -- mcp

# foreground HTTP MCP only
cargo run -- serve
```

MCP client configuration:

```json
{"type":"remote","url":"http://127.0.0.1:7346/mcp","enabled":true}
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

Prebuilt releases support **Ubuntu 24.04 or newer on x86_64**. Rust is not required. Download and run the installer from the [latest GitHub Release](https://github.com/daure/tuido/releases/latest):

```bash
installer="$(mktemp)"
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/daure/tuido/releases/latest/download/tuitodo-installer.sh \
  -o "$installer" && sh "$installer"
rm -f "$installer"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
tuido --help
```

The installer places `tuido` in `$CARGO_HOME/bin` (default `~/.cargo/bin`). Add that directory to your shell's PATH permanently if needed. Archives use the Cargo package name `tuitodo`; the executable is `tuido`. The Releases page also provides a `.tar.xz` archive and SHA-256 checksum for manual installation; the latest 30 stable releases remain available for rollback.

### Update

Rerun the installer commands above to update. After validating the download, the installer lists outdated processes from the destination installation and asks before stopping them. Declining or lacking an interactive terminal aborts the upgrade before executable replacement. Approval allows termination of active TUIs and MCP sessions, including forced termination of unresponsive processes.

The installer stops associated user services before replacement and starts them afterward, preserving their configuration and enablement. It also attempts to restore stopped services after an interrupted installation. A failed restart or remaining outdated process fails the installation. Identical builds keep running; separate installations and development checkouts remain independent.

Reopen stopped TUIs and reconnect client-managed MCP servers after updating. Inspect interrupted tool calls before retrying them. Settings and databases remain in their normal data directories; back up the database before an upgrade or rollback because older binaries may not support newer schemas.

## Build from source

Install Rust, then clone and build the repository for your platform:

```bash
git clone https://github.com/daure/tuido.git
cd tuido
cargo test --locked
cargo build --release --locked
sh scripts/install.sh
```

For source-based updates, run these commands inside the checkout:

```bash
git pull --ff-only
cargo build --release --locked
sh scripts/install.sh
```

The source installer applies the same process approval and service restart policy as the release installer. Direct `cargo install` and manual binary copies bypass that policy.

After installation:

```bash
tuido             # TUI
tuido mcp         # stdio MCP
tuido dev         # TUI + HTTP MCP on development port 7346
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

From `main` with committed source changes, a sibling `../tuicore` checkout, Git push access, Python 3.11+, Rust, and an authenticated GitHub CLI (`gh auth login`):

```bash
cargo release          # patch: 0.28.0 -> 0.28.1
cargo release minor    # minor: 0.28.0 -> 0.29.0
cargo release major   # major: 0.28.0 -> 1.0.0
# Equivalent command without compiling the tiny Cargo helper:
./scripts/release.sh patch
```

The command checks the branch and local Tuicore path, then runs the release workflow's formatting, script tests, Clippy, and Rust tests before changing the version. Cargo preflight commands use two build jobs and two test threads, disable debug info and incremental compilation, and reuse `target/release-check` without changing the repository's Cargo configuration or local Tuicore path. It then bumps Tuido, refreshes and commits the local lockfile, creates an annotated tag without opening an editor or pager, and atomically pushes `main` and its `vX.Y.Z` tag. Generated `Cargo.lock` changes are accepted; all other files must be clean. It returns without waiting for GitHub Actions. Existing `cargo patch`, `cargo minor`, and `cargo major` aliases use the same release flow.

GitHub Actions runs `scripts/prepare_ci.py` before Cargo checks and builds. It selects the highest stable, non-yanked Tuicore version on crates.io across all major versions, pins that exact version in the CI manifest, and resolves its registry lockfile. Checks and distribution builds use that resolution. These edits remain in the disposable CI checkout. The selected version is printed in the workflow log; rerunning a workflow can select a newer published version. Registry or compatibility failures stop the build. Publish required Tuicore changes before releasing Tuido.

The [Release workflow](https://github.com/daure/tuido/actions/workflows/release.yml) checks formatting, runs Clippy with warnings denied, runs tests, then uses cargo-dist to build and smoke-test an Ubuntu x86_64 archive. After checks pass it publishes the GitHub Release and installer using GitHub's built-in repository token. GitHub Releases is the distribution channel for current Tuido versions. All work runs in one job, with no Actions artifact uploads. Release downloads persist until deleted.

Every push to `main`, including release commits, runs the same checks and warms debug and optimized dependency caches. Tagged releases restore those caches; successful `main` runs save them so later tags can access them. Distribution builds disable LTO and strip symbols to favor build speed. The first build after a toolchain or dependency change can take longer. Cache storage is separate from release downloads and temporary Actions artifact storage. To warm the cache manually, run `gh workflow run release.yml --ref main`; this builds without publishing.

After a successful tagged release, a serialized retention job keeps the 30 most recently published stable releases, ordered by publication time. It deletes older GitHub release records and their downloads using the official GitHub API through `gh`, preserving all Git tags, drafts, and prereleases. Older version-specific download URLs stop working; their source tags remain available. The script checks all pages and rechecks each candidate before deletion; it refuses cleanup if the triggering release is missing or outside the retained set. This policy applies to Finery and Tuido only.

Preview cleanup without deleting anything:

```bash
python3 scripts/prune-releases.py --repo daure/tuido --keep 30
```

Deletion requires both `--apply` and `--published-tag TAG`; the release workflow supplies these only after publication succeeds. Main-branch builds run the preview only.

Cleanup also refuses to delete the release designated as GitHub's **Latest**, protecting the installer URL if an older version is pinned there.

Inspect runs with `gh run list --workflow release.yml` and `gh run watch RUN_ID`. Retry a transient failure with `gh run rerun RUN_ID --failed`. For a source fix, commit the fix and run a new patch release. Tags are immutable: do not move a published tag. If the local push fails, inspect the release commit/tag and use the exact retry command printed by the script.

### Local Tuicore development

Tuido declares a direct relative path dependency:

```toml
tuicore = { path = "../tuicore" }
```

Keep the repositories side by side and run `cargo run -- dev`. Local builds always compile that Tuicore working copy, regardless of its version. Cargo maintains the local lockfile automatically; `cargo release` handles it when releasing. Personal `[patch.crates-io]` overrides are unnecessary for this workflow and can produce unused-patch warnings in helper crates.
