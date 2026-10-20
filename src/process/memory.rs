use super::heap::Heap;
use super::pac::Pac;
use crate::{riscv_exec::MemoryAccess, EngineError};
use object::{
    read::elf::{FileHeader, ProgramHeader},
    Architecture, BinaryFormat, Object, ObjectKind, ObjectSegment, SegmentFlags,
};
use serde_json::{json, Value};

pub const ADDRESS_LIMIT: u64 = 1024 * 1024 * 1024;
pub const GUEST_BIAS: u64 = 0x1_0000_0000;
pub const CONTEXT_BASE: u64 = 0x2_0000_0000;
pub const RV_STACK_BASE: u64 = 0x3_0000_0000;
pub const STACK_TOP: u64 = 0x0700_0000;
pub const STACK_BOTTOM: u64 = STACK_TOP - 1024 * 1024;
pub const MMAP_BASE: u64 = 0x0800_0000;
const MAX_MAPPED: u64 = 512 * 1024 * 1024;

pub(super) fn error(message: impl ToString) -> EngineError {
    EngineError::Process(message.to_string())
}

#[derive(Clone)]
pub(super) struct Region {
    pub start: u64,
    pub data: Vec<u8>,
    pub writable: bool,
    pub executable: bool,
}
#[derive(Clone, Default)]
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
                object::elf::PT_INTERP
            )
        }) || file.section_by_name(".interp").is_some()
        {
            return Err(error(
                "dynamically linked binaries requiring PT_INTERP are not supported",
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
        let mut next_regions = Vec::with_capacity(self.regions.len());
        for r in self.regions.drain(..) {
            let r_end = r.start + r.data.len() as u64;
            if end <= r.start || start >= r_end {
                next_regions.push(r);
                continue;
            }
            if start > r.start {
                let left_len = (start - r.start) as usize;
                next_regions.push(Region {
                    start: r.start,
                    data: r.data[..left_len].to_vec(),
                    writable: r.writable,
                    executable: r.executable,
                });
            }
            if end < r_end {
                let right_off = (end - r.start) as usize;
                next_regions.push(Region {
                    start: end,
                    data: r.data[right_off..].to_vec(),
                    writable: r.writable,
                    executable: r.executable,
                });
            }
        }
        self.regions = next_regions;
        Ok(())
    }

    pub fn mprotect(&mut self, start: u64, len: u64, prot: i32) -> Result<(), EngineError> {
        let end = start.checked_add(len).ok_or_else(|| error("mprotect overflow"))?;
        let writable = prot & 2 != 0;
        let executable = prot & 4 != 0;
        if writable && executable {
            return Err(error("mprotect W^X violation"));
        }
        let mut found = false;
        let mut next_regions = Vec::with_capacity(self.regions.len() + 2);
        for r in self.regions.drain(..) {
            let r_end = r.start + r.data.len() as u64;
            if end <= r.start || start >= r_end {
                next_regions.push(r);
                continue;
            }
            found = true;
            if start > r.start {
                let left_len = (start - r.start) as usize;
                next_regions.push(Region {
                    start: r.start,
                    data: r.data[..left_len].to_vec(),
                    writable: r.writable,
                    executable: r.executable,
                });
            }
            let mid_start = start.max(r.start);
            let mid_end = end.min(r_end);
            let mid_off = (mid_start - r.start) as usize;
            let mid_len = (mid_end - mid_start) as usize;
            next_regions.push(Region {
                start: mid_start,
                data: r.data[mid_off..mid_off + mid_len].to_vec(),
                writable,
                executable,
            });
            if end < r_end {
                let right_off = (end - r.start) as usize;
                next_regions.push(Region {
                    start: end,
                    data: r.data[right_off..].to_vec(),
                    writable: r.writable,
                    executable: r.executable,
                });
            }
        }
        self.regions = next_regions;
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

    pub fn stack_bounds_for_sp(&self, sp: u64) -> (u64, u64) {
        for probe in [sp, sp.saturating_sub(8)] {
            if let Some(r) = self.regions.iter().find(|r| {
                r.writable && probe >= r.start && probe <= r.start + r.data.len() as u64
            }) {
                return (r.start, r.start + r.data.len() as u64);
            }
        }
        (STACK_BOTTOM, STACK_TOP)
    }
}

pub(super) struct ProcessBus<'a> {
    pub code: &'a [u8],
    pub memory: &'a mut GuestMemory,
    pub state: &'a mut [u64; super::CONTEXT_WORDS],
    pub state_prov: &'a mut u64,
    pub stack: &'a mut [u8],
    /// Sub-granule shadow tags. Every load and store is checked against it.
    pub heap: &'a mut Heap,
    /// Code-pointer authentication. Host-only; no guest address names it.
    pub pac: &'a mut Pac,
    pub stack_bottom: u64,
    pub stack_top: u64,
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
    fn is_guest_stack(&self, guest: u64) -> bool {
        (STACK_BOTTOM..STACK_TOP).contains(&guest)
            || (self.stack_bottom..self.stack_top).contains(&guest)
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
        if (CONTEXT_BASE..CONTEXT_BASE + (super::CONTEXT_WORDS * 8) as u64).contains(&address) {
            if size == 8 {
                let idx = ((address - CONTEXT_BASE) / 8) as usize;
                let prov = (*self.state_prov >> idx) & 1 != 0;
                return Ok((raw, prov));
            }
            return Ok((raw, false));
        }
        if (RV_STACK_BASE..RV_STACK_BASE + self.stack.len() as u64).contains(&address) {
            return Ok((raw, self.pac.stack_provenance(address, size as u64)));
        }
        let plain = self.heap.scheme().strip(address);
        if !(GUEST_BIAS..GUEST_BIAS + ADDRESS_LIMIT).contains(&plain) {
            return Ok((raw, false));
        }
        let guest = plain - GUEST_BIAS;
        if self.is_guest_stack(guest) {
            if self.pac.is_signed_slot(guest) {
                let value = self.pac.on_load(guest, raw, size as u64, pc)?;
                let prov =
                    self.pac.stack_provenance(guest, size as u64) || self.pac.is_trusted_slot(guest);
                return Ok((value, prov));
            }
            return Ok((raw, self.pac.stack_provenance(guest, size as u64)));
        }
        let value = self.pac.on_load(guest, raw, size as u64, pc)?;
        Ok((value, self.pac.is_trusted_slot(guest)))
    }
    fn authenticate_store(
        &mut self,
        address: u64,
        size: usize,
        value: u64,
        constant: bool,
        pc: u64,
    ) -> Result<u64, EngineError> {
        if (CONTEXT_BASE..CONTEXT_BASE + (super::CONTEXT_WORDS * 8) as u64).contains(&address) {
            if size == 8 {
                let idx = ((address - CONTEXT_BASE) / 8) as usize;
                if constant {
                    *self.state_prov |= 1u64 << idx;
                } else {
                    *self.state_prov &= !(1u64 << idx);
                }
            }
            return Ok(value);
        }
        if (RV_STACK_BASE..RV_STACK_BASE + self.stack.len() as u64).contains(&address) {
            self.pac.set_stack_provenance(address, size as u64, constant);
            return Ok(value);
        }
        let plain = self.heap.scheme().strip(address);
        if !(GUEST_BIAS..GUEST_BIAS + ADDRESS_LIMIT).contains(&plain) {
            return Ok(value);
        }
        let guest = plain - GUEST_BIAS;
        if self.is_guest_stack(guest) {
            self.pac.set_stack_provenance(guest, size as u64, constant);
            if self.pac.is_signed_slot(guest) {
                self.pac.unmark_stack_slot(guest);
            }
            return Ok(value);
        }
        self.pac.on_store(guest, value, size as u64, constant, pc)
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
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + (super::CONTEXT_WORDS * 8) as u64 {
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
        } else if address >= CONTEXT_BASE && end <= CONTEXT_BASE + (super::CONTEXT_WORDS * 8) as u64 {
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
