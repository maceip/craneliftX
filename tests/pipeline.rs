use cranelift_isr::{
    audit::{audit_trace, audit_trace_with_image},
    codegen::compile_to_riscv64,
    harvest::{harvest_bytes, harvest_trace},
    lower::lower_to_clif,
    riscv_exec::execute_riscv,
    try_randomize_and_execute, try_randomize_and_execute_with_image, EngineError, GuestContext,
    SecurityViolation,
};

fn run(code: &[u8], ctx: &mut GuestContext) -> Result<(), EngineError> {
    let (trace, exit) = harvest_bytes(code, 0x1000, 32)?;
    audit_trace(&trace, 0x1000, 0x2000)?;
    let function = lower_to_clif(&trace, exit)?;
    assert_eq!(function.signature.params.len(), 1);
    assert!(function.signature.returns.is_empty());
    let riscv = compile_to_riscv64(&function)?;
    assert!(!riscv.is_empty());
    // SAFETY: all test fixtures use registers or live, exclusively owned data.
    unsafe { execute_riscv(&riscv, ctx) }
}

#[test]
fn context_layout_matches_contract() {
    assert_eq!(std::mem::size_of::<GuestContext>(), 144);
    assert_eq!(std::mem::align_of::<GuestContext>(), 16);
    assert_eq!(std::mem::offset_of!(GuestContext, rsp), 56);
    assert_eq!(std::mem::offset_of!(GuestContext, r15), 120);
    assert_eq!(std::mem::offset_of!(GuestContext, rip), 128);
}

#[test]
fn harvester_stops_at_every_boundary_and_keeps_it_for_audit() {
    let boundaries: &[&[u8]] = &[
        &[0xeb, 0],
        &[0xe9, 0, 0, 0, 0],
        &[0xff, 0xe0], // jumps
        &[0x74, 0],
        &[0x0f, 0x85, 0, 0, 0, 0],
        &[0xe3, 0],
        &[0xe2, 0],
        &[0xc3],
        &[0xc2, 8, 0],
        &[0xcb], // returns
        &[0xe8, 0, 0, 0, 0],
        &[0xff, 0xd0], // calls
        &[0xcd, 0x80],
        &[0xcc],
        &[0xf1],
        &[0x0f, 0x05],
        &[0x0f, 0x34],
    ];
    for boundary in boundaries {
        let mut code = vec![0x48, 0x89, 0xd8]; // mov rax,rbx
        code.extend_from_slice(boundary);
        code.extend_from_slice(&[0x48, 0x83, 0xc0, 1]);
        let (trace, next) = harvest_bytes(&code, 0x1000, 32).unwrap();
        assert_eq!(trace.len(), 2, "{boundary:02x?}");
        assert_eq!(next, 0x1003);
        // SAFETY: the bounded code ends after a complete boundary instruction.
        let (live, live_next) = unsafe { harvest_trace(code.as_ptr() as u64, 32) }.unwrap();
        assert_eq!(live.len(), 2);
        assert_eq!(live_next, code.as_ptr() as u64 + 3);
    }
}

#[test]
fn harvester_caps_at_32_and_reports_truncation() {
    let code = [0x48, 0x89, 0xd8].repeat(40);
    for max in [32, 33, usize::MAX] {
        let (trace, next) = harvest_bytes(&code, 100, max).unwrap();
        assert_eq!(trace.len(), 32);
        assert_eq!(next, 196);
    }
    assert_eq!(harvest_bytes(&code, 100, 2).unwrap().1, 106);
    assert!(harvest_bytes(&[0x48, 0x8b], 0, 32).is_err());
    assert!(harvest_bytes(&[0xff, 0xff], 0, 32).is_err());
}

