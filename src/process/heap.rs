//! Out-of-band, randomized guest heap with sub-granule shadow tags.
//!
//! Design constraints, in order
//! ----------------------------
//! 1. Metadata never lives next to guest data. Chunk size, free state, page
//!    ownership and the quarantine queue are Rust values in this struct. The
//!    mapped heap region contains nothing but guest bytes, so a linear overflow
//!    has no allocator state to corrupt.
//! 2. Placement is unpredictable. Slots come from a per-page free vector and
//!    are drawn uniformly by CSPRNG rather than LIFO, so "free A then allocate
//!    B lands in A's slot" stops holding.
//! 3. Every chunk is tagged. The tag rides in the pointer's reserved upper bits
//!    and is mirrored into a shadow byte per 16-byte granule; any access whose
//!    tag disagrees with the granule it lands in faults.
//!
//! Guest-visible consequence: `malloc` returns a pointer whose upper bits are
//! non-zero. That is transparent to ordinary loads and stores because the bus
//! strips the tag before forming a host address, and it is transparent to guest
//! pointer arithmetic because the tag travels with the pointer. It is *not*
//! transparent to guest code that masks a pointer back to 48 bits or stores one
//! in a 32-bit field; such a pointer loses its tag and will fault.

use super::{csprng::Csprng, lam::TagScheme};
use crate::error::{EngineError, MitigationKind};

pub const HEAP_BASE: u64 = 0x0400_0000;
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;
pub const GRANULE: u64 = 16;
/// Tag written to granules of a freed chunk: matches no issued tag.
pub const QUARANTINE_TAG: u8 = 0xff;
/// Tag written to the unused tail of a slot and to page guard granules.
pub const GUARD_TAG: u8 = 0xfe;
/// Tag on heap granules that were never allocated: matches untagged access to
/// the heap, which is always a bug, so untagged heap accesses fault.
pub const UNALLOCATED_TAG: u8 = 0xfd;
/// Highest tag ever handed to a live chunk. 0xfd..=0xff are reserved for the
/// sentinels above; issuing one of those would make a freed chunk look
/// quarantined (or a guard look live) and turn a UAF into a silent success.
const ISSUED_TAG_MAX: u64 = UNALLOCATED_TAG as u64 - 1;

const PAGE_BYTES: u64 = 64 * 1024;
const CLASS_SIZES: [u64; 9] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096];
/// Objects at or below this go through the slab; larger ones get their own
/// randomized run with a trailing guard granule.
const LARGE_THRESHOLD: u64 = CLASS_SIZES[CLASS_SIZES.len() - 1];
const SLAB_PAGES: usize = 256;
/// Guests with no allocator of their own reach the heap through these. They sit
/// far above the Linux x86-64 syscall range and cannot collide with it.
pub const SYS_ISR_MALLOC: u64 = 0x4953_5200;
pub const SYS_ISR_CALLOC: u64 = 0x4953_5201;
pub const SYS_ISR_REALLOC: u64 = 0x4953_5202;
pub const SYS_ISR_FREE: u64 = 0x4953_5203;
const QUARANTINE_CAPACITY: usize = 256;

