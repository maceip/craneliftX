//! In-process x86-64 to RISC-V trace engine.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("the in-process ISR engine requires a 64-bit host");

pub mod audit;
pub mod codegen;
pub mod context;
pub mod error;
pub mod groups;
pub mod harvest;
pub mod ingest;
pub mod ledger;
pub mod live;
pub mod lower;
pub mod pipeline;
pub mod process;
pub mod riscv_exec;
mod x86;

pub use context::GuestContext;
pub use error::{EngineError, SecurityViolation};

/// Full linear pipeline with explicit failure stages and optional image bounds.
///
/// # Safety
/// ctx.rip must meet harvest_trace's readable-code contract. Guest memory operands
/// must meet execute_riscv's live/exclusive host-memory contract. Arithmetic flags
/// are not captured; this API returns state, not a resumable native flags context.
pub unsafe fn try_randomize_and_execute_with_image(
    ctx: &mut GuestContext,
    stack_min: u64,
    stack_max: u64,
    image: Option<std::ops::Range<u64>>,
) -> Result<(), EngineError> {
    let mut observer = |_| {};
    // SAFETY: forwarded caller code and memory contracts.
    unsafe { try_randomize_and_execute_observed(ctx, stack_min, stack_max, image, &mut observer) }
}

/// Observe preparation and execution, including rejected contexts and failures.
/// The caller owns the sink; the library never creates hidden files or global logs.
///
/// # Safety
/// See `try_randomize_and_execute_with_image` for live-code and memory contracts.
pub unsafe fn try_randomize_and_execute_observed(
    ctx: &mut GuestContext,
    stack_min: u64,
    stack_max: u64,
    image: Option<std::ops::Range<u64>>,
    observer: &mut (impl FnMut(pipeline::StageEvent) + ?Sized),
) -> Result<(), EngineError> {
    // SAFETY: forwarded caller code and memory contracts.
    let prepared = unsafe {
        pipeline::PreparedTrace::prepare(ctx.rip, stack_min, stack_max, image, observer)?
    };
    let mut executor = riscv_exec::RiscvExecutor::default();
    // SAFETY: forwarded caller contract.
    unsafe {
        prepared
            .execute_observed(ctx, &mut executor, observer)
            .map(|_| ())
    }
}

/// Full pipeline without an image range; absolute data addresses are rejected.
///
/// # Safety
/// See try_randomize_and_execute_with_image for the live-code and data contracts.
pub unsafe fn try_randomize_and_execute(
    ctx: &mut GuestContext,
    stack_min: u64,
    stack_max: u64,
) -> Result<(), EngineError> {
    // SAFETY: forwarded caller contract.
    unsafe { try_randomize_and_execute_with_image(ctx, stack_min, stack_max, None) }
}

/// The spec's panic-on-error driver. The fallible variant exposes the failing stage.
///
/// # Safety
/// See try_randomize_and_execute_with_image for the live-code and data contracts.
pub unsafe fn randomize_and_execute(ctx: &mut GuestContext, stack_min: u64, stack_max: u64) {
    // SAFETY: forwarded caller contract.
    unsafe { try_randomize_and_execute(ctx, stack_min, stack_max) }
        .expect("ISR pipeline failed (security violations abort before Cranelift)");
}
