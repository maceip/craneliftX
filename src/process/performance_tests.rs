use super::*;

struct Harness {
    runtime: runtime::Runtime,
    cache: cache::BlockCache,
    report: Report,
    ledger: RunLedger,
    root: std::path::PathBuf,
    options: Options,
}
impl Harness {
    fn new(code: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut memory = GuestMemory::default();
        memory.map(0x20000, 4096, false, true).unwrap();
        memory.regions[0].data[..code.len()].copy_from_slice(code);
        memory.map(0x30000, 0x20000, true, false).unwrap();
        memory
            .map(super::super::memory::STACK_BOTTOM, 1024 * 1024, true, false)
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "lockstep-fast-{}-{}-{}",
            std::process::id(),
            crate::ledger::timestamp_ns(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut ledger = RunLedger::create(&root).unwrap();
        let runtime =
            runtime::Runtime::initialize(memory, 0x20000, &Options::default(), &mut ledger)
                .unwrap();
        Self {
            runtime,
            cache: cache::BlockCache::with_backend(Backend::X86_64),
            report: Report::default(),
            ledger,
            root,
            options: Options::default(),
        }
    }
    fn run(&mut self) -> Result<()> {
        self.at(0x20000)
    }
    fn at(&mut self, rip: u64) -> Result<()> {
        self.runtime.state[16] = rip;
        let mut block = self.runtime.with_memory(|memory| {
            self.cache
                .get_or_compile(memory, rip, &mut self.report, &mut self.ledger)
                .unwrap()
                .execution_handle()
        });
        self.runtime.execute(
            &mut block,
            &mut self.report,
            &self.options,
            &mut self.ledger,
        )
    }
    fn write(&self, at: u64, bytes: &[u8]) {
        self.runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .memory
            .write(at, bytes)
            .unwrap();
    }
    fn read(&self, at: u64, bytes: usize) -> Vec<u8> {
        self.runtime
            .with_memory(|m| m.read(at, bytes).unwrap().to_vec())
    }
    fn sign(&self, at: u64) -> u64 {
        let mut space = self.runtime.process.address_space.lock().unwrap();
        let signed = space.pac.on_store(at, 0x20000, 8, true, 0).unwrap();
        space.memory.write(at, &signed.to_le_bytes()).unwrap();
        signed
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
const COPY_WORD: &[u8] = &[0x48, 0x8b, 0x06, 0x48, 0x89, 0x07, 0xeb, 0x00];

#[test]
fn native_cache_hit_shares_code_and_inline_memory_avoids_helpers() {
    let mut h = Harness::new(COPY_WORD);
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x30010;
    h.write(0x30000, &0x1122334455667788u64.to_le_bytes());
    h.run().unwrap();
    let calls = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(
        h.runtime.helper_calls, calls,
        "warm ordinary accesses must execute inline"
    );
    assert_eq!(h.read(0x30010, 8), h.read(0x30000, 8));
    let cached = h.cache.get_mut(0x20000).unwrap();
    cached.executions = 42;
    cached.rv64_steps = 99;
    let handle = cached.execution_handle();
    assert!(std::sync::Arc::ptr_eq(&cached.compiled, &handle.compiled));
    assert_eq!((handle.executions, handle.rv64_steps), (0, 0));
    h.runtime.inline_memory = false;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, (calls.0 + 1, calls.1 + 1));
}

#[test]
fn borrowed_memory_views_invalidate_warm_native_translations() {
    let mut h = Harness::new(COPY_WORD);
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x30010;
    h.write(0x30000, &[0x51; 8]);
    h.run().unwrap();
    let first = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, first);
    // No mapping changed, but the borrowed Rust slice ends the old raw
    // pointer's lifetime. Refill it through the helper before inlining again.
    assert_eq!(h.read(0x30000, 8), [0x51; 8]);
    h.run().unwrap();
    assert!(h.runtime.helper_calls.0 > first.0);
    let second = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, second);
    h.runtime
        .process
        .address_space
        .lock()
        .unwrap()
        .memory
        .read_mut(0x30000, 8)
        .unwrap()
        .fill(0x62);
    h.run().unwrap();
    assert!(h.runtime.helper_calls.0 > second.0);
    assert_eq!(h.read(0x30010, 8), [0x62; 8]);
}

