//! Isolated process orchestration. Major operations live in input, compile,
//! cache and runtime; stable ledger groups are defined in `crate::groups`.
mod audit;
mod cache;
mod cfi;
mod compile;
mod csprng;
mod diversify;
mod heap;
mod futex;
mod input;
mod jit;
mod lam;
mod lower;
mod memory;
mod mpk;
mod native;
mod observe;
mod pac;
mod runtime;
mod syscall;
pub use lower::Backend;
pub use diversify::Diversification;
use crate::ledger::RunLedger;
use serde::Serialize;
use serde_json::json;
use std::path::Path;

// GPRs, RIP, RFLAGS, FS base, then the low and high halves of all 16 XMM registers.
pub(super) const CONTEXT_WORDS: usize = 51;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Which mitigations are armed for a run. All default on; turning one off
/// is how a mitigation is A/B'd against the identical binary.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Hardening {
    /// Sub-granule heap tagging: chunk tags, shadow map, quarantine on free.
    pub tags: bool,
    /// Context-bound pointer authentication on code pointers in memory.
    pub pac: bool,
    /// Randomized slot selection instead of ordered reuse.
    pub random_placement: bool,
    /// Reject transfer to an address that falls inside a known instruction.
    pub cfi: bool,
    /// Try to enable Intel LAM_U57 (hardware masks pointer tag bits 62:57).
    /// Optional; on by default when the CPU and kernel allow it. Failure is
    /// not fatal — the software tag scheme is used instead.
    pub lam: bool,
}
impl Default for Hardening {
    fn default() -> Self {
        Self {
            tags: true,
            pac: true,
            random_placement: true,
            cfi: true,
            lam: true,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Options {
    pub max_rv64_steps: u64,
    pub max_seconds: u64,
    pub recompile_every_syscalls: u64,
    pub hardening: Hardening,
    pub backend: Backend,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            max_rv64_steps: 500_000_000,
            max_seconds: 120,
            recompile_every_syscalls: 0,
            hardening: Hardening::default(),
            backend: Backend::X86_64,
            args: Vec::new(),
            env: Vec::new(),
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
    /// Calls served by the runtime's allocator, by interception or by syscall.
    pub heap_calls: u64,
    pub heap_allocations: u64,
    pub heap_frees: u64,
    pub heap_live_bytes: u64,
    pub heap_quarantined: usize,
    /// Accesses rejected by the shadow tag map.
    pub heap_tag_faults: u64,
    pub pac_signings: u64,
    pub pac_verifications: u64,
    /// Loads or stores rejected by pointer authentication.
    pub pac_faults: u64,
}

/// Run the isolated process. The caller persists its terminal result.
pub fn run_file(path: &Path, options: &Options, ledger: &mut RunLedger) -> Result<Report> {
    if options.max_seconds == 0 || options.max_seconds > 3600 || options.max_rv64_steps == 0 {
        return Err(memory::error("invalid process execution budget").into());
    }
    let bytes = input::ingest(path, options, ledger)?;
    let (memory, entry, intercepts) = input::load(&bytes, ledger)?;
    let mut runtime = runtime::Runtime::initialize_with_intercepts(
        memory,
        entry,
        intercepts.clone(),
        options,
        ledger,
    )?;
    let mut report = Report::default();
    let execution = observe::operation(
        ledger,
        "execute",
        json!({"allocator_intercepts":intercepts.names(),"intercepted":!intercepts.is_empty()}),
        |ledger| {
            loop {
                if runtime
                    .process
                    .aborted
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    if let Some(err) = runtime.process.thread_error.lock().unwrap().take() {
                        return Err(memory::error(err).into());
                    }
                }
                if let Some(code) = *runtime.process.exit_code.lock().unwrap() {
                    report.exit_code = Some(code);
                    break;
                }
                runtime.check_budget(&report, options)?;
                let rip = runtime.state[16];
                // A call that landed on the guest's own allocator is served by
                // the runtime instead of being lifted: the binary is untouched.
                if let Some(function) = intercepts.by_address.get(&rip) {
                    runtime.allocator_call(*function, &mut report, ledger)?;
                    continue;
                }
                let mut block = {
                    let mut cs = runtime.process.compile_state.lock().unwrap();
                    if options.hardening.cfi && !cs.cfi_map.allows(rip) {
                        return Err(memory::error(format!(
                            "CFI violation: transfer to 0x{rip:x} lands inside a known instruction"
                        ))
                        .into());
                    }
                    let space = runtime.process.address_space.lock().unwrap();
                    let runtime::CompileState {
                        ref mut cache,
                        ref mut cfi_map,
                        ref mut report,
                    } = *cs;
                    let cloned = {
                        let cached = cache.get_or_compile(&space.memory, rip, report, ledger)?;
                        cfi_map.record_block(&cached.compiled.source_instructions);
                        cached.clone()
                    };
                    cache.mark_executed(rip);
                    cloned
                };
                runtime.execute(&mut block, &mut report, options, ledger)?;
                {
                    let mut cs = runtime.process.compile_state.lock().unwrap();
                    if let Some(entry) = cs.cache.get_mut(rip) {
                        entry.executions += 1;
                        entry.rv64_steps += block.rv64_steps;
                    }
                }
                let syscall = block.compiled.syscall;
                if let Some(syscall_rip) = syscall {
                    let outcome = runtime.syscall(rip, syscall_rip, &mut report, ledger)?;
                    if let Some(code) = outcome.exit {
                        report.exit_code = Some(code);
                        *runtime.process.exit_code.lock().unwrap() = Some(code);
                        break;
                    }
                    if options.recompile_every_syscalls != 0
                        && report
                            .syscalls
                            .is_multiple_of(options.recompile_every_syscalls)
                    {
                        let open_fds = runtime.process.kernel.open_descriptors();
                        let mut cs = runtime.process.compile_state.lock().unwrap();
                        cs.cache.invalidate(open_fds, &runtime.state, ledger)?;
                        report.cache_invalidations += 1;
                    }
                }
            }
            if runtime
                .process
                .aborted
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                if let Some(err) = runtime.process.thread_error.lock().unwrap().take() {
                    return Err(memory::error(err).into());
                }
            }
            Ok(())
        },
        |_| json!({}),
    );
    {
        let cs = runtime.process.compile_state.lock().unwrap();
        report.compiled_blocks = cs.report.compiled_blocks;
        report.emitted_rv64_bytes = cs.report.emitted_rv64_bytes;
        report.unique_lifted_instructions = cs.report.unique_lifted_instructions;
        report.unique_lifted_bytes = cs.report.unique_lifted_bytes;
        cs.cache.summarize(ledger)?;
    }
    runtime.update_counters(&mut report);
    let executed_addresses = {
        let cs = runtime.process.compile_state.lock().unwrap();
        cs.cache.addresses()
    };
    ledger.event("execution",json!({"status":if execution.is_ok(){"completed"}else{"failed"},"counters":report,
        "error":execution.as_ref().err().map(ToString::to_string),"rip":format!("0x{:x}",runtime.state[16]),
        "rflags":format!("0x{:x}",runtime.state[17]),"executed_block_addresses":executed_addresses,
        "open_descriptors_at_exit":runtime.process.kernel.open_descriptors()}))?;
    ledger.event("mitigations", runtime.mitigation_summary())?;
    if runtime.is_forked_child {
        let code = if execution.is_ok() {
            report.exit_code.unwrap_or(0)
        } else {
            1
        };
        unsafe { libc::_exit(code) };
    }
    execution?;
    Ok(report)
}

#[cfg(test)]
mod tests;
