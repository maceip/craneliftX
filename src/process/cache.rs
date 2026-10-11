//! Compiled code has a separate lifetime from guest state, memory and sockets.
use super::{
    compile::{self, CompiledBlock},
    memory::{error, GuestMemory},
    Report, Result,
};
use crate::ledger::RunLedger;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct CachedBlock {
    pub compiled: CompiledBlock,
    pub executions: u64,
    pub rv64_steps: u64,
}
#[derive(Default)]
pub(super) struct BlockCache {
    entries: BTreeMap<u64, CachedBlock>,
    unique: BTreeMap<u64, usize>,
    executed: BTreeSet<u64>,
}
impl BlockCache {
    pub fn get_or_compile(
        &mut self,
        memory: &GuestMemory,
        rip: u64,
        report: &mut Report,
        ledger: &mut RunLedger,
    ) -> Result<&mut CachedBlock> {
        if !self.entries.contains_key(&rip) {
            if self.entries.len() >= 4096 {
                return Err(error("compiled block cache limit reached").into());
            }
            let compiled =
                compile::prepare(memory, &self.unique, rip, report.compiled_blocks, ledger)?;
            for &(ip, len) in &compiled.source_instructions {
                self.unique.insert(ip, len);
            }
            report.compiled_blocks += 1;
            report.emitted_rv64_bytes += compiled.code.len() as u64;
            report.unique_lifted_instructions = self.unique.len();
            report.unique_lifted_bytes = self.unique.values().sum();
            ledger.event(
                "compilation_progress",
                json!({"block_id":compiled.id,"rip":format!("0x{rip:x}"),"counters":report,
                "coverage_basis":"reached instructions; total reachable code is unknown"}),
            )?;
            self.entries.insert(
                rip,
                CachedBlock {
                    compiled,
                    executions: 0,
                    rv64_steps: 0,
                },
            );
        }
        Ok(self.entries.get_mut(&rip).unwrap())
    }
    pub fn mark_executed(&mut self, rip: u64) {
        self.executed.insert(rip);
    }
    pub fn addresses(&self) -> Vec<String> {
        self.executed
            .iter()
            .map(|rip| format!("0x{rip:x}"))
            .collect()
    }
    pub fn summarize(&self, ledger: &mut RunLedger) -> std::io::Result<()> {
        ledger.event("block_execution_totals",json!({"blocks":self.entries.iter().map(|(rip,b)|json!({"block_id":b.compiled.id,
            "rip":format!("0x{rip:x}"),"executions":b.executions,"rv64_steps":b.rv64_steps})).collect::<Vec<_>>()}))
    }
    pub fn invalidate(
        &mut self,
        open_descriptors: usize,
        state: &[u64; 18],
        ledger: &mut RunLedger,
    ) -> Result<()> {
        ledger.event("cache_invalidation",json!({"reason":"requested recompilation interval","blocks":self.entries.len(),
            "open_descriptors":open_descriptors,"guest_rsp":format!("0x{:x}",state[7]),"next_rip":format!("0x{:x}",state[16])}))?;
        self.summarize(ledger)?;
        self.entries.clear();
        Ok(())
    }
}
