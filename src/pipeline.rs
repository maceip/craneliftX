//! Observable preparation and reusable execution of one immutable trace.
use crate::{
    audit, codegen,
    groups::PipelineGroup,
    harvest,
    ledger::{hex, registers, sha256, timestamp_ns},
    lower,
    riscv_exec::RiscvExecutor,
    x86::{boundary, control_boundary},
    EngineError, GuestContext,
};
use iced_x86::{Formatter, Instruction, IntelFormatter, OpKind};
use serde::Serialize;
use serde_json::{json, Value};
use std::{ops::Range, time::Instant};

#[derive(Clone, Debug, Serialize)]
pub struct StageEvent {
    pub group: PipelineGroup,
    pub stage: &'static str,
    pub state: &'static str,
    pub timestamp_unix_ns: String,
    pub elapsed_ns: u64,
    pub error: Option<String>,
    pub details: Value,
}
fn measured<T>(
    stage: &'static str,
    observer: &mut (impl FnMut(StageEvent) + ?Sized),
    f: impl FnOnce() -> Result<T, EngineError>,
    details: impl FnOnce(&T) -> Value,
) -> Result<T, EngineError> {
    observer(StageEvent {
        group: PipelineGroup::for_stage(stage),
        stage,
        state: "started",
        timestamp_unix_ns: timestamp_ns(),
        elapsed_ns: 0,
        error: None,
        details: json!({}),
    });
    let start = Instant::now();
    let result = f();
    let elapsed_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    let timestamp_unix_ns = timestamp_ns();
    observer(StageEvent {
        group: PipelineGroup::for_stage(stage),
        stage,
        state: if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        timestamp_unix_ns,
        elapsed_ns,
        error: result.as_ref().err().map(ToString::to_string),
        details: result.as_ref().map(details).unwrap_or_else(|_| json!({})),
    });
    result
}
fn decoded_details(trace: &[Instruction], next_rip: u64, source: &[u8]) -> Value {
    let mut formatter = IntelFormatter::new();
    let mut offset = 0;
    let instructions: Vec<Value> = trace
        .iter()
        .map(|insn| {
            let mut assembly = String::new();
            formatter.format(insn, &mut assembly);
            let item = json!({"rip":format!("0x{:016x}",insn.ip()), "offset":offset,
            "bytes":hex(&source[offset..offset+insn.len()]), "assembly":assembly,
            "boundary":boundary(insn)});
            offset += insn.len();
            item
        })
        .collect();
    json!({"decoder":"iced-x86", "decoded_instructions":trace.len(), "decoded_bytes":offset,
        "entry_rip":format!("0x{:016x}",trace[0].ip()), "exit_rip":format!("0x{next_rip:016x}"),
        "stop_reason":if trace.last().is_some_and(boundary) { "boundary" } else if trace.len() == harvest::MAX_INSNS { "instruction_limit" } else { "input_end" },
        "instructions":instructions})
}

