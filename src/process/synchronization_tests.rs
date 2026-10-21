//! Exercise translated code on real host threads, including the implicit
//! XCHG lock and memory lifecycle changes between already-compiled blocks.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn pair(code: &[u8]) -> (Harness, Harness) {
    let mut a = Harness::new(code);
    let mut b = Harness::new(code);
    a.options.thread_sync = ThreadSync::Access;
    b.options.thread_sync = ThreadSync::Access;
    b.runtime.process = a.runtime.process.clone();
    b.runtime.tid = a.runtime.tid + 1;
    a.runtime.process.live_threads.store(2, Ordering::Release);
    (a, b)
}

#[test]
fn native_threads_enter_without_holding_the_memory_lock() {
    // mov eax,42; jmp next -- no memory access is needed by this block.
    let (mut a, mut b) = pair(&[0xb8, 42, 0, 0, 0, 0xeb, 0]);
    let entered = Arc::new(AtomicUsize::new(0));
    for h in [&mut a, &mut b] {
        let entered = entered.clone();
        h.runtime.native_entry = Some(Arc::new(move |process| {
            assert!(
                process.address_space.try_lock().is_ok(),
                "ordinary native entry retained the process lock"
            );
            entered.fetch_add(1, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(5);
            while entered.load(Ordering::SeqCst) != 2 {
                assert!(
                    Instant::now() < deadline,
                    "second translated thread could not enter"
                );
                std::thread::yield_now();
            }
        }));
    }
    let child = std::thread::spawn(move || {
        b.run().unwrap();
        assert_eq!(b.runtime.state[0], 42);
    });
    a.run().unwrap();
    child.join().unwrap();
    assert_eq!(a.runtime.state[0], 42);
    assert_eq!(a.report.native_concurrent_blocks, 1);
}

#[test]
fn contended_lock_xadd_is_atomic_for_all_supported_widths_and_unaligned_words() {
    for (width, prefix) in [
        (1, vec![0xf0, 0x0f, 0xc0, 0x07]),
        (2, vec![0xf0, 0x66, 0x0f, 0xc1, 0x07]),
        (4, vec![0xf0, 0x0f, 0xc1, 0x07]),
        (8, vec![0xf0, 0x48, 0x0f, 0xc1, 0x07]),
    ] {
        let mut code = vec![0xb8, 1, 0, 0, 0];
        code.extend(prefix);
        code.extend([0xeb, 0]);
        let (mut a, mut b) = pair(&code);
        // Cross a page boundary, and deliberately avoid natural alignment.
        let address = 0x30fff;
        for h in [&mut a, &mut b] {
            h.runtime.state[5] = address;
        }
        let worker = |mut h: Harness| {
            let mut seen = Vec::new();
            for _ in 0..100 {
                h.run().unwrap();
                seen.push(h.runtime.state[0]);
            }
            assert_eq!(h.report.native_concurrent_blocks, 0);
            assert_eq!(h.report.native_serialized_blocks, 100);
            seen
        };
        let process = a.runtime.process.clone();
        let child = std::thread::spawn(move || worker(b));
        let mut values = worker(a);
        values.extend(child.join().unwrap());
        values.sort_unstable();
        assert_eq!(
            values,
            (0..200).collect::<Vec<_>>(),
            "lost an update at width {width}"
        );
        let mut bytes = [0u8; 8];
        bytes[..width].copy_from_slice(
            process
                .address_space
                .lock()
                .unwrap()
                .memory
                .read(address, width)
                .unwrap(),
        );
        assert_eq!(u64::from_le_bytes(bytes), 200);
    }
}

#[test]
fn memory_xchg_is_atomic_without_an_explicit_lock_prefix() {
    let (mut a, mut b) = pair(&[0x48, 0x87, 0x07, 0xeb, 0]);
    for h in [&mut a, &mut b] {
        h.runtime.state[5] = 0x30000;
    }
    let process = a.runtime.process.clone();
    let worker = |mut h: Harness, start: u64| {
        let mut values = Vec::new();
        for value in start..start + 600 {
            h.runtime.state[0] = value;
            h.run().unwrap();
            values.push(h.runtime.state[0]);
        }
        assert_eq!(h.report.native_serialized_blocks, 600);
        values
    };
    let child = std::thread::spawn(move || worker(b, 601));
    let mut values = worker(a, 1);
    values.extend(child.join().unwrap());
    values.push(u64::from_le_bytes(
        process
            .address_space
            .lock()
            .unwrap()
            .memory
            .read(0x30000, 8)
            .unwrap()
            .try_into()
            .unwrap(),
    ));
    values.sort_unstable();
    assert_eq!(values, (0..=1200).collect::<Vec<_>>());
}

#[test]
fn locked_cmpxchg_preserves_success_failure_and_flags_under_contention() {
    let (mut a, mut b) = pair(&[0xf0, 0x48, 0x0f, 0xb1, 0x0f, 0xeb, 0]);
    for h in [&mut a, &mut b] {
        h.runtime.state[5] = 0x30000;
    }
    let process = a.runtime.process.clone();
    let worker = |mut h: Harness| {
        let mut expected = 0;
        for _ in 0..600 {
            loop {
                h.runtime.state[0] = expected;
                h.runtime.state[2] = expected + 1;
                h.run().unwrap();
                if h.runtime.state[17] & (1 << 6) != 0 {
                    expected += 1;
                    break;
                }
                expected = h.runtime.state[0];
            }
        }
    };
    let child = std::thread::spawn(move || worker(b));
    worker(a);
    child.join().unwrap();
    assert_eq!(
        u64::from_le_bytes(
            process
                .address_space
                .lock()
                .unwrap()
                .memory
                .read(0x30000, 8)
                .unwrap()
                .try_into()
                .unwrap()
        ),
        1200
    );
}

#[test]
fn concurrent_accesses_recheck_free_permissions_and_mapping_before_writing() {
    // mov [rdi],rax; mov byte [rsi],0x7f; jmp next
    let (mut a, b) = pair(&[0x48, 0x89, 0x07, 0xc6, 0x06, 0x7f, 0xeb, 0]);
    let process = a.runtime.process.clone();
    let ptr = process
        .address_space
        .lock()
        .unwrap()
        .heap
        .allocate(32)
        .unwrap();
    a.runtime.state[5] = ptr;
    a.runtime.state[4] = 0x30000;
    a.runtime.state[0] = 0x1234;
    a.run().unwrap();
    a.write(0x30000, &[0]);
    let child = std::thread::spawn(move || {
        b.runtime
            .process
            .address_space
            .lock()
            .unwrap()
            .heap
            .release(ptr, 0)
            .unwrap();
    });
    child.join().unwrap();
    assert!(a.run().unwrap_err().to_string().contains("UseAfterFree"));
    assert_eq!(a.read(0x30000, 1), [0], "store after the fault executed");
    a.runtime.state[5] = 0x40000;
    {
        let mut space = process.address_space.lock().unwrap();
        space.memory.mprotect(0x40000, 4096, 1).unwrap();
    }
    assert!(a.run().is_err());
    assert_eq!(a.read(0x30000, 1), [0]);
    {
        let mut space = process.address_space.lock().unwrap();
        space.memory.unmap(0x40000, 4096).unwrap();
        space.memory.map(0x40000, 4096, true, false).unwrap();
    }
    a.run().unwrap();
    assert_eq!(a.read(0x40000, 8), 0x1234u64.to_le_bytes());
    assert_eq!(a.read(0x30000, 1), [0x7f]);
}

#[test]
fn concurrent_mode_keeps_pointer_authentication_and_bulk_valid_prefixes() {
    let (mut a, _b) = pair(COPY_WORD);
    a.sign(0x30000);
    a.runtime.state[4] = 0x30000;
    a.runtime.state[5] = 0x30010;
    a.run().unwrap(); // Authenticated load may be signed at the new destination.
    a.write(0x30000, &0x20000u64.to_le_bytes());
    let old = a.read(0x30010, 8);
    assert!(a.run().unwrap_err().to_string().contains("PointerAuth"));
    assert_eq!(a.read(0x30010, 8), old);
    let (mut copy, _other) = pair(&[0xf3, 0xa4, 0xc6, 0x02, 0x7f, 0xeb, 0]);
    let ptr = copy
        .runtime
        .process
        .address_space
        .lock()
        .unwrap()
        .heap
        .allocate(32)
        .unwrap();
    copy.write(0x30000, &[0x42; 48]);
    copy.runtime.state[4] = 0x30000;
    copy.runtime.state[5] = ptr;
    copy.runtime.state[2] = 48;
    copy.runtime.state[3] = 0x30080;
    assert!(copy.run().unwrap_err().to_string().contains("TagMismatch"));
    assert_eq!(copy.runtime.state[2], 16);
    assert_eq!(copy.read(0x30080, 1), [0]);
}

#[test]
fn mappings_can_change_while_another_thread_executes_checked_stores() {
    let (mut a, b) = pair(&[0x48, 0x89, 0x07, 0xc6, 0x06, 0x7f, 0xeb, 0]);
    a.runtime.state[5] = 0x40000;
    a.runtime.state[4] = 0x30000;
    a.runtime.state[0] = 0x1234;
    let ready = Arc::new(std::sync::Barrier::new(2));
    let other_ready = ready.clone();
    let child = std::thread::spawn(move || {
        other_ready.wait();
        for _ in 0..1000 {
            {
                let mut space = b.runtime.process.address_space.lock().unwrap();
                space.memory.unmap(0x40000, 4096).unwrap();
            }
            std::thread::yield_now();
            {
                let mut space = b.runtime.process.address_space.lock().unwrap();
                space.memory.map(0x40000, 4096, false, false).unwrap();
            }
            std::thread::yield_now();
            b.runtime
                .process
                .address_space
                .lock()
                .unwrap()
                .memory
                .mprotect(0x40000, 4096, 3)
                .unwrap();
        }
    });
    ready.wait();
    for _ in 0..1000 {
        a.write(0x30000, &[0]);
        let result = a.run();
        assert_eq!(
            a.read(0x30000, 1),
            [if result.is_ok() { 0x7f } else { 0 }],
            "execution continued after a mapping/permission fault"
        );
    }
    child.join().unwrap();
}

#[test]
fn ordered_guest_stores_publish_data_before_the_ready_flag() {
    let mut code = vec![
        0x48, 0x89, 0x37, 0x0f, 0xae, 0xf0, 0xc7, 0x47, 0x08, 1, 0, 0, 0, 0xeb, 0,
    ];
    code.resize(0x100, 0x90);
    code.extend([
        0x8b, 0x47, 0x08, 0x0f, 0xae, 0xe8, 0x48, 0x8b, 0x17, 0xeb, 0,
    ]);
    let (mut a, mut b) = pair(&code);
    for h in [&mut a, &mut b] {
        h.runtime.state[5] = 0x30000;
    }
    let phase = Arc::new(std::sync::Barrier::new(2));
    let reader_phase = phase.clone();
    let child = std::thread::spawn(move || {
        for expected in 1..=100 {
            reader_phase.wait();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                b.at(0x20100).unwrap();
                if b.runtime.state[0] == 1 {
                    assert_eq!(b.runtime.state[3], expected);
                    break;
                }
                assert!(Instant::now() < deadline, "publisher stalled");
                std::thread::yield_now();
            }
            reader_phase.wait();
        }
    });
    for value in 1..=100 {
        a.write(0x30000, &[0; 16]);
        a.runtime.state[4] = value;
        phase.wait();
        a.run().unwrap();
        phase.wait();
    }
    child.join().unwrap();
}

