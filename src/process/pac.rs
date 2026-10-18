//! Context-bound pointer authentication for function pointers in memory.
//!
//! Memory tagging (see `heap.rs`) stops a write that crosses into another
//! chunk. It does not stop a write that stays inside a chunk and lands on a
//! function pointer field, and it does not stop an attacker who learns a chunk
//! tag. This module covers that gap.
//!
//! Scheme
//! ------
//! A truncated SipHash-2-4 MAC is computed over (pointer value, storage
//! address, 128-bit per-process secret) and carried in bits 48:55 of the stored
//! word -- the byte that is unused in a 48-bit user address and, importantly,
//! is *outside* the LAM_U57 mask (57:62), so tagging and PAC do not collide.
//!
//! Registers never hold signed pointers: a load verifies and strips, a store
//! signs. The guest therefore sees its own values unchanged, and pointer
//! arithmetic on a clean pointer is unaffected.
//!
//! What this stops, and the one thing it does not
//! ----------------------------------------------
//! Stopped: overwriting an authenticated pointer slot with an unsigned value --
//! the load finds no valid code and faults. Stopped: copying a signed pointer
//! from one slot to another -- the code is bound to the storage address, and
//! re-signing a slot that already holds an authenticated pointer requires the
//! value to come from an immediate/rip-relative source, which attacker data is
//! not.
//!
//! Not stopped: an attacker with an arbitrary 8-byte write who targets a slot
//! that has never held a pointer. Such a slot is not marked, so its first
//! initialization is signed for free. Closing that requires knowing the type
//! layout, which a binary-only lift does not have. Tagging is what covers a
//! write of that shape when it crosses a chunk boundary.
//!
//! The secret never enters guest memory or the code cache: it lives only in
//! this struct, which no guest address can name.

use crate::error::{EngineError, MitigationKind};
use std::collections::HashSet;

/// Bits 48:55. Chosen because a 48-bit user address leaves them zero, and
/// because LAM_U57 masks 57:62 rather than this field.
const PAC_SHIFT: u32 = 48;
const PAC_MASK: u64 = 0x00ff_0000_0000_0000;

fn sip_round(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
    *v0 = v0.wrapping_add(*v1);
    *v1 = v1.rotate_left(13) ^ *v0;
    *v0 = v0.rotate_left(32);
    *v2 = v2.wrapping_add(*v3);
    *v3 = v3.rotate_left(16) ^ *v2;
    *v0 = v0.wrapping_add(*v3);
    *v3 = v3.rotate_left(21) ^ *v0;
    *v2 = v2.wrapping_add(*v1);
    *v1 = v1.rotate_left(17) ^ *v2;
    *v2 = v2.rotate_left(32);
}

