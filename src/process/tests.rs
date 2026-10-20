use super::heap::Heap;
use super::lam::TagScheme;
use super::memory::{GuestMemory, ProcessBus, CONTEXT_BASE, RV_STACK_BASE};
use super::memory::{ADDRESS_LIMIT, GUEST_BIAS, STACK_TOP};
use super::pac::Pac;
use super::*;
use crate::process::csprng::Csprng;
use crate::EngineError;
use crate::{
    codegen::compile_to_riscv64,
    harvest::harvest_bytes,
    riscv_exec::{RiscvCpu, RETURN_ADDRESS},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn execute(
    code: &[u8],
    state: &mut [u64; super::CONTEXT_WORDS],
    memory: &mut GuestMemory,
) -> std::result::Result<u64, EngineError> {
    execute_hardened(code, state, memory, false)
}
fn execute_hardened(
    code: &[u8],
    state: &mut [u64; super::CONTEXT_WORDS],
    memory: &mut GuestMemory,
    tags: bool,
) -> std::result::Result<u64, EngineError> {
    let mut stack = vec![0u8; 65536];
    let mut heap = Heap::new(TagScheme::SOFTWARE, Csprng::from_seed_test(), tags, true);
    let mut pac = Pac::new([0x5a; 16], Vec::new(), false);
    let mut state_prov = 0u64;
    let mut bus = ProcessBus {
        code,
        memory,
        state,
        state_prov: &mut state_prov,
        stack: &mut stack,
        heap: &mut heap,
        pac: &mut pac,
        stack_bottom: super::memory::STACK_BOTTOM,
        stack_top: super::memory::STACK_TOP,
    };
    let mut cpu = RiscvCpu::default();
    cpu.regs[1] = RETURN_ADDRESS;
    cpu.regs[2] = RV_STACK_BASE + 65536;
    cpu.regs[10] = CONTEXT_BASE;
    // SAFETY: ProcessBus bounds-checks every access to its owned mappings.
    unsafe { cpu.run(&mut bus, 100_000) }
}
fn compile(bytes: &[u8], ip: u64) -> Vec<u8> {
    let (trace, _) = harvest_bytes(bytes, ip, 32).unwrap();
    compile_to_riscv64(
        &lower::lower_block(
            &trace,
            lower::Backend::Rv64,
            &Diversification::identity(),
            None,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn native_pac_fault_stops_the_block_before_later_stores() {
    let mut memory = GuestMemory::default();
    memory.map(0x20000, 4096, false, true).unwrap();
    // mov eax,20000h; mov [rdi],rax; mov qword [rsi],55h; syscall
    let code = [
        0xb8, 0, 0, 2, 0, 0x48, 0x89, 0x07, 0x48, 0xc7, 0x06, 0x55, 0, 0, 0, 0x0f, 0x05,
    ];
    memory.regions[0].data[..code.len()].copy_from_slice(&code);
    memory.map(0x30000, 4096, true, false).unwrap();
    memory.write(0x30000, &0x20000u64.to_le_bytes()).unwrap();
    memory
        .map(super::memory::STACK_BOTTOM, 1024 * 1024, true, false)
        .unwrap();
    let root = std::env::temp_dir().join(format!(
        "isr-pac-block-{}-{}",
        std::process::id(),
        crate::ledger::timestamp_ns()
    ));
    let mut ledger = RunLedger::create(&root).unwrap();
    let options = Options::default();
    let mut runtime = runtime::Runtime::initialize(memory, 0x20000, &options, &mut ledger).unwrap();
    runtime.state[5] = 0x30000;
    runtime.state[4] = 0x30008;
    let mut cache = cache::BlockCache::with_backend(Backend::X86_64);
    let mut report = Report::default();
    let mut block = runtime.with_memory(|mem| {
        cache
            .get_or_compile(mem, 0x20000, &mut report, &mut ledger)
            .unwrap()
            .clone()
    });
    let error = runtime
        .execute(&mut block, &mut report, &options, &mut ledger)
        .unwrap_err();
    assert!(error.to_string().contains("PointerAuth"));
    assert_eq!(
        runtime.with_memory(|mem| mem.read(0x30008, 8).unwrap().to_vec()),
        vec![0; 8]
    );
    assert_eq!(runtime.state[16], 0x20008); // next RIP; the fault reports the store's PC
    assert!(matches!(
        error.downcast_ref::<EngineError>(),
        Some(EngineError::Mitigation { pc: 0x20005, .. })
    ));
    drop(ledger);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn arithmetic_flags_match_integer_reference_at_all_operand_widths() {
    for width in [8, 16, 32, 64] {
        for operation in ["add", "sub", "cmp", "test"] {
            let opcode = match operation {
                "add" => 0x00,
                "sub" => 0x28,
                "cmp" => 0x38,
                _ => 0x84,
            };
            let mut bytes = match width {
                16 => vec![0x66],
                64 => vec![0x48],
                _ => vec![],
            };
            bytes.extend([opcode + u8::from(width != 8), 0xc8]);
            let code = compile(&bytes, 0x20000);
            let mask = if width == 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            let sign = 1u64 << (width - 1);
            for a in [
                0,
                1,
                sign - 1,
                sign,
                mask - 1,
                mask,
                0xa5a5a5a5a5a5a5a5 & mask,
            ] {
                for b in [0, 1, sign - 1, sign, mask] {
                    let mut state = [0u64; super::CONTEXT_WORDS];
                    state[0] = a;
                    state[2] = b;
                    state[17] = 0x202;
                    execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
                    let sub = operation == "sub" || operation == "cmp";
                    let logical = operation == "test";
                    let result = if sub {
                        a.wrapping_sub(b)
                    } else if logical {
                        a & b
                    } else {
                        a.wrapping_add(b)
                    } & mask;
                    let carry = !logical
                        && if sub {
                            a < b
                        } else {
                            a as u128 + b as u128 > mask as u128
                        };
                    let overflow = !logical
                        && if sub {
                            ((a ^ b) & (a ^ result) & sign) != 0
                        } else {
                            (!(a ^ b) & (a ^ result) & sign) != 0
                        };
                    let auxiliary = !logical && (a ^ b ^ result) & 16 != 0;
                    let flags = u64::from(carry)
                        | (u64::from((result as u8).count_ones().is_multiple_of(2)) << 2)
                        | (u64::from(auxiliary) << 4)
                        | (u64::from(result == 0) << 6)
                        | (u64::from(result & sign != 0) << 7)
                        | (u64::from(overflow) << 11)
                        | 0x202;
                    assert_eq!(state[17], flags, "{operation}/{width}: {a:x}, {b:x}");
                    assert_eq!(
                        state[0],
                        if operation == "cmp" || logical {
                            a
                        } else {
                            result
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn calls_stack_loop_branch_and_indirect_return_execute_in_rv64() {
    // mov ecx,5; xor eax,eax; call subroutine; dec ecx; jne call; ret
    // subroutine: push rbx; mov ebx,3; add eax,ebx; pop rbx; ret
    let bytes = [
        0xb9, 5, 0, 0, 0, 0x31, 0xc0, 0xe8, 5, 0, 0, 0, 0xff, 0xc9, 0x75, 0xf7, 0xc3, 0x53, 0xbb,
        3, 0, 0, 0, 0x01, 0xd8, 0x5b, 0xc3,
    ];
    let mut memory = GuestMemory::default();
    memory.map(0x20000, 4096, false, true).unwrap();
    memory.regions[0].data[..bytes.len()].copy_from_slice(&bytes);
    memory.map(STACK_TOP - 4096, 4096, true, false).unwrap();
    memory
        .write(STACK_TOP - 16, &0x40000u64.to_le_bytes())
        .unwrap();
    let mut state = [0u64; super::CONTEXT_WORDS];
    state[7] = STACK_TOP - 16;
    state[16] = 0x20000;
    state[17] = 2;
    state[1] = 0x12345678;
    let mut cache = BTreeMap::new();
    let mut total_steps = 0;
    for _ in 0..100 {
        let rip = state[16];
        if rip == 0x40000 {
            break;
        }
        let code = cache
            .entry(rip)
            .or_insert_with(|| compile(memory.fetch(rip).unwrap(), rip));
        total_steps += execute(code, &mut state, &mut memory).unwrap();
    }
    assert_eq!(state[16], 0x40000);
    assert_eq!(state[0], 15);
    assert_eq!(state[1], 0x12345678);
    assert_eq!(state[7], STACK_TOP - 8);
    assert!(total_steps > 100);
}

#[test]
fn guest_access_cannot_escape_to_private_context_stack_or_code() {
    for bytes in [&[0x48, 0x8b, 0x07][..], &[0x48, 0x89, 0x07][..]] {
        let code = compile(bytes, 0x20000);
        for address in [
            0,
            0x1000,
            CONTEXT_BASE,
            CONTEXT_BASE - GUEST_BIAS,
            RV_STACK_BASE,
            u64::MAX,
            ADDRESS_LIMIT,
        ] {
            let mut state = [0u64; super::CONTEXT_WORDS];
            state[5] = address;
            assert!(
                execute(&code, &mut state, &mut GuestMemory::default()).is_err(),
                "{address:x}"
            );
        }
    }
    let mut memory = GuestMemory::default();
    memory.map(0x20000, 4096, false, true).unwrap();
    assert!(memory.write(0x20000, &[1]).is_err());
    assert!(memory.map(0x20000, 4096, true, false).is_err());
    assert!(memory.map(0x30000, 4096, true, true).is_err());
    memory.map(0x30000, 4096, true, false).unwrap();
    assert!(memory.fetch(0x30000).is_err());
    assert!(memory.read(0x30fff, 2).is_err());
}

#[test]
fn signed_multiply_flags_and_partial_registers_are_preserved() {
    let code = compile(&[0x48, 0x0f, 0xaf, 0xc1], 0x20000); // imul rax,rcx
    for (a, b) in [
        (2i64, 3i64),
        (-2, 3),
        (i64::MIN, 1),
        (i64::MAX, 2),
        (i64::MIN, -1),
    ] {
        let mut state = [0u64; super::CONTEXT_WORDS];
        state[0] = a as u64;
        state[2] = b as u64;
        state[17] = 0x202;
        execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
        let expected = a as i128 * b as i128;
        let overflow = i64::try_from(expected).is_err();
        assert_eq!(state[0], expected as u64);
        assert_eq!(state[17] & 0x801, if overflow { 0x801 } else { 0 });
    }
    let code = compile(&[0x88, 0xcc, 0x66, 0xb8, 0x12, 0x34], 0x20000); // mov ah,cl; mov ax,3412h
    let mut state = [0u64; super::CONTEXT_WORDS];
    state[0] = u64::MAX;
    state[2] = 0x77;
    execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
    assert_eq!(state[0], 0xffff_ffff_ffff_3412);
}

#[test]
fn every_integer_condition_uses_preserved_flags() {
    for condition in 0..16u8 {
        let code = compile(&[0x0f, 0x80 + condition, 64, 0, 0, 0], 0x20000);
        for bits in 0..32 {
            let c = bits & 1 != 0;
            let p = bits & 2 != 0;
            let z = bits & 4 != 0;
            let s = bits & 8 != 0;
            let o = bits & 16 != 0;
            let expected = match condition {
                0 => o,
                1 => !o,
                2 => c,
                3 => !c,
                4 => z,
                5 => !z,
                6 => c || z,
                7 => !c && !z,
                8 => s,
                9 => !s,
                10 => p,
                11 => !p,
                12 => s != o,
                13 => s == o,
                14 => z || s != o,
                _ => !z && s == o,
            };
            let mut state = [0u64; super::CONTEXT_WORDS];
            state[17] = 2
                | u64::from(c)
                | (u64::from(p) << 2)
                | (u64::from(z) << 6)
                | (u64::from(s) << 7)
                | (u64::from(o) << 11);
            let flags = state[17];
            execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
            assert_eq!(state[16], 0x20006 + if expected { 64 } else { 0 });
            assert_eq!(state[17], flags);
        }
    }
}

#[test]
fn rotates_inc_dec_and_unsupported_stack_widths() {
    for left in [true, false] {
        for count in [0, 1, 7, 8, 9, 31, 32] {
            let code = compile(&[0xc0, if left { 0xc0 } else { 0xc8 }, count], 0x20000);
            for value in [0u8, 1, 0x7f, 0x80, 0xff] {
                let mut state = [0u64; super::CONTEXT_WORDS];
                state[0] = 0xaabbcc00 | u64::from(value);
                state[17] = 0xa03;
                execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
                let count = u32::from(count) & 31;
                let result = if left {
                    value.rotate_left(count)
                } else {
                    value.rotate_right(count)
                };
                assert_eq!(state[0], 0xaabbcc00 | u64::from(result));
                assert_eq!(state[17] & !0x801, 0xa03 & !0x801);
                if count == 0 {
                    assert_eq!(state[17], 0xa03);
                } else {
                    assert_eq!(
                        state[17] & 1,
                        if left {
                            u64::from(result & 1)
                        } else {
                            u64::from(result >> 7)
                        }
                    );
                }
                if count == 1 {
                    let overflow = if left {
                        (result >> 7) ^ (result & 1)
                    } else {
                        (result >> 7) ^ ((result >> 6) & 1)
                    };
                    assert_eq!((state[17] >> 11) & 1, u64::from(overflow));
                }
            }
        }
    }
    for bytes in [[0x48, 0xff, 0xc0], [0x48, 0xff, 0xc8]] {
        let code = compile(&bytes, 0x20000);
        for carry in [0, 1] {
            let mut state = [0u64; super::CONTEXT_WORDS];
            state[0] = u64::MAX;
            state[17] = 2 | carry;
            execute(&code, &mut state, &mut GuestMemory::default()).unwrap();
            assert_eq!(state[17] & 1, carry);
        }
    }
    for bytes in [
        &[0x66, 0x50][..],
        &[0x66, 0x68, 0, 0][..],
        &[0xcb][..],
        &[0x0f, 0x0b][..],
    ] {
        let (trace, _) = harvest_bytes(bytes, 0x20000, 32).unwrap();
        assert!(
            lower::lower_block(
                &trace,
                lower::Backend::Rv64,
                &Diversification::identity(),
                None
            )
            .is_err(),
            "{bytes:x?}: {trace:?}"
        );
    }
}

#[test]
fn kernel_keeps_guest_descriptors_private_and_rejects_unsupported_services() {
    let mut kernel = syscall::Kernel::new(Instant::now() + Duration::from_secs(1), 0x40000);
    let mut memory = GuestMemory::default();
    memory.map(0x30000, 4096, true, false).unwrap();
    let mut state = [0u64; super::CONTEXT_WORDS];
    let invoke = |kernel: &mut syscall::Kernel,
                  state: &mut [u64; super::CONTEXT_WORDS],
                  memory: &mut GuestMemory,
                  n,
                  a,
                  b,
                  c| {
        state[0] = n;
        state[5] = a;
        state[4] = b;
        state[3] = c;
        kernel.call(
            state,
            memory,
            &mut Heap::new(TagScheme::SOFTWARE, Csprng::from_seed_test(), false, true),
        )
    };
    invoke(&mut kernel, &mut state, &mut memory, 41, 2, 1, 0).unwrap();
    assert_eq!(state[0], 3);
    invoke(&mut kernel, &mut state, &mut memory, 41, 2, 1, 0).unwrap();
    assert_eq!(state[0], 4);
    // Closing guest fd 1 must not close the runner's stdout.
    invoke(&mut kernel, &mut state, &mut memory, 3, 1, 0, 0).unwrap();
    assert_eq!(state[0], (-9i64) as u64);
    let mut address = [0u8; 16];
    address[0] = 2;
    address[4..8].copy_from_slice(&[10, 0, 0, 1]);
    memory.write(0x30000, &address).unwrap();
    invoke(&mut kernel, &mut state, &mut memory, 49, 3, 0x30000, 16).unwrap();
    assert_eq!(state[0], (-13i64) as u64);
    invoke(&mut kernel, &mut state, &mut memory, 3, 3, 0, 0).unwrap();
    assert_eq!(state[0], 0);
    invoke(&mut kernel, &mut state, &mut memory, 41, 2, 1, 0).unwrap();
    assert_eq!(state[0], 3);
    // Unknown syscall numbers return -ENOSYS rather than killing the guest.
    invoke(&mut kernel, &mut state, &mut memory, 999, 0, 0, 0).unwrap();
    assert_eq!(state[0], (-38i64) as u64);
    // Writes into the private context window are still refused.
    assert!(invoke(&mut kernel, &mut state, &mut memory, 1, 1, CONTEXT_BASE, 1).is_err());
    assert_eq!(kernel.open_descriptors(), 2);
}

#[test]
fn pac_provenance_survives_register_and_xmm_copies_across_blocks_for_seeded_slots() {
    let mut memory = GuestMemory::default();
    memory.map(0x20000, 4096, false, true).unwrap();
    // Block 1 (ends at jmp +0):
    // mov rax, [rdi]       ; load seeded function pointer from image slot 0x30000
    // movq xmm0, rax       ; move into xmm0
    // jmp +0               ; block boundary -> provenance must survive in state_prov
    // Block 2:
    // movq rcx, xmm0       ; move back to rcx
    // mov [rsi], rcx       ; store authenticated function pointer to new slot 0x30008
    // mov eax, 60; xor edi, edi; syscall
    let code = [
        0x48, 0x8b, 0x07, // mov rax, [rdi]
        0x66, 0x48, 0x0f, 0x6e, 0xc0, // movq xmm0, rax
        0xeb, 0x00, // jmp $+2
        0x66, 0x48, 0x0f, 0x7e, 0xc1, // movq rcx, xmm0
        0x48, 0x89, 0x0e, // mov [rsi], rcx
        0xb8, 60, 0, 0, 0, 0x31, 0xff, 0x0f, 0x05,
    ];
    memory.regions[0].data[..code.len()].copy_from_slice(&code);
    memory.map(0x30000, 4096, true, false).unwrap();
    memory.write(0x30000, &0x20000u64.to_le_bytes()).unwrap();
    memory
        .map(super::memory::STACK_BOTTOM, 1024 * 1024, true, false)
        .unwrap();
    let root = std::env::temp_dir().join(format!(
        "isr-pac-prov-{}-{}",
        std::process::id(),
        crate::ledger::timestamp_ns()
    ));
    let mut ledger = RunLedger::create(&root).unwrap();
    let options = Options {
        backend: Backend::Rv64,
        ..Options::default()
    };
    let mut runtime = runtime::Runtime::initialize(memory, 0x20000, &options, &mut ledger).unwrap();
    runtime.state[5] = 0x30000; // rdi
    runtime.state[4] = 0x30008; // rsi
    let mut cache = cache::BlockCache::with_backend(Backend::Rv64);
    let mut report = Report::default();
    for &rip in &[0x20000u64, 0x2000a] {
        let mut block = runtime.with_memory(|mem| {
            cache
                .get_or_compile(mem, rip, &mut report, &mut ledger)
                .unwrap()
                .clone()
        });
        runtime
            .execute(&mut block, &mut report, &options, &mut ledger)
            .unwrap();
    }
    let stored = runtime.with_memory(|mem| {
        u64::from_le_bytes(mem.read(0x30008, 8).unwrap().try_into().unwrap())
    });
    assert_ne!(stored, 0x20000, "copied code pointer must be PAC-signed at new slot");
    drop(ledger);
    std::fs::remove_dir_all(root).unwrap();
}

