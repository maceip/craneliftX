use super::heap::Heap;
use super::pac::Pac;
use crate::{riscv_exec::MemoryAccess, EngineError};
use object::{
    read::elf::{FileHeader, ProgramHeader},
    Architecture, BinaryFormat, Object, ObjectKind, ObjectSegment, SegmentFlags,
};
use serde_json::{json, Value};

pub const ADDRESS_LIMIT: u64 = 256 * 1024 * 1024;
pub const GUEST_BIAS: u64 = 0x1_0000_0000;
pub const CONTEXT_BASE: u64 = 0x2_0000_0000;
pub const RV_STACK_BASE: u64 = 0x3_0000_0000;
pub const STACK_TOP: u64 = 0x0700_0000;
pub const STACK_BOTTOM: u64 = STACK_TOP - 1024 * 1024;
pub const MMAP_BASE: u64 = 0x0800_0000;
const MAX_MAPPED: u64 = 128 * 1024 * 1024;

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
    pub phdr: u64,
    pub phent: u64,
    pub phnum: u64,
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
        let elf_header = elf.elf_header();
        let endian = elf.endian();
        let phoff = elf_header.e_phoff(endian);
        let phent = elf_header.e_phentsize(endian) as u64;
        let phnum = elf_header.e_phnum(endian) as u64;

        let mut memory = Self::default();
        memory.phent = phent;
        memory.phnum = phnum;
        for p in elf.elf_program_headers() {
            if p.p_type(endian) == object::elf::PT_PHDR {
                memory.phdr = p.p_vaddr(endian);
                break;
            }
        }
        if memory.phdr == 0 {
            if let Some(first) = file.segments().next() {
                memory.phdr = first.address() + phoff;
            }
        }

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
    pub fn read_mut(&mut self, address: u64, size: usize) -> Result<&mut [u8], EngineError> {
        let (i, o) = self.locate(address, size, true)?;
        Ok(&mut self.regions[i].data[o..o + size])
    }
    pub fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), EngineError> {
        let (i, o) = self.locate(address, bytes.len(), true)?;
        self.regions[i].data[o..o + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    pub fn writable(&self, address: u64, size: usize) -> Result<(), EngineError> {
        self.locate(address, size, true).map(|_| ())
    }
    pub fn unmap(&mut self, start: u64, len: u64) -> Result<(), EngineError> {
        let end = start.checked_add(len).ok_or_else(|| error("unmap overflow"))?;
        if len == 0 {
            return Ok(());
        }
        // Remove any regions that fall entirely within [start, end).
        self.regions.retain(|r| {
            let r_end = r.start + r.data.len() as u64;
            !(start <= r.start && r_end <= end)
        });
        Ok(())
    }

    pub fn mprotect(&mut self, start: u64, len: u64, prot: i32) -> Result<(), EngineError> {
        let end = start.checked_add(len).ok_or_else(|| error("mprotect overflow"))?;
        let mut found = false;
        for r in &mut self.regions {
            let r_end = r.start + r.data.len() as u64;
            if start < r_end && end > r.start {
                found = true;
                let writable = prot & 2 != 0;
                let executable = prot & 4 != 0;
                if writable && executable {
                    return Err(error("mprotect W^X violation"));
                }
                r.writable = writable;
                r.executable = executable;
            }
        }
        if !found {
            return Err(error("mprotect on unmapped region"));
        }
        Ok(())
    }

    pub fn find_free_mmap(&self, hint: u64, len: u64) -> Option<u64> {
        let mut addr = if hint >= MMAP_BASE && hint + len <= ADDRESS_LIMIT {
            hint
        } else {
            MMAP_BASE
        };
        // Simple first-fit search in mmap area.
        while addr + len <= ADDRESS_LIMIT {
            let end = addr + len;
            let overlaps = self.regions.iter().any(|r| {
                let r_end = r.start + r.data.len() as u64;
                addr < r_end && end > r.start
            });
            if !overlaps {
                return Some(addr);
            }
            // Advance past the overlapping region.
            let next = self
                .regions
                .iter()
                .filter(|r| {
                    let r_end = r.start + r.data.len() as u64;
                    addr < r_end && end > r.start
                })
                .map(|r| r.start + r.data.len() as u64)
                .max()
                .unwrap_or(addr + 4096);
            addr = next.max(addr + 4096);
            addr = (addr + 4095) & !4095; // page align
        }
        None
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
    pub state: &'a mut [u64; 19],
    pub stack: &'a mut [u8],
    /// Sub-granule shadow tags. Every load and store is checked against it.
    pub heap: &'a mut Heap,
    /// Code-pointer authentication. Host-only; no guest address names it.
    pub pac: &'a mut Pac,
}
impl ProcessBus<'_> {
    /// Screen an address through the shadow tag map before it is used.
    ///
    /// The heap's tag map is indexed by guest address, while the bus window is
    /// biased by `GUEST_BIAS`; the two are reconciled here so a heap access is
    /// actually checked and everything else merely has to be untagged.
    fn screen(&mut self, address: u64, size: usize, pc: u64) -> Result<u64, EngineError> {
        // Test the window on the address alone: the tag sits above bit 47 and
        // would put every tagged pointer outside a numeric range check.
        let scheme = self.heap.scheme();
        let plain = scheme.strip(address);
        if (GUEST_BIAS..GUEST_BIAS + ADDRESS_LIMIT).contains(&plain) {
            let guest = scheme
                .tag(plain - GUEST_BIAS, scheme.tag_of(address))
                .unwrap_or(plain - GUEST_BIAS);
            return Ok(self.heap.check(guest, size as u64, pc)? + GUEST_BIAS);
        }
        self.heap.reject_tagged(address, pc)
    }
}

impl ProcessBus<'_> {
    /// Address a pointer signature binds to, or `None` for memory PAC does not
    /// cover.
    ///
    /// Only guest data mappings qualify. The stack is excluded on purpose: a
    /// `call` pushes a return address there, so signing it would put a code on
    /// every frame and any frame the guest rewrites would fault. Return
    /// addresses are guarded by the stack bounds checks instead.
    fn pac_scope(&self, address: u64) -> Option<u64> {
        // Strip first: the tag sits above bit 47 and would put every tagged
        // pointer outside the numeric window.
        let plain = self.heap.scheme().strip(address);
        if !(GUEST_BIAS..GUEST_BIAS + ADDRESS_LIMIT).contains(&plain) {
            return None;
        }
        let guest = plain - GUEST_BIAS;
        if (STACK_BOTTOM..STACK_TOP).contains(&guest) {
            return None;
        }
        // Pointer metadata is not part of the address a signature binds to.
        Some(guest)
    }
}

