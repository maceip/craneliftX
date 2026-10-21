//! Guest initialization, per-thread execution (`Runtime`), shared process state
//! (`SharedProcess`), allocator interception and syscall continuation.
//!
//! Modeled on Blink's `System` / `Machine` split:
//! - `SharedProcess` owns the process-wide address space (`GuestMemory`, `Heap`,
//!   `Pac`), file-descriptor table (`Kernel`), translation cache (`BlockCache`,
//!   `InstructionMap`), and durable ledger (`RunLedger`).
//! - Each guest thread (the initial thread plus every thread created via
//!   `clone(CLONE_THREAD)`) owns its own `Runtime` with independent registers,
//!   provenance bitmap, stack bounds, TID, and `clear_child_tid`.
use super::{
    audit,
    cache::{BlockCache, CachedBlock},
    cfi::InstructionMap,
    csprng::Csprng,
    heap::{Heap, HEAP_BASE, HEAP_SIZE},
    input::{AllocFn, AllocIntercepts},
    jit::NativeBlockFn,
    lam::{self, LamStatus},
    lower::Backend,
    memory::{self, GuestMemory, ProcessBus, CONTEXT_BASE, RV_STACK_BASE, STACK_BOTTOM, STACK_TOP},
    mpk,
    observe::operation,
    pac::Pac,
    syscall::{Kernel, Outcome, SyscallSpace, ThreadSpawn},
    Options, Report, Result,
};
use crate::{
    ledger::RunLedger,
    riscv_exec::{MemoryAccess, RiscvCpu, RETURN_ADDRESS},
    EngineError,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) struct AddressSpace {
    pub memory: GuestMemory,
    pub heap: Heap,
    pub pac: Pac,
    native_owner: Option<u64>,
}

impl AddressSpace {
    fn claim_native(&mut self, tid: u64) {
        if self.native_owner != Some(tid) {
            self.memory.invalidate_native_views();
            self.native_owner = Some(tid);
        }
    }
}

impl SyscallSpace for &Mutex<AddressSpace> {
    fn with<R>(&mut self, f: impl FnOnce(&mut GuestMemory, &mut Heap) -> R) -> R {
        let mut guard = self.lock().unwrap();
        let space = &mut *guard;
        space.native_owner = None;
        f(&mut space.memory, &mut space.heap)
    }
}

pub(super) struct CompileState {
    pub cache: BlockCache,
    pub cfi_map: InstructionMap,
    pub report: Report,
}