fn fail(pc: u64, kind: MitigationKind, detail: String) -> EngineError {
    EngineError::Mitigation { pc, kind, detail }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chunk {
    Slab {
        class: usize,
        page: usize,
        slot: u32,
    },
    Large {
        granules: u64,
    },
}
#[derive(Clone, Copy, Debug)]
struct Live {
    size: u64,
    tag: u64,
    chunk: Chunk,
}
#[derive(Clone, Default)]
struct Class {
    pages: Vec<Page>,
}
#[derive(Clone)]
struct Page {
    index: usize,
    free: Vec<u32>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HeapStats {
    pub allocations: u64,
    pub frees: u64,
    pub live_chunks: usize,
    pub live_bytes: u64,
    pub quarantined: usize,
    pub slab_pages: usize,
    pub tag_faults: u64,
    pub placement_draws: u64,
}

#[derive(Clone)]
pub struct Heap {
    scheme: TagScheme,
    rng: Csprng,
    /// One byte per 16-byte granule of the heap region.
    shadow: Vec<u8>,
    classes: Vec<Class>,
    pages_used: usize,
    large_cursor: u64,
    large_free: Vec<(u64, u64)>,
    live: std::collections::BTreeMap<u64, Live>,
    /// Tags currently issued to live chunks, so a fresh draw cannot collide
    /// with another live allocation and silently pass `check`.
    live_tags: std::collections::HashMap<u64, usize>,
    quarantine: std::collections::VecDeque<Chunk>,
    /// Large frees wait here before `large_free` may reuse them, matching the
    /// slab quarantine so "free then immediately reallocate" is not free.
    large_quarantine: std::collections::VecDeque<(u64, u64)>,
    /// When false the allocator still works but issues tag 0 and never faults.
    /// This exists so a run can be A/B'd against the same binary.
    tags_enabled: bool,
    /// When false slots are handed back from the first non-empty page in order,
    /// which is the predictable behaviour an attacker grooms against.
    random_placement: bool,
    stats: HeapStats,
}
impl Heap {
    pub fn new(scheme: TagScheme, rng: Csprng, tags_enabled: bool, random_placement: bool) -> Self {
        Self {
            scheme,
            rng,
            shadow: vec![UNALLOCATED_TAG; (HEAP_SIZE / GRANULE) as usize],
            classes: (0..CLASS_SIZES.len()).map(|_| Class::default()).collect(),
            pages_used: 0,
            large_cursor: HEAP_BASE + HEAP_SIZE / 2,
            large_free: Vec::new(),
            live: std::collections::BTreeMap::new(),
            live_tags: std::collections::HashMap::new(),
            quarantine: std::collections::VecDeque::new(),
            large_quarantine: std::collections::VecDeque::new(),
            tags_enabled,
            random_placement,
            stats: HeapStats::default(),
        }
    }
    pub fn stats(&self) -> HeapStats {
        let mut stats = self.stats;
        stats.live_chunks = self.live.len();
        stats.live_bytes = self.live.values().map(|c| c.size).sum();
        stats.quarantined = self.quarantine.len();
        stats.slab_pages = self.pages_used;
        stats
    }
    pub fn scheme(&self) -> TagScheme {
        self.scheme
    }
    pub fn tags_enabled(&self) -> bool {
        self.tags_enabled
    }
    /// Requested size of the chunk at a bare address, if it is live.
    pub fn live_size(&self, address: u64) -> Option<u64> {
        self.live.get(&address).map(|c| c.size)
    }
    /// Whether a bare (untagged) address falls inside the heap region.
    pub fn contains(&self, address: u64) -> bool {
        (HEAP_BASE..HEAP_BASE + HEAP_SIZE).contains(&address)
    }
    fn set_granules(&mut self, start: u64, granules: u64, tag: u8) {
        let first = ((start - HEAP_BASE) / GRANULE) as usize;
        for index in first..first + granules as usize {
            if let Some(byte) = self.shadow.get_mut(index) {
                *byte = tag;
            }
        }
    }
    fn issue_tag(&mut self) -> u64 {
        if !self.tags_enabled {
            return 0;
        }
        // 1..=ISSUED_TAG_MAX. Zero means "not a heap pointer". The top three
        // values of the u8 tag space are the guard/unallocated/quarantine
        // sentinels and must never name a live chunk.
        let limit = self
            .scheme
            .tag_limit()
            .min(ISSUED_TAG_MAX + 1)
            .max(2) as usize;
        // Prefer a tag no live chunk currently holds so `check` cannot pass
        // two different objects that happen to share a tag.
        for _ in 0..8 {
            let tag = u64::try_from(self.rng.below(limit - 1) + 1).expect("small tag");
            if !self.live_tags.contains_key(&tag) {
                return tag;
            }
        }
        u64::try_from(self.rng.below(limit - 1) + 1).expect("small tag")
    }

    /// Allocate `size` bytes and return the tagged guest pointer.
    pub fn allocate(&mut self, size: u64) -> Result<u64, EngineError> {
        let size = size.max(1);
        if size > HEAP_SIZE / 4 {
            return Err(super::memory::error(format!(
                "heap allocation of {size} bytes exceeds the per-object limit"
            )));
        }
        self.stats.allocations += 1;
        let (address, tag, chunk) = if size > LARGE_THRESHOLD {
            self.allocate_large(size)?
        } else {
            self.allocate_slab(size)?
        };
        self.stats.placement_draws = self.rng.draws();
        if tag != 0 {
            *self.live_tags.entry(tag).or_insert(0) += 1;
        }
        self.live.insert(address, Live { size, tag, chunk });
        Ok(self.scheme.tag(address, tag).unwrap_or(address))
    }

    /// Allocate `size` bytes aligned to `alignment` (power of two) and return the tagged pointer.
    pub fn allocate_aligned(&mut self, alignment: u64, size: u64) -> Result<u64, EngineError> {
        if alignment <= GRANULE {
            return self.allocate(size);
        }
        let size = size.max(1);
        if size > HEAP_SIZE / 4 || !alignment.is_power_of_two() {
            return Err(super::memory::error(format!(
                "invalid aligned heap allocation: alignment={alignment}, size={size}"
            )));
        }
        self.stats.allocations += 1;
        let granules = size.div_ceil(GRANULE);
        let gap = u64::try_from(self.rng.below(5)).expect("small gap") * GRANULE;
        let raw_start = self.large_cursor + gap;
        let start = (raw_start + alignment - 1) & !(alignment - 1);
        let needed = (granules + 1) * GRANULE;
        if start + needed > HEAP_BASE + HEAP_SIZE {
            return Err(super::memory::error("guest heap exhausted"));
        }
        if start > self.large_cursor {
            let pad_granules = (start - self.large_cursor) / GRANULE;
            self.set_granules(self.large_cursor, pad_granules, GUARD_TAG);
        }
        self.large_cursor = start + needed;
        let tag = self.issue_tag();
        self.set_granules(start, granules, u8::try_from(tag).unwrap_or(GUARD_TAG));
        self.set_granules(start + granules * GRANULE, 1, GUARD_TAG);
        self.stats.placement_draws = self.rng.draws();
        if tag != 0 {
            *self.live_tags.entry(tag).or_insert(0) += 1;
        }
        self.live.insert(
            start,
            Live {
                size,
                tag,
                chunk: Chunk::Large { granules },
            },
        );
        Ok(self.scheme.tag(start, tag).unwrap_or(start))
    }
    fn allocate_slab(&mut self, size: u64) -> Result<(u64, u64, Chunk), EngineError> {
        let class = CLASS_SIZES
            .iter()
            .position(|c| *c >= size)
            .ok_or_else(|| super::memory::error("no size class covers the request"))?;
        let class_size = CLASS_SIZES[class];
        let slots_per_page = (PAGE_BYTES / class_size) as u32;
        // Candidate pages are those with at least one free slot; one of them is
        // chosen uniformly, then one of its free slots is chosen uniformly.
        let candidates: Vec<usize> = self.classes[class]
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.free.is_empty())
            .map(|(i, _)| i)
            .collect();
        let page_position = if candidates.is_empty() {
            if self.pages_used >= SLAB_PAGES {
                return Err(super::memory::error("guest heap exhausted"));
            }
            let index = self.pages_used;
            self.pages_used += 1;
            self.classes[class].pages.push(Page {
                index,
                free: (1..slots_per_page).collect(),
            });
            self.classes[class].pages.len() - 1
        } else {
            if self.random_placement {
                candidates[self.rng.below(candidates.len())]
            } else {
                candidates[0]
            }
        };
        let page = &mut self.classes[class].pages[page_position];
        let pick = if self.random_placement {
            self.rng.below(page.free.len())
        } else {
            page.free.len() - 1
        };
        let slot = page.free.swap_remove(pick);
        let page_index = page.index;
        // The first slot of every page is skipped: its granules stay UNALLOCATED
        // so an underflow off the front of a chunk lands on a guard.
        let address = HEAP_BASE
            + u64::try_from(page_index).expect("page index") * PAGE_BYTES
            + u64::from(slot) * class_size;
        let granules = size.div_ceil(GRANULE);
        let slot_granules = class_size / GRANULE;
        let tag = self.issue_tag();
        self.set_granules(address, granules, u8::try_from(tag).unwrap_or(GUARD_TAG));
        self.set_granules(
            address + granules * GRANULE,
            slot_granules - granules,
            GUARD_TAG,
        );
        Ok((
            address,
            tag,
            Chunk::Slab {
                class,
                page: page_position,
                slot,
            },
        ))
    }
    fn allocate_large(&mut self, size: u64) -> Result<(u64, u64, Chunk), EngineError> {
        let granules = size.div_ceil(GRANULE);
        // Half the time reuse a freed run chosen at random; otherwise bump into
        // fresh space behind a random gap. Either way the address is not a
        // function of the allocation sequence.
        if !self.large_free.is_empty() && self.rng.below(2) == 0 {
            let index = self.rng.below(self.large_free.len());
            let (start, capacity) = self.large_free[index];
            if capacity > granules {
                self.large_free.swap_remove(index);
                let tag = self.issue_tag();
                self.set_granules(start, granules, u8::try_from(tag).unwrap_or(GUARD_TAG));
                self.set_granules(start + granules * GRANULE, 1, GUARD_TAG);
                if capacity > granules + 2 {
                    self.large_free
                        .push((start + (granules + 1) * GRANULE, capacity - granules - 1));
                }
                return Ok((start, tag, Chunk::Large { granules }));
            }
        }
        let gap = u64::try_from(self.rng.below(5)).expect("small gap") * GRANULE;
        let start = self.large_cursor + gap;
        let needed = (granules + 1) * GRANULE;
        if start + needed > HEAP_BASE + HEAP_SIZE {
            return Err(super::memory::error("guest heap exhausted"));
        }
        self.large_cursor = start + needed;
        let tag = self.issue_tag();
        self.set_granules(start, granules, u8::try_from(tag).unwrap_or(GUARD_TAG));
        self.set_granules(start + granules * GRANULE, 1, GUARD_TAG);
        Ok((start, tag, Chunk::Large { granules }))
    }

    /// Free a chunk. The caller supplies the pointer exactly as issued; a
    /// pointer whose tag does not match the chunk it names is a corrupted
    /// pointer and faults rather than freeing something arbitrary.
    pub fn release(&mut self, pointer: u64, pc: u64) -> Result<(), EngineError> {
        let address = self.scheme.strip(pointer);
        let live = self
            .live
            .get(&address)
            .copied()
            .ok_or_else(|| super::memory::error(format!("free of non-chunk 0x{address:x}")))?;
        // Checked before the chunk is removed, so a refused free leaves the
        // allocation intact rather than silently dropping it.
        if self.tags_enabled && self.scheme.tag_of(pointer) != live.tag {
            self.stats.tag_faults += 1;
            return Err(fail(
                pc,
                MitigationKind::TagMismatch,
                format!(
                    "free of 0x{address:x} carries tag {} but the chunk is tagged {}",
                    self.scheme.tag_of(pointer),
                    live.tag
                ),
            ));
        }
        self.live.remove(&address);
        if live.tag != 0 {
            if let Some(count) = self.live_tags.get_mut(&live.tag) {
                *count -= 1;
                if *count == 0 {
                    self.live_tags.remove(&live.tag);
                }
            }
        }
        self.stats.frees += 1;
        match live.chunk {
            Chunk::Slab { class, page, slot } => {
                let slot_granules = CLASS_SIZES[class] / GRANULE;
                // Every granule of the slot, including its guard tail, becomes
                // quarantined: a read or write anywhere in the freed slot faults.
                self.set_granules(address, slot_granules, QUARANTINE_TAG);
                self.quarantine.push_back(Chunk::Slab { class, page, slot });
                while self.quarantine.len() > QUARANTINE_CAPACITY {
                    let oldest = self.quarantine.pop_front().expect("non-empty");
                    self.reclaim(oldest);
                }
            }
            Chunk::Large { granules } => {
                let total = granules + 1;
                self.set_granules(address, total, QUARANTINE_TAG);
                // Large runs wait in quarantine too: reuse is delayed so a
                // dangling pointer cannot be re-aimed by an immediate realloc.
                self.large_quarantine.push_back((address, total));
                while self.large_quarantine.len() > QUARANTINE_CAPACITY {
                    let oldest = self.large_quarantine.pop_front().expect("non-empty");
                    self.large_free.push(oldest);
                }
            }
        }
        Ok(())
    }
    fn reclaim(&mut self, chunk: Chunk) {
        if let Chunk::Slab { class, page, slot } = chunk {
            if let Some(page) = self.classes[class].pages.get_mut(page) {
                if !page.free.contains(&slot) {
                    page.free.push(slot);
                }
            }
        }
    }

    /// Validate a data access and return the untagged address.
    ///
    /// A pointer into the heap must carry the tag of every granule it touches.
    /// A pointer outside the heap must be untagged, so a heap pointer that is
    /// redirected at the stack or at globals still faults.
    pub fn check(&mut self, address: u64, size: u64, pc: u64) -> Result<u64, EngineError> {
        let tag = self.scheme.tag_of(address);
        let base = self.scheme.strip(address);
        if !self.tags_enabled {
            return Ok(base);
        }
        if !self.contains(base) {
            if tag != 0 {
                self.stats.tag_faults += 1;
                return Err(fail(
                    pc,
                    MitigationKind::TagMismatch,
                    format!("tagged pointer 0x{address:x} dereferenced outside the heap"),
                ));
            }
            return Ok(base);
        }
        let end = base
            .checked_add(size)
            .ok_or_else(|| super::memory::error("heap access overflow"))?;
        if end > HEAP_BASE + HEAP_SIZE {
            self.stats.tag_faults += 1;
            return Err(fail(
                pc,
                MitigationKind::TagMismatch,
                format!("heap access 0x{base:x}+{size} leaves the heap"),
            ));
        }
        let first = ((base - HEAP_BASE) / GRANULE) as usize;
        let last = ((end - 1 - HEAP_BASE) / GRANULE) as usize;
        for index in first..=last {
            let expected = self.shadow.get(index).copied().unwrap_or(UNALLOCATED_TAG);
            if u64::from(expected) != tag {
                self.stats.tag_faults += 1;
                let kind = if expected == QUARANTINE_TAG {
                    MitigationKind::UseAfterFree
                } else {
                    MitigationKind::TagMismatch
                };
                return Err(fail(pc, kind, format!(
                    "access at 0x{base:x} ({size} bytes) carries tag {tag} but granule {} is tagged {expected:#04x} at 0x{:x}",
                    index, HEAP_BASE + index as u64 * GRANULE
                )));
            }
        }
        Ok(base)
    }
    /// A pointer used outside the heap must be untagged: a heap pointer that
    /// gets pointed at the stack, at globals or at the image is a bug.
    pub fn reject_tagged(&mut self, address: u64, pc: u64) -> Result<u64, EngineError> {
        let tag = self.scheme.tag_of(address);
        if self.tags_enabled && tag != 0 {
            self.stats.tag_faults += 1;
            return Err(fail(
                pc,
                MitigationKind::TagMismatch,
                format!("tagged pointer 0x{address:x} dereferenced outside the heap"),
            ));
        }
        Ok(self.scheme.strip(address))
    }
    /// Shadow tag currently recorded for a bare address, for diagnostics.
    #[cfg(test)]
    pub fn shadow_tag(&self, address: u64) -> Option<u8> {
        if !self.contains(address) {
            return None;
        }
        self.shadow
            .get(((address - HEAP_BASE) / GRANULE) as usize)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heap(tags: bool) -> Heap {
        Heap::new(TagScheme::SOFTWARE, Csprng::from_seed_test(), tags, true)
    }

    #[test]
    fn slab_allocation_returns_tagged_pointers_inside_the_heap() {
        let mut heap = heap(true);
        for size in [1u64, 16, 17, 64, 100, 4096] {
            let pointer = heap.allocate(size).expect("allocate");
            let address = TagScheme::SOFTWARE.strip(pointer);
            assert!(heap.contains(address), "{size} -> 0x{address:x}");
            assert_ne!(TagScheme::SOFTWARE.tag_of(pointer), 0);
        }
    }

    #[test]
    fn overflow_into_the_next_granule_faults_before_it_lands() {
        let mut heap = heap(true);
        let a = heap.allocate(16).expect("a");
        // Write 16 bytes legitimately, then one granule past the end.
        assert!(heap.check(a, 16, 0).is_ok());
        let error = heap.check(a.wrapping_add(16), 1, 0x100).unwrap_err();
        assert!(
            matches!(
                error,
                EngineError::Mitigation {
                    kind: MitigationKind::TagMismatch | MitigationKind::UseAfterFree,
                    ..
                }
            ),
            "{error:?}"
        );
        // Untagged access into the heap is rejected outright.
        let address = TagScheme::SOFTWARE.strip(a);
        assert!(heap.check(address, 8, 0x100).is_err());
    }

    #[test]
    fn freed_chunks_are_quarantined_and_tagged_as_such() {
        let mut heap = heap(true);
        let pointer = heap.allocate(32).expect("allocate");
        heap.release(pointer, 0).expect("free");
        let error = heap.check(pointer, 4, 0x200).unwrap_err();
        assert!(
            matches!(
                error,
                EngineError::Mitigation {
                    kind: MitigationKind::UseAfterFree,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            heap.shadow_tag(TagScheme::SOFTWARE.strip(pointer)),
            Some(QUARANTINE_TAG)
        );
    }

    #[test]
    fn free_with_a_corrupted_tag_is_refused() {
        let mut heap = heap(true);
        let pointer = heap.allocate(32).expect("allocate");
        let forged = TagScheme::SOFTWARE
            .tag(TagScheme::SOFTWARE.strip(pointer), 7)
            .expect("tag fits");
        assert!(heap.release(forged, 0x300).is_err());
        assert_eq!(heap.stats().live_chunks, 1);
    }

    #[test]
    fn slot_placement_is_not_the_allocation_order() {
        let mut heap = heap(false);
        let first = heap.allocate(32).expect("first");
        heap.release(first, 0).expect("free");
        // Drain the quarantine so the slot is genuinely available again.
        for _ in 0..QUARANTINE_CAPACITY + 1 {
            let pointer = heap.allocate(32).expect("allocate");
            heap.release(pointer, 0).expect("free");
        }
        let mut addresses = std::collections::BTreeSet::new();
        for _ in 0..32 {
            let pointer = heap.allocate(32).expect("allocate");
            addresses.insert(TagScheme::SOFTWARE.strip(pointer));
        }
        // Randomized draws spread a single size class across many slots rather
        // than reusing one LIFO slot.
        assert!(addresses.len() > 8, "distinct slots: {}", addresses.len());
    }

    /// Run one fixed allocation/free sequence and record the addresses issued.
    fn sequence(random: bool, seed: u8) -> Vec<u64> {
        let mut heap = Heap::new(
            TagScheme::SOFTWARE,
            Csprng::from_seed([seed; 32], [seed; 12]),
            false,
            random,
        );
        let mut addresses = Vec::new();
        let mut held: Vec<u64> = Vec::new();
        for round in 0..64u64 {
            let pointer = heap.allocate(32).expect("allocate");
            addresses.push(TagScheme::SOFTWARE.strip(pointer));
            if round % 3 == 0 && !held.is_empty() {
                let victim = held.remove(held.len() - 1);
                heap.release(victim, 0).expect("free");
            } else {
                held.push(pointer);
            }
        }
        addresses
    }

    #[test]
    fn placement_is_a_function_of_the_allocation_sequence_only_when_unrandomized() {
        // Grooming needs "the same frees and allocations give the same layout".
        // With ordered reuse that holds for any key; with randomized draws the
        // layout additionally depends on key material the attacker lacks.
        let predictable = sequence(false, 0x11) == sequence(false, 0x9e);
        let keyed = sequence(true, 0x11) == sequence(true, 0x9e);
        assert!(predictable, "ordered placement must be reproducible");
        assert!(!keyed, "randomized placement must depend on the key");
    }

    #[test]
    fn tags_disabled_makes_the_heap_behave_like_a_flat_region() {
        let mut heap = heap(false);
        let pointer = heap.allocate(32).expect("allocate");
        assert_eq!(TagScheme::SOFTWARE.tag_of(pointer), 0);
        assert!(heap.check(pointer.wrapping_add(64), 8, 0).is_ok());
    }

    #[test]
    fn issued_tags_never_collide_with_shadow_sentinels() {
        // A live chunk tagged 0xff would look quarantined after free (silent
        // UAF); 0xfe would look like a guard tail. Draw many tags and require
        // every one to stay clear of the sentinel range.
        let mut heap = heap(true);
        for i in 0..512 {
            let pointer = heap.allocate(16).expect("allocate");
            let tag = TagScheme::SOFTWARE.tag_of(pointer);
            assert!(
                tag >= 1 && tag <= ISSUED_TAG_MAX,
                "draw {i} issued reserved tag {tag:#04x}"
            );
        }
    }

    #[test]
    fn live_chunks_do_not_share_a_tag() {
        let mut heap = heap(true);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let pointer = heap.allocate(16).expect("allocate");
            let tag = TagScheme::SOFTWARE.tag_of(pointer);
            assert!(seen.insert(tag), "duplicate live tag {tag}");
        }
    }

    #[test]
    fn a_tag_that_would_match_quarantine_is_never_issued() {
        // Directly pin the collision case: after free the shadow is 0xff, so
        // a pointer carrying 0xff would pass `check` instead of faulting.
        let mut heap = heap(true);
        for _ in 0..1024 {
            let pointer = heap.allocate(16).expect("allocate");
            let tag = TagScheme::SOFTWARE.tag_of(pointer);
            assert_ne!(tag, QUARANTINE_TAG as u64);
            assert_ne!(tag, GUARD_TAG as u64);
            assert_ne!(tag, UNALLOCATED_TAG as u64);
            heap.release(pointer, 0).expect("free");
        }
    }

    #[test]
    fn large_frees_are_quarantined_before_reuse() {
        let mut heap = heap(true);
        let first = heap.allocate(8192).expect("large");
        let address = TagScheme::SOFTWARE.strip(first);
        heap.release(first, 0).expect("free");
        // Still quarantined: the tag map says so even before capacity fills.
        assert_eq!(heap.shadow_tag(address), Some(QUARANTINE_TAG));
        // Fill the quarantine with other large frees so the first becomes
        // reusable, then take one allocation and require it not to silently
        // accept the old tagged pointer.
        for _ in 0..QUARANTINE_CAPACITY {
            let p = heap.allocate(8192).expect("fill");
            heap.release(p, 0).expect("free");
        }
        let reused = heap.allocate(8192).expect("reuse");
        // Old pointer still carries the freed chunk's tag; the new chunk has
        // its own. Access through the stale pointer must not succeed as if it
        // named the live chunk.
        if TagScheme::SOFTWARE.strip(reused) == address {
            let error = heap.check(first, 8, 0x400).unwrap_err();
            assert!(
                matches!(
                    error,
                    EngineError::Mitigation {
                        kind: MitigationKind::TagMismatch | MitigationKind::UseAfterFree,
                        ..
                    }
                ),
                "{error:?}"
            );
        }
    }
}