impl MemoryAccess for ProcessBus<'_> {
    fn code(&self) -> &[u8] {
        self.code
    }
    fn stack_bounds(&self) -> (u64, u64) {
        (RV_STACK_BASE, RV_STACK_BASE + self.stack.len() as u64)
    }
    fn authenticate_load(
        &mut self,
        address: u64,
        size: usize,
        raw: u64,
        pc: u64,
    ) -> Result<(u64, bool), EngineError> {
        let Some(key) = self.pac_scope(address) else {
            return Ok((raw, false));
        };
        let value = self.pac.on_load(key, raw, size as u64, pc)?;
        Ok((value, self.pac.is_signed_slot(key)))
    }
    fn authenticate_store(
        &mut self,
        address: u64,
        size: usize,
        value: u64,
        constant: bool,
        pc: u64,
    ) -> Result<u64, EngineError> {
        let Some(key) = self.pac_scope(address) else {
            return Ok(value);
        };
        self.pac.on_store(key, value, size as u64, constant, pc)
    }
    unsafe fn load(&mut self, address: u64, size: usize, pc: u64) -> Result<u64, EngineError> {
        // Tag check first: the access is rejected before any byte is read.
        let address = self.screen(address, size, pc)?;
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
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + 152 {
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
        let address = self.screen(address, size, pc)?;
        let fail = || EngineError::Execute {
            pc,
            reason: "unmapped process RV64 store",
        };
        let end = address.checked_add(size as u64).ok_or_else(fail)?;
        let bytes = value.to_le_bytes();
        if address >= GUEST_BIAS && end <= GUEST_BIAS + ADDRESS_LIMIT {
            self.memory.write(address - GUEST_BIAS, &bytes[..size])?;
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + 152 {
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
