//! Forward-edge control-flow integrity: inline bitmap checks for indirect
//! branches in generated native code.
//!
//! Every compiled block has a set of valid entry points (the start address of
//! each block). When the generated code encounters an indirect call or jump,
//! it checks the target against this bitmap before transferring control. An
//! unlisted target traps with a CFI violation.
//!
//! The bitmap is a dense bitset covering the guest's executable region. One
//! bit per byte of code; a set bit means "this address is a valid entry point".
//! For a typical 16 MB code region the bitmap is 2 MB.

/// A bitmap of valid control-flow entry points in guest code.
pub struct EntryBitmap {
    /// Guest address that bitmap[0] corresponds to.
    base: u64,
    /// Bitmap bytes. Bit i of byte j is set iff address `base + j*8 + i`
    /// is a valid entry point.
    bits: Vec<u8>,
}

impl EntryBitmap {
    /// Create a bitmap covering `base..base+size` bytes of guest code.
    pub fn new(base: u64, size: usize) -> Self {
        let bytes = (size + 7) / 8;
        Self {
            base,
            bits: vec![0u8; bytes],
        }
    }

    /// Mark `addr` as a valid entry point.
    pub fn insert(&mut self, addr: u64) {
        let offset = addr.saturating_sub(self.base) as usize;
        if offset / 8 >= self.bits.len() {
            return; // outside coverage — silently ignored
        }
        let byte = offset / 8;
        let bit = offset % 8;
        self.bits[byte] |= 1 << bit;
    }

    /// Query whether `addr` is a valid entry point.
    pub fn contains(&self, addr: u64) -> bool {
        let offset = addr.saturating_sub(self.base) as usize;
        if offset / 8 >= self.bits.len() {
            return false;
        }
        let byte = offset / 8;
        let bit = offset % 8;
        (self.bits[byte] & (1 << bit)) != 0
    }

    /// Guest base address.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Bitmap as a raw byte slice (for passing to generated code).
    pub fn as_slice(&self) -> &[u8] {
        &self.bits
    }

    /// Number of bytes in the bitmap.
    pub fn len_bytes(&self) -> usize {
        self.bits.len()
    }

    /// Number of set bits (valid entry points).
    pub fn count(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// Build a bitmap from a collection of block start addresses.
    pub fn from_entries(base: u64, size: usize, entries: &[u64]) -> Self {
        let mut bitmap = Self::new(base, size);
        for &addr in entries {
            bitmap.insert(addr);
        }
        bitmap
    }
}

/// Tracks the extents of every decoded guest instruction so a transfer into
/// the middle of one can be rejected. This is the binary-only form of
/// forward-edge CFI: without a full CFG we cannot list every legal target,
/// but a jump into an instruction body is never a legal target.
#[derive(Default)]
pub struct InstructionMap {
    /// Inclusive start -> exclusive end for each decoded instruction.
    extents: std::collections::BTreeMap<u64, u64>,
}

impl InstructionMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one decoded instruction.
    pub fn record(&mut self, ip: u64, len: usize) {
        if len == 0 {
            return;
        }
        self.extents.insert(ip, ip + len as u64);
    }

    /// Record every instruction of a compiled block (`(ip, len)` pairs).
    pub fn record_block(&mut self, instructions: &[(u64, usize)]) {
        for &(ip, len) in instructions {
            self.record(ip, len);
        }
    }

    /// True when `rip` falls strictly inside a known instruction body.
    pub fn is_mid_instruction(&self, rip: u64) -> bool {
        if let Some((&start, &end)) = self.extents.range(..=rip).next_back() {
            return rip > start && rip < end;
        }
        false
    }

    /// True when `rip` is a known instruction start, or has never been decoded.
    pub fn allows(&self, rip: u64) -> bool {
        !self.is_mid_instruction(rip)
    }

    pub fn known_instructions(&self) -> usize {
        self.extents.len()
    }
}

/// The result of an inline CFI check, returned to the generated code.
pub enum CfiResult {
    Valid,
    Invalid,
}

/// Rust helper called from generated native code to validate an indirect
/// branch target. Returns 0 if valid, 1 if invalid.
///
/// # Safety
/// `bitmap` must point to a valid `EntryBitmap`.
#[no_mangle]
pub unsafe extern "C" fn isr_cfi_check(bitmap: *const EntryBitmap, target: u64) -> u32 {
    if bitmap.is_null() {
        return 1;
    }
    let bmp = unsafe { &*bitmap };
    if bmp.contains(target) {
        0
    } else {
        1
    }
}

/// Rust helper that records a CFI fault and returns an error code.
///
/// # Safety
/// `detail` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn isr_cfi_trap(_pc: u64, _target: u64) -> u64 {
    // This function is called from generated code when a CFI check fails.
    // We return a sentinel value that the runtime interprets as a CFI fault.
    // The actual error handling is done by the runtime when the function returns.
    u64::MAX - 3 // sentinel: CFI violation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_instruction_targets_are_rejected() {
        let mut map = InstructionMap::new();
        // Two consecutive instructions: 0x1000..0x1005 and 0x1005..0x1007.
        map.record(0x1000, 5);
        map.record(0x1005, 2);
        assert!(map.allows(0x1000));
        assert!(map.allows(0x1005));
        assert!(map.allows(0x1007)); // unknown, but not inside an instruction
        assert!(map.allows(0x2000)); // never decoded
        assert!(map.is_mid_instruction(0x1001));
        assert!(map.is_mid_instruction(0x1004));
        assert!(map.is_mid_instruction(0x1006));
        assert!(!map.allows(0x1003));
    }

    #[test]
    fn bitmap_coverage() {
        let mut bmp = EntryBitmap::new(0x1000, 64);
        bmp.insert(0x1000);
        bmp.insert(0x1005);
        bmp.insert(0x103F);
        assert!(bmp.contains(0x1000));
        assert!(bmp.contains(0x1005));
        assert!(bmp.contains(0x103F));
        assert!(!bmp.contains(0x1001));
        assert!(!bmp.contains(0x1040));
        assert_eq!(bmp.count(), 3);
    }

    #[test]
    fn bitmap_out_of_range() {
        let bmp = EntryBitmap::new(0x1000, 8);
        assert!(!bmp.contains(0x0FFF));
        assert!(!bmp.contains(0x1008));
        assert!(!bmp.contains(0xFFFF));
    }
}