#[test]
fn inline_scalar_widths_match_helpers_at_unaligned_and_page_boundaries() {
    for (width, code) in [
        (1, vec![0x8a, 0x06, 0x88, 0x07, 0xeb, 0x00]),
        (2, vec![0x66, 0x8b, 0x06, 0x66, 0x89, 0x07, 0xeb, 0x00]),
        (4, vec![0x8b, 0x06, 0x89, 0x07, 0xeb, 0x00]),
        (8, COPY_WORD.to_vec()),
    ] {
        let mut fast = Harness::new(&code);
        let mut scalar = Harness::new(&code);
        scalar.runtime.inline_memory = false;
        for offset in [0, 1, 15, 4093, 4095] {
            for h in [&mut fast, &mut scalar] {
                h.write(0x30000 + offset, &[0x51; 8]);
                h.runtime.state[4] = 0x30000 + offset;
                h.runtime.state[5] = 0x40000 + offset;
                h.run().unwrap();
                h.run().unwrap(); // Exercise a populated translation cache.
                assert_eq!(h.read(0x40000 + offset, width), vec![0x51; width]);
            }
            assert_eq!(fast.runtime.state, scalar.runtime.state);
            assert_eq!(fast.runtime.state_prov, scalar.runtime.state_prov);
        }
        // A read-only image load must preserve ProcessBus's trusted provenance,
        // even for narrow accesses and after the mapping cache has warmed.
        for h in [&mut fast, &mut scalar] {
            h.runtime
                .process
                .address_space
                .lock()
                .unwrap()
                .pac
                .set_image_ranges(vec![], vec![0x30000..0x32000]);
            h.run().unwrap();
        }
        assert_eq!(fast.runtime.state, scalar.runtime.state);
        assert_eq!(fast.runtime.state_prov, scalar.runtime.state_prov);
    }
}

#[test]
fn checked_rep_permission_fault_keeps_only_the_completed_prefix() {
    for fast in [true, false] {
        let mut h = Harness::new(&[0xf3, 0xa4, 0xeb, 0x00]);
        h.runtime.inline_memory = fast;
        h.write(0x30000, &[0x51; 8192]);
        h.write(0x40000, &[0xa5; 8192]);
        h.runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .memory
            .mprotect(0x41000, 4096, libc::PROT_READ)
            .unwrap();
        h.runtime.state[4] = 0x30000;
        h.runtime.state[5] = 0x40000;
        h.runtime.state[2] = 8192;
        assert!(h.run().is_err());
        assert_eq!(h.read(0x40000, 4096), vec![0x51; 4096]);
        assert_eq!(h.read(0x41000, 4096), vec![0xa5; 4096]);
        assert_eq!(h.runtime.state[4], 0x31000);
        assert_eq!(h.runtime.state[5], 0x41000);
        assert_eq!(h.runtime.state[2], 4096);
    }
}

#[test]
fn inline_tags_check_both_granules_and_observe_free_with_a_warm_mapping() {
    for scheme in [TagScheme::SOFTWARE, TagScheme::LAM_U57] {
        let mut h = Harness::new(COPY_WORD);
        let pointer = {
            let mut s = h.runtime.process.address_space.lock().unwrap();
            s.heap = Heap::new(scheme, Csprng::from_seed_test(), true, true);
            s.heap.allocate(16).unwrap()
        };
        h.write(scheme.strip(pointer), &[0x51; 16]);
        h.runtime.state[4] = pointer;
        h.runtime.state[5] = 0x30000;
        h.run().unwrap();
        h.write(0x30000, &[0xa5; 8]);
        h.runtime.state[4] = pointer + 12;
        assert!(h.run().unwrap_err().to_string().contains("TagMismatch"));
        assert_eq!(h.read(0x30000, 8), [0xa5; 8]);
        h.runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .heap
            .release(pointer, 0)
            .unwrap();
        h.runtime.state[4] = pointer;
        assert!(h.run().unwrap_err().to_string().contains("UseAfterFree"));
        assert_eq!(h.read(0x30000, 8), [0xa5; 8]);
    }
}