#[test]
fn returning_to_one_thread_refills_then_reuses_inline_views() {
    let (mut a, mut b) = pair(COPY_WORD);
    for h in [&mut a, &mut b] {
        h.runtime.state[4] = 0x30000;
        h.runtime.state[5] = 0x30010;
    }
    a.write(0x30000, &0x1234u64.to_le_bytes());
    a.run().unwrap();
    b.run().unwrap();
    drop(b);
    a.runtime.process.live_threads.store(1, Ordering::Release);
    a.run().unwrap();
    let calls = a.runtime.helper_calls;
    a.run().unwrap();
    assert_eq!(a.runtime.helper_calls, calls);
    assert_eq!(a.read(0x30010, 8), 0x1234u64.to_le_bytes());
}

#[test]
fn adaptive_uses_concurrent_checks_only_with_another_native_entrant() {
    let mut h = Harness::new(COPY_WORD);
    h.runtime.state[4] = 0x30000;
    h.runtime.state[5] = 0x30010;
    h.run().unwrap();
    h.run().unwrap();
    let calls = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, calls);
    assert_eq!(h.report.native_concurrent_blocks, 0);
    // A peer has entered the executor, before its initial stack/memory guard.
    h.runtime.process.native_threads.store(1, Ordering::Release);
    h.run().unwrap();
    assert_eq!(h.report.native_concurrent_blocks, 1);
    assert_eq!(h.runtime.helper_calls, (calls.0 + 1, calls.1 + 1));
    assert_eq!(h.runtime.process.native_threads.load(Ordering::Acquire), 1);
    h.runtime.process.native_threads.store(0, Ordering::Release);
    h.run().unwrap();
    let refilled = h.runtime.helper_calls;
    h.run().unwrap();
    assert_eq!(h.runtime.helper_calls, refilled);
    assert_eq!(h.runtime.process.native_threads.load(Ordering::Acquire), 0);
}