pub(super) struct SharedProcess {
    pub address_space: Mutex<AddressSpace>,
    pub kernel: Kernel,
    pub compile_state: Mutex<CompileState>,
    pub ledger: Mutex<RunLedger>,
    pub intercepts: AllocIntercepts,
    pub options: Options,
    pub lam: LamStatus,
    pub domain: mpk::Domain,
    pub next_tid: AtomicU64,
    /// Includes the initial guest thread. Incremented before host spawn,
    /// decremented by a drop guard on every child exit (including errors).
    pub live_threads: AtomicU64,
    /// Native entrants, including those waiting for a block's initial memory
    /// guard. Increment before acquiring that guard so contention is visible.
    pub native_threads: AtomicU64,
    pub aborted: AtomicBool,
    pub exit_code: Mutex<Option<i32>>,
    pub thread_error: Mutex<Option<String>>,
    pub child_handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

#[path = "bulk_memory.rs"]
mod bulk_memory;

pub(super) struct Runtime {
    pub process: Arc<SharedProcess>,
    pub state: [u64; super::CONTEXT_WORDS],
    pub state_prov: u64,
    pub stack_bottom: u64,
    pub stack_top: u64,
    pub tid: u64,
    pub is_child_thread: bool,
    pub is_forked_child: bool,
    pub clear_child_tid: u64,
    pub robust_list_head: u64,
    stack: Vec<u8>,
    deadline: Instant,
    last_progress: Instant,
    /// Set by the native helpers when a tag, PAC or mapping check fails.
    pub native_fault: Option<(u64, EngineError)>,
    /// Non-null only while a native block or checked helper owns
    /// `process.address_space` on this thread.
    active_space: *mut AddressSpace,
    fast_memory: super::fast_memory::FastMemory,
    #[cfg(test)]
    pub native_entry: Option<Arc<dyn Fn(&SharedProcess) + Send + Sync>>,
    #[cfg(test)]
    pub inline_memory: bool,
    #[cfg(test)]
    pub helper_calls: (u64, u64),
    #[cfg(test)]
    pub bulk_fast_bytes: u64,
}

// SAFETY: `active_space` is null outside native execution/checked helper
// scopes, and all shared state is behind `Arc<SharedProcess>`.
unsafe impl Send for Runtime {}

impl Runtime {
    pub fn initialize(
        memory: GuestMemory,
        entry: u64,
        options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<Self> {
        Self::initialize_with_intercepts(
            memory,
            entry,
            AllocIntercepts::default(),
            options,
            ledger,
        )
    }

    pub fn initialize_with_intercepts(
        memory: GuestMemory,
        entry: u64,
        intercepts: AllocIntercepts,
        options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<Self> {
        operation(
            ledger,
            "initialize",
            json!({}),
            |ledger| {
                let mut rng = Csprng::new().map_err(memory::error)?;
                let mut secret = [0u8; 16];
                rng.fill(&mut secret);
                let mut rand_bytes = [0u8; 16];
                rng.fill(&mut rand_bytes);

                let at_random_addr = STACK_TOP - 64;
                let mut string_cursor = STACK_TOP - 4096;
                let mut memory = memory;
                memory.write(at_random_addr, &rand_bytes)?;

                let mut argv_addrs = Vec::new();
                let program_name = "guest";
                let prog_bytes = [program_name.as_bytes(), b"\0"].concat();
                memory.write(string_cursor, &prog_bytes)?;
                argv_addrs.push(string_cursor);
                string_cursor += prog_bytes.len() as u64;

                for arg in &options.args {
                    let arg_bytes = [arg.as_bytes(), b"\0"].concat();
                    memory.write(string_cursor, &arg_bytes)?;
                    argv_addrs.push(string_cursor);
                    string_cursor += arg_bytes.len() as u64;
                }

                let mut env_addrs = Vec::new();
                for (k, v) in &options.env {
                    let env_str = format!("{k}={v}\0");
                    memory.write(string_cursor, env_str.as_bytes())?;
                    env_addrs.push(string_cursor);
                    string_cursor += env_str.len() as u64;
                }

                let mut stack_words = Vec::new();
                stack_words.push(argv_addrs.len() as u64); // argc
                for addr in argv_addrs {
                    stack_words.push(addr); // argv[i]
                }
                stack_words.push(0); // argv NULL
                for addr in env_addrs {
                    stack_words.push(addr); // envp[i]
                }
                stack_words.push(0); // envp NULL

                let auxv = [
                    3, memory.phdr,     // AT_PHDR
                    4, memory.phent,    // AT_PHENT
                    5, memory.phnum,    // AT_PHNUM
                    6, 4096,            // AT_PAGESZ
                    7, 0,               // AT_BASE
                    8, 0,               // AT_FLAGS
                    9, entry,           // AT_ENTRY
                    11, 1000,           // AT_UID
                    12, 1000,           // AT_EUID
                    13, 1000,           // AT_GID
                    14, 1000,           // AT_EGID
                    17, 100,            // AT_CLKTCK
                    23, 0,              // AT_SECURE
                    25, at_random_addr, // AT_RANDOM
                    0, 0,               // AT_NULL
                ];
                stack_words.extend_from_slice(&auxv);

                let sp = (STACK_TOP - 4096 - (stack_words.len() * 8) as u64) & !15;
                for (i, word) in stack_words.iter().enumerate() {
                    memory.write(sp + (i * 8) as u64, &word.to_le_bytes())?;
                }

                let mut state = [0u64; super::CONTEXT_WORDS];
                state[7] = sp;
                state[16] = entry;
                state[17] = 2;
                state[18] = 0;
                let deadline = Instant::now() + Duration::from_secs(options.max_seconds);
                // The heap is an ordinary writable guest mapping. Its metadata
                // is not: Heap keeps every free list, size and tag in Rust.
                memory.map(HEAP_BASE, HEAP_SIZE, true, false)?;
                // Optional LAM kernel capability: enabled by default when the
                // CPU and kernel allow it; software tags otherwise.
                let lam = lam::enable(options.hardening.lam);
                // Signing binds to the address a pointer is stored at, so the
                // PAC map is seeded from the image's own writable data.
                let code: Vec<std::ops::Range<u64>> = memory
                    .regions
                    .iter()
                    .filter(|r| r.executable)
                    .map(|r| r.start..r.start + r.data.len() as u64)
                    .collect();
                let image_ranges: Vec<std::ops::Range<u64>> = memory
                    .regions
                    .iter()
                    .filter(|r| r.writable && !r.executable && r.start < HEAP_BASE)
                    .map(|r| r.start..r.start + r.data.len() as u64)
                    .collect();
                let readonly_ranges: Vec<std::ops::Range<u64>> = memory
                    .regions
                    .iter()
                    .filter(|r| !r.writable)
                    .map(|r| r.start..r.start + r.data.len() as u64)
                    .collect();
                let mut pac = Pac::new(secret, code, options.hardening.pac);
                pac.set_image_ranges(image_ranges, readonly_ranges);
                let data: Vec<(u64, Vec<u8>)> = memory
                    .regions
                    .iter()
                    .filter(|r| r.writable && !r.executable && r.start < HEAP_BASE)
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
                    // Sign any initial stack words (such as AT_ENTRY) that name code.
                    for (i, &word) in stack_words.iter().enumerate() {
                        let slot_addr = sp + (i * 8) as u64;
                        if let Ok(signed) = pac.on_store(slot_addr, word, 8, true, entry) {
                            if signed != word {
                                pac.set_stack_provenance(slot_addr, 8, true);
                                memory.write(slot_addr, &signed.to_le_bytes())?;
                            }
                        }
                    }
                }
                let heap = Heap::new(
                    lam.scheme(),
                    rng,
                    options.hardening.tags,
                    options.hardening.random_placement,
                );
                let initial_brk = memory
                    .regions
                    .iter()
                    .filter(|r| r.start < STACK_BOTTOM)
                    .map(|r| ((r.start + r.data.len() as u64 + 4095) / 4096) * 4096)
                    .max()
                    .unwrap_or(0x100_0000);
                let host_pid = unsafe { libc::getpid() as u64 };
                let shared = Arc::new(SharedProcess {
                    address_space: Mutex::new(AddressSpace { memory, heap, pac, native_owner: None }),
                    kernel: Kernel::new(deadline, initial_brk),
                    compile_state: Mutex::new(CompileState {
                        cache: BlockCache::with_backend(options.backend),
                        cfi_map: InstructionMap::new(),
                        report: Report::default(),
                    }),
                    ledger: Mutex::new(ledger.try_clone()?),
                    intercepts,
                    options: options.clone(),
                    lam,
                    domain: mpk::acquire(),
                    next_tid: AtomicU64::new(host_pid.saturating_add(1000).max(2000)),
                    live_threads: AtomicU64::new(1),
                    native_threads: AtomicU64::new(0),
                    aborted: AtomicBool::new(false),
                    exit_code: Mutex::new(None),
                    thread_error: Mutex::new(None),
                    child_handles: Mutex::new(Vec::new()),
                });
                Ok(Self {
                    process: shared,
                    state,
                    state_prov: 0,
                    stack_bottom: STACK_BOTTOM,
                    stack_top: STACK_TOP,
                    tid: host_pid,
                    is_child_thread: false,
                    is_forked_child: false,
                    clear_child_tid: 0,
                    robust_list_head: 0,
                    stack: vec![0; 64 * 1024],
                    deadline,
                    last_progress: Instant::now(),
                    native_fault: None,
                    active_space: std::ptr::null_mut(),
                    fast_memory: Default::default(),
                    #[cfg(test)]
                    native_entry: None,
                    #[cfg(test)]
                    inline_memory: true,
                    #[cfg(test)]
                    helper_calls: (0, 0),
                    #[cfg(test)]
                    bulk_fast_bytes: 0,
                })
            },
            |runtime| {
                let space = runtime.process.address_space.lock().unwrap();
                json!({"entry_rip":format!("0x{:x}",runtime.state[16]),"guest_rsp":format!("0x{:x}",runtime.state[7]),
            "rv64_stack_bytes":runtime.stack.len(),"state_words":runtime.state.len(),"options":options,
            "heap_base":format!("0x{HEAP_BASE:x}"),"heap_bytes":HEAP_SIZE,
            "tag_scheme":runtime.process.lam.scheme().label,"lam":runtime.process.lam.label(),"lam_detail":runtime.process.lam.detail(),
            "mpk":runtime.process.domain.available(),"mpk_reason":runtime.process.domain.reason(),
            "hardening":options.hardening,"pac_static_pointers":space.pac.stats().static_pointers})
            },
        )
    }

    pub fn with_memory<R>(&self, f: impl FnOnce(&GuestMemory) -> R) -> R {
        let space = self.process.address_space.lock().unwrap();
        f(&space.memory)
    }

    pub fn dispatch(&self, rip: u64, ledger: &mut RunLedger) -> Result<CachedBlock> {
        let mut cs = self.process.compile_state.lock().unwrap();
        if self.process.options.hardening.cfi && !cs.cfi_map.allows(rip) {
            return Err(memory::error(format!(
                "CFI violation: transfer to 0x{rip:x} lands inside a known instruction"
            )).into());
        }
        let CompileState { cache, cfi_map, report } = &mut *cs;
        let handle = if let Some(cached) = cache.get_mut(rip) {
            cached.execution_handle()
        } else {
            let space = self.process.address_space.lock().unwrap();
            let cached = cache.get_or_compile(&space.memory, rip, report, ledger)?;
            cfi_map.record_block(&cached.compiled.source_instructions);
            cached.cfi_recorded = true;
            cached.execution_handle()
        };
        cache.mark_executed(rip);
        Ok(handle)
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
        let process = self.process.clone();
        struct NativeEntry<'a>(&'a AtomicU64);
        impl Drop for NativeEntry<'_> {
            fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Release); }
        }
        let _entry = if block.compiled.backend == Backend::X86_64 {
            process.native_threads.fetch_add(1, Ordering::AcqRel);
            Some(NativeEntry(&process.native_threads))
        } else { None };
        match block.compiled.backend {
            Backend::Rv64 => {
                {
                    let space = process.address_space.lock().unwrap();
                    (self.stack_bottom, self.stack_top) = space.memory.stack_bounds_for_sp(self.state[7]);
                }
                audit::check_stack(&self.state, self.stack_bottom, self.stack_top, block.compiled.id, ledger)?;
                self.execute_rv64(block, report, options, ledger)
            }
            Backend::X86_64 => self.execute_x86_64(block, report, options, ledger, &process),
        }
    }

    fn execute_rv64(
        &mut self,
        block: &mut CachedBlock,
        report: &mut Report,
        options: &Options,
        ledger: &mut RunLedger,
    ) -> Result<()> {
        let steps = {
            let mut space_guard = self.process.address_space.lock().unwrap();
            let space = &mut *space_guard;
            let (sb, st) = space.memory.stack_bounds_for_sp(self.state[7]);
            self.stack_bottom = sb;
            self.stack_top = st;
            let mut cpu = RiscvCpu::default();
            cpu.regs[1] = RETURN_ADDRESS;
            cpu.regs[2] = RV_STACK_BASE + self.stack.len() as u64;
            cpu.regs[10] = CONTEXT_BASE;
            cpu.authenticate_calls = space.pac.enabled();
            let mut bus = ProcessBus {
                code: &block.compiled.code,
                memory: &mut space.memory,
                state: &mut self.state,
                state_prov: &mut self.state_prov,
                stack: &mut self.stack,
                heap: &mut space.heap,
                pac: &mut space.pac,
                stack_bottom: self.stack_bottom,
                stack_top: self.stack_top,
            };
            // SAFETY: ProcessBus maps only owned slices and checked guest regions,
            // and every access is tag-checked before it is issued.
            unsafe {
                cpu.run(
                    &mut bus,
                    (options.max_rv64_steps - report.rv64_steps).min(100_000) as usize,
                )
            }?
        };
        report.rv64_steps += steps;
        block.rv64_steps += steps;
        {
            let space = self.process.address_space.lock().unwrap();
            let (sb, st) = space.memory.stack_bounds_for_sp(self.state[7]);
            self.stack_bottom = sb;
            self.stack_top = st;
        }
        audit::check_stack(
            &self.state,
            self.stack_bottom,
            self.stack_top,
            block.compiled.id,
            ledger,
        )?;
        report.block_executions += 1;
        report.x86_instructions_executed += block.compiled.source_instructions.len() as u64;
        block.executions += 1;
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
        options: &Options,
        ledger: &mut RunLedger,
        process_arc: &Arc<SharedProcess>,
    ) -> Result<()> {
        let jit_page = block
            .jit_page
            .as_ref()
            .ok_or_else(|| memory::error("x86_64 block has no JIT page"))?
            .clone();

        let concurrent = !block.compiled.atomic_memory && match options.thread_sync {
            super::ThreadSync::Block => false,
            super::ThreadSync::Access => process_arc.live_threads.load(Ordering::Acquire) > 1,
            super::ThreadSync::Adaptive => process_arc.native_threads.load(Ordering::Acquire) > 1,
        };
        // Ordinary multi-threaded blocks carry no borrowed guest-memory views.
        // Each helper checks and accesses under the same mutex. Atomic blocks
        // and the single-threaded fast path retain the whole-block guard.
        let mut space_guard = if concurrent {
            None
        } else {
            Some(process_arc.address_space.lock().unwrap())
        };
        (self.stack_bottom, self.stack_top) = if let Some(space) = space_guard.as_ref() {
            space.memory.stack_bounds_for_sp(self.state[7])
        } else {
            process_arc.address_space.lock().unwrap().memory.stack_bounds_for_sp(self.state[7])
        };
        audit::check_stack(&self.state, self.stack_bottom, self.stack_top, block.compiled.id, ledger)?;
        let cookie = self as *mut Runtime;
        let mut guest = super::native::NativeGuest::new(cookie);
        guest.cookie = (&mut guest as *mut super::native::NativeGuest).cast();
        guest.regs.copy_from_slice(&self.state);
        guest.provenance = self.state_prov;
        guest.stack_bottom = self.stack_bottom;
        guest.stack_top = self.stack_top;
        guest.fast_enabled = u64::from(!concurrent);
        if let Some(space) = space_guard.as_mut() {
            space.claim_native(self.tid);
            self.active_space = &mut **space as *mut AddressSpace;
            self.fast_memory.refresh(&space.memory);
            guest.fast_pages = self.fast_memory.as_ptr();
            guest.shadow = space.heap.shadow_ptr();
            guest.pac_pages = space.pac.slow_pages_ptr();
            guest.tag_mask = space.heap.scheme().mask();
            guest.tag_shift = space.heap.scheme().shift as u64;
            guest.tags_enabled = u64::from(space.heap.tags_enabled());
            guest.pac_enabled = u64::from(space.pac.enabled());
        }
        #[cfg(test)]
        { guest.fast_enabled &= u64::from(self.inline_memory); }
        self.native_fault = None;

        // SAFETY: RX code came from audited Cranelift IR. In concurrent mode
        // FAST_ENABLED is zero, so no inline host pointer can be dereferenced.
        // Scalar and bulk callbacks lock before checking or accessing memory.
        // For LOCK/XCHG the guard spans the entire RMW, including fault paths.
        let f: NativeBlockFn = unsafe { std::mem::transmute(jit_page.ptr()) };
        #[cfg(test)]
        if let Some(entry) = &self.native_entry { entry(&process_arc); }
        let steps = unsafe { f(&mut guest, std::ptr::null()) };
        self.active_space = std::ptr::null_mut();
        self.state.copy_from_slice(&guest.regs);
        self.state_prov = guest.provenance;
        let fault = guest.take_fault();
        (self.stack_bottom, self.stack_top) = if let Some(space) = space_guard.as_ref() {
            space.memory.stack_bounds_for_sp(self.state[7])
        } else {
            process_arc.address_space.lock().unwrap().memory.stack_bounds_for_sp(self.state[7])
        };
        drop(space_guard);
        if concurrent {
            report.native_concurrent_blocks += 1;
        } else {
            report.native_serialized_blocks += 1;
        }

        if let Some((kind, pc)) = fault {
            let err = self
                .native_fault
                .take()
                .map(|(_, e)| e)
                .unwrap_or_else(|| {
                    memory::error(format!("native memory fault kind {kind} at 0x{pc:x}"))
                });
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
        audit::check_stack(
            &self.state,
            self.stack_bottom,
            self.stack_top,
            block.compiled.id,
            ledger,
        )?;
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

    /// Helpers may mutate the translation table and PAC classification, or
    /// borrow a guest-memory slice. Re-expose pointers after those Rust borrows
    /// end, before translated code can inspect any host metadata again.
    pub(super) fn refresh_native_views(&mut self, guest: &mut super::native::NativeGuest) {
        // Concurrent callbacks release their guard before returning; never
        // publish dangling views into a NativeGuest running outside the lock.
        if self.active_space.is_null() { return; }
        // SAFETY: the serialized/atomic block still owns address_space.
        let space = unsafe { &*self.active_space };
        self.fast_memory.refresh(&space.memory);
        guest.fast_pages = self.fast_memory.as_ptr();
        guest.pac_pages = space.pac.slow_pages_ptr();
        guest.shadow = space.heap.shadow_ptr();
    }

    /// Keep each check + access + metadata update indivisible with respect to
    /// free, reuse, unmap, permissions and every other guest memory operation.
    fn with_native_space<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        if !self.active_space.is_null() { return f(self); }
        let process = self.process.clone();
        let mut space = process.address_space.lock().unwrap();
        space.claim_native(self.tid);
        self.active_space = &mut *space;
        struct Reset(*mut *mut AddressSpace);
        impl Drop for Reset {
            fn drop(&mut self) {
                // SAFETY: the Runtime remains at this address for this scoped
                // call. Reset drops before the mutex guard, including unwind.
                unsafe { *self.0 = std::ptr::null_mut(); }
            }
        }
        let _reset = Reset(&raw mut self.active_space);
        f(self)
    }

    /// Checked load for native-compiled blocks. Mirrors the interpreter's
    /// load path: tag screen, then PAC verify on code pointers.
    pub fn native_load(&mut self, guest_ptr: u64, size: u64, pc: u64) -> (u64, bool) {
        self.with_native_space(|rt| rt.native_load_locked(guest_ptr, size, pc))
    }

    fn native_load_locked(&mut self, guest_ptr: u64, size: u64, pc: u64) -> (u64, bool) {
        #[cfg(test)]
        { self.helper_calls.0 += 1; }
        let biased = guest_ptr.wrapping_add(super::memory::GUEST_BIAS);
        assert!(
            !self.active_space.is_null(),
            "native_load outside active block"
        );
        // SAFETY: active_space points to the MutexGuard<AddressSpace> held by
        // execute_x86_64 on this thread.
        let space = unsafe { &mut *self.active_space };
        self.fast_memory.fill(&mut space.memory, space.heap.scheme().strip(guest_ptr));
        let mut bus = ProcessBus {
            code: &[],
            memory: &mut space.memory,
            state: &mut self.state,
            state_prov: &mut self.state_prov,
            stack: &mut self.stack,
            heap: &mut space.heap,
            pac: &mut space.pac,
            stack_bottom: self.stack_bottom,
            stack_top: self.stack_top,
        };
        let loaded = unsafe { bus.load(biased, size as usize, pc) };
        match loaded {
            Ok(raw) => match bus.authenticate_load(biased, size as usize, raw, pc) {
                Ok((value, prov)) => (value, prov),
                Err(err) => {
                    self.native_fault = Some((pc, err));
                    (0, false)
                }
            },
            Err(err) => {
                self.native_fault = Some((pc, err));
                (0, false)
            }
        }
    }

    /// Checked store for native-compiled blocks. PAC signing runs first so an
    /// unsigned substitution is refused before any byte is written.
    pub fn native_store(&mut self, guest_ptr: u64, size: u64, value: u64, constant_provenance: bool, pc: u64) {
        self.with_native_space(|rt| rt.native_store_locked(guest_ptr, size, value, constant_provenance, pc));
    }

    fn native_store_locked(
        &mut self,
        guest_ptr: u64,
        size: u64,
        value: u64,
        constant_provenance: bool,
        pc: u64,
    ) {
        #[cfg(test)]
        { self.helper_calls.1 += 1; }
        let biased = guest_ptr.wrapping_add(super::memory::GUEST_BIAS);
        assert!(
            !self.active_space.is_null(),
            "native_store outside active block"
        );
        // SAFETY: active_space points to the MutexGuard<AddressSpace> held by
        // execute_x86_64 on this thread.
        let space = unsafe { &mut *self.active_space };
        self.fast_memory.fill(&mut space.memory, space.heap.scheme().strip(guest_ptr));
        let mut bus = ProcessBus {
            code: &[],
            memory: &mut space.memory,
            state: &mut self.state,
            state_prov: &mut self.state_prov,
            stack: &mut self.stack,
            heap: &mut space.heap,
            pac: &mut space.pac,
            stack_bottom: self.stack_bottom,
            stack_top: self.stack_top,
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
        if let Err(err) = unsafe { bus.store(biased, size as usize, signed, pc) } {
            self.native_fault = Some((pc, err));
        }
    }

    /// Serve a call to the guest's own allocator (`malloc`/`calloc`/`realloc`/`free`/
    /// `posix_memalign`/`aligned_alloc`/`memalign`).
    pub fn allocator_call(
        &mut self,
        function: AllocFn,
        report: &mut Report,
        ledger: &mut RunLedger,
    ) -> Result<()> {
        let rip = self.state[16];
        let arg0 = self.state[5]; // rdi
        let arg1 = self.state[4]; // rsi
        let arg2 = self.state[3]; // rdx
        let (result, returned_rip) = {
            let mut space_guard = self.process.address_space.lock().unwrap();
            let space = &mut *space_guard;
            let scheme = space.heap.scheme();
            // Pad libc allocator requests by one 64-byte SIMD line so glibc's
            // 64-byte unrolled SSE2 string routines (__strlen_sse2, __strchr_sse2)
            // do not fault on speculative in-page vector loads past a short string.
            let pad_simd = |sz: u64| sz.max(1).saturating_add(48).div_ceil(64) * 64;
            let res: u64 = match function {
                AllocFn::Malloc => {
                    let req = pad_simd(arg0);
                    let address = space.heap.allocate(req)?;
                    let bare = scheme.strip(address);
                    let actual_size = space.heap.live_size(bare).unwrap_or(req);
                    space.pac.clear_range(bare, actual_size);
                    address
                }
                AllocFn::Calloc => {
                    let total = arg0.saturating_mul(arg1);
                    let req = pad_simd(total);
                    let address = space.heap.allocate(req)?;
                    let bytes = scheme.strip(address);
                    let actual_size = space.heap.live_size(bytes).unwrap_or(req);
                    space.pac.clear_range(bytes, actual_size);
                    if req != 0 {
                        if let Ok(slice) = space.memory.read_mut(bytes, req as usize) {
                            slice.fill(0);
                        }
                    }
                    address
                }
                AllocFn::Realloc => {
                    let pointer = arg0;
                    let size = arg1;
                    if pointer == 0 {
                        let req = pad_simd(size);
                        let address = space.heap.allocate(req)?;
                        let bare = scheme.strip(address);
                        let actual_size = space.heap.live_size(bare).unwrap_or(req);
                        space.pac.clear_range(bare, actual_size);
                        address
                    } else if size == 0 {
                        let old = scheme.strip(pointer);
                        let old_size = space.heap.live_size(old).unwrap_or(0);
                        space.pac.clear_range(old, old_size);
                        space.heap.release(pointer, rip)?;
                        0
                    } else {
                        let old = scheme.strip(pointer);
                        let old_size = space.heap.live_size(old).unwrap_or(0);
                        let req = pad_simd(size);
                        let address = space.heap.allocate(req)?;
                        let new_bare = scheme.strip(address);
                        let new_size = space.heap.live_size(new_bare).unwrap_or(req);
                        space.pac.clear_range(new_bare, new_size);
                        let copy_len = req.min(old_size) as usize;
                        let copy = space
                            .memory
                            .read(old, copy_len)
                            .map(|b| b.to_vec())
                            .unwrap_or_default();
                        space.memory.write(new_bare, &copy)?;
                        // Re-sign any PAC-signed code pointers inside the reallocated block.
                        if space.pac.enabled() {
                            let mut off = 0usize;
                            while off + 8 <= copy_len {
                                let old_slot = old + off as u64;
                                if space.pac.is_signed_slot(old_slot) {
                                    let raw = u64::from_le_bytes(
                                        copy[off..off + 8].try_into().unwrap(),
                                    );
                                    if let Ok(plain) = space.pac.on_load(old_slot, raw, 8, rip) {
                                        if let Ok(resigned) = space.pac.on_store(
                                            new_bare + off as u64,
                                            plain,
                                            8,
                                            true,
                                            rip,
                                        ) {
                                            let _ = space.memory.write(
                                                new_bare + off as u64,
                                                &resigned.to_le_bytes(),
                                            );
                                        }
                                    }
                                }
                                off += 8;
                            }
                        }
                        space.pac.clear_range(old, old_size);
                        space.heap.release(pointer, rip)?;
                        address
                    }
                }
                AllocFn::Free => {
                    let pointer = arg0;
                    if pointer != 0 {
                        let old = scheme.strip(pointer);
                        let old_size = space.heap.live_size(old).unwrap_or(0);
                        space.pac.clear_range(old, old_size);
                        space.heap.release(pointer, rip)?;
                    }
                    0
                }
                AllocFn::PosixMemalign => {
                    let memptr = scheme.strip(arg0);
                    let alignment = arg1;
                    let size = pad_simd(arg2);
                    if alignment == 0 || !alignment.is_power_of_two() || alignment % 8 != 0 {
                        22 // EINVAL
                    } else {
                        match space.heap.allocate_aligned(alignment.max(64), size) {
                            Ok(ptr) => {
                                let bare = scheme.strip(ptr);
                                let actual_size = space.heap.live_size(bare).unwrap_or(size);
                                space.pac.clear_range(bare, actual_size);
                                space.memory.write(memptr, &ptr.to_le_bytes())?;
                                0
                            }
                            Err(_) => 12, // ENOMEM
                        }
                    }
                }
                AllocFn::AlignedAlloc | AllocFn::Memalign => {
                    let req = pad_simd(arg1);
                    let ptr = space.heap.allocate_aligned(arg0.max(64), req)?;
                    let bare = scheme.strip(ptr);
                    let actual_size = space.heap.live_size(bare).unwrap_or(req);
                    space.pac.clear_range(bare, actual_size);
                    ptr
                }
            };
            let rsp = self.state[7];
            let mut returned = [0u8; 8];
            returned.copy_from_slice(space.memory.read(rsp, 8)?);
            let ret_addr = u64::from_le_bytes(returned);
            let plain_ret = if space.pac.is_signed_slot(rsp) {
                space.pac.on_load(rsp, ret_addr, 8, rip)?
            } else {
                ret_addr
            };
            space.pac.unmark_stack_slot(rsp);
            (res, plain_ret)
        };
        self.state[7] = self.state[7].wrapping_add(8);
        self.state[0] = result;
        self.state_prov &= !(1u64 << 0);
        self.state[16] = returned_rip;
        report.heap_calls += 1;
        self.update_counters(report);
        ledger.event(
            "allocator_call",
            json!({"rip":format!("0x{rip:x}"),"function":function.name(),
            "size":arg0,"result":format!("0x{result:x}"),"counters":report}),
        )?;
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
            "next_rip":format!("0x{:x}",self.state[16]),"syscall_seq":report.syscalls,"rv64_steps":report.rv64_steps,"block_executions":report.block_executions,"tid":self.tid}))?;
        let start = Instant::now();
        let mut space_ref = &self.process.address_space;
        let outcome = match self.process.kernel.call_shared(
            &mut self.state,
            self.tid,
            self.is_child_thread,
            &mut self.clear_child_tid,
            &mut self.robust_list_head,
            Some(&self.process.next_tid),
            &mut space_ref,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                ledger.event("syscall_failed",json!({"syscall_seq":report.syscalls,"number":self.state[0],"elapsed_ns":start.elapsed().as_nanos() as u64,"error":error.to_string(),"tid":self.tid}))?;
                return Err(error.into());
            }
        };
        self.state_prov &= !(1u64 << 0);
        if outcome.forked_child {
            // Host fork retains only its calling thread in the child process.
            self.process.live_threads.store(1, Ordering::Release);
            self.process.native_threads.store(0, Ordering::Release);
            self.is_forked_child = true;
            self.tid = unsafe { libc::getpid() as u64 };
            let child_ledger = ledger.fork_child()?;
            *ledger = child_ledger.try_clone()?;
            *self.process.ledger.lock().unwrap() = child_ledger;
        }
        if let Some(spawn) = outcome.spawn_thread.clone() {
            self.spawn_child_thread(spawn, ledger)?;
        }
        if outcome.thread_exit {
            self.finish_child_thread(ledger)?;
        }
        self.update_counters(report);
        ledger.event("syscall_completed",json!({"syscall_seq":report.syscalls,"elapsed_ns":start.elapsed().as_nanos() as u64,
            "detail":outcome.detail,"rv64_steps":report.rv64_steps,"block_executions":report.block_executions,"tid":self.tid}))?;
        Ok(outcome)
    }

    fn spawn_child_thread(&self, spawn: ThreadSpawn, ledger: &mut RunLedger) -> Result<()> {
        const CLONE_SETTLS: u64 = 0x0008_0000;
        const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;

        let mut child_state = self.state;
        child_state[0] = 0; // rax = 0 in child thread
        child_state[7] = spawn.child_stack; // rsp = child_stack
        child_state[2] = self.state[16]; // rcx = next_rip
        child_state[11] = self.state[17]; // r11 = rflags
        if (spawn.flags & CLONE_SETTLS) != 0 {
            child_state[18] = spawn.tls;
        }
        let child_prov = self.state_prov & !((1u64 << 0) | (1u64 << 7));
        let (stack_bottom, stack_top) = {
            let space = self.process.address_space.lock().unwrap();
            space.memory.stack_bounds_for_sp(spawn.child_stack)
        };
        let clear_child_tid = if (spawn.flags & CLONE_CHILD_CLEARTID) != 0 {
            spawn.ctid
        } else {
            0
        };
        ledger.event(
            "thread_started",
            json!({
                "tid": spawn.child_tid,
                "parent_tid": self.tid,
                "rip": format!("0x{:x}", child_state[16]),
                "rsp": format!("0x{:x}", child_state[7]),
                "fs_base": format!("0x{:x}", child_state[18]),
                "stack_bottom": format!("0x{stack_bottom:x}"),
                "stack_top": format!("0x{stack_top:x}"),
            }),
        )?;

        let mut child_runtime = Runtime {
            process: self.process.clone(),
            state: child_state,
            state_prov: child_prov,
            stack_bottom,
            stack_top,
            tid: spawn.child_tid,
            is_child_thread: true,
            is_forked_child: self.is_forked_child,
            clear_child_tid,
            robust_list_head: 0,
            stack: vec![0; 64 * 1024],
            deadline: self.deadline,
            last_progress: Instant::now(),
            native_fault: None,
            active_space: std::ptr::null_mut(),
            fast_memory: Default::default(),
            #[cfg(test)]
            native_entry: None,
            #[cfg(test)]
            inline_memory: true,
            #[cfg(test)]
            helper_calls: (0, 0),
            #[cfg(test)]
            bulk_fast_bytes: 0,
        };

        struct LiveThread(Arc<SharedProcess>);
        impl Drop for LiveThread {
            fn drop(&mut self) {
                // Invalidate views before restoring the single-threaded fast
                // path. No pointer exposed by the departing thread survives.
                self.0.address_space.lock().unwrap().memory.invalidate_native_views();
                self.0.live_threads.fetch_sub(1, Ordering::Release);
            }
        }
        self.process.live_threads.fetch_add(1, Ordering::AcqRel);
        let live = LiveThread(self.process.clone());
        let handle = std::thread::Builder::new()
            .name(format!("isr-tid-{}", spawn.child_tid))
            .spawn(move || {
                let _live = live;
                if let Err(err) = child_runtime.run_child_loop() {
                    let msg = err.to_string();
                    let mut slot = child_runtime.process.thread_error.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(msg);
                    }
                    child_runtime.process.aborted.store(true, Ordering::SeqCst);
                }
            })
            .map_err(memory::error)?;
        self.process.child_handles.lock().unwrap().push(handle);
        Ok(())
    }

    fn finish_child_thread(&mut self, ledger: &mut RunLedger) -> Result<()> {
        if self.robust_list_head != 0 {
            let head = self.robust_list_head;
            let mut wakes = Vec::new();
            {
                let mut space = self.process.address_space.lock().unwrap();
                if let Ok(hdr) = space.memory.read(head, 24).map(|b| b.to_vec()) {
                    let mut entry = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
                    let futex_offset = i64::from_le_bytes(hdr[8..16].try_into().unwrap());
                    for _ in 0..64 {
                        if entry == 0 || entry == head {
                            break;
                        }
                        let futex_addr = (entry as i64).wrapping_add(futex_offset) as u64;
                        if let Ok(word_bytes) = space.memory.read(futex_addr, 4) {
                            let val = u32::from_le_bytes(word_bytes.try_into().unwrap());
                            if (val & 0x3fff_ffff) == (self.tid as u32) {
                                let new_val = (val & 0x8000_0000) | 0x4000_0000;
                                let _ = space.memory.write(futex_addr, &new_val.to_le_bytes());
                                wakes.push(futex_addr);
                            }
                        }
                        let Ok(next_bytes) = space.memory.read(entry, 8) else {
                            break;
                        };
                        entry = u64::from_le_bytes(next_bytes.try_into().unwrap());
                    }
                }
            }
            for addr in wakes {
                self.process.kernel.futex_manager.wake(addr, 1, u32::MAX);
            }
        }
        if self.clear_child_tid != 0 {
            let ctid = self.clear_child_tid;
            self.clear_child_tid = 0;
            {
                let mut space = self.process.address_space.lock().unwrap();
                let _ = space.memory.write(ctid, &0u32.to_le_bytes());
            }
            self.process.kernel.futex_manager.wake(ctid, 1, u32::MAX);
        }
        ledger.event("thread_exited", json!({"tid": self.tid}))?;
        Ok(())
    }

    fn run_child_loop(&mut self) -> Result<()> {
        let mut local_ledger = self.process.ledger.lock().unwrap().try_clone()?;
        let mut local_report = Report::default();
        let options = self.process.options.clone();
        loop {
            if self.process.aborted.load(Ordering::Relaxed)
                || self.process.exit_code.lock().unwrap().is_some()
            {
                break;
            }
            self.check_budget(&local_report, &options)?;
            let rip = self.state[16];
            if let Some(function) = self.process.intercepts.by_address.get(&rip) {
                self.allocator_call(*function, &mut local_report, &mut local_ledger)?;
                continue;
            }
            let mut block = self.dispatch(rip, &mut local_ledger)?;
            self.execute(&mut block, &mut local_report, &options, &mut local_ledger)?;
            if let Some(syscall_rip) = block.compiled.syscall {
                let outcome =
                    self.syscall(rip, syscall_rip, &mut local_report, &mut local_ledger)?;
                if outcome.thread_exit {
                    break;
                }
                if let Some(code) = outcome.exit {
                    *self.process.exit_code.lock().unwrap() = Some(code);
                    break;
                }
            }
        }
        local_ledger.event("thread_execution_totals", json!({"tid": self.tid, "counters": local_report}))?;
        Ok(())
    }

    pub fn update_counters(&self, report: &mut Report) {
        self.update_network_counters(report);
        let space = self.process.address_space.lock().unwrap();
        let heap = space.heap.stats();
        report.heap_allocations = heap.allocations;
        report.heap_frees = heap.frees;
        report.heap_live_bytes = heap.live_bytes;
        report.heap_quarantined = heap.quarantined;
        report.heap_tag_faults = heap.tag_faults;
        let pac = space.pac.stats();
        report.pac_signings = pac.signings;
        report.pac_verifications = pac.verifications;
        report.pac_faults = pac.auth_faults;
    }

    pub fn update_network_counters(&self, report: &mut Report) {
        report.accepted_connections = self.process.kernel.accepted();
        report.network_read_bytes = self.process.kernel.read_bytes();
        report.network_written_bytes = self.process.kernel.written_bytes();
    }

    /// Report the final mitigation state, including honest "unavailable" notes.
    pub fn mitigation_summary(&self) -> serde_json::Value {
        let space = self.process.address_space.lock().unwrap();
        let heap = space.heap.stats();
        let pac = space.pac.stats();
        json!({
            "tags": {"enabled": space.heap.tags_enabled(), "scheme": space.heap.scheme().label,
                     "granule_bytes": super::heap::GRANULE, "lam": self.process.lam.label(),
                     "lam_detail": self.process.lam.detail(), "faults": heap.tag_faults},
            "pac": {"enabled": space.pac.enabled(), "signed_slots": pac.signed_slots,
                    "static_pointers": pac.static_pointers, "verifications": pac.verifications,
                    "faults": pac.auth_faults},
            "heap": {"base": format!("0x{HEAP_BASE:x}"), "bytes": HEAP_SIZE,
                     "allocations": heap.allocations, "frees": heap.frees,
                     "live_bytes": heap.live_bytes, "quarantined": heap.quarantined,
                     "placement_draws": heap.placement_draws},
            "mpk": {"available": self.process.domain.available(), "reason": self.process.domain.reason()},
            "thread_sync": self.process.options.thread_sync,
            "memory_order": "serialized checked accesses; LOCK and memory XCHG hold a block-wide guard",
            "mitigation_path": "process_bus",
            "native_execution_path": self.process.options.backend == Backend::X86_64,
            "native_note": "single-threaded and atomic blocks can inline mapping/tag checks under a block guard; concurrent ordinary blocks use checked memory helpers under per-access guards; bulk spans, PAC/provenance and mediated syscalls retain their checks"
        })
    }
}
