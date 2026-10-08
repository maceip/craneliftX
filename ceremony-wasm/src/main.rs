//! LIFT/DROP convergence runner.
//!
//! DROP is always the same: Cranelift (Wasmtime) compiles the wasm module to
//! Pulley bytecode, and the Pulley interpreter executes it. This is the single
//! shared "secret/portable" execution core both lift edges feed into.
//!
//! Two lift edges produce the wasm fed in here:
//!   Edge 1  (clang)      : ceremony/sign.wasm        (source available)
//!   Edge 2  (LLVM/remill): ceremony/sign_llvm.wasm   (LLVM toolchain -> wasm;
//!                          remill would substitute the first step when only a
//!                          native binary exists)
//!
//! Usage: ceremony-wasm [path-to.wasm]   (defaults to ceremony/sign.wasm)

use wasmtime::{Config, Engine, Instance, Module, Store, TypedFunc};

fn run(wasm_path: &str, func: &str, a: i64, b: i64) -> Result<(), Box<dyn std::error::Error>> {
    let wasm = std::fs::read(wasm_path)?;

    // DROP: force Cranelift to emit Pulley bytecode (the portable representation)
    // instead of host-native machine code.
    let mut cfg = Config::new();
    cfg.target("pulley64")?;
    let engine = Engine::new(&cfg)?;
    let module = Module::new(&engine, &wasm[..])?;

    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])?;
    let op: TypedFunc<(i64, i64), i64> =
        instance.get_typed_func(&mut store, func)?;

    let r = op.call(&mut store, (a, b))?;

    // ceremony_op(a, b) = (a * b) + a  ->  (3 * 4) + 3 = 15
    let expected = a * b + a;
    println!("  artifact : {wasm_path}");
    println!("  target   : pulley64 (Cranelift's portable interpreter)");
    println!("  {func}({a}, {b}) = {r}  (expected {expected})");
    println!("  RESULT   : {}", if r == expected { "OK" } else { "WRONG" });
    Ok(())
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        format!("{}/../ceremony/sign.wasm", env!("CARGO_MANIFEST_DIR"))
    });
    let func = std::env::args().nth(2).unwrap_or_else(|| "ceremony_op".to_string());
    let a: i64 = std::env::args().nth(3).unwrap_or_else(|| "3".to_string()).parse().unwrap_or(3);
    let b: i64 = std::env::args().nth(4).unwrap_or_else(|| "4".to_string()).parse().unwrap_or(4);

    println!("[DROP] wasm -> Cranelift -> Pulley : {path}  func={func}");
    if let Err(e) = run(&path, &func, a, b) {
        eprintln!("  ERROR: {e}");
        std::process::exit(1);
    }
    println!();
}
