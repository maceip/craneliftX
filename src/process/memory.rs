use crate::{riscv_exec::MemoryAccess, EngineError};
use object::{
    read::elf::ProgramHeader, Architecture, BinaryFormat, Object, ObjectKind, ObjectSegment,
    SegmentFlags,
};
use serde_json::{json, Value};

pub const ADDRESS_LIMIT: u64 = 128 * 1024 * 1024;
pub const GUEST_BIAS: u64 = 0x1_0000_0000;
pub const CONTEXT_BASE: u64 = 0x2_0000_0000;
pub const RV_STACK_BASE: u64 = 0x3_0000_0000;
pub const STACK_TOP: u64 = 0x0700_0000;
pub const STACK_BOTTOM: u64 = STACK_TOP - 1024 * 1024;
const MAX_MAPPED: u64 = 64 * 1024 * 1024;

pub(super) fn error(message: impl ToString) -> EngineError {
    EngineError::Process(message.to_string())
}

pub(super) struct Region {
    pub start: u64,
    pub data: Vec<u8>,
    pub writable: bool,
    pub executable: bool,
}
#[derive(Default)]
pub(super) struct GuestMemory {
    pub regions: Vec<Region>,
}
impl GuestMemory {
    pub fn load_elf(bytes: &[u8]) -> Result<(Self, u64, Value), EngineError> {
        let file = object::File::parse(bytes).map_err(error)?;
        if file.format() != BinaryFormat::Elf
            || file.architecture() != Architecture::X86_64
            || file.kind() != ObjectKind::Executable
            || !file.is_little_endian()
        {
            return Err(error(
                "process runtime requires a static little-endian x86-64 ELF executable",
            ));
        }
        let object::File::Elf64(elf) = &file else {
            return Err(error("expected ELF64"));
        };
        if elf.elf_program_headers().iter().any(|p| {
            matches!(
                p.p_type(elf.endian()),
                object::elf::PT_INTERP | object::elf::PT_DYNAMIC
            )
        }) || file.section_by_name(".interp").is_some()
            || file.section_by_name(".dynamic").is_some()
            || file
                .dynamic_relocations()
                .is_some_and(|mut r| r.next().is_some())
        {
            return Err(error(
                "dynamic linking and runtime relocations are not supported",
            ));
        }
        let mut memory = Self::default();
        let mut mappings = Vec::new();
        for segment in file.segments() {
            if segment.size() == 0 {
                continue;
            }
            let data = segment.data().map_err(error)?;
            if data.len() as u64 > segment.size() {
                return Err(error("ELF file span exceeds memory span"));
            }
            let SegmentFlags::Elf { p_flags } = segment.flags() else {
                return Err(error("missing ELF permissions"));
            };
            if p_flags & 4 == 0 || p_flags & 3 == 3 {
                return Err(error(
                    "ELF mapping must be readable and cannot be writable executable",
                ));
            }
            memory.map(
                segment.address(),
                segment.size(),
                p_flags & 2 != 0,
                p_flags & 1 != 0,
            )?;
            memory.regions.last_mut().unwrap().data[..data.len()].copy_from_slice(data);
            mappings.push(json!({"address":format!("0x{:x}", segment.address()),"memory_bytes":segment.size(),
                "file_bytes":data.len(),"file_offset":segment.file_range().0,"writable":p_flags & 2 != 0,"executable":p_flags & 1 != 0}));
        }
        memory.fetch(file.entry())?;
        memory.map(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, true, false)?;
        // argc=0, argv/envp NULL and AT_NULL. No host pointers enter guest state.
        Ok((
            memory,
            file.entry(),
            json!({"format":"ELF", "entry":format!("0x{:x}",file.entry()),"segments":mappings,
            "stack_start":STACK_TOP-1024*1024,"stack_end":STACK_TOP,"dynamic_linker":false}),
        ))
    }
    pub fn map(
        &mut self,
        start: u64,
        len: u64,
        writable: bool,
        executable: bool,
    ) -> Result<(), EngineError> {
        let end = start
            .checked_add(len)
            .ok_or_else(|| error("mapping overflow"))?;
        if len == 0
            || start < 0x10000
            || end > ADDRESS_LIMIT
            || writable && executable
            || self
                .regions
                .iter()
                .map(|r| r.data.len() as u64)
                .sum::<u64>()
                + len
                > MAX_MAPPED
            || self
                .regions
                .iter()
                .any(|r| start < r.start + r.data.len() as u64 && end > r.start)
        {
            return Err(error(
                "invalid, overlapping, oversized, or writable executable guest mapping",
            ));
        }
        self.regions.push(Region {
            start,
            data: vec![0; len as usize],
            writable,
            executable,
        });
        Ok(())
    }
    fn locate(
        &self,
        address: u64,
        size: usize,
        write: bool,
    ) -> Result<(usize, usize), EngineError> {
        let end = address
            .checked_add(size as u64)
            .ok_or_else(|| error("guest memory overflow"))?;
        self.regions
            .iter()
            .enumerate()
            .find_map(|(index, r)| {
                (address >= r.start
                    && end <= r.start + r.data.len() as u64
                    && (!write || r.writable))
                    .then_some((index, address.wrapping_sub(r.start) as usize))
            })
            .ok_or_else(|| {
                error(format!(
                    "unmapped or forbidden guest {} at 0x{address:x}, {size} bytes",
                    if write { "write" } else { "read" }
                ))
            })
    }
    pub fn read(&self, address: u64, size: usize) -> Result<&[u8], EngineError> {
        let (i, o) = self.locate(address, size, false)?;
        Ok(&self.regions[i].data[o..o + size])
    }
    pub fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), EngineError> {
        let (i, o) = self.locate(address, bytes.len(), true)?;
        self.regions[i].data[o..o + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    pub fn writable(&self, address: u64, size: usize) -> Result<(), EngineError> {
        self.locate(address, size, true).map(|_| ())
    }
    pub fn fetch(&self, address: u64) -> Result<&[u8], EngineError> {
        let (i, o) = self.locate(address, 1, false)?;
        if !self.regions[i].executable {
            return Err(error("execution outside executable guest mapping"));
        }
        Ok(&self.regions[i].data[o..])
    }
}

pub(super) struct ProcessBus<'a> {
    pub code: &'a [u8],
    pub memory: &'a mut GuestMemory,
    pub state: &'a mut [u64; 18],
    pub stack: &'a mut [u8],
}
impl MemoryAccess for ProcessBus<'_> {
    fn code(&self) -> &[u8] {
        self.code
    }
    fn stack_bounds(&self) -> (u64, u64) {
        (RV_STACK_BASE, RV_STACK_BASE + self.stack.len() as u64)
    }
    unsafe fn load(&mut self, address: u64, size: usize, pc: u64) -> Result<u64, EngineError> {
        let mut bytes = [0u8; 8];
        let fail = || EngineError::Execute {
            pc,
            reason: "unmapped process RV64 load",
        };
        let end = address.checked_add(size as u64).ok_or_else(fail)?;
        if end <= self.code.len() as u64 {
            bytes[..size].copy_from_slice(&self.code[address as usize..end as usize]);
        } else if address >= GUEST_BIAS && end <= GUEST_BIAS + ADDRESS_LIMIT {
            bytes[..size].copy_from_slice(self.memory.read(address - GUEST_BIAS, size)?);
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + 144 {
            for (i, b) in bytes[..size].iter_mut().enumerate() {
                let at = (address - CONTEXT_BASE) as usize + i;
                *b = (self.state[at / 8] >> (at % 8 * 8)) as u8;
            }
        } else if address >= RV_STACK_BASE && end <= RV_STACK_BASE + self.stack.len() as u64 {
            bytes[..size].copy_from_slice(
                &self.stack[(address - RV_STACK_BASE) as usize..(end - RV_STACK_BASE) as usize],
            );
        } else {
            return Err(fail());
        }
        Ok(u64::from_le_bytes(bytes))
    }
    unsafe fn store(
        &mut self,
        address: u64,
        size: usize,
        value: u64,
        pc: u64,
    ) -> Result<(), EngineError> {
        let fail = || EngineError::Execute {
            pc,
            reason: "unmapped process RV64 store",
        };
        let end = address.checked_add(size as u64).ok_or_else(fail)?;
        let bytes = value.to_le_bytes();
        if address >= GUEST_BIAS && end <= GUEST_BIAS + ADDRESS_LIMIT {
            self.memory.write(address - GUEST_BIAS, &bytes[..size])?;
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + 144 {
            for (i, b) in bytes[..size].iter().enumerate() {
                let at = (address - CONTEXT_BASE) as usize + i;
                let shift = at % 8 * 8;
                self.state[at / 8] =
                    (self.state[at / 8] & !(255 << shift)) | (u64::from(*b) << shift);
            }
        } else if address >= RV_STACK_BASE && end <= RV_STACK_BASE + self.stack.len() as u64 {
            self.stack[(address - RV_STACK_BASE) as usize..(end - RV_STACK_BASE) as usize]
                .copy_from_slice(&bytes[..size]);
        } else {
            return Err(fail());
        }
        Ok(())
    }
}
