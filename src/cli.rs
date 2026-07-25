use std::{error::Error, fs, net::SocketAddr, path::PathBuf, process::Command, thread};

use clap::{Parser, Subcommand};

const ABOUT: &str = "Keyboard-first task manager with full read/write MCP access.

Run `tuido` for TUI. MCP clients normally spawn stdio server with config:
  {\"command\": \"tuido\", \"args\": [\"mcp\"]}

`tuido dev` runs TUI plus Streamable HTTP MCP at http://127.0.0.1:7345/mcp.
`tuido serve` runs HTTP MCP in foreground until signal/session ends.
HTTP is loopback-only. Installed service has no bearer middleware in current rmcp integration;
local-only binding is security boundary. Configure database with TUIDO_DATABASE_URL.
Lifecycle commands: `tuido service install|start|stop|uninstall`.
Lifecycle supports Linux systemd-user and macOS launchd; Windows is unsupported.";

#[derive(Parser)]
#[command(name="tuido", version, about="Tuido TUI and MCP server", long_about=ABOUT)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Run protocol-only MCP server over stdin/stdout",
        long_about = "Run full CRUD MCP server over stdio. stdout is reserved for JSON-RPC protocol output; diagnostics belong on stderr."
    )]
    Mcp,
    #[command(
        about = "Run TUI and loopback HTTP MCP for development",
        long_about = "Run TUI plus Streamable HTTP MCP at http://127.0.0.1:7345/mcp. Server ends with process."
    )]
    Dev,
    #[command(about = "Run loopback Streamable HTTP MCP in foreground")]
    Serve {
        #[arg(long, default_value="127.0.0.1:7345", value_parser=parse_loopback)]
        bind: SocketAddr,
    },
    #[command(about = "Manage persistent user service (systemd-user or launchd)")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    #[command(about = "Install user service definition; idempotently replaces existing definition")]
    Install,
    #[command(about = "Start installed user service")]
    Start,
    #[command(about = "Stop installed user service")]
    Stop,
    #[command(about = "Stop and remove user service definition")]
    Uninstall,
}

fn parse_loopback(value: &str) -> Result<SocketAddr, String> {
    let addr = value.parse::<SocketAddr>().map_err(|e| e.to_string())?;
    if !addr.ip().is_loopback() {
        return Err("address must be loopback".into());
    }
    Ok(addr)
}

