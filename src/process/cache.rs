//! Compiled code has a separate lifetime from guest state, memory and sockets.
use super::{
    compile::{self, CompiledBlock},
    csprng::Csprng,
    diversify::Diversification,
    jit::JitPage,
    lower::Backend,
    memory::{error, GuestMemory},
    Report, Result,
};
use crate::ledger::RunLedger;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct CachedBlock {
    pub compiled: CompiledBlock,
    pub executions: u64,
    pub rv64_steps: u64,
    /// JIT page for native x86-64 code (None for RV64 interpreted blocks).
    pub jit_page: Option<Arc<JitPage>>,
}
pub(super) struct BlockCache {
    entries: BTreeMap<u64, CachedBlock>,
    unique: BTreeMap<u64, usize>,
    executed: BTreeSet<u64>,
    /// Which backend to compile new blocks for.
    backend: Backend,
    /// CSPRNG for per-block diversification. Initialized on first use.
    rng: Option<Csprng>,
}
impl BlockCache {
    pub fn with_backend(backend: Backend) -> Self {
        Self {
            backend,
            entries: BTreeMap::new(),
            unique: BTreeMap::new(),
            executed: BTreeSet::new(),
            rng: None,
        }
    }
    pub fn backend(&self) -> Backend {
        self.backend
    }
    /// Ensure the CSPRNG is initialized. Called lazily on first compile.
    fn rng(&mut self) -> Result<&mut Csprng> {
        if self.rng.is_none() {
            self.rng = Some(Csprng::new().map_err(error)?);
        }
        Ok(self.rng.as_mut().unwrap())
    }
    pub fn get_or_compile(
        &mut self,
        memory: &GuestMemory,
        rip: u64,
        report: &mut Report,
        ledger: &mut RunLedger,
    ) -> Result<&mut CachedBlock> {
        if !self.entries.contains_key(&rip) {
            if self.entries.len() >= 65536 {
                return Err(error("compiled block cache limit reached").into());
            }
            // Generate a fresh diversification for this block compilation.
            // Register permutation is x86_64-only; keep identity for RV64.
            // NOTE: permutation must not reuse RIP/RFLAGS variable ids 16/17.
            let div = if self.backend == Backend::X86_64 {
                Diversification::new(self.rng()?)
            } else {
                Diversification::identity()
            };
            let compiled =
                compile::prepare(memory, &self.unique, rip, report.compiled_blocks, self.backend, &div, ledger)?;
            for &(ip, len) in &compiled.source_instructions {
                self.unique.insert(ip, len);
            }
            report.compiled_blocks += 1;
            report.emitted_rv64_bytes += compiled.code.len() as u64;
            report.unique_lifted_instructions = self.unique.len();
            report.unique_lifted_bytes = self.unique.values().sum();

            // For x86_64 native, allocate W^X pages and copy the code.
            let jit_page = if self.backend == Backend::X86_64 {
                let mut page = JitPage::allocate(compiled.code.len())
                    .map_err(|e| error(&format!("JIT allocation failed: {e}")))?;
                // SAFETY: code fits in the allocated page.
                unsafe { page.copy_code(&compiled.code) };
                page.make_executable()
                    .map_err(|e| error(&format!("W^X transition failed: {e}")))?;
                ledger.event(
                    "jit_page",
                    json!({"rip":format!("0x{rip:x}"),"bytes":compiled.code.len(),"wx":true}),
                )?;
                Some(Arc::new(page))
            } else {
                None
            };

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
                    jit_page,
                },
            );
        }
        Ok(self.entries.get_mut(&rip).unwrap())
    }
    pub fn mark_executed(&mut self, rip: u64) {
        self.executed.insert(rip);
    }
    pub fn get_mut(&mut self, rip: u64) -> Option<&mut CachedBlock> {
        self.entries.get_mut(&rip)
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
        state: &[u64; super::CONTEXT_WORDS],
        ledger: &mut RunLedger,
    ) -> Result<()> {
        ledger.event("cache_invalidation",json!({"reason":"requested recompilation interval","blocks":self.entries.len(),
            "open_descriptors":open_descriptors,"guest_rsp":format!("0x{:x}",state[7]),"next_rip":format!("0x{:x}",state[16])}))?;
        self.summarize(ledger)?;
        self.entries.clear();
        Ok(())
    }
}
