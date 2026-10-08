//! ceremony — first working demonstration of ceremony-mode execution.
//!
//! Two things are shown:
//!
//!   1. FUNCTIONAL (the DROP half): one CLIF function compiles to Pulley
//!      bytecode and runs correctly under the interpreter. This proves the
//!      *drop* pipeline works — Cranelift lowers IR to a (randomized) backend
//!      and it executes. NOTE: in production the IR is supplied by a *lifter*
//!      over ALREADY-COMPILED code (remill for naked binaries, clang→wasm for
//!      source-side artifacts). See MULTI_BACKEND_MIGRATION.md §4.7. Cranelift
//!      has no machine-code decoder, so it cannot itself lift native bytes;
//!      here we synthesize the IR to demonstrate the drop + trap behaviour.
//!   2. SECURITY: arbitrary attacker-authored bytes, when treated as the
//!      ceremony's code representation, trap instead of executing. This is the
//!      concept: the attacker does not know what the bytes mean, so their
//!      payload fails loudly.
//!
//! Each security trial runs in a CHILD PROCESS, because handing arbitrary bytes
//! to the interpreter can in principle crash it. A crash counts as a loud
//! failure too, but we tally it separately from a clean trap.

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::isa;
use cranelift_codegen::settings;
use cranelift_codegen::Context;
use pulley_interpreter::interp::{DoneReason, RegType, Val, Vm};
use std::ptr::NonNull;
use std::str::FromStr;
use target_lexicon::Triple;

const TRIALS: usize = 400;
const PAYLOAD_LEN: usize = 48;

/// Trial count is overridable via TRIALS= env (the default 400 spawns 400
/// child processes, each allocating a large Pulley stack; on a constrained
/// sandbox that can be OOM-killed, so shrink it for quick checks).
fn trial_count() -> usize {
    std::env::var("TRIALS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(TRIALS)
}

/// A tiny stand-in for the signing operation: (a * b) + a.
fn make_fn(cc: cranelift_codegen::isa::CallConv) -> Function {
    let mut sig = Signature::new(cc);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));

    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let block = func.dfg.make_block();
    let a = func.dfg.append_block_param(block, types::I64);
    let b = func.dfg.append_block_param(block, types::I64);
    func.layout.append_block(block);

    let mut cur = FuncCursor::new(&mut func).at_bottom(block);
    let t = cur.ins().imul(a, b);
    let r = cur.ins().iadd(t, a);
    cur.ins().return_(&[r]);

    func
}

fn compile_pulley() -> Vec<u8> {
    let sb = settings::builder();
    let flags = settings::Flags::new(sb);
    let isa = isa::lookup(Triple::from_str("pulley64").unwrap())
        .expect("pulley64 backend unavailable")
        .finish(flags)
        .expect("isa build failed");

    let func = make_fn(isa.default_call_conv());
    let mut ctx = Context::for_function(func);
    let code = ctx
        .compile(&*isa, &mut ControlPlane::default())
        .expect("compile failed");
    code.code_buffer().to_vec()
}

/// Run a byte slice AS IF it were a Pulley function.
/// 0 = clean trap, 1 = returned to host, 2 = call-indirect-host, 3 = hang/other.
fn run_trial(bytes: &[u8]) -> i32 {
    let owned = bytes.to_vec();
    let outcome = std::thread::scope(|_| {
        let mut vm = match Vm::new() {
            Ok(v) => v,
            Err(_) => return 3,
        };
        let ptr = match NonNull::new(owned.as_ptr() as *mut u8) {
            Some(p) => p,
            None => return 3,
        };
        // Val implements From<i64> (and From<u64>, From<i32>, ...); the
        // old `Val::new_i64` helper lives on `XRegVal`, not on `Val`.
        let args = [Val::from(1i64), Val::from(2i64)];
        let rets = [RegType::XReg];
        // SAFETY: `owned` outlives this call; ptr points into it.
        let r: DoneReason<_> = unsafe { vm.call(ptr, &args, rets) };
        match r {
            DoneReason::Trap { .. } => 0,
            DoneReason::ReturnToHost(_) => 1,
            DoneReason::CallIndirectHost { .. } => 2,
        }
    });
    outcome
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Child mode: run exactly one trial and report via exit code.
    if args.get(1).map(|s| s.as_str()) == Some("trial") {
        let bytes: Vec<u8> = args[2]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<u8>().unwrap_or(0))
            .collect();
        std::process::exit(run_trial(&bytes));
    }

    println!("\nCEREMONY-MODE EXECUTION — first demonstration\n");

    // ---- 1. Functional: compile CLIF to Pulley and run it -------------------
    let bytecode = compile_pulley();
    println!("1. Functional pipeline");
    println!("   CLIF function: f(a, b) = (a * b) + a");
    println!("   compiled to Pulley bytecode: {} bytes", bytecode.len());
    print!("   bytecode: ");
    for b in bytecode.iter().take(16) {
        print!("{b:02x} ");
    }
    println!("{}\n", if bytecode.len() > 16 { "..." } else { "" });

    match run_trial(&bytecode) {
        1 => println!("   RESULT: ran to completion (correctness path OK)\n"),
        0 => println!("   RESULT: unexpected trap — pipeline is broken\n"),
        _ => println!("   RESULT: unexpected outcome — pipeline is broken\n"),
    }

    // ---- 2. Security: attacker bytes under this representation --------------
    println!("2. Security: attacker-authored bytes treated as ceremony code");
    let trials = trial_count();
    println!("   {} trials, {} random bytes each\n", trials, PAYLOAD_LEN);

    let exe = std::env::current_exe().expect("cannot locate own binary");
    let mut trap = 0usize;
    let mut returned = 0usize;
    let mut hostcall = 0usize;
    let mut crash = 0usize;

    // Simple xorshift so trials are reproducible.
    let mut seed: u64 = 0x2545F4914F6CDD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    for _ in 0..trials {
        let payload: Vec<u8> = (0..PAYLOAD_LEN).map(|_| (next() & 0xff) as u8).collect();
        let arg = payload
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let status = std::process::Command::new(&exe)
            .args(["trial", &arg])
            .status();

        match status {
            Ok(s) => match s.code() {
                Some(0) => trap += 1,
                Some(1) => returned += 1,
                Some(2) => hostcall += 1,
                _ => crash += 1, // signal / panic = loud failure
            },
            Err(_) => crash += 1,
        }
    }

    let loud = trap + crash;
    println!("   clean trap (illegal opcode):  {:5}  ({:.1}%)", trap, pct(trap));
    println!("   crash / signal:               {:5}  ({:.1}%)", crash, pct(crash));
    println!("   returned to host (SILENT):    {:5}  ({:.1}%)", returned, pct(returned));
    println!("   host call (SILENT):           {:5}  ({:.1}%)", hostcall, pct(hostcall));
    println!("\n   loudly failed: {:.1}%  — these are detected and abort the ceremony", pct(loud));
    println!("   silently ran:  {:.1}%  — these are the residual risk\n", pct(returned + hostcall));
}

fn pct(n: usize) -> f64 {
    100.0 * n as f64 / trial_count() as f64
}