/// SipHash-2-4 over `value || address`, keyed by the process secret.
fn siphash(k0: u64, k1: u64, words: [u64; 2]) -> u64 {
    let mut v0 = k0 ^ 0x736f_6d65_7073_6575;
    let mut v1 = k1 ^ 0x646f_7261_6e64_6f6d;
    let mut v2 = k0 ^ 0x6c79_6765_6e65_7261;
    let mut v3 = k1 ^ 0x7465_6462_7974_6573;
    for word in words {
        v3 ^= word;
        for _ in 0..2 {
            sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        }
        v0 ^= word;
    }
    // Termination block: no remaining data bytes, so only the length byte.
    let last = 0x10u64 << 56;
    v3 ^= last;
    for _ in 0..2 {
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    }
    v0 ^= last;
    v2 ^= 0xff;
    for _ in 0..4 {
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    }
    v0 ^ v1 ^ v2 ^ v3
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PacStats {
    pub signed_slots: usize,
    pub signings: u64,
    pub verifications: u64,
    pub auth_faults: u64,
    pub static_pointers: usize,
}

pub struct Pac {
    k0: u64,
    k1: u64,
    /// Addresses of 8-byte slots currently holding an authenticated pointer.
    slots: HashSet<u64>,
    /// Addresses that have *ever* held an authenticated pointer. A data store
    /// clears `slots` so the word reads back as ordinary data, but this set
    /// keeps the slot classified: a later code-pointer store still requires
    /// constant provenance. Without it an attacker unmarks the slot with a
    /// data write and then signs an arbitrary code pointer into it.
    pointer_history: HashSet<u64>,
    /// Guest virtual ranges that hold executable code.
    code: Vec<std::ops::Range<u64>>,
    enabled: bool,
    stats: PacStats,
}
impl Pac {
    pub fn new(secret: [u8; 16], code: Vec<std::ops::Range<u64>>, enabled: bool) -> Self {
        let mut k0 = [0u8; 8];
        let mut k1 = [0u8; 8];
        k0.copy_from_slice(&secret[..8]);
        k1.copy_from_slice(&secret[8..]);
        Self {
            k0: u64::from_le_bytes(k0),
            k1: u64::from_le_bytes(k1),
            slots: HashSet::new(),
            pointer_history: HashSet::new(),
            code,
            enabled,
            stats: PacStats::default(),
        }
    }
    pub fn stats(&self) -> PacStats {
        let mut stats = self.stats;
        stats.signed_slots = self.slots.len();
        stats
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    /// A pointer value lying inside an executable segment: a plausible target
    /// for an indirect call, and the only kind of value this signs.
    pub fn is_code_address(&self, value: u64) -> bool {
        self.code.iter().any(|r| r.contains(&value))
    }
    /// Truncated MAC in 1..=255. Zero is reserved for "no code present".
    fn code(&self, value: u64, address: u64) -> u64 {
        let mac = siphash(self.k0, self.k1, [value, address]) & 0xff;
        if mac == 0 {
            1
        } else {
            mac
        }
    }
    pub fn is_signed_slot(&self, address: u64) -> bool {
        self.enabled && self.slots.contains(&address)
    }
    /// Word value as it must be stored at `address`.
    pub fn signed_word(&self, value: u64, address: u64) -> u64 {
        (self.code(value, address) << PAC_SHIFT) | value
    }

    /// Authenticate an 8-byte store. Returns the word to write.
    ///
    /// `constant_provenance` says the value came from an immediate or a
    /// rip-relative/literal-pool load rather than from guest data. Overwriting
    /// a slot that has ever held an authenticated pointer without that
    /// provenance is rejected: that is exactly the shape of a pointer-
    /// substitution attack, including the unmark-then-sign two-store variant.
    pub fn on_store(
        &mut self,
        address: u64,
        value: u64,
        size: u64,
        constant_provenance: bool,
        pc: u64,
    ) -> Result<u64, EngineError> {
        if !self.enabled || size != 8 {
            return Ok(value);
        }
        if self.is_code_address(value) {
            let was_pointer_slot =
                self.slots.contains(&address) || self.pointer_history.contains(&address);
            if was_pointer_slot && !constant_provenance {
                self.stats.auth_faults += 1;
                return Err(EngineError::Mitigation {
                    pc,
                    kind: MitigationKind::PointerAuth,
                    detail: format!(
                        "store of code pointer 0x{value:x} into pointer slot 0x{address:x} without constant provenance"
                    ),
                });
            }
            self.slots.insert(address);
            self.pointer_history.insert(address);
            self.stats.signings += 1;
            return Ok(self.signed_word(value, address));
        }
        // Not a code pointer: the word reads back as ordinary data, so drop
        // the live marking. The history entry remains, so a later code-pointer
        // store into this field still needs constant provenance.
        self.slots.remove(&address);
        Ok(value)
    }

    /// Authenticate an 8-byte load. Returns the clean pointer value.
    ///
    /// A word is treated as a signed pointer when it carries a non-zero code
    /// and its address part lies in executable code. That test is safe against
    /// false positives -- a data word that happens to have high bits set is not
    /// a code address, and a code address has no high bits of its own -- and it
    /// catches a signed pointer that was copied into a slot this runtime never
    /// saw initialized, which is the substitution case.
    pub fn on_load(
        &mut self,
        address: u64,
        raw: u64,
        size: u64,
        pc: u64,
    ) -> Result<u64, EngineError> {
        if !self.enabled || size != 8 {
            return Ok(raw);
        }
        let presented = (raw >> PAC_SHIFT) & 0xff;
        let value = raw & !PAC_MASK;
        let marked = self.slots.contains(&address);
        // An unmarked slot holding an ordinary word is none of our business.
        if !marked && (presented == 0 || !self.is_code_address(value)) {
            return Ok(raw);
        }
        // Otherwise the word must be signed for exactly this address: a marked
        // slot may not be read back unsigned, and a signed pointer may not be
        // read from anywhere but the slot it was signed for.
        let expected = self.code(value, address);
        self.stats.verifications += 1;
        if !marked || presented == 0 || presented != expected {
            self.stats.auth_faults += 1;
            let relocated = if !marked {
                "slot never held a signed pointer"
            } else if presented == 0 {
                "authenticated slot read back without a code"
            } else {
                "code does not match this storage address"
            };
            return Err(EngineError::Mitigation {
                pc,
                kind: MitigationKind::PointerAuth,
                detail: format!(
                    "pointer 0x{value:x} at 0x{address:x} carries code {presented:#04x} but {relocated} (expected {expected:#04x})"
                ),
            });
        }
        Ok(value)
    }

    /// Sign and mark statically initialized pointer words already present in a
    /// writable image. Without this, a vtable built by the linker would never
    /// be covered, because no runtime store ever touched it.
    ///
    /// Returns how many slots were signed. The caller supplies the region; this
    /// writes the signed word back through `write`.
    pub fn seed_image(
        &mut self,
        base: u64,
        bytes: &[u8],
        write: &mut impl FnMut(u64, [u8; 8]),
    ) -> usize {
        if !self.enabled {
            return 0;
        }
        let mut count = 0;
        for offset in (0..bytes.len().saturating_sub(8)).step_by(8) {
            let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("8 bytes"));
            if word & PAC_MASK == 0 && self.is_code_address(word) {
                let address = base + offset as u64;
                let signed = self.signed_word(word, address);
                self.slots.insert(address);
                self.pointer_history.insert(address);
                write(address, signed.to_le_bytes());
                count += 1;
            }
        }
        self.stats.static_pointers = count;
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pac() -> Pac {
        let code: Vec<std::ops::Range<u64>> = std::iter::once(0x200000..0x201000).collect();
        Pac::new([0x33; 16], code, true)
    }

    #[test]
    fn siphash_matches_the_reference_vector() {
        // SipHash-2-4 of 16 zero bytes under an all-zero key. The value below
        // is the reference implementation's output; a change to the round
        // count, the constants or the termination block must move it.
        assert_eq!(siphash(0, 0, [0, 0]), 0x32ca_ecc2_8017_2976);
        // Every input must reach the output: key, value and address each move it.
        assert_ne!(siphash(0, 0, [0, 0]), siphash(0, 0, [1, 0]));
        assert_ne!(siphash(0, 0, [0, 0]), siphash(1, 0, [0, 0]));
        assert_ne!(siphash(0, 0, [0, 0]), siphash(0, 0, [0, 1]));
    }

    #[test]
    fn a_signed_pointer_round_trips_at_its_own_address_only() {
        let mut pac = pac();
        let target = 0x200100u64;
        let stored = pac.on_store(0x30000, target, 8, true, 0).expect("sign");
        assert_ne!(stored, target);
        assert_eq!(stored & !PAC_MASK, target);
        assert_eq!(pac.on_load(0x30000, stored, 8, 0).expect("verify"), target);
        // Same bytes at a different storage address: the code is bound to where
        // the pointer is stored, so relocating it invalidates it.
        assert!(pac.on_load(0x30008, stored, 8, 0).is_err());
        // A plain data word with high bits set is untouched: not a code address.
        assert_eq!(
            pac.on_load(0x30010, 0x00ab_0000_dead_beef, 8, 0).unwrap(),
            0x00ab_0000_dead_beef
        );
        // An unsigned code address in an unmarked slot is left alone; it is
        // caught later, if the guest actually branches through it.
        assert!(!pac.is_signed_slot(0x30018));
    }

    #[test]
    fn overwriting_an_authenticated_slot_with_unsigned_data_faults() {
        let mut pac = pac();
        let stored = pac.on_store(0x30000, 0x200100, 8, true, 0).expect("sign");
        assert_eq!(pac.on_load(0x30000, stored, 8, 0).expect("ok"), 0x200100);
        // Attacker writes the address with no code, from tainted provenance.
        let error = pac
            .on_load(0x30000, 0x200100, 8, 0x400)
            .expect_err("unsigned");
        assert!(matches!(
            error,
            EngineError::Mitigation {
                kind: MitigationKind::PointerAuth,
                ..
            }
        ));
    }

    #[test]
    fn substitution_from_another_slot_is_refused_at_store_time() {
        let mut pac = pac();
        let at_b = pac.on_store(0x30008, 0x200100, 8, true, 0).expect("sign b");
        let moved = pac.on_load(0x30008, at_b, 8, 0).expect("load b");
        assert_eq!(moved, 0x200100);
        // Copying it into an already-authenticated slot from data provenance.
        pac.on_store(0x30000, 0x200200, 8, true, 0).expect("sign a");
        let error = pac
            .on_store(0x30000, moved, 8, false, 0x500)
            .expect_err("substitution");
        assert!(matches!(
            error,
            EngineError::Mitigation {
                kind: MitigationKind::PointerAuth,
                ..
            }
        ));
    }

    #[test]
    fn non_pointer_words_are_untouched_but_keep_pointer_history() {
        let mut pac = pac();
        assert_eq!(
            pac.on_store(0x30000, 0xdead_beef, 8, true, 0).unwrap(),
            0xdead_beef
        );
        assert!(!pac.is_signed_slot(0x30000));
        pac.on_store(0x30000, 0x200100, 8, true, 0).expect("sign");
        assert!(pac.is_signed_slot(0x30000));
        // A data overwrite clears the live marking so the word reads back as
        // ordinary data...
        pac.on_store(0x30000, 0xdead_beef, 8, true, 0)
            .expect("reuse");
        assert!(!pac.is_signed_slot(0x30000));
        assert_eq!(
            pac.on_load(0x30000, 0xdead_beef, 8, 0).unwrap(),
            0xdead_beef
        );
        // ...but a later code-pointer store without constant provenance is
        // still refused: the slot's pointer history is permanent.
        let error = pac
            .on_store(0x30000, 0x200100, 8, false, 0x600)
            .expect_err("unmark-then-sign");
        assert!(matches!(
            error,
            EngineError::Mitigation {
                kind: MitigationKind::PointerAuth,
                ..
            }
        ));
        // Constant provenance may legitimately re-initialize the field.
        pac.on_store(0x30000, 0x200200, 8, true, 0).expect("reinit");
        // Narrow stores never carry a pointer.
        assert_eq!(
            pac.on_store(0x30010, 0x200100, 4, true, 0).unwrap(),
            0x200100
        );
        assert!(!pac.is_signed_slot(0x30010));
    }

    #[test]
    fn unmark_then_sign_laundering_is_refused() {
        let mut pac = pac();
        // Victim slot starts as a real pointer field.
        pac.on_store(0x30000, 0x200100, 8, true, 0).expect("sign");
        // Attacker step 1: data write clears the live marking.
        pac.on_store(0x30000, 0x1111_1111_1111_1111, 8, false, 0)
            .expect("data");
        assert!(!pac.is_signed_slot(0x30000));
        // Attacker step 2: code-pointer write from tainted provenance must
        // still fail even though the slot looks unmarked.
        let error = pac
            .on_store(0x30000, 0x200200, 8, false, 0x700)
            .expect_err("laundered substitution");
        assert!(matches!(
            error,
            EngineError::Mitigation {
                kind: MitigationKind::PointerAuth,
                ..
            }
        ));
    }

    #[test]
    fn seed_image_signs_static_pointer_words() {
        let mut pac = pac();
        let mut image = vec![0u8; 32];
        image[0..8].copy_from_slice(&0x200100u64.to_le_bytes());
        image[8..16].copy_from_slice(&0xdead_beefu64.to_le_bytes());
        image[16..24].copy_from_slice(&0x200200u64.to_le_bytes());
        let mut written = Vec::new();
        let count = pac.seed_image(0x30000, &image, &mut |at, bytes| {
            written.push((at, bytes));
        });
        assert_eq!(count, 2);
        assert_eq!(written.len(), 2);
        assert!(pac.is_signed_slot(0x30000));
        assert!(pac.is_signed_slot(0x30010));
        assert!(!pac.is_signed_slot(0x30008));
    }
}