#[test]
fn inline_mapping_cache_observes_mprotect_unmap_and_remap() {
    let mut h = Harness::new(COPY_WORD);
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x40000;
    h.write(0x30000, &[0x51; 8]);
    h.run().unwrap();
    {
        let mut s = h.runtime.process.address_space.lock().unwrap();
        s.memory.mprotect(0x40000, 4096, libc::PROT_READ).unwrap();
    }
    h.write(0x30000, &[0x62; 8]);
    assert!(h.run().is_err());
    assert_eq!(h.read(0x40000, 8), [0x51; 8]);
    {
        let mut s = h.runtime.process.address_space.lock().unwrap();
        s.memory.unmap(0x30000, 4096).unwrap();
        s.memory
            .mprotect(0x40000, 4096, libc::PROT_READ | libc::PROT_WRITE)
            .unwrap();
    }
    assert!(h.run().is_err());
    {
        let mut s = h.runtime.process.address_space.lock().unwrap();
        s.memory.map(0x30000, 4096, true, false).unwrap();
    }
    h.write(0x30000, &[0x73; 8]);
    h.run().unwrap();
    assert_eq!(h.read(0x40000, 8), [0x73; 8]);
}

#[test]
fn inline_pac_classification_updates_and_forged_unmarked_slots_still_fault() {
    // mov [rdi],rax; mov rcx,[rdi]; jmp +0; mov rax,[rsi]; jmp +0
    let code = [
        0x48, 0x89, 0x07, 0x48, 0x8b, 0x0f, 0xeb, 0x00, 0x48, 0x8b, 0x06, 0xeb, 0x00,
    ];
    let mut h = Harness::new(&code);
    h.runtime.state[5] = 0x30000;
    h.runtime.state[0] = 1;
    h.run().unwrap();
    h.runtime.state[0] = 0x20000;
    h.runtime.state_prov = 1;
    h.run().unwrap();
    assert_eq!(h.runtime.state[2], 0x20000);
    assert_eq!((h.runtime.state_prov >> 2) & 1, 1);
    // Attack bypasses mediated stores, as if an earlier narrow corruption had
    // copied a signature into a different, previously ordinary page.
    let signed = h.read(0x30000, 8);
    h.write(0x40000, &signed);
    h.runtime.state[4] = 0x40000;
    assert!(h
        .at(0x20008)
        .unwrap_err()
        .to_string()
        .contains("PointerAuth"));
}

#[test]
fn checked_rep_copies_match_scalar_helpers_for_all_widths() {
    for (width, code) in [
        (1, vec![0xf3, 0xa4, 0xeb, 0x00]),
        (2, vec![0xf3, 0x66, 0xa5, 0xeb, 0x00]),
        (4, vec![0xf3, 0xa5, 0xeb, 0x00]),
        (8, vec![0xf3, 0x48, 0xa5, 0xeb, 0x00]),
    ] {
        let mut fast = Harness::new(&code);
        let mut scalar = Harness::new(&code);
        scalar.runtime.inline_memory = false;
        let bytes = vec![0x51; 8192];
        for h in [&mut fast, &mut scalar] {
            h.write(0x30000, &bytes);
            h.runtime.state[4] = 0x30000;
            h.runtime.state[5] = 0x40000;
            h.runtime.state[2] = 8192 / width;
            h.run().unwrap();
            assert_eq!(h.read(0x40000, 8192), bytes);
        }
        assert_eq!(fast.runtime.state, scalar.runtime.state);
        assert_eq!(fast.runtime.state_prov, scalar.runtime.state_prov);
        assert_eq!(fast.runtime.bulk_fast_bytes, 8192);
        assert_eq!(scalar.runtime.bulk_fast_bytes, 0);
    }
}

