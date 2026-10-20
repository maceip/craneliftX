//! Bounded instruction-policy inspection on cache misses, before CLIF lowering.
//! This is not taint analysis or a general shellcode/ROP detector. Unknown
//! indirect targets are checked when reached; only known instruction interiors
//! can be identified as overlapping code without whole-image disassembly.
use super::{
    memory::{error, GuestMemory, STACK_BOTTOM, STACK_TOP},
    observe::operation,
    Result,
};
use crate::{ledger::RunLedger, x86};
use iced_x86::{
    FlowControl, Formatter, Instruction, InstructionInfoFactory, IntelFormatter, Mnemonic,
    OpAccess, OpKind, Register,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const POLICY: &str = "process-block-v1";

fn writes(access: OpAccess) -> bool {
    matches!(
        access,
        OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite
    )
}

fn interior(known: &BTreeMap<u64, usize>, address: u64) -> bool {
    known
        .range(..=address)
        .next_back()
        .is_some_and(|(&start, &len)| address > start && address < start + len as u64)
}

struct Check {
    rule: &'static str,
    label: &'static str,
    verdict: &'static str,
    detail: &'static str,
    findings: Vec<Value>,
}
impl Check {
    fn new(rule: &'static str, label: &'static str, detail: &'static str) -> Self {
        Self {
            rule,
            label,
            verdict: "passed",
            detail,
            findings: Vec::new(),
        }
    }
    fn finding(&mut self, i: &Instruction, reject: bool, detail: impl ToString) {
        let verdict = if reject { "rejected" } else { "guarded" };
        if reject || self.verdict == "passed" {
            self.verdict = verdict;
        }
        let mut asm = String::new();
        IntelFormatter::new().format(i, &mut asm);
        self.findings.push(json!({"rip":format!("0x{:x}", i.ip()),"instruction":asm,"verdict":verdict,"detail":detail.to_string()}));
    }
    fn json(&self) -> Value {
        json!({"rule":self.rule,"label":self.label,"verdict":self.verdict,"detail":self.detail,"findings":self.findings})
    }
}

pub(super) fn inspect(
    memory: &GuestMemory,
    known: &BTreeMap<u64, usize>,
    trace: &[Instruction],
    context: Value,
    ledger: &mut RunLedger,
) -> Result<()> {
    operation(
        ledger,
        "audit",
        context.clone(),
        |ledger| {
            let mut privilege = Check::new("privileged_control", "Instructions and syscall boundary", "Inspected every decoded instruction for privileged operations and forbidden interrupt mechanisms.");
            let mut stack = Check::new("stack_policy", "Stack-pointer operations", "Explicit stack-pointer writes must be supported adjustments or frame restores; every resulting RSP is checked during RV64 execution.");
            let mut addresses = Check::new("memory_policy", "Memory operand bounds", "Fixed memory operands must fit a mapped region with the required permissions. Register-derived addresses remain checked on every access.");
            let mut branches = Check::new("control_flow", "Executable targets and known boundaries", "Direct targets must be executable and must not enter a known instruction interior. This does not identify arbitrary gadget chains.");
            let mut info = InstructionInfoFactory::new();
            // At most 32 entries; no copy of the growing, run-wide boundary map.
            let local: BTreeMap<_, _> = trace.iter().map(|i| (i.ip(), i.len())).collect();
            for i in trace {
                if i.mnemonic() == Mnemonic::Syscall {
                    privilege.finding(i, false, "SYSCALL is handed to the existing Linux bridge; the live syscall number and arguments are checked there.");
                } else if i.is_privileged()
                    || x86::privileged(i)
                    || matches!(
                        i.mnemonic(),
                        Mnemonic::Retf | Mnemonic::Vmcall | Mnemonic::Vmmcall
                    )
                    || matches!(i.op0_kind(), OpKind::FarBranch16 | OpKind::FarBranch32)
                {
                    privilege.finding(i, true, "Privileged instruction or interrupt/far-transfer mechanism is outside the process policy.");
                }
                let usage = info.info(i);
                for operand in 0..i.op_count() {
                    if i.op_kind(operand) == OpKind::Register
                        && x86::register_view(i.op_register(operand))
                            .is_some_and(|(index, _, _)| index == 7)
                        && writes(usage.op_access(operand))
                    {
                        let allowed = operand == 0
                            && i.op0_register() == Register::RSP
                            && match i.mnemonic() {
                                Mnemonic::Add | Mnemonic::Sub => x86::immediate(i, 1).is_some(),
                                Mnemonic::And => x86::immediate(i, 1).is_some_and(|mask| {
                                    (!mask).checked_add(1).is_some_and(u64::is_power_of_two)
                                }),
                                Mnemonic::Mov => {
                                    i.op1_kind() == OpKind::Register
                                        && i.op1_register() == Register::RBP
                                }
                                Mnemonic::Lea => {
                                    matches!(i.memory_base(), Register::RSP | Register::RBP)
                                        && i.memory_index() == Register::None
                                }
                                _ => false,
                            };
                        stack.finding(i, !allowed, if allowed { "Supported stack adjustment/frame restore; its resulting RSP is guarded at execution time." } else { "Stack-pointer reassignment is outside the supported process policy." });
                    }
                    if i.op_kind(operand) != OpKind::Memory
                        || matches!(
                            usage.op_access(operand),
                            OpAccess::None | OpAccess::NoMemAccess
                        )
                    {
                        continue;
                    }
                    if i.segment_prefix() != Register::None {
                        if matches!(
                            i.segment_prefix(),
                            Register::FS | Register::CS | Register::DS | Register::SS | Register::ES
                        ) {
                            addresses.finding(
                                i,
                                false,
                                "Guarded segment-relative or zero-based default segment addressing.",
                            );
                        } else {
                            addresses.finding(
                                i,
                                true,
                                "Segment-relative data addressing is outside this process profile.",
                            );
                            continue;
                        }
                    }
                    let absolute = (i.is_ip_rel_memory_operand()
                        || (i.memory_base() == Register::None
                            && i.memory_index() == Register::None))
                        && i.segment_prefix() != Register::FS;
                    if absolute {
                        let address = if i.is_ip_rel_memory_operand() {
                            i.ip_rel_memory_address()
                        } else {
                            i.memory_displacement64()
                        };
                        let size = i.memory_size().size();
                        let valid = size > 0
                            && if writes(usage.op_access(operand)) {
                                memory.writable(address, size).is_ok()
                            } else {
                                memory.read(address, size).is_ok()
                            };
                        if !valid {
                            addresses.finding(i, true, format!("Fixed memory operand 0x{address:x} ({size} bytes) is unmapped or lacks required permissions."));
                        }
                    } else {
                        addresses.finding(i, false, "Register-derived address: mapped-region bounds and write permission are enforced by the RV64 memory bus.");
                    }
                }
                if interior(known, i.ip()) || known.range(i.ip() + 1..i.next_ip()).next().is_some()
                {
                    branches.finding(
                        i,
                        true,
                        "Decoded instruction overlaps a previously compiled instruction boundary.",
                    );
                }
                if matches!(
                    i.op0_kind(),
                    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                ) {
                    let target = i.near_branch_target();
                    if memory.fetch(target).is_err()
                        || interior(known, target)
                        || interior(&local, target)
                    {
                        branches.finding(i, true, format!("Direct target 0x{target:x} is non-executable or inside a known instruction."));
                    }
                } else if matches!(
                    i.flow_control(),
                    FlowControl::IndirectBranch | FlowControl::IndirectCall | FlowControl::Return
                ) {
                    branches.finding(i, false, "Dynamic target: executable mapping and known instruction boundaries are checked when reached; no gadget-chain classification is performed.");
                }
            }
            let checks = [privilege, stack, addresses, branches];
            let rejected = checks.iter().any(|c| c.verdict == "rejected");
            let reason = checks
                .iter()
                .flat_map(|c| &c.findings)
                .find(|f| f["verdict"] == "rejected")
                .map(|f| {
                    format!(
                        "{} at {}: {}",
                        f["instruction"].as_str().unwrap(),
                        f["rip"].as_str().unwrap(),
                        f["detail"].as_str().unwrap()
                    )
                });
            let mut event = context.clone();
            event["policy"] = json!(POLICY);
            event["phase"] = json!("compile");
            event["verdict"] = json!(if rejected { "rejected" } else { "accepted" });
            event["instruction_count"] = json!(trace.len());
            event["max_instructions"] = json!(crate::harvest::MAX_INSNS);
            event["checks"] = json!(checks.iter().map(Check::json).collect::<Vec<_>>());
            ledger.event("security", event)?;
            match reason {
                Some(reason) => Err(error(format!("instruction policy rejected {reason}")).into()),
                None => Ok(()),
            }
        },
        |_| json!({"policy":POLICY,"verdict":"accepted","instruction_count":trace.len()}),
    )
}

pub(super) fn check_stack(state: &[u64; super::CONTEXT_WORDS], block_id: u64, ledger: &mut RunLedger) -> Result<()> {
    if (STACK_BOTTOM..=STACK_TOP).contains(&state[7]) {
        return Ok(());
    }
    let detail = format!(
        "RSP 0x{:x} is outside guest stack 0x{STACK_BOTTOM:x}..=0x{STACK_TOP:x} at x86 0x{:x}",
        state[7], state[16]
    );
    ledger.event("security", json!({"policy":POLICY,"phase":"execute","status":"failed","verdict":"rejected","block_id":block_id,"rip":format!("0x{:x}",state[16]),
        "checks":[{"rule":"stack_bounds","label":"Live stack bounds","verdict":"rejected","detail":detail,"findings":[]}]}))?;
    Err(error(detail).into())
}
