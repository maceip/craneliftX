//! Input acquisition and image loading are separate observable operations.
use super::{memory::GuestMemory, observe::operation, Options, Result};
use crate::{
    ingest::BinaryInput,
    ledger::{sha256, RunLedger},
};
use object::{Object, ObjectSymbol};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

/// An allocator entry point found in the guest's own symbol table. Calls to
/// these addresses are served by the runtime's heap instead of by executing the
/// guest's allocator, so the guest binary needs no change and no relinking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocFn {
    Malloc,
    Calloc,
    Realloc,
    Free,
    PosixMemalign,
    AlignedAlloc,
    Memalign,
}
impl AllocFn {
    pub fn name(self) -> &'static str {
        match self {
            Self::Malloc => "malloc",
            Self::Calloc => "calloc",
            Self::Realloc => "realloc",
            Self::Free => "free",
            Self::PosixMemalign => "posix_memalign",
            Self::AlignedAlloc => "aligned_alloc",
            Self::Memalign => "memalign",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct AllocIntercepts {
    pub by_address: BTreeMap<u64, AllocFn>,
}
impl AllocIntercepts {
    pub fn is_empty(&self) -> bool {
        self.by_address.is_empty()
    }
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .by_address
            .values()
            .map(|f| f.name().to_string())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}
/// Bind allocator symbols in the loaded image. Only exact, non-weak function
/// symbols are bound, and only names the runtime can serve.
pub(super) fn bind_allocators(bytes: &[u8]) -> AllocIntercepts {
    let mut intercepts = AllocIntercepts::default();
    let Ok(file) = object::File::parse(bytes) else {
        return intercepts;
    };
    for symbol in file.symbols() {
        let Ok(raw_name) = symbol.name() else { continue };
        let stripped = raw_name.trim_start_matches('_');
        let name = stripped
            .strip_prefix("libc_")
            .or_else(|| stripped.strip_prefix("GI_"))
            .unwrap_or(stripped)
            .trim_start_matches('_');
        if symbol.address() == 0 {
            continue;
        }
        let kind = match name {
            "malloc" => Some(AllocFn::Malloc),
            "calloc" => Some(AllocFn::Calloc),
            "realloc" => Some(AllocFn::Realloc),
            "free" | "cfree" => Some(AllocFn::Free),
            "posix_memalign" => Some(AllocFn::PosixMemalign),
            "aligned_alloc" => Some(AllocFn::AlignedAlloc),
            "memalign" => Some(AllocFn::Memalign),
            _ => None,
        };
        if let Some(kind) = kind {
            intercepts.by_address.insert(symbol.address(), kind);
        }
    }
    intercepts
}

pub(super) fn ingest(path: &Path, options: &Options, ledger: &mut RunLedger) -> Result<Vec<u8>> {
    operation(
        ledger,
        "ingest",
        json!({"path":path}),
        |ledger| {
            let bytes = BinaryInput::read_file(path)?;
            ledger.event("input",json!({"path":path.canonicalize()?,"bytes":bytes.len(),"sha256":sha256(&bytes),"options":options}))?;
            Ok(bytes)
        },
        |bytes| json!({"input_bytes":bytes.len()}),
    )
}
pub(super) fn load(
    bytes: &[u8],
    ledger: &mut RunLedger,
) -> Result<(GuestMemory, u64, AllocIntercepts)> {
    let ((memory, entry, _), intercepts) = operation(
        ledger,
        "load",
        json!({}),
        |_| Ok((GuestMemory::load_elf(bytes)?, bind_allocators(bytes))),
        |((_, _, mappings), intercepts)| {
            json!({"details":mappings,"allocator_intercepts":intercepts.names(),
                "intercept_count":intercepts.by_address.len()})
        },
    )?;
    Ok((memory, entry, intercepts))
}
