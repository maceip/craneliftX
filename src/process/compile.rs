//! Decode -> lower -> generate -> persist. The outer `lift` event remains the
//! cache-miss compilation span; leaf operations each have their own group.
use super::{
    lower,
    memory::{error, GuestMemory},
    observe::operation,
    Result,
};
use crate::{
    codegen,
    harvest::harvest_bytes,
    ledger::{hex, sha256, RunLedger},
};
use cranelift_codegen::ir::Function;
use iced_x86::{Formatter, Instruction, IntelFormatter, Mnemonic};
use serde_json::{json, Value};

pub(super) struct CompiledBlock {
    pub id: u64,
    pub rip: u64,
    pub code: Vec<u8>,
    pub syscall: Option<u64>,
    pub source_instructions: Vec<(u64, usize)>,
    pub x86_bytes: usize,
}
struct DecodedBlock {
    instructions: Vec<Instruction>,
    bytes: Vec<u8>,
}
fn decode(
    memory: &GuestMemory,
    rip: u64,
    block_id: u64,
    ledger: &mut RunLedger,
) -> Result<DecodedBlock> {
    operation(
        ledger,
        "decode",
        json!({"rip":format!("0x{rip:x}"),"block_id":block_id}),
        |ledger| {
            let source = memory.fetch(rip)?;
            let (instructions, _) = harvest_bytes(source, rip, 32)?;
            let end = instructions
                .last()
                .ok_or_else(|| error("empty block"))?
                .next_ip();
            let bytes = source[..(end - rip) as usize].to_vec();
            let mut formatter = IntelFormatter::new();
            let decoded:Vec<Value>=instructions.iter().map(|i|{
            let mut asm=String::new();formatter.format(i,&mut asm);let at=(i.ip()-rip) as usize;
            json!({"ip":format!("0x{:x}",i.ip()),"bytes":hex(&bytes[at..at+i.len()]),"asm":asm})
        }).collect();
            ledger.event(
                "harvest",
                json!({"rip":format!("0x{rip:x}"),"block_id":block_id,"instructions":decoded,
            "memory_policy":"isolated mapped guest memory; read-only executable segments"}),
            )?;
            Ok(DecodedBlock {
                instructions,
                bytes,
            })
        },
        |b| json!({"x86_bytes":b.bytes.len(),"x86_instructions":b.instructions.len()}),
    )
}
fn lower(decoded: &DecodedBlock, context: Value, ledger: &mut RunLedger) -> Result<Function> {
    operation(
        ledger,
        "lower",
        context,
        |_| Ok(lower::lower_block(&decoded.instructions)?),
        |f| json!({"clif_bytes":f.display().to_string().len()}),
    )
}
fn generate(function: &Function, context: Value, ledger: &mut RunLedger) -> Result<Vec<u8>> {
    operation(
        ledger,
        "codegen",
        context,
        |_| Ok(codegen::compile_to_riscv64(function)?),
        |code| json!({"target":codegen::TARGET,"rv64_bytes":code.len(),"rv64_sha256":sha256(code)}),
    )
}
fn persist(
    decoded: &DecodedBlock,
    function: &Function,
    code: &[u8],
    context: Value,
    ledger: &mut RunLedger,
) -> Result<()> {
    let id = context["block_id"].as_u64().unwrap();
    let rip = decoded.instructions[0].ip();
    operation(
        ledger,
        "artifacts",
        context,
        |ledger| {
            let prefix = format!("block-{id:05}-{rip:x}");
            ledger.artifact(&format!("{prefix}.x86"), &decoded.bytes)?;
            ledger.artifact(
                &format!("{prefix}.clif"),
                function.display().to_string().as_bytes(),
            )?;
            ledger.artifact(&format!("{prefix}.rv64"), code)?;
            Ok(())
        },
        |_| json!({"artifact_count":3}),
    )
}
pub(super) fn prepare(
    memory: &GuestMemory,
    rip: u64,
    id: u64,
    ledger: &mut RunLedger,
) -> Result<CompiledBlock> {
    let context = json!({"rip":format!("0x{rip:x}"),"block_id":id});
    operation(
        ledger,
        "lift",
        context.clone(),
        |ledger| {
            let decoded = decode(memory, rip, id, ledger)?;
            let function = lower(&decoded, context.clone(), ledger)?;
            let code = generate(&function, context.clone(), ledger)?;
            persist(&decoded, &function, &code, context.clone(), ledger)?;
            let syscall = decoded
                .instructions
                .last()
                .filter(|i| i.mnemonic() == Mnemonic::Syscall)
                .map(|i| i.ip());
            Ok(CompiledBlock {
                id,
                rip,
                code,
                syscall,
                x86_bytes: decoded.bytes.len(),
                source_instructions: decoded
                    .instructions
                    .iter()
                    .map(|i| (i.ip(), i.len()))
                    .collect(),
            })
        },
        |b| {
            json!({"x86_bytes":b.x86_bytes,"x86_instructions":b.source_instructions.len(),"rv64_bytes":b.code.len(),
        "rv64_sha256":sha256(&b.code),"target":codegen::TARGET,"syscall_boundary":b.syscall.is_some()})
        },
    )
}