pub fn run() -> Result<(), Box<dyn Error>> {
    match Cli::parse().command {
        None => crate::app::run(),
        Some(Commands::Mcp) => runtime()?.block_on(crate::run_stdio()),
        Some(Commands::Serve { bind }) => runtime()?.block_on(crate::run_http(bind)),
        Some(Commands::Dev) => {
            let (startup_tx, startup_rx) = std::sync::mpsc::channel();
            thread::Builder::new()
                .name("tuido-mcp-http".into())
                .spawn(|| {
                    if let Ok(rt) = runtime()
                        && let Err(error) = rt.block_on(crate::mcp::run_http_with_startup(
                            "127.0.0.1:7345".parse().expect("static address"),
                            startup_tx,
                        ))
                    {
                        eprintln!("HTTP MCP stopped: {error}");
                    }
                })?;
            startup_rx
                .recv()
                .map_err(|_| "HTTP MCP startup thread exited without status")?
                .map_err(|error| format!("HTTP MCP startup failed: {error}"))?;
            crate::app::run()
        }
        Some(Commands::Service { action }) => lifecycle(action),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, Box<dyn Error>> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

#[cfg(target_os = "linux")]
fn lifecycle(action: ServiceAction) -> Result<(), Box<dyn Error>> {
    let home = std::env::var("HOME")?;
    let dir = PathBuf::from(home).join(".config/systemd/user");
    let path = dir.join("tuido-mcp.service");
    match action {
        ServiceAction::Install => {
            fs::create_dir_all(&dir)?;
            let exe = std::env::current_exe()?;
            let database_url = configured_database_url()?;
            write_owner_only(&path, &systemd_definition(&exe, database_url.as_deref()))?;
            run_command("systemctl", &["--user", "daemon-reload"])?;
            run_command("systemctl", &["--user", "enable", "tuido-mcp.service"])?;
        }
        ServiceAction::Start => {
            run_command("systemctl", &["--user", "start", "tuido-mcp.service"])?
        }
        ServiceAction::Stop => run_command("systemctl", &["--user", "stop", "tuido-mcp.service"])?,
        ServiceAction::Uninstall => {
            let _ = run_command(
                "systemctl",
                &["--user", "disable", "--now", "tuido-mcp.service"],
            );
            if path.exists() {
                fs::remove_file(path)?;
            }
            run_command("systemctl", &["--user", "daemon-reload"])?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn lifecycle(action: ServiceAction) -> Result<(), Box<dyn Error>> {
    let home = std::env::var("HOME")?;
    let dir = PathBuf::from(home).join("Library/LaunchAgents");
    let path = dir.join("dev.tuido.mcp.plist");
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    match action {
        ServiceAction::Install => {
            fs::create_dir_all(&dir)?;
            let exe = std::env::current_exe()?;
            let database_url = configured_database_url()?;
            write_owner_only(&path, &launchd_definition(&exe, database_url.as_deref()))?;
            let _ = run_command(
                "launchctl",
                &["bootout", &domain, path.to_str().unwrap_or_default()],
            );
            run_command(
                "launchctl",
                &["bootstrap", &domain, path.to_str().unwrap_or_default()],
            )?;
        }
        ServiceAction::Start => {
            let _ = run_command(
                "launchctl",
                &["bootout", &format!("{domain}/dev.tuido.mcp")],
            );
            run_command(
                "launchctl",
                &["bootstrap", &domain, path.to_str().unwrap_or_default()],
            )?;
        }
        ServiceAction::Stop => run_command(
            "launchctl",
            &["bootout", &format!("{domain}/dev.tuido.mcp")],
        )?,
        ServiceAction::Uninstall => {
            let _ = run_command(
                "launchctl",
                &["bootout", &domain, path.to_str().unwrap_or_default()],
            );
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn lifecycle(_: ServiceAction) -> Result<(), Box<dyn Error>> {
    Err("service lifecycle unsupported on this platform; use `tuido serve`".into())
}

fn run_command(program: &str, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        return Err(format!("{program} failed with {status}").into());
    }
    Ok(())
}

fn configured_database_url() -> Result<Option<String>, Box<dyn Error>> {
    match std::env::var("TUIDO_DATABASE_URL") {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_owner_only(path: &std::path::Path, contents: &str) -> Result<(), Box<dyn Error>> {
    use std::{
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.set_len(0)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn systemd_definition(exe: &std::path::Path, database_url: Option<&str>) -> String {
    let environment = database_url.map_or_else(String::new, |url| {
        format!(
            "Environment={}\n",
            systemd_quote_arg(&format!("TUIDO_DATABASE_URL={url}"))
        )
    });
    format!(
        "[Unit]\nDescription=Tuido MCP service\n\n[Service]\n{environment}ExecStart={} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote_arg(&exe.to_string_lossy())
    )
}

#[cfg(any(target_os = "linux", test))]
fn systemd_quote_arg(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(any(target_os = "macos", test))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "macos", test))]
fn launchd_definition(exe: &std::path::Path, database_url: Option<&str>) -> String {
    let environment = database_url.map_or_else(String::new, |url| {
        format!(
            "<key>EnvironmentVariables</key><dict><key>TUIDO_DATABASE_URL</key><string>{}</string></dict>",
            xml_escape(url)
        )
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>dev.tuido.mcp</string><key>ProgramArguments</key><array><string>{}</string><string>serve</string></array>{environment}<key>KeepAlive</key><true/></dict></plist>",
        xml_escape(&exe.to_string_lossy())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_definition_values_are_escaped() {
        assert_eq!(systemd_quote_arg("/tmp/a \\\"%n"), "\"/tmp/a \\\\\\\"%%n\"");
        assert_eq!(xml_escape("a&<>'\""), "a&amp;&lt;&gt;&apos;&quot;");

        let systemd = systemd_definition(
            std::path::Path::new("/tmp/tuido %i"),
            Some("postgres://user:p%ss@host/db\"name"),
        );
        assert!(
            systemd.contains(
                "Environment=\"TUIDO_DATABASE_URL=postgres://user:p%%ss@host/db\\\"name\""
            )
        );
        assert!(systemd.contains("ExecStart=\"/tmp/tuido %%i\" serve"));

        let launchd = launchd_definition(
            std::path::Path::new("/tmp/tuido&bin"),
            Some("postgres://user:p<&@host/db"),
        );
        assert!(launchd.contains("<string>/tmp/tuido&amp;bin</string>"));
        assert!(launchd.contains(
            "<key>TUIDO_DATABASE_URL</key><string>postgres://user:p&lt;&amp;@host/db</string>"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn service_definition_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path =
            std::env::temp_dir().join(format!("tuido-service-definition-{}", uuid::Uuid::new_v4()));
        write_owner_only(&path, "secret").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret");

        let _ = fs::remove_file(path);
    }
}
