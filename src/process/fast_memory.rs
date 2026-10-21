//! Per-thread translation cache. Entries contain host pointers, never guest
//! writable state. The process address-space mutex pins all backing allocations
//! throughout a native block. Mapping changes invalidate entries before reentry.
use super::memory::{GuestMemory, Region};

pub const PAGE_SHIFT: u32 = 12;
pub const PAGE_BYTES: u64 = 1 << PAGE_SHIFT;
pub const CACHE_ENTRIES: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FastPage {
    pub page: u64,
    pub start: u64,
    pub end: u64,
    pub bias: u64,
    pub writable: u64,
}
impl FastPage {
    const EMPTY: Self = Self {
        page: u64::MAX,
        start: 0,
        end: 0,
        bias: 0,
        writable: 0,
    };
    fn new(region: &mut Region, address: u64) -> Self {
        let page = address >> PAGE_SHIFT;
        Self {
            page,
            start: region.start.max(page << PAGE_SHIFT),
            end: (region.start + region.data.len() as u64).min((page + 1) << PAGE_SHIFT),
            bias: (region.data.as_mut_ptr() as u64).wrapping_sub(region.start),
            writable: u64::from(region.writable),
        }
    }
}

pub struct FastMemory {
    entries: Box<[FastPage; CACHE_ENTRIES]>,
    generation: u64,
}
impl Default for FastMemory {
    fn default() -> Self {
        Self {
            entries: Box::new([FastPage::EMPTY; CACHE_ENTRIES]),
            generation: u64::MAX,
        }
    }
}
impl FastMemory {
    pub fn refresh(&mut self, memory: &GuestMemory) {
        if self.generation != memory.generation() {
            self.entries.fill(FastPage::EMPTY);
            self.generation = memory.generation();
        }
    }
    pub fn as_ptr(&mut self) -> *mut FastPage {
        self.entries.as_mut_ptr()
    }
    pub fn fill(&mut self, memory: &mut GuestMemory, address: u64) {
        self.refresh(memory);
        let page = address >> PAGE_SHIFT;
        let cached = self.entries[page as usize & (CACHE_ENTRIES - 1)];
        if cached.page == page && address >= cached.start && address < cached.end {
            return;
        }
        if let Some(region) = memory
            .regions
            .iter_mut()
            .find(|r| address >= r.start && address < r.start + r.data.len() as u64)
        {
            let entry = FastPage::new(region, address);
            self.entries[entry.page as usize & (CACHE_ENTRIES - 1)] = entry;
        }
    }
}
