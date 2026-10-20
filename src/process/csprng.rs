//! Host-side cryptographically seeded random stream for mitigation randomness.
//!
//! Heap slot selection, chunk tags and the pointer-authentication secret all
//! come from here. The stream is ChaCha20 (RFC 8439) keyed from OS entropy; a
//! process that cannot read OS entropy is a hard error rather than a silent
//! downgrade to a predictable stream, because predictable placement is the
//! property an attacker is grooming against.
//!
//! This generator is deliberately host-only: its state lives in Rust memory
//! that no guest address can name, and it is never exposed through the guest
//! syscall ABI.
/// ChaCha20 is 20 rounds: ten column/diagonal double rounds.
const DOUBLE_ROUNDS: usize = 10;

/// One ChaCha quarter-round on four named lanes of the working state.
fn quarter(w: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    let (mut va, mut vb, mut vc, mut vd) = (w[a], w[b], w[c], w[d]);
    va = va.wrapping_add(vb);
    vd = (vd ^ va).rotate_left(16);
    vc = vc.wrapping_add(vd);
    vb = (vb ^ vc).rotate_left(12);
    va = va.wrapping_add(vb);
    vd = (vd ^ va).rotate_left(8);
    vc = vc.wrapping_add(vd);
    vb = (vb ^ vc).rotate_left(7);
    w[a] = va;
    w[b] = vb;
    w[c] = vc;
    w[d] = vd;
}

fn block(state: &[u32; 16], out: &mut [u8; 64]) {
    let mut w = [0u32; 16];
    w.copy_from_slice(state);
    for _ in 0..DOUBLE_ROUNDS {
        quarter(&mut w, 0, 4, 8, 12);
        quarter(&mut w, 1, 5, 9, 13);
        quarter(&mut w, 2, 6, 10, 14);
        quarter(&mut w, 3, 7, 11, 15);
        quarter(&mut w, 0, 5, 10, 15);
        quarter(&mut w, 1, 6, 11, 12);
        quarter(&mut w, 2, 7, 8, 13);
        quarter(&mut w, 3, 4, 9, 14);
    }
    for (word, state_word) in w.iter_mut().zip(state.iter()) {
        *word = word.wrapping_add(*state_word);
    }
    for (index, word) in w.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// 32-byte key followed by a 12-byte nonce, as RFC 8439 lays out the state.
fn os_entropy() -> Result<([u8; 32], [u8; 12]), std::io::Error> {
    let mut seed = [0u8; 44];
    // Both paths are read-only and open a kernel device; no guest state involved.
    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => {
            use std::io::Read;
            file.read_exact(&mut seed)?;
            Ok((
                seed[..32].try_into().expect("32 bytes"),
                seed[32..].try_into().expect("12 bytes"),
            ))
        }
        Err(error) => Err(error),
    }
}

