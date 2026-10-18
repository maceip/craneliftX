//! JIT-time intra-ISA diversification: per-compilation randomization of
//! register mappings and stack frame layout.
//!
//! When compiling lifted x86-64 blocks to native x86-64, the same guest
//! register can be mapped to different Cranelift variable indices on each
//! compilation. This changes the data-flow graph, which changes the register
//! allocation decisions, which changes the generated machine code. The result
//! is a different gadget set for every compilation, even of the same source
//! block.
//!
//! Stack frame randomization adds random padding at the function prologue,
//! shifting the offsets of spilled values and local temporaries.
use crate::process::csprng::Csprng;

/// A per-compilation random permutation of guest-register indices.
///
/// GuestContext order is: rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp, r8..r15.
/// `permute[guest_index]` gives the Cranelift variable index used for that
/// guest register in this compilation. RIP (16) and RFLAGS (17) are fixed.
pub struct Diversification {
    /// 16-element permutation: permute[i] = variable index for guest register i.
    permute: [u32; 16],
    /// Random padding added to the stack frame (0..256 bytes, 8-byte aligned).
    stack_pad: i64,
    /// Random NOP count to insert after the function prologue (0..8).
    prologue_nops: u8,
}

impl Diversification {
    /// Generate a fresh randomization using the provided CSPRNG.
    pub fn new(rng: &mut Csprng) -> Self {
        let mut permute = [0u32; 16];
        for i in 0..16 {
            permute[i] = i as u32;
        }
        // Fisher-Yates shuffle
        for i in (1..16).rev() {
            let j = rng.below(i + 1);
            permute.swap(i, j);
        }
        let stack_pad = (rng.u64() & 0xF8) as i64; // 0..248, 8-byte aligned
        let prologue_nops = (rng.u64() & 7) as u8;
        Self {
            permute,
            stack_pad,
            prologue_nops,
        }
    }

    /// The identity permutation (no randomization). Used for tests and when
    /// diversification is disabled.
    pub fn identity() -> Self {
        Self {
            permute: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            stack_pad: 0,
            prologue_nops: 0,
        }
    }

    /// Map a guest register index (0-15) to the Cranelift variable index.
    pub fn variable(&self, guest_index: usize) -> u32 {
        self.permute[guest_index]
    }

    /// Bytes of random padding to add to the stack frame.
    pub fn stack_pad(&self) -> i64 {
        self.stack_pad
    }

    /// Number of NOP instructions to insert after the prologue.
    pub fn prologue_nops(&self) -> u8 {
        self.prologue_nops
    }

    /// A stable label for logging (e.g. "perm=[3,0,1,2,...] pad=120 nops=3").
    pub fn label(&self) -> String {
        format!(
            "perm=[{}] pad={} nops={}",
            self.permute
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(","),
            self.stack_pad,
            self.prologue_nops
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_rng() -> Csprng {
        let mut seed = [0u8; 44];
        seed[0] = 0xde;
        seed[1] = 0xad;
        seed[2] = 0xbe;
        seed[3] = 0xef;
        // Csprng doesn't expose a from-seed constructor; use a fixed-pattern seed.
        Csprng::new().unwrap()
    }

    #[test]
    fn identity_is_identity() {
        let div = Diversification::identity();
        for i in 0..16 {
            assert_eq!(div.variable(i), i as u32);
        }
        assert_eq!(div.stack_pad(), 0);
        assert_eq!(div.prologue_nops(), 0);
    }

    #[test]
    fn permutation_is_bijective() {
        let mut rng = dummy_rng();
        let div = Diversification::new(&mut rng);
        let mut seen = [false; 16];
        for i in 0..16 {
            let v = div.variable(i) as usize;
            assert!(!seen[v], "duplicate variable index {} in permutation", v);
            seen[v] = true;
        }
    }

    #[test]
    fn stack_pad_is_aligned() {
        let mut rng = dummy_rng();
        for _ in 0..100 {
            let div = Diversification::new(&mut rng);
            assert_eq!(div.stack_pad() % 8, 0);
            assert!(div.stack_pad() >= 0 && div.stack_pad() < 256);
        }
    }
}