#[test]
fn syscall_is_rejected_before_lowering_or_any_memory_write() {
    let code = [0x48u8, 0x89, 0x07, 0x0f, 0x05]; // mov [rdi],rax; syscall
    let mut data = 123u64;
    let mut ctx = GuestContext {
        rip: code.as_ptr() as u64,
        rdi: &mut data as *mut u64 as u64,
        rax: 999,
        ..GuestContext::default()
    };
    let before = ctx;
    // SAFETY: complete live trace; the (unexecuted) store points to data.
    let error = unsafe { try_randomize_and_execute(&mut ctx, 0x1000, 0x2000) }.unwrap_err();
    assert_eq!(
        error,
        EngineError::Security(SecurityViolation::PrivilegedInstruction {
            rip: code.as_ptr() as u64 + 3
        })
    );
    assert_eq!(ctx, before);
    assert_eq!(data, 123);

    // IMUL cannot lower, but audit must find the later SYSCALL first.
    let code = [0x48u8, 0x0f, 0xaf, 0xc0, 0x0f, 0x05];
    ctx.rip = code.as_ptr() as u64;
    // SAFETY: complete live trace; no data memory access.
    assert!(matches!(
        unsafe { try_randomize_and_execute(&mut ctx, 1, 2) },
        Err(EngineError::Security(_))
    ));
}

#[test]
fn audit_rejects_interrupts_and_stack_alias_pivots() {
    for code in [
        &[0x0f, 0x05][..],
        &[0x0f, 0x34],
        &[0xcd, 0x80],
        &[0xcc],
        &[0xf1],
    ] {
        let (trace, _) = harvest_bytes(code, 0, 32).unwrap();
        assert!(matches!(
            audit_trace(&trace, 1, 2),
            Err(SecurityViolation::PrivilegedInstruction { .. })
        ));
    }
    let pivots: &[&[u8]] = &[
        &[0x48, 0x89, 0xc4],
        &[0x89, 0xc4],
        &[0x66, 0x89, 0xc4],
        &[0x40, 0x88, 0xc4],
        &[0x48, 0x87, 0xe0],
        &[0x48, 0x87, 0xc4],
        &[0x48, 0x31, 0xe4],
        &[0x48, 0x01, 0xc4],
        &[0x48, 0xc7, 0xc4, 0, 0, 0, 0],
        &[0x5c],
        &[0xc9],
    ];
    for code in pivots {
        let (trace, _) = harvest_bytes(code, 0, 32).unwrap();
        assert!(
            matches!(
                audit_trace(&trace, 1, 2),
                Err(SecurityViolation::StackPivot { .. })
            ),
            "{code:02x?}"
        );
    }
}

#[test]
fn immediate_stack_adjustments_are_bounded_at_each_step() {
    let code = [0x48u8, 0x83, 0xec, 0x20, 0x48, 0x83, 0xc4, 0x10, 0xc3];
    let mut ctx = GuestContext {
        rip: code.as_ptr() as u64,
        rsp: 0x1800,
        ..GuestContext::default()
    };
    // SAFETY: complete live trace; it only adjusts the numeric guest stack register.
    unsafe { try_randomize_and_execute(&mut ctx, 0x1000, 0x2000) }.unwrap();
    assert_eq!(ctx.rsp, 0x17f0);
    ctx.rip = code.as_ptr() as u64;
    ctx.rsp = 0x1010;
    let before = ctx;
    // SAFETY: same fixture, rejected before execution.
    assert!(matches!(
        unsafe { try_randomize_and_execute(&mut ctx, 0x1000, 0x2000) },
        Err(EngineError::Security(
            SecurityViolation::StackOutOfBounds { .. }
        ))
    ));
    assert_eq!(ctx, before);
}

