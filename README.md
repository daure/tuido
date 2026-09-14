# Tuido

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

The installer places `tuido` in `$CARGO_HOME/bin` (default `~/.cargo/bin`). Add that directory to your shell's PATH permanently if needed. Archives use the Cargo package name `tuitodo`; the executable is `tuido`. The Releases page also provides a `.tar.xz` archive and SHA-256 checksum for manual installation; versioned releases remain available for rollback.

### Update

Close running Tuido sessions, then rerun the installer commands above to download the latest stable binary. If you installed the background MCP service, run `tuido service stop` before updating and `tuido service start` afterwards. Restart stdio MCP clients to load the updated executable. Your settings and database stay in their normal data directories; back up the database before an upgrade or rollback because older binaries may not support newer database schemas.

## Build from source

Install Rust, then clone and build the repository for your platform:

```bash
git clone https://github.com/daure/tuido.git
cd tuido
cargo test --locked
cargo install --path . --locked
```

For source-based updates, run these commands inside the checkout:

```bash
git pull --ff-only
cargo install --path . --locked --force
```

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

From a clean `main` checkout with Git push access, Python 3.11+, Rust, and an authenticated GitHub CLI (`gh auth login`):

```bash
cargo release          # patch: 0.28.0 -> 0.28.1
cargo release minor    # minor: 0.28.0 -> 0.29.0
cargo release major   # major: 0.28.0 -> 1.0.0
# Equivalent command without compiling the tiny Cargo helper:
./scripts/release.sh patch
```

The command checks the branch and published Tuicore dependency, bumps Tuido, resolves the release lockfile against crates.io, commits, and atomically pushes `main` and its `vX.Y.Z` tag. It returns without waiting for compilation. Existing `cargo patch`, `cargo minor`, and `cargo major` aliases use the same release flow.

The [Release workflow](https://github.com/daure/tuido/actions/workflows/release.yml) checks formatting, runs Clippy with warnings denied, runs tests, then uses cargo-dist to build and smoke-test an Ubuntu x86_64 archive. After checks pass it publishes the GitHub Release and installer using GitHub's built-in repository token. GitHub Releases is the distribution channel for current Tuido versions. All work runs in one job, with no Actions artifact uploads. Release downloads persist until deleted.

Normal pushes to `main` run the same checks and warm debug and optimized dependency caches. Tagged releases restore those caches; only `main` saves them so later tags can access them. Release-version commits skip the redundant branch build. Distribution builds disable LTO and strip symbols to favor build speed. The first build after a toolchain or dependency change can take longer. Cache storage is separate from release downloads and temporary Actions artifact storage. To warm the cache manually, run `gh workflow run release.yml --ref main`; this builds without publishing.

Inspect runs with `gh run list --workflow release.yml` and `gh run watch RUN_ID`. Retry a transient failure with `gh run rerun RUN_ID --failed`. For a source fix, commit the fix and run a new patch release. Tags are immutable: do not move a published tag. If the local push fails, inspect the release commit/tag and use the exact retry command printed by the script.

### Local Tuicore development

Tuido declares Tuicore as a crates.io dependency. To compile against your local working copy, put this in your personal `~/.cargo/config.toml`:

```toml
[patch.crates-io]
tuicore = { path = "/absolute/path/to/tuicore" }
```

Local builds compile the working copy whenever it satisfies the dependency requirement and is selected by Cargo; `cargo update -p tuicore` selects the override when needed. This can modify `Cargo.lock`, which must be committed or deliberately restored before releasing. The release command resolves Tuicore from crates.io without the personal override. Publish required Tuicore changes first, update Tuido's declared dependency version, and commit those changes before releasing. CI builds use the committed registry lockfile; users only download the compiled Tuido binary.