#[derive(Clone)]
pub struct Csprng {
    state: [u32; 16],
    buffer: [u8; 64],
    offset: usize,
    draws: u64,
}
impl Csprng {
    /// Seed from OS entropy. Host time, pid and an address are mixed in as
    /// additional input, not as a substitute: they add per-process variation
    /// even if two processes somehow receive identical device output.
    pub fn new() -> Result<Self, std::io::Error> {
        let (mut key, nonce) = os_entropy()?;
        // Host time, pid and an address are mixed into the key as additional
        // input, not as a substitute: they add per-process variation even if
        // two processes somehow receive identical device output.
        let extra = (std::process::id() as u64)
            ^ (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0))
            ^ (&key as *const u8 as u64);
        for (i, byte) in extra.to_le_bytes().iter().enumerate() {
            key[i] ^= byte;
        }
        Ok(Self::from_seed(key, nonce))
    }
    /// Deterministic stream for tests: never used to place a real heap.
    #[cfg(test)]
    pub fn from_seed_test() -> Self {
        Self::from_seed([0xa5; 32], [0; 12])
    }
    /// Build a generator from explicit key material. Tests use this; the
    /// runtime always goes through `new`, which reads OS entropy.
    pub fn from_seed(key: [u8; 32], nonce: [u8; 12]) -> Self {
        // "expand 32-byte k" || key || counter || nonce
        let mut state = [0u32; 16];
        state[0] = 0x6170_7865;
        state[1] = 0x3320_646e;
        state[2] = 0x7962_2d32;
        state[3] = 0x6b20_6574;
        for (i, word) in state[4..12].iter_mut().enumerate() {
            *word = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().expect("four bytes"));
        }
        state[12] = 0;
        for (i, word) in state[13..16].iter_mut().enumerate() {
            *word = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().expect("four bytes"));
        }
        Self {
            state,
            buffer: [0; 64],
            offset: 64,
            draws: 0,
        }
    }
    fn refill(&mut self) {
        block(&self.state, &mut self.buffer);
        self.state[12] = self.state[12].wrapping_add(1);
        if self.state[12] == 0 {
            self.state[13] = self.state[13].wrapping_add(1);
        }
        self.offset = 0;
    }
    pub fn fill(&mut self, bytes: &mut [u8]) {
        for byte in bytes.iter_mut() {
            if self.offset == 64 {
                self.refill();
            }
            *byte = self.buffer[self.offset];
            self.offset += 1;
        }
        self.draws += 1;
    }
    pub fn u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill(&mut bytes);
        u64::from_le_bytes(bytes)
    }
    /// Uniform index below `limit`, by rejection sampling over a 64-bit draw.
    pub fn below(&mut self, limit: usize) -> usize {
        debug_assert!(limit > 0);
        if limit == 1 {
            return 0;
        }
        // Largest multiple of `limit` that fits in u64; draws at or above it
        // are discarded so every slot has exactly equal probability.
        let limit = limit as u64;
        let ceiling = u64::MAX - (u64::MAX % limit) - 1;
        loop {
            let draw = self.u64();
            if draw <= ceiling {
                return (draw % limit) as usize;
            }
        }
    }
    pub fn draws(&self) -> u64 {
        self.draws
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chacha20_matches_the_rfc8439_test_vector() {
        // RFC 8439 section 2.4.2: key = 00:01:..:1f, nonce =
        // 00:00:00:09:00:00:00:4a:00:00:00:00, counter = 1.
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        let nonce = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        let mut rng = Csprng::from_seed(key, nonce);
        rng.state[12] = 1;
        rng.refill();
        let expected: [u8; 64] = [
            0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20,
            0x71, 0xc4, 0xc7, 0xd1, 0xf4, 0xc7, 0x33, 0xc0, 0x68, 0x03, 0x04, 0x22, 0xaa, 0x9a,
            0xc3, 0xd4, 0x6c, 0x4e, 0xd2, 0x82, 0x64, 0x46, 0x07, 0x9f, 0xaa, 0x09, 0x14, 0xc2,
            0xd7, 0x05, 0xd9, 0x8b, 0x02, 0xa2, 0xb5, 0x12, 0x9c, 0xd1, 0xde, 0x16, 0x4e, 0xb9,
            0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e,
        ];
        assert_eq!(rng.buffer, expected);
    }

    #[test]
    fn below_is_uniform_enough_and_never_reaches_the_limit() {
        let mut rng = Csprng::from_seed([0x5a; 32], [0; 12]);
        let mut counts = [0usize; 8];
        for _ in 0..40_000 {
            let index = rng.below(8);
            assert!(index < 8);
            counts[index] += 1;
        }
        // 40000/8 = 5000 expected; a fair stream stays within 10% here.
        for count in counts {
            assert!((4_500..=5_500).contains(&count), "{counts:?}");
        }
    }

    #[test]
    fn below_one_always_returns_zero() {
        let mut rng = Csprng::from_seed([0x11; 32], [0; 12]);
        for _ in 0..4 {
            assert_eq!(rng.below(1), 0);
        }
    }
}
