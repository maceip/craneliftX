//! ISR Server Harness — production supervisor for hardened guest binaries.
//!
//! Loads a static x86-64 ELF, runs it under the ISR mitigations, and provides
//! an HTTP admin endpoint for health and metrics. Restarts the guest on crash
//! with configurable backoff.
//!
//! Usage:
//!   isr-server /etc/isr/guest.toml
//!   isr-server --config /etc/isr/guest.toml --once
//!
//! The guest binary must be a static x86-64 ELF. If your binary is dynamically
//! linked, use `scripts/static-link.sh` to convert it first.

use cranelift_isr::{
    ledger::RunLedger,
    process::{self, Backend, Hardening, Options},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------------
// Config file schema (TOML)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
struct Config {
    guest: GuestConfig,
    #[serde(default)]
    execution: ExecutionConfig,
    #[serde(default)]
    hardening: HardeningConfig,
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    restart: RestartConfig,
    #[serde(default)]
    logging: LoggingConfig,
}

#[derive(Clone, Debug, Deserialize)]
struct GuestConfig {
    /// Path to the static x86-64 ELF guest binary.
    path: PathBuf,
    /// Optional pre-processing command (e.g. `musl-gcc -static`).
    #[serde(default)]
    preprocess: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct ExecutionConfig {
    #[serde(default = "default_backend")]
    backend: String,
    #[serde(default = "default_max_seconds")]
    max_seconds: u64,
    #[serde(default = "default_max_rv64_steps")]
    max_rv64_steps: u64,
    #[serde(default)]
    recompile_every_syscalls: u64,
    /// Allow guest bind() to non-loopback addresses.
    #[serde(default)]
    allow_non_loopback: bool,
    /// Opt in to the experimental x86_64 native backend. The hardened
    /// production path is rv64: heap tags, PAC and CFI all apply there.
    #[serde(default)]
    allow_experimental_native: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct HardeningConfig {
    #[serde(default = "default_true")]
    tags: bool,
    #[serde(default = "default_true")]
    pac: bool,
    #[serde(default = "default_true")]
    random_placement: bool,
    #[serde(default = "default_true")]
    cfi: bool,
    /// Try Intel LAM_U57 when the CPU and kernel allow it (default on).
    #[serde(default = "default_true")]
    lam: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct ServerConfig {
    /// Address for the admin HTTP endpoint (health, metrics).
    #[serde(default = "default_admin_bind")]
    admin_bind: String,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct RestartConfig {
    #[serde(default = "default_restart_policy")]
    policy: String,
    #[serde(default = "default_backoff")]
    backoff_seconds: u64,
    #[serde(default = "default_max_restarts")]
    max_restarts: u32,
}

#[derive(Clone, Debug, Deserialize, Default)]
struct LoggingConfig {
    #[serde(default = "default_ledger_dir")]
    ledger_dir: PathBuf,
    /// Optional live SSE event stream bind address.
    #[serde(default)]
    events_bind: Option<String>,
}

fn default_backend() -> String { "x86_64".into() }
fn default_max_seconds() -> u64 { 300 }
fn default_max_rv64_steps() -> u64 { 5_000_000_000 }
fn default_true() -> bool { true }
fn default_admin_bind() -> String { "127.0.0.1:8080".into() }
fn default_restart_policy() -> String { "always".into() }
fn default_backoff() -> u64 { 5 }
fn default_max_restarts() -> u32 { 10 }
fn default_ledger_dir() -> PathBuf { PathBuf::from("/var/log/isr-server") }

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
struct ServerState {
    guest_path: PathBuf,
    start_time: Option<String>,
    restart_count: u32,
    last_exit_code: Option<i32>,
    last_error: Option<String>,
    healthy: bool,
}

static SERVER_STATE: std::sync::Mutex<Option<ServerState>> = std::sync::Mutex::new(None);

fn update_state(f: impl FnOnce(&mut ServerState)) {
    let mut guard = SERVER_STATE.lock().unwrap();
    if let Some(ref mut s) = *guard {
        f(s);
    }
}

// ---------------------------------------------------------------------------
// Admin HTTP endpoint
// ---------------------------------------------------------------------------

fn serve_admin(bind: SocketAddr, shutdown: Arc<AtomicBool>) {
    let listener = match TcpListener::bind(bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("isr-server: cannot bind admin endpoint {bind}: {e}");
            return;
        }
    };
    listener.set_nonblocking(true).ok();
    eprintln!("isr-server: admin http://{bind}/health");

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => handle_admin_request(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {}
        }
    }
}

fn handle_admin_request(mut stream: TcpStream) {
    let mut buf = [0u8; 512];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");

    let (status, body, content_type) = if line.starts_with("GET /health") {
        let state = SERVER_STATE.lock().unwrap().clone();
        let body = serde_json::to_string_pretty(&json!({
            "status": if state.as_ref().map_or(false, |s| s.healthy) { "healthy" } else { "unhealthy" },
            "server_state": state,
        }))
        .unwrap_or_else(|_| "{}".into());
        ("200 OK", body, "application/json")
    } else if line.starts_with("GET /metrics") {
        let body = generate_prometheus_metrics();
        ("200 OK", body, "text/plain; version=0.0.4")
    } else if line.starts_with("GET /") {
        let body = r#"<!doctype html>
<title>ISR Server</title>
<ul>
<li><a href="/health">/health</a> — JSON health status</li>
<li><a href="/metrics">/metrics</a> — Prometheus metrics</li>
</ul>"#
            .to_string();
        ("200 OK", body, "text/html")
    } else {
        ("404 Not Found", "not found".into(), "text/plain")
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn generate_prometheus_metrics() -> String {
    let mut out = String::new();
    let state = SERVER_STATE.lock().unwrap().clone();
    if let Some(s) = state {
        out.push_str(&format!(
            "# TYPE isr_server_restarts counter\nisr_server_restarts {}\n",
            s.restart_count
        ));
        out.push_str(&format!(
            "# TYPE isr_server_healthy gauge\nisr_server_healthy {}\n",
            if s.healthy { 1 } else { 0 }
        ));
        if let Some(code) = s.last_exit_code {
            out.push_str(&format!(
                "# TYPE isr_server_last_exit_code gauge\nisr_server_last_exit_code {}\n",
                code
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Guest execution loop
// ---------------------------------------------------------------------------

fn run_guest(config: &Config, _shutdown: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    let backend = match config.execution.backend.as_str() {
        "rv64" => Backend::Rv64,
        "x86_64" => Backend::X86_64,
        _ => return Err(format!("unknown backend: {}", config.execution.backend).into()),
    };

    let options = Options {
        max_rv64_steps: config.execution.max_rv64_steps,
        max_seconds: config.execution.max_seconds,
        recompile_every_syscalls: config.execution.recompile_every_syscalls,
        hardening: Hardening {
            tags: config.hardening.tags,
            pac: config.hardening.pac,
            random_placement: config.hardening.random_placement,
            cfi: config.hardening.cfi,
            lam: config.hardening.lam,
        },
        backend,
    };

    let ledger_dir = &config.logging.ledger_dir;
    std::fs::create_dir_all(ledger_dir)?;
    let mut ledger = RunLedger::create(ledger_dir)?;

    let events_bind = config
        .logging
        .events_bind
        .as_ref()
        .and_then(|s| s.parse::<SocketAddr>().ok());
    let live_feed = events_bind
        .map(|addr| ledger.serve_events(addr))
        .transpose()?;
    if let Some(ref feed) = live_feed {
        eprintln!("isr-server: events http://{}/events", feed.endpoint());
    }

    ledger.event(
        "started",
        json!({
            "mode": "server harness",
            "decoder": "iced-x86",
            "backend": backend.label(),
            "guest": config.guest.path.display().to_string(),
            "live_events": live_feed.as_ref().map(|f| f.endpoint()),
            "host_arch": std::env::consts::ARCH,
            "options": options,
        }),
    )?;

    update_state(|s| {
        s.healthy = true;
        s.start_time = Some(format!("{:?}", Instant::now()));
    });

    let result = process::run_file(&config.guest.path, &options, &mut ledger);

    match result {
        Ok(report) => {
            ledger.event(
                "result",
                json!({"status": if report.exit_code == Some(0) { "completed" } else { "failed" }, "report": report}),
            )?;
            update_state(|s| {
                s.last_exit_code = report.exit_code;
                s.healthy = false;
            });
            if report.exit_code != Some(0) {
                return Err(format!("guest exited with code {:?}", report.exit_code).into());
            }
        }
        Err(e) => {
            ledger.event("result", json!({"status": "failed", "error": e.to_string()}))?;
            update_state(|s| {
                s.last_error = Some(e.to_string());
                s.healthy = false;
            });
            return Err(e);
        }
    }

    Ok(())
}

fn supervise(config: Config, shutdown: Arc<AtomicBool>, once: bool) {
    let mut restarts = 0u32;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        update_state(|s| {
            s.restart_count = restarts;
            s.healthy = true;
            s.last_error = None;
        });

        let start = Instant::now();
        let result = run_guest(&config, shutdown.clone());
        let elapsed = start.elapsed();

        match result {
            Ok(()) => {
                eprintln!("isr-server: guest completed normally after {elapsed:?}");
                if once || config.restart.policy == "never" {
                    break;
                }
            }
            Err(e) => {
                eprintln!("isr-server: guest failed after {elapsed:?}: {e}");
                if once || config.restart.policy == "never" {
                    break;
                }
            }
        }

        restarts += 1;
        if config.restart.max_restarts > 0 && restarts >= config.restart.max_restarts {
            eprintln!("isr-server: max restarts ({}) reached, giving up", config.restart.max_restarts);
            break;
        }

        let backoff = Duration::from_secs(config.restart.backoff_seconds);
        eprintln!("isr-server: restarting in {backoff:?} (attempt {}/{})",
            restarts + 1,
            if config.restart.max_restarts > 0 { config.restart.max_restarts.to_string() } else { "∞".into() }
        );

        // Sleep in small increments so we can still respond to shutdown.
        let deadline = Instant::now() + backoff;
        while Instant::now() < deadline {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        eprintln!("isr-server: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut config_path = None;
    let mut once = false;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => {
                println!(r#"isr-server [OPTIONS] <CONFIG.toml>

Production harness for running hardened x86-64 guest binaries.

Options:
  --config PATH   Path to TOML configuration file
  --once          Run once and exit; do not restart on failure
  --help          Print this message

The guest binary must be a static x86-64 ELF. Dynamic binaries can be
converted with scripts/static-link.sh before deployment.

Config example:
  [guest]
  path = "/opt/isr/guest-server"

  [execution]
  backend = "x86_64"
  max_seconds = 300

  [hardening]
  tags = true
  pac = true
  random_placement = true
  cfi = true

  [server]
  admin_bind = "0.0.0.0:8080"

  [restart]
  policy = "always"
  backoff_seconds = 5
  max_restarts = 10
"#);
                return Ok(());
            }
            "--config" => config_path = Some(PathBuf::from(args.next().ok_or("missing config path")?)),
            "--once" => once = true,
            _ if !a.starts_with('-') && config_path.is_none() => config_path = Some(PathBuf::from(a)),
            _ => return Err(format!("unknown argument: {a}").into()),
        }
    }

    let config_path = config_path.ok_or("expected a TOML config file; use --help")?;
    let config_text = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("cannot read config {}: {e}", config_path.display()))?;
    let config: Config = toml::from_str(&config_text)
        .map_err(|e| format!("invalid config: {e}"))?;

    if !config.guest.path.exists() {
        return Err(format!("guest binary not found: {}", config.guest.path.display()).into());
    }

    // Initialize global server state.
    {
        let mut guard = SERVER_STATE.lock().unwrap();
        *guard = Some(ServerState {
            guest_path: config.guest.path.clone(),
            start_time: None,
            restart_count: 0,
            last_exit_code: None,
            last_error: None,
            healthy: false,
        });
    }

    // Signal handling.
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        ctrlc::set_handler(move || {
            eprintln!("isr-server: received SIGINT, shutting down...");
            shutdown.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("cannot install signal handler: {e}"))?;
    }

    // Admin HTTP endpoint.
    let admin_bind = config.server.admin_bind.parse::<SocketAddr>()
        .map_err(|e| format!("invalid admin_bind: {e}"))?;
    let admin_shutdown = shutdown.clone();
    let admin_thread = thread::spawn(move || serve_admin(admin_bind, admin_shutdown));

    // Run supervisor.
    supervise(config, shutdown.clone(), once);

    // Tear down.
    shutdown.store(true, Ordering::Relaxed);
    let _ = admin_thread.join();
    eprintln!("isr-server: shutdown complete");
    Ok(())
}