#[test]
fn absolute_access_requires_image_and_full_operand_fit() {
    let mut data = 0xdead_beef_9876_5432u64;
    let address = &mut data as *mut u64 as u64;
    let mut code = vec![0x48, 0xa1]; // mov rax,moffs64
    code.extend_from_slice(&address.to_le_bytes());
    code.push(0xc3);
    let (trace, _) = harvest_bytes(&code, 0, 32).unwrap();
    assert!(audit_trace(&trace, 1, 2).is_err());
    assert!(audit_trace_with_image(&trace, 1, 2, Some(address..address + 7)).is_err());
    assert!(audit_trace_with_image(&trace, 1, 2, Some(address + 1..address + 8)).is_err());
    let mut ctx = GuestContext {
        rip: code.as_ptr() as u64,
        ..GuestContext::default()
    };
    // SAFETY: the sole absolute load reads the live data object.
    unsafe { try_randomize_and_execute_with_image(&mut ctx, 1, 2, Some(address..address + 8)) }
        .unwrap();
    assert_eq!(ctx.rax, data);
    let (trace, _) = harvest_bytes(&[0x48, 0x8b, 0x05, 0x10, 0, 0, 0], 0x1000, 32).unwrap();
    assert!(audit_trace(&trace, 1, 2).is_err());
    audit_trace_with_image(&trace, 1, 2, Some(0x1017..0x101f)).unwrap();
}

#[test]
fn all_primitives_and_memory_execute_bit_exactly() {
    // mov rax,rdi; mov rbx,7; add rax,rbx; sub rax,3; xor rax,rsi;
    // and rax,-16; or rax,5; shl rax,3; shr rax,1;
    // mov [rdx+8],rax; mov r8,[rdx+8]; ret
    let code = [
        0x48, 0x89, 0xf8, 0x48, 0xc7, 0xc3, 7, 0, 0, 0, 0x48, 0x01, 0xd8, 0x48, 0x83, 0xe8, 3,
        0x48, 0x31, 0xf0, 0x48, 0x83, 0xe0, 0xf0, 0x48, 0x83, 0xc8, 5, 0x48, 0xc1, 0xe0, 3, 0x48,
        0xd1, 0xe8, 0x48, 0x89, 0x42, 8, 0x4c, 0x8b, 0x42, 8, 0xc3,
    ];
    let mut data = [0x1234u64, 0];
    let mut ctx = GuestContext {
        rdi: u64::MAX - 2,
        rsi: 0xaabbccdd,
        rdx: data.as_mut_ptr() as u64,
        r15: 0x123456789abcdef0,
        ..GuestContext::default()
    };
    let expected = ((((ctx.rdi.wrapping_add(7).wrapping_sub(3) ^ ctx.rsi) & !15) | 5) << 3) >> 1;
    run(&code, &mut ctx).unwrap();
    assert_eq!(ctx.rax, expected);
    assert_eq!(ctx.r8, expected);
    assert_eq!(ctx.rbx, 7);
    assert_eq!(ctx.r15, 0x123456789abcdef0);
    assert_eq!(data, [0x1234, expected]);
    assert_eq!(ctx.rip, 0x1000 + code.len() as u64 - 1);
}

#[test]
fn partial_register_writes_and_shift_counts_match_x86() {
    let cases: &[(&[u8], u64, u64)] = &[
        (&[0xb0, 0x12], 0xaabbccdd55667788, 0xaabbccdd55667712),
        (&[0xb4, 0x12], 0xaabbccdd55667788, 0xaabbccdd55661288),
        (&[0x66, 0xb8, 0x12, 0x34], u64::MAX, 0xffffffffffff3412),
        (&[0xb8, 0x12, 0x34, 0x56, 0x78], u64::MAX, 0x78563412),
        (&[0x48, 0x83, 0xc0, 0xff], 0, u64::MAX),
        (&[0x48, 0x05, 0, 0, 0, 0x80], 0, 0xffffffff80000000),
        (&[0x04, 1], u64::MAX, 0xffffffffffffff00),
        (&[0x66, 0x83, 0xc0, 1], u64::MAX, 0xffffffffffff0000),
        (&[0x83, 0xc0, 1], u64::MAX, 0),
        (&[0xc0, 0xe0, 8], u64::MAX, 0xffffffffffffff00),
        (&[0x66, 0xc1, 0xe8, 16], u64::MAX, 0xffffffffffff0000),
        (&[0xc1, 0xe8, 32], u64::MAX, 0xffffffff),
        (&[0x48, 0xc1, 0xe0, 64], 0x1234, 0x1234),
        (&[0x48, 0xc1, 0xe8, 255], u64::MAX, 1),
        (&[0x48, 0x89, 0xd8], 0, 0x123456789abcdef0),
    ];
    for &(code, input, expected) in cases {
        let mut ctx = GuestContext {
            rax: input,
            rbx: 0x123456789abcdef0,
            ..GuestContext::default()
        };
        run(code, &mut ctx).unwrap();
        assert_eq!(ctx.rax, expected, "{code:02x?}");
    }
}

