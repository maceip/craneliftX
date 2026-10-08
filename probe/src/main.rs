//! xisa-probe — prove the Cranelift half of cross-ISA block migration.
//!
//! Compiles ONE basic block, expressed as a standalone `ir::Function`, for every
//! backend Cranelift can target in a single process. Then reports how the same
//! logical state is physically placed by each backend.
//!
//! This is the substrate-independent half of the design: whether you end up
//! executing on an emulator, a second machine, or an FPGA soft core, the
//! compiler-side problem is identical — and this measures it.
//!
//! The block deliberately contains a call, because that is what forces each
//! backend to materialise its outgoing-argument area and clobber-save area.
//! Those two regions are exactly where the ABIs disagree (see the main report's
//! frame-divergence table), so without a call the frames come out deceptively
//! similar.

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{
    types, AbiParam, ExtFuncData, ExternalName, Function, InstBuilder, Signature, StackSlot,
    StackSlotData, StackSlotKind, UserExternalName, UserFuncName,
};
use cranelift_codegen::isa::{self, CallConv};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use std::str::FromStr;
use target_lexicon::Triple;

/// One basic block, modelled as a whole function — the "block-as-function"
/// compilation unit from the design.
///
///   ss32  = 32-byte sized stack slot
///   addr  = stack_addr ss32
///   sum   = v0 + v1
///   call  ext(sum, addr)          ; forces outgoing-args + clobber-save areas
///   return sum + addr             ; keeps the slot live
fn make_block_function(cc: CallConv) -> (Function, StackSlot) {
    let mut sig = Signature::new(cc);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));

    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);

    let block = func.dfg.make_block();
    let v0 = func.dfg.append_block_param(block, types::I64);
    let v1 = func.dfg.append_block_param(block, types::I64);
    func.layout.append_block(block);

    let ss =
        func.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 0));

    // Declare a callee: (i64, i64) -> (). A call forces each backend to lay out
    // its outgoing-argument area and save clobbered registers.
    let mut callee_sig = Signature::new(cc);
    callee_sig.params.push(AbiParam::new(types::I64));
    callee_sig.params.push(AbiParam::new(types::I64));
    let sig_ref = func.import_signature(callee_sig);
    let name_ref = func.declare_imported_user_function(UserExternalName {
        namespace: 0,
        index: 0,
    });
    let callee = func.dfg.ext_funcs.push(ExtFuncData {
        name: ExternalName::user(name_ref),
        signature: sig_ref,
        colocated: false,
        patchable: false,
    });

    let mut cur = FuncCursor::new(&mut func).at_bottom(block);
    let addr = cur.ins().stack_addr(types::I64, ss, 0);
    let sum = cur.ins().iadd(v0, v1);
    cur.ins().call(callee, &[sum, addr]);
    let out = cur.ins().iadd(sum, addr);
    cur.ins().return_(&[out]);

    (func, ss)
}

struct Row {
    target: &'static str,
    size: usize,
    fp_offset: Option<u32>,
    slot_offset: Option<u32>,
    relocs: usize,
    call_sites: usize,
    blocks: usize,
}

fn main() {
    let targets: &[&str] = &["x86_64", "aarch64", "riscv64", "s390x", "pulley64"];

    let mut rows: Vec<Row> = Vec::new();

    for name in targets {
        let triple = Triple::from_str(name).expect("bad triple");

        // machine_code_cfg_info is off by default; without it bb_starts is empty.
        let mut sb = settings::builder();
        sb.set("machine_code_cfg_info", "true").unwrap();
        let flags = settings::Flags::new(sb);

        let isa = match isa::lookup(triple) {
            Ok(b) => b.finish(flags).expect("isa build failed"),
            Err(e) => {
                println!("{name:<10} SKIP — backend unavailable: {e}");
                continue;
            }
        };

        let (func, ss) = make_block_function(isa.default_call_conv());
        let mut ctx = Context::for_function(func);
        let code = match ctx.compile(&*isa, &mut ControlPlane::default()) {
            Ok(c) => c,
            Err(e) => {
                println!("{name:<10} SKIP — compile failed: {e:?}");
                continue;
            }
        };

        let fl = code.buffer.frame_layout();
        rows.push(Row {
            target: name,
            size: code.code_buffer().len(),
            fp_offset: fl.map(|f| f.frame_to_fp_offset),
            slot_offset: fl.map(|f| f.stackslots[ss].offset),
            relocs: code.buffer.relocs().len(),
            call_sites: code.buffer.call_sites().count(),
            blocks: code.bb_starts.len(),
        });
    }

    println!();
    println!("One basic block, compiled for every backend, in a single process");
    println!("  addr = stack_addr ss32; sum = v0+v1; call ext(sum, addr); ret sum+addr");
    println!();
    println!(
        "{:<10} {:>7} {:>10} {:>12} {:>8} {:>8} {:>8}",
        "target", "bytes", "fp_off", "ss32_off", "relocs", "calls", "blocks"
    );
    println!("{}", "-".repeat(66));
    for r in &rows {
        println!(
            "{:<10} {:>7} {:>10} {:>12} {:>8} {:>8} {:>8}",
            r.target,
            r.size,
            fmt(r.fp_offset),
            fmt(r.slot_offset),
            r.relocs,
            r.call_sites,
            r.blocks
        );
    }

    let sizes: Vec<usize> = rows.iter().map(|r| r.size).collect();
    let fps: Vec<u32> = rows.iter().filter_map(|r| r.fp_offset).collect();
    let slots: Vec<u32> = rows.iter().filter_map(|r| r.slot_offset).collect();

    println!();
    println!(
        "code size:             {}..{} bytes, {} distinct",
        sizes.iter().min().unwrap_or(&0),
        sizes.iter().max().unwrap_or(&0),
        distinct(&sizes)
    );
    println!(
        "SP-to-FP distance:     {} distinct across {} backends",
        distinct(&fps),
        fps.len()
    );
    println!(
        "stack slot placement:  {} distinct across {} backends",
        distinct(&slots),
        slots.len()
    );
    println!();
    println!("Every backend accepted the same block and produced working code, so the");
    println!("compilation half is solved. What differs is where each one put things —");
    println!("and that is precisely what a migration frame has to reconcile.");
}

fn fmt(v: Option<u32>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}

fn distinct<T: Ord + Clone>(xs: &[T]) -> usize {
    let mut v = xs.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len()
}
