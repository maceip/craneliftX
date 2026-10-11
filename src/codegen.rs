use crate::error::EngineError;
use cranelift_codegen::{
    control::ControlPlane,
    ir::Function,
    isa,
    settings::{self, Configurable},
    Context,
};

pub const TARGET: &str = "riscv64gc-unknown-none-elf";

/// Compile only for RISC-V, independent of the host architecture.
pub fn compile_to_riscv64(function: &Function) -> Result<Vec<u8>, EngineError> {
    let fail = |error: String| EngineError::Codegen(error);
    let triple: target_lexicon::Triple = TARGET.parse().map_err(|e| fail(format!("{e}")))?;
    // Cranelift 0.115 internally uses target-lexicon 0.12. Reparse its canonical
    // spelling at the API boundary; our direct dependency remains 0.13.
    let mut isa = isa::lookup(
        triple
            .to_string()
            .parse()
            .map_err(|e| fail(format!("{e}")))?,
    )
    .map_err(|e| fail(format!("{e}")))?;
    // GC permits uncompressed RV64I. Never emit C or optional bit-manipulation
    // instructions into this fixed-width interpreter.
    for flag in [
        "has_zca",
        "has_zcd",
        "has_zcb",
        "has_zba",
        "has_zbb",
        "has_zbc",
        "has_zbs",
        "has_zbkb",
        "has_zicond",
        "has_v",
    ] {
        isa.set(flag, "false").map_err(|e| fail(format!("{e}")))?;
    }
    let mut flags = settings::builder();
    flags
        .set("opt_level", "none")
        .map_err(|e| fail(format!("{e}")))?;
    flags
        .set("enable_verifier", "true")
        .map_err(|e| fail(format!("{e}")))?;
    let isa = isa
        .finish(settings::Flags::new(flags))
        .map_err(|e| fail(format!("{e}")))?;
    let mut context = Context::for_function(function.clone());
    let compiled = context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|e| fail(format!("{e:?}")))?;
    if !compiled.buffer.relocs().is_empty() {
        return Err(fail(
            "external relocations are not supported by the embedded interpreter".into(),
        ));
    }
    Ok(compiled.code_buffer().to_vec())
}
