//! Guest initialization, RV64 execution and syscall continuation. Translation
//! cache operations never replace this state or its descriptor table.
use super::{
    cache::CachedBlock,
    memory::{self, GuestMemory, ProcessBus, CONTEXT_BASE, RV_STACK_BASE, STACK_TOP},
    observe::operation,
    syscall::{Kernel, Outcome},
    Options, Report, Result,
};
use crate::{
    ledger::RunLedger,
    riscv_exec::{RiscvCpu, RETURN_ADDRESS},
};
use serde_json::json;
use std::time::{Duration, Instant};

pub(super) struct Runtime {
    pub memory: GuestMemory,
    pub state: [u64; 18],
    pub kernel: Kernel,
    stack: Vec<u8>,
    deadline: Instant,
    last_progress: Instant,
}
impl Runtime {
    pub fn initialize(
        memory: GuestMemory,
        entry: u64,
        options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<Self> {
        operation(
            ledger,
            "initialize",
            json!({}),
            |_| {
                let mut state = [0u64; 18];
                state[7] = STACK_TOP - 48;
                state[16] = entry;
                state[17] = 2;
                let deadline = Instant::now() + Duration::from_secs(options.max_seconds);
                Ok(Self {
                    memory,
                    state,
                    kernel: Kernel::new(deadline),
                    stack: vec![0; 64 * 1024],
                    deadline,
                    last_progress: Instant::now(),
                })
            },
            |runtime| {
                json!({"entry_rip":format!("0x{:x}",runtime.state[16]),"guest_rsp":format!("0x{:x}",runtime.state[7]),
            "rv64_stack_bytes":runtime.stack.len(),"state_words":runtime.state.len(),"options":options})
            },
        )
    }
    pub fn check_budget(&self, report: &Report, options: &Options) -> Result<()> {
        if report.rv64_steps >= options.max_rv64_steps || Instant::now() >= self.deadline {
            return Err(memory::error("process execution budget exhausted").into());
        }
        Ok(())
    }
    pub fn execute(
        &mut self,
        block: &mut CachedBlock,
        report: &mut Report,
        options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<()> {
        let mut cpu = RiscvCpu::default();
        cpu.regs[1] = RETURN_ADDRESS;
        cpu.regs[2] = RV_STACK_BASE + self.stack.len() as u64;
        cpu.regs[10] = CONTEXT_BASE;
        let mut bus = ProcessBus {
            code: &block.compiled.code,
            memory: &mut self.memory,
            state: &mut self.state,
            stack: &mut self.stack,
        };
        // SAFETY: ProcessBus maps only owned slices and checked guest regions.
        // Generated code never supplies a host pointer to this bus.
        let steps = unsafe {
            cpu.run(
                &mut bus,
                (options.max_rv64_steps - report.rv64_steps).min(100_000) as usize,
            )
        }?;
        report.rv64_steps += steps;
        report.block_executions += 1;
        report.x86_instructions_executed += block.compiled.source_instructions.len() as u64;
        block.executions += 1;
        block.rv64_steps += steps;
        // Bounded, time-based progress for hot cached code. No per-instruction I/O.
        if self.last_progress.elapsed() >= Duration::from_millis(250) {
            self.update_network_counters(report);
            ledger.event("progress",json!({"status":"running","block_id":block.compiled.id,"rip":format!("0x{:x}",block.compiled.rip),
                "next_rip":format!("0x{:x}",self.state[16]),"counters":report}))?;
            self.last_progress = Instant::now();
        }
        Ok(())
    }
    pub fn syscall(
        &mut self,
        block_rip: u64,
        syscall_rip: u64,
        report: &mut Report,
        ledger: &mut RunLedger,
    ) -> Result<Outcome> {
        report.syscalls += 1;
        ledger.event("syscall_started",json!({"number":self.state[0],"block_rip":format!("0x{block_rip:x}"),"syscall_rip":format!("0x{syscall_rip:x}"),
            "next_rip":format!("0x{:x}",self.state[16]),"syscall_seq":report.syscalls,"rv64_steps":report.rv64_steps,"block_executions":report.block_executions}))?;
        let start = Instant::now();
        let outcome = match self.kernel.call(&mut self.state, &mut self.memory) {
            Ok(outcome) => outcome,
            Err(error) => {
                ledger.event("syscall_failed",json!({"syscall_seq":report.syscalls,"number":self.state[0],"elapsed_ns":start.elapsed().as_nanos() as u64,"error":error.to_string()}))?;
                return Err(error.into());
            }
        };
        self.update_network_counters(report);
        ledger.event("syscall_completed",json!({"syscall_seq":report.syscalls,"elapsed_ns":start.elapsed().as_nanos() as u64,
            "detail":outcome.detail,"rv64_steps":report.rv64_steps,"block_executions":report.block_executions}))?;
        Ok(outcome)
    }
    pub fn update_network_counters(&self, report: &mut Report) {
        report.accepted_connections = self.kernel.accepted;
        report.network_read_bytes = self.kernel.read_bytes;
        report.network_written_bytes = self.kernel.written_bytes;
    }
}