#[test]
fn memory_widths_unaligned_offsets_and_high_bytes() {
    // mov [rdi-1],ah; mov al,[rdi-1]; mov [rdi+1],ax; mov bx,[rdi+1];
    // mov [rdi+3],eax; mov ecx,[rdi+3]; mov [rdi+7],rax; mov rdx,[rdi+7]
    let code = [
        0x88, 0x67, 0xff, 0x8a, 0x47, 0xff, 0x66, 0x89, 0x47, 1, 0x66, 0x8b, 0x5f, 1, 0x89, 0x47,
        3, 0x8b, 0x4f, 3, 0x48, 0x89, 0x47, 7, 0x48, 0x8b, 0x57, 7,
    ];
    let mut data = [0u8; 17];
    let mut ctx = GuestContext {
        rax: 0xdeadbeef12345678,
        rbx: u64::MAX,
        rcx: u64::MAX,
        rdi: data.as_mut_ptr() as u64 + 1,
        ..GuestContext::default()
    };
    run(&code, &mut ctx).unwrap();
    assert_eq!(ctx.rax, 0xdeadbeef12345656);
    assert_eq!(ctx.rbx, 0xffffffffffff5656);
    assert_eq!(ctx.rcx, 0x12345656);
    assert_eq!(ctx.rdx, ctx.rax);
    assert_eq!(data[0], 0x56);
    assert_eq!(&data[8..16], &ctx.rax.to_le_bytes());
}

#[test]
fn all_registers_survive_round_trip_and_extended_registers_update() {
    let mut ctx = GuestContext {
        rax: 1,
        rbx: 2,
        rcx: 3,
        rdx: 4,
        rsi: 5,
        rdi: 6,
        rbp: 7,
        rsp: 8,
        r8: 9,
        r9: 10,
        r10: 11,
        r11: 12,
        r12: 13,
        r13: 14,
        r14: 15,
        r15: 16,
        rip: 0,
    };
    let mut expected = ctx;
    run(&[0xc3], &mut ctx).unwrap();
    expected.rip = 0x1000;
    assert_eq!(ctx, expected);
    // mov r15,r14; add r15,r13; xor r8d,r8d; mov r9b,0xff
    run(
        &[
            0x4d, 0x89, 0xf7, 0x4d, 0x01, 0xef, 0x45, 0x31, 0xc0, 0x41, 0xb1, 0xff,
        ],
        &mut ctx,
    )
    .unwrap();
    assert_eq!(ctx.r15, 29);
    assert_eq!(ctx.r8, 0);
    assert_eq!(ctx.r9, 255);
}

#[test]
fn unsupported_forms_fail_before_execution() {
    for code in [
        &[0x48, 0x0f, 0xaf, 0xc3][..],
        &[0x48, 0xd3, 0xe0],
        &[0x48, 0x01, 0x07],
        &[0x48, 0x8b, 0x04, 0x77],
        &[0x64, 0x48, 0x8b, 0x07],
        &[0x90],
        &[0xf0, 0x48, 0x01, 0x07],
    ] {
        let mut ctx = GuestContext {
            rax: 7,
            ..GuestContext::default()
        };
        let before = ctx;
        assert!(
            matches!(run(code, &mut ctx), Err(EngineError::Lower { .. })),
            "{code:02x?}"
        );
        assert_eq!(ctx, before);
    }
}
