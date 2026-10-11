//! Bounded file ingestion. File virtual addresses are never treated as host pointers.
use crate::{ledger::sha256, EngineError};
use object::{
    Architecture, BinaryFormat, Object, ObjectKind, ObjectSection, ObjectSymbol, SectionKind,
    SymbolKind,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    Entry,
    Symbol(String),
    Address(u64),
    Raw {
        address: u64,
    },
}
pub struct BinaryInput {
    pub bytes: Vec<u8>,
    pub address: u64,
    pub metadata: Value,
}
fn fail(message: impl ToString) -> EngineError {
    EngineError::Ingest(message.to_string())
}

impl BinaryInput {
    pub fn read(path: &Path, selection: &Selection) -> Result<Self, EngineError> {
        let data = Self::read_file(path)?;
        let mut input = Self::parse(&data, selection)?;
        input.metadata["path"] = json!(path.canonicalize().map_err(fail)?);
        Ok(input)
    }
    /// Bound the file read before parsing; callers can record the input's hash
    /// even if its format or architecture is subsequently rejected.
    pub fn read_file(path: &Path) -> Result<Vec<u8>, EngineError> {
        // Avoid blocking on an ordinary FIFO/device input before opening it.
        if !fs::metadata(path).map_err(fail)?.is_file() {
            return Err(fail("input must be a regular file"));
        }
        let file = File::open(path).map_err(fail)?;
        if !file.metadata().map_err(fail)?.is_file() {
            return Err(fail("input must be a regular file"));
        }
        // Bound the read itself, including files that grow after stat.
        let mut data = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(fail)?;
        if data.is_empty() || data.len() as u64 > MAX_FILE_BYTES {
            return Err(fail("input must contain 1..=67108864 bytes"));
        }
        Ok(data)
    }
    pub fn parse(data: &[u8], selection: &Selection) -> Result<Self, EngineError> {
        if data.is_empty() || data.len() as u64 > MAX_FILE_BYTES {
            return Err(fail("invalid input length"));
        }
        let mut metadata =
            json!({"input_bytes":data.len(), "input_sha256":sha256(data), "architecture":"x86_64"});
        if let Selection::Raw { address } = selection {
            address
                .checked_add(data.len() as u64)
                .ok_or_else(|| fail("raw address range overflow"))?;
            metadata["format"] = json!("raw-x86_64");
            metadata["file_offset"] = json!(0);
            metadata["selected_bytes"] = json!(data.len());
            metadata["entry_rip"] = json!(format!("0x{address:016x}"));
            metadata["selection"] = json!("explicit raw bytes");
            return Ok(Self {
                bytes: data.to_vec(),
                address: *address,
                metadata,
            });
        }
        let object = object::File::parse(data).map_err(fail)?;
        if object.architecture() != Architecture::X86_64
            || !object.is_64()
            || !object.is_little_endian()
        {
            return Err(fail(format!(
                "expected little-endian x86-64, found {:?}",
                object.architecture()
            )));
        }
        if !matches!(object.kind(), ObjectKind::Executable | ObjectKind::Dynamic) {
            return Err(fail("input must be a linked executable or shared image; relocatable objects are not loaded"));
        }
        let symbol = if let Selection::Symbol(name) = selection {
            let mut matches = object
                .symbols()
                .chain(object.dynamic_symbols())
                .filter(|s| {
                    s.is_definition()
                        && s.kind() == SymbolKind::Text
                        && s.name().is_ok_and(|n| n == name)
                });
            let symbol = matches.next().ok_or_else(|| {
                fail(format!(
                    "text symbol {name:?} not found; use an exact symbol name or --address"
                ))
            })?;
            if matches.any(|s| {
                s.address() != symbol.address() || s.section_index() != symbol.section_index()
            }) {
                return Err(fail("ambiguous symbol; select an explicit address"));
            }
            Some(symbol)
        } else {
            None
        };
        let address = match selection {
            Selection::Address(a) => *a,
            Selection::Entry if object.format() == BinaryFormat::MachO => {
                // object 0.36 exposes LC_MAIN.entryoff as a file offset. It is
                // not an x86 virtual address like the ELF/PE entry value.
                let entry_offset = object.entry();
                metadata["entry_file_offset"] = json!(entry_offset);
                metadata["entry_resolution"] =
                    json!("Mach-O LC_MAIN file offset to text virtual address");
                object
                    .sections()
                    .find_map(|section| {
                        if section.kind() != SectionKind::Text {
                            return None;
                        }
                        let (file_offset, file_bytes) = section.file_range()?;
                        let delta = entry_offset.checked_sub(file_offset)?;
                        if delta >= file_bytes || delta >= section.size() {
                            return None;
                        }
                        section.address().checked_add(delta)
                    })
                    .ok_or_else(|| {
                        fail("Mach-O entry file offset is not in a file-backed text section")
                    })?
            }
            _ => symbol
                .as_ref()
                .map_or_else(|| object.entry(), |s| s.address()),
        };
        let section = object
            .sections()
            .find(|s| {
                s.kind() == SectionKind::Text
                    && address >= s.address()
                    && address
                        .checked_sub(s.address())
                        .is_some_and(|o| o < s.size())
                    && symbol
                        .as_ref()
                        .is_none_or(|sym| sym.section_index() == Some(s.index()))
            })
            .ok_or_else(|| fail("selected address is not in a file-backed text section"))?;
        let offset = address - section.address();
        let section_data = section.data().map_err(fail)?;
        let available = section_data
            .get(usize::try_from(offset).map_err(fail)?..)
            .ok_or_else(|| fail("selected address is outside section file data"))?;
        let declared_size = symbol.as_ref().map_or(0, |s| s.size());
        if declared_size > available.len() as u64 {
            return Err(fail("symbol extends past section file data"));
        }
        let next_symbol = object
            .symbols()
            .chain(object.dynamic_symbols())
            .filter(|s| {
                s.is_definition()
                    && s.kind() == SymbolKind::Text
                    && s.section_index() == Some(section.index())
                    && s.address() > address
            })
            .map(|s| s.address() - address)
            .min();
        let (len, bound) = if declared_size != 0 {
            (declared_size, "symbol size")
        } else if let Some(next) = next_symbol {
            (
                next.min(available.len() as u64),
                "next text symbol or section end",
            )
        } else {
            (available.len() as u64, "section end")
        };
        if len == 0 {
            return Err(fail("selected code range is empty"));
        }
        address
            .checked_add(len)
            .ok_or_else(|| fail("selected address range overflow"))?;
        // No relocation resolution or runtime linker is hidden in this loader.
        if section
            .relocations()
            .any(|(at, _)| at >= offset.saturating_sub(8) && at < offset + len)
            || object.dynamic_relocations().is_some_and(|mut relocs| {
                relocs.any(|(at, _)| at >= address.saturating_sub(8) && at < address + len)
            })
        {
            return Err(fail(
                "selected code contains relocations; runtime relocation is unsupported",
            ));
        }
        let (file_start, file_size) = section
            .file_range()
            .ok_or_else(|| fail("text section has no file data"))?;
        if offset.checked_add(len).is_none_or(|end| end > file_size) {
            return Err(fail("selected bytes exceed section file range"));
        }
        metadata["format"] = json!(format!("{:?}", object.format()));
        metadata["kind"] = json!(format!("{:?}", object.kind()));
        metadata["section"] = json!(section.name().map_err(fail)?);
        metadata["symbol"] = json!(symbol.as_ref().and_then(|s| s.name().ok()));
        metadata["selection"] = json!(format!("{selection:?}"));
        metadata["range_bound"] = json!(bound);
        metadata["entry_rip"] = json!(format!("0x{address:016x}"));
        metadata["file_offset"] = json!(file_start
            .checked_add(offset)
            .ok_or_else(|| fail("file offset overflow"))?);
        metadata["selected_bytes"] = json!(len);
        Ok(Self {
            bytes: available[..len as usize].to_vec(),
            address,
            metadata,
        })
    }
}
