use crate::error::EngineError;
use crate::process::Backend;
use cranelift_codegen::{
    control::ControlPlane,
    ir::Function,
    isa,
    settings::{self, Configurable},
    Context,
};

pub const TARGET_RV64: &str = "riscv64gc-unknown-none-elf";

/// Host triple for the native backend: compiled blocks must be executable by
/// the current process, so we emit the host ISA rather than a fixed x86-64
/// encoding (which would SIGILL on aarch64 hosts).
fn host_target() -> String {
    target_lexicon::Triple::host().to_string()
}

/// Compile Cranelift IR to machine code for the specified backend.
pub fn compile(function: &Function, backend: Backend) -> Result<Vec<u8>, EngineError> {
    match backend {
        Backend::Rv64 => compile_to_riscv64(function),
        Backend::X86_64 => compile_to_native(function),
    }
}

fn make_isa(
    target: &str,
    flag_overrides: &[(&str, &str)],
) -> Result<std::sync::Arc<dyn isa::TargetIsa>, EngineError> {
    let fail = |error: String| EngineError::Codegen(error);
    let triple: target_lexicon::Triple = target.parse().map_err(|e| fail(format!("{e}")))?;
    let mut isa = isa::lookup(
        triple
            .to_string()
            .parse()
            .map_err(|e| fail(format!("{e}")))?,
    )
    .map_err(|e| fail(format!("{e}")))?;
    for (flag, value) in flag_overrides {
        isa.set(flag, value).map_err(|e| fail(format!("{e}")))?;
    }
    let mut flags = settings::builder();
    // The interpreter intentionally uses the small RV64 instruction profile.
    // Native guest code should be optimized even when its host runner is a
    // release build: Cargo's optimization level does not apply to JIT output.
    flags
        .set("opt_level", if target == TARGET_RV64 { "none" } else { "speed" })
        .map_err(|e| fail(format!("{e}")))?;
    flags
        .set("enable_verifier", "true")
        .map_err(|e| fail(format!("{e}")))?;
    isa.finish(settings::Flags::new(flags))
        .map_err(|e| fail(format!("{e}")))
}

/// Compile only for RISC-V, independent of the host architecture.
pub fn compile_to_riscv64(function: &Function) -> Result<Vec<u8>, EngineError> {
    let isa = make_isa(
        TARGET_RV64,
        &[
            ("has_zca", "false"),
            ("has_zcd", "false"),
            ("has_zcb", "false"),
            ("has_zba", "false"),
            ("has_zbb", "false"),
            ("has_zbc", "false"),
            ("has_zbs", "false"),
            ("has_zbkb", "false"),
            ("has_zicond", "false"),
            ("has_v", "false"),
        ],
    )?;
    let mut context = Context::for_function(function.clone());
    let compiled = context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|e| EngineError::Codegen(format!("{e:?}")))?;
    if !compiled.buffer.relocs().is_empty() {
        return Err(EngineError::Codegen(
            "external relocations are not supported by the embedded interpreter".into(),
        ));
    }
    Ok(compiled.code_buffer().to_vec())
}

/// Compile to host-native code for direct execution on this CPU.
pub fn compile_to_native(function: &Function) -> Result<Vec<u8>, EngineError> {
    let isa = make_isa(&host_target(), &[])?;
    let mut context = Context::for_function(function.clone());
    let compiled = context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|e| EngineError::Codegen(format!("{e:?}")))?;
    // Internal PC-relative relocations are applied when the buffer is copied
    // into the JIT page; external symbols are not supported.
    Ok(compiled.code_buffer().to_vec())
}

/// Target label for logging.
pub fn target_label(backend: Backend) -> &'static str {
    backend.label()
}