#[test]
fn checked_rep_stores_preserve_width_count_and_unrelated_registers() {
    for (width, code) in [
        (1, vec![0xf3, 0xaa, 0xeb, 0x00]),
        (2, vec![0xf3, 0x66, 0xab, 0xeb, 0x00]),
        (4, vec![0xf3, 0xab, 0xeb, 0x00]),
        (8, vec![0xf3, 0x48, 0xab, 0xeb, 0x00]),
    ] {
        let mut h = Harness::new(&code);
        h.runtime.state[0] = 0x5152535455565758;
        h.runtime.state[2] = 4096 / width;
        h.runtime.state[5] = 0x40000;
        h.runtime.state[4] = 0xdead;
        h.run().unwrap();
        assert_eq!(
            h.read(0x40000, 4096),
            h.runtime.state[0].to_le_bytes()[..width as usize].repeat(4096 / width as usize)
        );
        assert_eq!(h.runtime.bulk_fast_bytes, 4096);
        assert_eq!(
            (h.runtime.state[2], h.runtime.state[4], h.runtime.state[5]),
            (0, 0xdead, 0x41000)
        );
    }
}

#[test]
fn bulk_overflow_keeps_valid_prefix_and_stops_before_the_following_store() {
    // rep movsb; mov qword [r8],55h; jmp +0
    let code = [0xf3, 0xa4, 0x49, 0xc7, 0x00, 0x55, 0, 0, 0, 0xeb, 0x00];
    for fast in [false, true] {
        let mut h = Harness::new(&code);
        h.runtime.inline_memory = fast;
        let pointer = h
            .runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .heap
            .allocate(16)
            .unwrap();
        let bare = h
            .runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .heap
            .scheme()
            .strip(pointer);
        h.write(0x30000, &[0x42; 32]);
        h.runtime.state[4] = 0x30000;
        h.runtime.state[5] = pointer;
        h.runtime.state[2] = 32;
        h.runtime.state[8] = 0x40000;
        assert!(h.run().unwrap_err().to_string().contains("TagMismatch"));
        assert_eq!(h.read(bare, 32), [[0x42; 16], [0; 16]].concat());
        assert_eq!(h.read(0x40000, 8), [0; 8]);
        assert_eq!(
            (h.runtime.state[2], h.runtime.state[4], h.runtime.state[5]),
            (16, 0x30010, pointer + 16)
        );
    }
}

#[test]
fn bulk_overlap_zero_count_and_authenticated_pointer_copy_retain_semantics() {
    let mut h = Harness::new(&[0xf3, 0xa4, 0xeb, 0x00]);
    h.write(0x30000, b"abcdefghijklmnopq");
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x30001;
    h.runtime.state[2] = 16;
    h.run().unwrap();
    assert_eq!(h.read(0x30000, 17), [b'a'; 17]);
    assert_eq!(h.runtime.bulk_fast_bytes, 0);
    h.runtime.state[4] = u64::MAX;
    h.runtime.state[5] = u64::MAX;
    h.runtime.state[2] = 0;
    h.runtime.state_prov = (1 << 4) | (1 << 5) | (1 << 2);
    let calls = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, calls);
    assert_eq!(
        h.runtime.state_prov & ((1 << 4) | (1 << 5) | (1 << 2)),
        (1 << 4) | (1 << 5) | (1 << 2)
    );
    let mut h = Harness::new(&[0xf3, 0x48, 0xa5, 0xeb, 0x00]);
    h.sign(0x30000);
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x40000;
    h.runtime.state[2] = 1;
    h.run().unwrap();
    let raw = u64::from_le_bytes(h.read(0x40000, 8).try_into().unwrap());
    assert_eq!(
        h.runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .pac
            .on_load(0x40000, raw, 8, 0)
            .unwrap(),
        0x20000
    );
    assert_eq!(h.runtime.bulk_fast_bytes, 0);
}

#[path = "synchronization_tests.rs"]
mod synchronization;
