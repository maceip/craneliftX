//! Guest initialization, execution, allocator interception and syscall
//! continuation. Translation cache operations never replace this state or its
//! descriptor table.
//!
//! Hardening state lives here and nowhere the guest can reach: the shadow tag
//! map, the pointer-authentication secret and the allocator metadata are Rust
//! values owned by `Runtime`, and `ProcessBus` is the only path from a guest
//! address to memory.
use super::{
    audit,
    cache::CachedBlock,
    csprng::Csprng,
    heap::{Heap, HEAP_BASE, HEAP_SIZE},
    input::AllocFn,
    jit::NativeBlockFn,
    lam::{self, LamStatus},
    lower::Backend,
    memory::{self, GuestMemory, ProcessBus, CONTEXT_BASE, RV_STACK_BASE, STACK_TOP},
    mpk,
    observe::operation,
    pac::Pac,
    syscall::{Kernel, Outcome},
    Options, Report, Result,
};
use crate::{
    ledger::RunLedger,
    riscv_exec::{MemoryAccess, RiscvCpu, RETURN_ADDRESS},
    EngineError,
};
use serde_json::json;
use std::time::{Duration, Instant};

pub(super) struct Runtime {
    pub memory: GuestMemory,
    pub state: [u64; 18],
    pub kernel: Kernel,
    pub heap: Heap,
    pub pac: Pac,
    pub lam: LamStatus,
    pub domain: mpk::Domain,
    stack: Vec<u8>,
    deadline: Instant,
    last_progress: Instant,
    /// Set by the native helpers when a tag, PAC or mapping check fails.
    pub native_fault: Option<(u64, EngineError)>,
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
                let mut memory = memory;
                // The heap is an ordinary writable guest mapping. Its metadata
                // is not: Heap keeps every free list, size and tag in Rust.
                memory.map(HEAP_BASE, HEAP_SIZE, true, false)?;
                // Optional LAM kernel capability: enabled by default when the
                // CPU and kernel allow it; software tags otherwise.
                let lam = lam::enable(options.hardening.lam);
                let mut rng = Csprng::new().map_err(memory::error)?;
                let mut secret = [0u8; 16];
                rng.fill(&mut secret);
                // Signing binds to the address a pointer is stored at, so the
                // PAC map is seeded from the image's own writable data.
                let code: Vec<std::ops::Range<u64>> = memory
                    .regions
                    .iter()
                    .filter(|r| r.executable)
                    .map(|r| r.start..r.start + r.data.len() as u64)
                    .collect();
                let mut pac = Pac::new(secret, code, options.hardening.pac);
                let data: Vec<(u64, Vec<u8>)> = memory
                    .regions
                    .iter()
                    .filter(|r| r.writable && !r.executable)
                    .map(|r| (r.start, r.data.clone()))
                    .collect();
                if options.hardening.pac {
                    for (base, bytes) in &data {
                        let mut writes: Vec<(u64, [u8; 8])> = Vec::new();
                        pac.seed_image(*base, bytes, &mut |at, word| writes.push((at, word)));
                        for (at, word) in writes {
                            memory.write(at, &word)?;
                        }
                    }
                }
                let heap = Heap::new(
                    lam.scheme(),
                    rng,
                    options.hardening.tags,
                    options.hardening.random_placement,
                );
                Ok(Self {
                    memory,
                    state,
                    kernel: Kernel::new(deadline),
                    heap,
                    pac,
                    lam,
                    domain: mpk::acquire(),
                    stack: vec![0; 64 * 1024],
                    deadline,
                    last_progress: Instant::now(),
                    native_fault: None,
                })
            },
            |runtime| {
                json!({"entry_rip":format!("0x{:x}",runtime.state[16]),"guest_rsp":format!("0x{:x}",runtime.state[7]),
            "rv64_stack_bytes":runtime.stack.len(),"state_words":runtime.state.len(),"options":options,
            "heap_base":format!("0x{HEAP_BASE:x}"),"heap_bytes":HEAP_SIZE,
            "tag_scheme":runtime.lam.scheme().label,"lam":runtime.lam.label(),"lam_detail":runtime.lam.detail(),
            "mpk":runtime.domain.available(),"mpk_reason":runtime.domain.reason(),
            "hardening":options.hardening,"pac_static_pointers":runtime.pac.stats().static_pointers})
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
        audit::check_stack(&self.state, block.compiled.id, ledger)?;

        match block.compiled.backend {
            Backend::Rv64 => self.execute_rv64(block, report, options, ledger),
            Backend::X86_64 => self.execute_x86_64(block, report, options, ledger),
        }
    }

    fn execute_rv64(
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
        cpu.authenticate_calls = self.pac.enabled();
        let mut bus = ProcessBus {
            code: &block.compiled.code,
            memory: &mut self.memory,
            state: &mut self.state,
            stack: &mut self.stack,
            heap: &mut self.heap,
            pac: &mut self.pac,
        };
        // SAFETY: ProcessBus maps only owned slices and checked guest regions,
        // and every access is tag-checked before it is issued.
        let steps = unsafe {
            cpu.run(
                &mut bus,
                (options.max_rv64_steps - report.rv64_steps).min(100_000) as usize,
            )
        }?;
        report.rv64_steps += steps;
        block.rv64_steps += steps;
        audit::check_stack(&self.state, block.compiled.id, ledger)?;
        report.block_executions += 1;
        report.x86_instructions_executed += block.compiled.source_instructions.len() as u64;
        block.executions += 1;
        // Bounded, time-based progress for hot cached code. No per-instruction I/O.
        if self.last_progress.elapsed() >= Duration::from_millis(250) {
            self.update_counters(report);
            ledger.event("progress",json!({"status":"running","block_id":block.compiled.id,"rip":format!("0x{:x}",block.compiled.rip),
                "next_rip":format!("0x{:x}",self.state[16]),"counters":report}))?;
            self.last_progress = Instant::now();
        }
        Ok(())
    }

    fn execute_x86_64(
        &mut self,
        block: &mut CachedBlock,
        report: &mut Report,
        _options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<()> {
        let jit_page = block
            .jit_page
            .as_ref()
            .ok_or_else(|| memory::error("x86_64 block has no JIT page"))?;

        // Every native access goes through the helpers on this context, so the
        // block is built around a NativeGuest rather than a bare register file.
        let cookie = self as *mut Runtime;
        let mut guest = super::native::NativeGuest::new(cookie);
        guest.regs.copy_from_slice(&self.state);
        self.native_fault = None;

        // Cast the JIT page pointer to a native function and call it.
        // SAFETY: the JIT page contains valid x86-64 machine code compiled
        // from audited/lowered Cranelift IR, the page is marked RX, and the
        // helpers are ordinary C-ABI functions that only touch Runtime.
        let f: NativeBlockFn = unsafe { std::mem::transmute(jit_page.ptr()) };
        let steps = unsafe { f(&mut guest, std::ptr::null()) };

        self.state.copy_from_slice(&guest.regs);
        if let Some((kind, pc)) = guest.take_fault() {
            let err = self
                .native_fault
                .take()
                .map(|(_, e)| e)
                .unwrap_or_else(|| memory::error(format!("native memory fault kind {kind} at 0x{pc:x}")));
            return Err(err.into());
        }
        if let Some((_, err)) = self.native_fault.take() {
            return Err(err.into());
        }

        if steps == super::native::FAULT_STACK as u64 || steps == 2 {
            return Err(memory::error("stack fault in native block").into());
        }

        report.rv64_steps += steps;
        block.rv64_steps += steps;
        audit::check_stack(&self.state, block.compiled.id, ledger)?;
        report.block_executions += 1;
        report.native_x86_instructions_executed += block.compiled.source_instructions.len() as u64;
        block.executions += 1;
        if self.last_progress.elapsed() >= Duration::from_millis(250) {
            self.update_counters(report);
            ledger.event("progress",json!({"status":"running native","block_id":block.compiled.id,"rip":format!("0x{:x}",block.compiled.rip),
                "next_rip":format!("0x{:x}",self.state[16]),"counters":report}))?;
            self.last_progress = Instant::now();
        }
        Ok(())
    }

    /// Checked load for native-compiled blocks. Mirrors the interpreter's
    /// load path: tag screen, then PAC verify on code pointers.
    pub fn native_load(&mut self, guest_ptr: u64, size: u64, pc: u64) -> u64 {
        let biased = guest_ptr.wrapping_add(super::memory::GUEST_BIAS);
        let mut bus = ProcessBus {
            code: &[],
            memory: &mut self.memory,
            state: &mut self.state,
            stack: &mut self.stack,
            heap: &mut self.heap,
            pac: &mut self.pac,
        };
        // SAFETY: ProcessBus maps only owned slices and checked guest regions.
        let loaded = unsafe { bus.load(biased, size as usize, pc) };
        match loaded {
            Ok(raw) => match bus.authenticate_load(biased, size as usize, raw, pc) {
                Ok((value, _)) => value,
                Err(err) => {
                    self.native_fault = Some((pc, err));
                    0
                }
            },
            Err(err) => {
                self.native_fault = Some((pc, err));
                0
            }
        }
    }

    /// Checked store for native-compiled blocks. PAC signing runs first so an
    /// unsigned substitution is refused before any byte is written.
    pub fn native_store(
        &mut self,
        guest_ptr: u64,
        size: u64,
        value: u64,
        constant_provenance: bool,
        pc: u64,
    ) {
        let biased = guest_ptr.wrapping_add(super::memory::GUEST_BIAS);
        let mut bus = ProcessBus {
            code: &[],
            memory: &mut self.memory,
            state: &mut self.state,
            stack: &mut self.stack,
            heap: &mut self.heap,
            pac: &mut self.pac,
        };
        let signed = match bus.authenticate_store(
            biased,
            size as usize,
            value,
            constant_provenance,
            pc,
        ) {
            Ok(v) => v,
            Err(err) => {
                self.native_fault = Some((pc, err));
                return;
            }
        };
        // SAFETY: ProcessBus maps only owned slices and checked guest regions.
        if let Err(err) = unsafe { bus.store(biased, size as usize, signed, pc) } {
            self.native_fault = Some((pc, err));
        }
    }

    /// Serve a call to the guest's own `malloc`/`calloc`/`realloc`/`free`.
    ///
    /// The guest binary is not modified and is not re-executed: the call is
    /// intercepted at its entry address, the return address is popped exactly
    /// as the guest's `call` left it, and the result is placed in rax.
    pub fn allocator_call(
        &mut self,
        function: AllocFn,
        report: &mut Report,
        ledger: &mut RunLedger,
    ) -> Result<()> {
        let rip = self.state[16];
        let size = self.state[5];
        let second = self.state[4];
        let pointer = self.state[5];
        let result: u64 = match function {
            AllocFn::Malloc => self.heap.allocate(size)?,
            AllocFn::Calloc => {
                let total = size.saturating_mul(second);
                let address = self.heap.allocate(total)?;
                let bytes = self.heap.scheme().strip(address);
                if total != 0 {
                    if let Ok(slice) = self.memory.read_mut(bytes, total as usize) {
                        slice.fill(0);
                    }
                }
                address
            }
            AllocFn::Realloc => {
                let old = self.heap.scheme().strip(pointer);
                let address = self.heap.allocate(size)?;
                let copy = self
                    .memory
                    .read(old, size.min(self.live_size(old)) as usize)
                    .map(|b| b.to_vec())
                    .unwrap_or_default();
                self.memory
                    .write(self.heap.scheme().strip(address), &copy)?;
                if pointer != 0 {
                    self.heap.release(pointer, rip)?;
                }
                address
            }
            AllocFn::Free => {
                if pointer != 0 {
                    self.heap.release(pointer, rip)?;
                }
                0
            }
        };
        // Return exactly as the guest's `call` expects.
        let mut rsp = self.state[7];
        let mut returned = [0u8; 8];
        returned.copy_from_slice(self.memory.read(rsp, 8)?);
        rsp = rsp.wrapping_add(8);
        self.state[7] = rsp;
        self.state[0] = result;
        self.state[16] = u64::from_le_bytes(returned);
        report.heap_calls += 1;
        self.update_counters(report);
        ledger.event(
            "allocator_call",
            json!({"rip":format!("0x{rip:x}"),"function":function.name(),
            "size":size,"result":format!("0x{result:x}"),"counters":report}),
        )?;
        Ok(())
    }
    fn live_size(&self, address: u64) -> u64 {
        self.heap.live_size(address).unwrap_or(0)
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
        let outcome = match self
            .kernel
            .call(&mut self.state, &mut self.memory, &mut self.heap)
        {
            Ok(outcome) => outcome,
            Err(error) => {
                ledger.event("syscall_failed",json!({"syscall_seq":report.syscalls,"number":self.state[0],"elapsed_ns":start.elapsed().as_nanos() as u64,"error":error.to_string()}))?;
                return Err(error.into());
            }
        };
        self.update_counters(report);
        ledger.event("syscall_completed",json!({"syscall_seq":report.syscalls,"elapsed_ns":start.elapsed().as_nanos() as u64,
            "detail":outcome.detail,"rv64_steps":report.rv64_steps,"block_executions":report.block_executions}))?;
        Ok(outcome)
    }
    pub fn update_counters(&self, report: &mut Report) {
        self.update_network_counters(report);
        let heap = self.heap.stats();
        report.heap_allocations = heap.allocations;
        report.heap_frees = heap.frees;
        report.heap_live_bytes = heap.live_bytes;
        report.heap_quarantined = heap.quarantined;
        report.heap_tag_faults = heap.tag_faults;
        let pac = self.pac.stats();
        report.pac_signings = pac.signings;
        report.pac_verifications = pac.verifications;
        report.pac_faults = pac.auth_faults;
    }
    pub fn update_network_counters(&self, report: &mut Report) {
        report.accepted_connections = self.kernel.accepted;
        report.network_read_bytes = self.kernel.read_bytes;
        report.network_written_bytes = self.kernel.written_bytes;
    }
    /// Report the final mitigation state, including honest "unavailable" notes.
    pub fn mitigation_summary(&self) -> serde_json::Value {
        let heap = self.heap.stats();
        let pac = self.pac.stats();
        json!({
            "tags": {"enabled": self.heap.tags_enabled(), "scheme": self.heap.scheme().label,
                     "granule_bytes": super::heap::GRANULE, "lam": self.lam.label(),
                     "lam_detail": self.lam.detail(), "faults": heap.tag_faults},
            "pac": {"enabled": self.pac.enabled(), "signed_slots": pac.signed_slots,
                    "static_pointers": pac.static_pointers, "verifications": pac.verifications,
                    "faults": pac.auth_faults},
            "heap": {"base": format!("0x{HEAP_BASE:x}"), "bytes": HEAP_SIZE,
                     "allocations": heap.allocations, "frees": heap.frees,
                     "live_bytes": heap.live_bytes, "quarantined": heap.quarantined,
                     "placement_draws": heap.placement_draws},
            "mpk": {"available": self.domain.available(), "reason": self.domain.reason()},
            // Heap tags and PAC run on ProcessBus, which both the RV64
            // interpreter and the native backend's checked helpers use.
            "mitigation_path": "process_bus",
            "native_execution_path": true,
            "native_note": "native blocks call checked load/store helpers (tags + PAC); syscalls are served by the runtime at block boundaries, never as host syscalls"
        })
    }
}
