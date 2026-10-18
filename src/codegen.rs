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
pub const TARGET_X86_64: &str = "x86_64-unknown-linux-gnu";

/// Compile Cranelift IR to machine code for the specified backend.
pub fn compile(function: &Function, backend: Backend) -> Result<Vec<u8>, EngineError> {
    match backend {
        Backend::Rv64 => compile_to_riscv64(function),
        Backend::X86_64 => compile_to_x86_64(function),
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
    flags.set("opt_level", "none").map_err(|e| fail(format!("{e}")))?;
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

/// Compile to x86-64 native code for direct execution on the host.
pub fn compile_to_x86_64(function: &Function) -> Result<Vec<u8>, EngineError> {
    let isa = make_isa(TARGET_X86_64, &[])?;
    let mut context = Context::for_function(function.clone());
    let compiled = context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|e| EngineError::Codegen(format!("{e:?}")))?;
    // x86-64 may have PC-relative relocations for the CFI bitmap; we allow
    // internal relocations and patch them after copying to the JIT page.
    Ok(compiled.code_buffer().to_vec())
}

/// Target label for logging.
pub fn target_label(backend: Backend) -> &'static str {
    backend.label()
}
