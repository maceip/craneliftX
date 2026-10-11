//! Isolated process orchestration. Major operations live in input, compile,
//! cache and runtime; stable ledger groups are defined in `crate::groups`.
mod cache;
mod compile;
mod input;
mod lower;
mod memory;
mod observe;
mod runtime;
mod syscall;
use crate::ledger::RunLedger;
use serde::Serialize;
use serde_json::json;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Clone, Debug, Serialize)]
pub struct Options {
    pub max_rv64_steps: u64,
    pub max_seconds: u64,
    pub recompile_every_syscalls: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            max_rv64_steps: 500_000_000,
            max_seconds: 120,
            recompile_every_syscalls: 0,
        }
    }
}
#[derive(Default, Debug, Serialize)]
pub struct Report {
    pub exit_code: Option<i32>,
    pub compiled_blocks: u64,
    pub unique_lifted_instructions: usize,
    pub unique_lifted_bytes: usize,
    pub block_executions: u64,
    pub x86_instructions_executed: u64,
    pub rv64_steps: u64,
    pub emitted_rv64_bytes: u64,
    pub syscalls: u64,
    pub cache_invalidations: u64,
    pub accepted_connections: u64,
    pub network_read_bytes: u64,
    pub network_written_bytes: u64,
    pub native_x86_instructions_executed: u64,
}

/// Run the isolated process. The caller persists its terminal result.
pub fn run_file(path: &Path, options: &Options, ledger: &mut RunLedger) -> Result<Report> {
    if options.max_seconds == 0 || options.max_seconds > 3600 || options.max_rv64_steps == 0 {
        return Err(memory::error("invalid process execution budget").into());
    }
    let bytes = input::ingest(path, options, ledger)?;
    let (memory, entry) = input::load(&bytes, ledger)?;
    let mut runtime = runtime::Runtime::initialize(memory, entry, options, ledger)?;
    let mut cache = cache::BlockCache::default();
    let mut report = Report::default();
    let execution = observe::operation(
        ledger,
        "execute",
        json!({}),
        |ledger| {
            loop {
                runtime.check_budget(&report, options)?;
                let rip = runtime.state[16];
                let block = cache.get_or_compile(&runtime.memory, rip, &mut report, ledger)?;
                runtime.execute(block, &mut report, options, ledger)?;
                let syscall = block.compiled.syscall;
                cache.mark_executed(rip);
                if let Some(syscall_rip) = syscall {
                    let outcome = runtime.syscall(rip, syscall_rip, &mut report, ledger)?;
                    if let Some(code) = outcome.exit {
                        report.exit_code = Some(code);
                        break;
                    }
                    if options.recompile_every_syscalls != 0
                        && report
                            .syscalls
                            .is_multiple_of(options.recompile_every_syscalls)
                    {
                        cache.invalidate(
                            runtime.kernel.open_descriptors(),
                            &runtime.state,
                            ledger,
                        )?;
                        report.cache_invalidations += 1;
                    }
                }
            }
            Ok(())
        },
        |_| json!({}),
    );
    runtime.update_network_counters(&mut report);
    cache.summarize(ledger)?;
    ledger.event("execution",json!({"status":if execution.is_ok(){"completed"}else{"failed"},"counters":report,
        "error":execution.as_ref().err().map(ToString::to_string),"rip":format!("0x{:x}",runtime.state[16]),
        "rflags":format!("0x{:x}",runtime.state[17]),"executed_block_addresses":cache.addresses(),
        "open_descriptors_at_exit":runtime.kernel.open_descriptors()}))?;
    execution?;
    Ok(report)
}

#[cfg(test)]
mod tests;
