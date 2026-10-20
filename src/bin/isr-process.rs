//! Static x86-64 ELF process -> Cranelift -> native or interpreted execution.
use cranelift_isr::{
    ledger::RunLedger,
    process::{self, Backend, Options},
};
use serde_json::json;
use std::path::PathBuf;
fn main() {
    if let Err(e) = run() {
        eprintln!("isr-process: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut options = Options::default();
    let mut file = None;
    let mut events_bind = None;
    let mut root = std::env::var_os("ISR_LEDGER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".isr-state/process"));
    while let Some(a) = args.next() {
        match a.as_str() {
            "--groups" => {
                println!("{}", cranelift_isr::groups::PipelineGroup::catalog());
                return Ok(());
            }
            "--help" => {
                println!("isr-process FILE [--ledger-dir DIR] [--max-rv64-steps N] [--max-seconds N] [--recompile-every-syscalls N] [--events-bind 127.0.0.1:0] [--backend x86_64|rv64] [--no-tags] [--no-pac] [--no-random-placement] [--no-cfi] [--no-lam]\nisr-process --groups\nRuns a static Linux x86-64 ELF through Cranelift. Loopback TCP syscall profile; no host-kernel fallback.\n--backend selects the compilation target (x86_64=host-native with checked loads/stores, tags, PAC, CFI, W^X; rv64=interpreted).\n--no-tags disables heap shadow tagging, --no-pac pointer authentication, --no-random-placement deterministic slot reuse, --no-cfi mid-instruction transfer rejection, --no-lam forces the software tag scheme; each is for A/B against the same binary.");
                return Ok(());
            }
            "--events-bind" => {
                events_bind = Some(
                    args.next()
                        .ok_or("missing event bind address")?
                        .parse::<std::net::SocketAddr>()?,
                )
            }
            "--ledger-dir" => root = PathBuf::from(args.next().ok_or("missing ledger directory")?),
            "--max-rv64-steps" => {
                options.max_rv64_steps = args.next().ok_or("missing step budget")?.parse()?
            }
            "--max-seconds" => {
                options.max_seconds = args.next().ok_or("missing deadline")?.parse()?
            }
            "--recompile-every-syscalls" => {
                options.recompile_every_syscalls =
                    args.next().ok_or("missing recompile interval")?.parse()?
            }
            "--no-tags" => options.hardening.tags = false,
            "--no-pac" => options.hardening.pac = false,
            "--no-random-placement" => options.hardening.random_placement = false,
            "--no-cfi" => options.hardening.cfi = false,
            "--no-lam" => options.hardening.lam = false,
            "--backend" => {
                let b = args.next().ok_or("missing backend name")?;
                options.backend = match b.as_str() {
                    "rv64" => Backend::Rv64,
                    "x86_64" => Backend::X86_64,
                    _ => return Err(format!("unknown backend: {b}").into()),
                };
            }
            "--" => {
                while let Some(arg) = args.next() {
                    options.args.push(arg);
                }
                break;
            }
            _ if !a.starts_with('-') && file.is_none() => file = Some(PathBuf::from(a)),
            _ => return Err(format!("unknown or duplicate input: {a}").into()),
        }
    }
    let file = file.ok_or("expected a static x86-64 ELF file; use --help")?;
    if options.max_seconds == 0 || options.max_seconds > 3600 || options.max_rv64_steps == 0 {
        return Err("invalid execution budget".into());
    }
    let mut ledger = RunLedger::create(&root)?;
    eprintln!(
        "ledger: {}",
        ledger.directory().join("events.jsonl").display()
    );
    let live_feed = events_bind
        .map(|address| ledger.serve_events(address))
        .transpose()?;
    if let Some(feed) = &live_feed {
        eprintln!("events: {}", feed.endpoint());
    }
    let backend_label = options.backend.label();
    ledger.event(
        "started",
        json!({"mode":"static ELF process","decoder":"iced-x86","backend":backend_label,
        "live_events":live_feed.as_ref().map(|feed|feed.endpoint()),"native_fallback":false,"host_arch":std::env::consts::ARCH,"options":options}),
    )?;
    let result = process::run_file(&file, &options, &mut ledger);
    match result {
        Ok(report) => {
            ledger.event(
                "result",
                json!({"status":if report.exit_code==Some(0){"completed"}else{"failed"},"report":report}),
            )?;
            if report.exit_code != Some(0) {
                return Err(format!("guest exited with code {:?}", report.exit_code).into());
            }
        }
        Err(e) => {
            ledger.event("result", json!({"status":"failed","error":e.to_string()}))?;
            return Err(e);
        }
    }
    Ok(())
}