/// Immutable audited compilation; source addresses may be file virtual addresses
/// or live host addresses, as identified by the ingestion event.
pub struct PreparedTrace {
    trace: Vec<Instruction>,
    start_rip: u64,
    next_rip: u64,
    stack_min: u64,
    stack_max: u64,
    code: Vec<u8>,
    clif: String,
    x86: Vec<u8>,
}
impl PreparedTrace {
    /// Prepare a snapshot of readable live code.
    ///
    /// # Safety
    /// `start_rip` must satisfy `harvest_trace`'s readable, stable code contract.
    pub unsafe fn prepare(
        start_rip: u64,
        stack_min: u64,
        stack_max: u64,
        image: Option<Range<u64>>,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
    ) -> Result<Self, EngineError> {
        let (trace, next_rip, x86) = measured(
            "harvest",
            observer,
            || {
                // SAFETY: forwarded caller contract.
                let (trace, next_rip) =
                    unsafe { harvest::harvest_trace(start_rip, harvest::MAX_INSNS) }?;
                let len = trace.iter().map(Instruction::len).sum();
                // SAFETY: exactly the decoded readable bytes; caller keeps them immutable.
                let x86 =
                    unsafe { std::slice::from_raw_parts(start_rip as *const u8, len) }.to_vec();
                Ok((trace, next_rip, x86))
            },
            |(trace, next_rip, x86)| decoded_details(trace, *next_rip, x86),
        )?;
        Self::compile(trace, next_rip, x86, stack_min, stack_max, image, observer)
    }
    /// Safe, bounded ingestion. Decode at most 32 instructions from this slice;
    /// preserve the supplied virtual addresses, never dereference them.
    pub fn prepare_bytes(
        bytes: &[u8],
        start_rip: u64,
        stack_min: u64,
        stack_max: u64,
        image: Option<Range<u64>>,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
    ) -> Result<Self, EngineError> {
        let (trace, next_rip) = measured(
            "harvest",
            observer,
            || {
                if bytes.is_empty() {
                    return Err(EngineError::Harvest {
                        rip: start_rip,
                        reason: "empty code input",
                    });
                }
                if start_rip.checked_add(bytes.len() as u64).is_none() {
                    return Err(EngineError::Harvest {
                        rip: start_rip,
                        reason: "code address range overflow",
                    });
                }
                harvest::harvest_bytes(bytes, start_rip, harvest::MAX_INSNS)
            },
            |(trace, next_rip)| decoded_details(trace, *next_rip, bytes),
        )?;
        let len = trace.iter().map(Instruction::len).sum();
        Self::compile(
            trace,
            next_rip,
            bytes[..len].to_vec(),
            stack_min,
            stack_max,
            image,
            observer,
        )
    }
    fn compile(
        trace: Vec<Instruction>,
        next_rip: u64,
        x86: Vec<u8>,
        stack_min: u64,
        stack_max: u64,
        image: Option<Range<u64>>,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
    ) -> Result<Self, EngineError> {
        measured(
            "audit",
            observer,
            || {
                audit::audit_trace_with_image(&trace, stack_min, stack_max, image.clone())
                    .map_err(Into::into)
            },
            |_| json!({"stack_min":format!("0x{stack_min:016x}"), "stack_max":format!("0x{stack_max:016x}"), "image":image.as_ref().map(|r| json!({"start":format!("0x{:016x}",r.start), "end":format!("0x{:016x}",r.end)}))}),
        )?;
        let lifted_instructions = trace.iter().filter(|i| !control_boundary(i)).count();
        let lifted_bytes: usize = trace
            .iter()
            .filter(|i| !control_boundary(i))
            .map(Instruction::len)
            .sum();
        let function = measured(
            "lower",
            observer,
            || lower::lower_to_clif(&trace, next_rip),
            |f| json!({"lifted_instructions":lifted_instructions, "lifted_bytes":lifted_bytes, "clif_bytes":f.display().to_string().len(), "clif":f.display().to_string()}),
        )?;
        let code = measured(
            "codegen",
            observer,
            || codegen::compile_to_riscv64(&function),
            |code| json!({"target":codegen::TARGET, "execution_isa":"RV64I", "code_bytes":code.len(), "code_sha256":sha256(code)}),
        )?;
        Ok(Self {
            start_rip: trace[0].ip(),
            trace,
            next_rip,
            stack_min,
            stack_max,
            code,
            clif: function.display().to_string(),
            x86,
        })
    }
    pub fn code(&self) -> &[u8] {
        &self.code
    }
    pub fn clif(&self) -> &str {
        &self.clif
    }
    pub fn x86(&self) -> &[u8] {
        &self.x86
    }
    pub fn next_rip(&self) -> u64 {
        self.next_rip
    }
    /// Decoded count, including the retained, unexecuted boundary.
    pub fn instruction_count(&self) -> usize {
        self.trace.len()
    }
    pub fn lifted_instruction_count(&self) -> usize {
        self.trace.iter().filter(|i| !control_boundary(i)).count()
    }
    pub fn lifted_bytes(&self) -> usize {
        self.trace
            .iter()
            .filter(|i| !control_boundary(i))
            .map(Instruction::len)
            .sum()
    }
    pub fn summary(&self) -> Value {
        json!({"decoded_instructions":self.instruction_count(), "decoded_bytes":self.x86.len(),
            "lifted_instructions":self.lifted_instruction_count(), "lifted_bytes":self.lifted_bytes(),
            "riscv_code_bytes":self.code.len(), "entry_rip":format!("0x{:016x}",self.start_rip), "exit_rip":format!("0x{:016x}",self.next_rip),
            "stop_reason":if self.trace.last().is_some_and(boundary) { "boundary" } else if self.trace.len() == harvest::MAX_INSNS { "instruction_limit" } else { "input_end" },
            "boundary":self.trace.last().filter(|i| control_boundary(i)).map(|i| format!("{:?}",i.mnemonic())),
            "x86_sha256":sha256(&self.x86), "riscv_sha256":sha256(&self.code)})
    }
    fn validate_context(&self, ctx: &GuestContext) -> Result<(), EngineError> {
        if ctx.rip != self.start_rip {
            return Err(EngineError::Execute {
                pc: 0,
                reason: "context RIP does not match prepared trace",
            });
        }
        audit::audit_stack_updates(&self.trace, ctx.rsp, self.stack_min, self.stack_max)?;
        Ok(())
    }
    /// Execute a register-only trace with checked interpreter memory. Guest data
    /// operands are rejected: file virtual addresses must never accidentally
    /// resolve to the interpreter's code, stack, or context.
    pub fn execute_bounded(
        &self,
        ctx: &mut GuestContext,
        executor: &mut RiscvExecutor,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
    ) -> Result<u64, EngineError> {
        self.execute_observed_inner(ctx, observer, |ctx| {
            if self.trace.iter().any(|insn| {
                !control_boundary(insn)
                    && (0..insn.op_count()).any(|i| insn.op_kind(i) == OpKind::Memory)
            }) {
                return Err(EngineError::Execute {
                    pc: 0,
                    reason: "unmapped guest memory operand: file execution requires a register-only trace",
                });
            }
            executor.execute_bounded(&self.code, ctx)
        })
    }
    fn execute_observed_inner(
        &self,
        ctx: &mut GuestContext,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
        execute: impl FnOnce(&mut GuestContext) -> Result<u64, EngineError>,
    ) -> Result<u64, EngineError> {
        observer(StageEvent {
            group: PipelineGroup::Execute,
            stage: "execute",
            state: "started",
            timestamp_unix_ns: timestamp_ns(),
            elapsed_ns: 0,
            error: None,
            details: json!({"registers_before":registers(ctx), "trace":self.summary()}),
        });
        let start = Instant::now();
        let result = self.validate_context(ctx).and_then(|()| execute(ctx));
        observer(StageEvent {
            group: PipelineGroup::Execute,
            stage: "execute",
            state: if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            timestamp_unix_ns: timestamp_ns(),
            elapsed_ns: start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            error: result.as_ref().err().map(ToString::to_string),
            details: json!({"riscv_steps":result.as_ref().ok(), "registers_after":registers(ctx), "partial_stores_possible":result.is_err()}),
        });
        result
    }
    /// Observed execution with the live host-memory contract of `execute`.
    ///
    /// # Safety
    /// Guest pointers must satisfy `execute_riscv`'s live, exclusive memory contract.
    pub unsafe fn execute_observed(
        &self,
        ctx: &mut GuestContext,
        executor: &mut RiscvExecutor,
        observer: &mut (impl FnMut(StageEvent) + ?Sized),
    ) -> Result<u64, EngineError> {
        self.execute_observed_inner(ctx, observer, |ctx| {
            // SAFETY: forwarded caller contract.
            unsafe { executor.execute(&self.code, ctx) }
        })
    }
    /// Execute without observation; suitable for the demo's counted hot loop.
    ///
    /// # Safety
    /// Guest pointers must satisfy `execute_riscv`'s live, exclusive memory contract.
    pub unsafe fn execute(
        &self,
        ctx: &mut GuestContext,
        executor: &mut RiscvExecutor,
    ) -> Result<u64, EngineError> {
        self.validate_context(ctx)?;
        // SAFETY: audited immutable compilation and forwarded data contract.
        unsafe { executor.execute(&self.code, ctx) }
    }
}
