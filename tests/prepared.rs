use cranelift_isr::{
    pipeline::PreparedTrace, riscv_exec::RiscvExecutor, EngineError, GuestContext,
};

#[test]
fn security_failure_is_observed_and_never_reaches_cranelift() {
    let bytes = [0x0fu8, 0x05];
    let mut events = vec![];
    // SAFETY: complete readable SYSCALL fixture, retained for audit and never executed.
    let result = unsafe {
        PreparedTrace::prepare(bytes.as_ptr() as u64, 0x1000, 0x2000, None, &mut |event| {
            events.push(event)
        })
    };
    assert!(matches!(result, Err(EngineError::Security(_))));
    assert_eq!(
        events
            .iter()
            .map(|e| (e.stage, e.state))
            .collect::<Vec<_>>(),
        vec![
            ("harvest", "started"),
            ("harvest", "completed"),
            ("audit", "started"),
            ("audit", "failed")
        ]
    );
    assert!(events.last().unwrap().error.is_some());
}

#[test]
fn reused_trace_checks_each_context_before_writing_memory() {
    // add rsp, 8; mov rax, 7; ret
    let bytes = [0x48u8, 0x83, 0xc4, 8, 0x48, 0xc7, 0xc0, 7, 0, 0, 0, 0xc3];
    let rip = bytes.as_ptr() as u64;
    let mut events = vec![];
    // SAFETY: readable, complete immutable fixture.
    let trace = unsafe {
        PreparedTrace::prepare(rip, 0x1000, 0x2000, None, &mut |event| events.push(event))
    }
    .unwrap();
    assert_eq!(events.len(), 8);
    let mut executor = RiscvExecutor::default();
    for rsp in [0x1000, 0x1800, 0x1ff8] {
        let mut ctx = GuestContext {
            rip,
            rsp,
            ..Default::default()
        };
        // SAFETY: fixture has no external memory operands.
        let steps = unsafe { trace.execute(&mut ctx, &mut executor) }.unwrap();
        assert!(steps > 0);
        assert_eq!(ctx.rax, 7);
        assert_eq!(ctx.rsp, rsp + 8);
        assert_eq!(ctx.rip, trace.next_rip());
    }
    for (start, stack) in [(rip, 0x1ff9), (rip + 1, 0x1800)] {
        let mut ctx = GuestContext {
            rip: start,
            rsp: stack,
            ..Default::default()
        };
        let original = ctx;
        // SAFETY: fixture has no external memory operands.
        assert!(unsafe { trace.execute(&mut ctx, &mut executor) }.is_err());
        assert_eq!(ctx, original);
    }
}
