//! Input acquisition and image loading are separate observable operations.
use super::{memory::GuestMemory, observe::operation, Options, Result};
use crate::{
    ingest::BinaryInput,
    ledger::{sha256, RunLedger},
};
use serde_json::json;
use std::path::Path;

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
pub(super) fn load(bytes: &[u8], ledger: &mut RunLedger) -> Result<(GuestMemory, u64)> {
    let (memory, entry, _) = operation(
        ledger,
        "load",
        json!({}),
        |_| Ok(GuestMemory::load_elf(bytes)?),
        |(_, _, mappings)| json!({"details":mappings}),
    )?;
    Ok((memory, entry))
}
