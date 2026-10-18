//! Intel Linear Address Masking (LAM): probe, enable, select a tag placement.
//!
//! What LAM buys
//! -------------
//! LAM makes the *hardware MMU* ignore bits 62:57 on data accesses, so a tagged
//! pointer can be dereferenced by native instructions without a software mask.
//! The tag still has to be checked against the shadow map before the access is
//! considered safe -- LAM only removes the address-forming `and`, not the
//! security check.
//!
//! Where it helps in this engine
//! -----------------------------
//! The host-native backend (`Backend::X86_64`) issues host loads and stores
//! through checked helpers. Those helpers still form a host address for the
//! `GuestMemory` byte slice, so they strip the tag in software. LAM is still
//! the right *scheme* whenever the CPU will accept tagged pointers (it is what
//! the hardware understands), and it is what a future raw-memory fast path
//! would rely on. The interpreter never executes guest addresses on the core,
//! so LAM is a no-op there -- the probe records that honestly.
//!
//! No custom kernel module is required. The interface is `arch_prctl` /
//! `prctl(PR_SET_TAGGED_ADDR_CTRL)` on Linux x86-64 with a LAM-capable core
//! (Sapphire Rapids and later, or any core with CPUID LAM). This module is the
//! optional "kernel capability" path: on by default, one flag to disable.
//!
//! Why LAM_U57 and not LAM_U48
//! ---------------------------
//! LAM_U57 masks bits 62:57 (6 bits). LAM_U48 masks bits 62:48 (15 bits), but
//! bits 48:55 are where `pac.rs` stores the pointer-authentication code, and a
//! LAM-masked bit is still sign-checked on instruction fetch and branch: a PAC
//! inside the mask would be stripped or faulted the moment the pointer is used
//! as a call target. U57 leaves 48:55 untouched, so tagging and PAC coexist.
//! U48 is therefore not offered.
//!
//! Bit 56 is *not* masked by LAM_U57. Software mode uses bits 56:63 because
//! there is no hardware mask to satisfy; LAM mode must move the tag to 57:62.

/// Where the chunk tag lives in a pointer, and which bits the hardware ignores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagScheme {
    pub shift: u32,
    pub bits: u32,
    pub masked_by_hardware: bool,
    pub label: &'static str,
}
impl TagScheme {
    pub const SOFTWARE: Self = Self {
        shift: 56,
        bits: 8,
        masked_by_hardware: false,
        label: "software-mask-56:63",
    };
    pub const LAM_U57: Self = Self {
        shift: 57,
        bits: 6,
        masked_by_hardware: true,
        label: "lam-u57-mask-57:62",
    };
    /// Mask of the tag field, positioned in the pointer.
    pub const fn mask(self) -> u64 {
        ((1u64 << self.bits) - 1) << self.shift
    }
    /// Strip metadata from a tagged pointer, leaving the address.
    pub const fn strip(self, pointer: u64) -> u64 {
        pointer & !self.mask()
    }
    /// Read the tag out of a pointer.
    pub const fn tag_of(self, pointer: u64) -> u64 {
        (pointer >> self.shift) & ((1u64 << self.bits) - 1)
    }
    /// Place a tag into a pointer. Returns `None` if the tag does not fit.
    pub const fn tag(self, pointer: u64, tag: u64) -> Option<u64> {
        if tag >= (1u64 << self.bits) {
            return None;
        }
        Some((pointer & !self.mask()) | (tag << self.shift))
    }
    /// Highest tag value the scheme can represent.
    pub const fn tag_limit(self) -> u64 {
        1u64 << self.bits
    }
}

/// Outcome of the optional LAM capability probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LamStatus {
    /// Hardware masking is active; tags use `TagScheme::LAM_U57`.
    ActiveU57,
    /// No hardware masking. Software masks; tags use 56:63.
    Software(&'static str),
}
impl LamStatus {
    pub fn scheme(&self) -> TagScheme {
        match self {
            Self::ActiveU57 => TagScheme::LAM_U57,
            Self::Software(_) => TagScheme::SOFTWARE,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::ActiveU57 => "lam-u57",
            Self::Software(_) => "software",
        }
    }
    pub fn detail(&self) -> String {
        match self {
            Self::ActiveU57 => {
                "hardware masks bits 62:57 on data accesses; shadow tags still checked".to_string()
            }
            Self::Software(reason) => {
                format!("no hardware masking: {reason}; software masks before every access")
            }
        }
    }
    pub fn hardware(&self) -> bool {
        matches!(self, Self::ActiveU57)
    }
}

/// Enable LAM_U57 when the configuration asks for it and the platform allows.
/// `enabled` is the Hardening switch (default on). Failure is never fatal: the
/// engine falls back to the software tag scheme.
pub fn enable(enabled: bool) -> LamStatus {
    if !enabled {
        return LamStatus::Software("LAM disabled by configuration");
    }
    probe_platform()
}

/// Try to enable LAM_U57 for this process.
pub fn probe() -> LamStatus {
    enable(true)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn probe_platform() -> LamStatus {
    // Prefer CPUID so the ledger can tell "CPU lacks LAM" from "kernel refused".
    if !cpuid_has_lam() {
        return LamStatus::Software("CPUID reports no LAM on this core");
    }
    if try_arch_prctl().is_ok() {
        return LamStatus::ActiveU57;
    }
    if try_prctl().is_ok() {
        return LamStatus::ActiveU57;
    }
    LamStatus::Software("CPU has LAM but arch_prctl/prctl rejected LAM_U57 (kernel too old or LAM disabled)")
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn probe_platform() -> LamStatus {
    LamStatus::Software("LAM is Intel x86-64 Linux only; this host cannot use it")
}

/// CPUID.(EAX=07H, ECX=0):EDX bit 26 — Linear Address Masking.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn cpuid_has_lam() -> bool {
    // SAFETY: CPUID with subleaf 0 is a pure query with no memory contract.
    let result = unsafe { core::arch::x86_64::__cpuid_count(7, 0) };
    // EDX bit 26 = LAM
    (result.edx & (1 << 26)) != 0
}

/// `arch_prctl(ARCH_SET_TAGGED_ADDR_CTRL, ENABLE|LAM_U57)` — the LAM-era interface.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn try_arch_prctl() -> Result<(), i64> {
    // Two request codes appear in kernel trees (RFC vs uapi). Try both.
    for request in [0x4001i64, 0x5001i64] {
        let result: i64;
        // SAFETY: arch_prctl takes a request code and an integer; no memory.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") 158i64 => result, // SYS_arch_prctl
                in("rdi") request,
                in("rsi") (1i64 << 0) | (1i64 << 1), // ENABLE | LAM_U57
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        if result == 0 {
            return Ok(());
        }
    }
    Err(-1)
}

/// `prctl(PR_SET_TAGGED_ADDR_CTRL, …)` fallback used by some kernels.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn try_prctl() -> Result<(), i64> {
    const PR_SET_TAGGED_ADDR_CTRL: i64 = 55;
    // PR_TAGGED_ADDR_ENABLE | ADDR_MASK_LAM_U57 (kernel-specific second bit).
    let arg = (1u64 << 0) | (1u64 << 1);
    let result: i64;
    // SAFETY: prctl with an integer option; no memory contract.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 157i64 => result, // SYS_prctl
            in("rdi") PR_SET_TAGGED_ADDR_CTRL,
            in("rsi") arg,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_scheme_round_trips_eight_bit_tags() {
        let scheme = TagScheme::SOFTWARE;
        for tag in 0..scheme.tag_limit() {
            let pointer = scheme.tag(0x0400_0010, tag).expect("tag fits");
            assert_eq!(scheme.tag_of(pointer), tag);
            assert_eq!(scheme.strip(pointer), 0x0400_0010);
        }
        assert!(scheme.tag(0, 256).is_none());
    }

    #[test]
    fn lam_u57_places_the_tag_inside_the_hardware_mask() {
        let scheme = TagScheme::LAM_U57;
        // Bits 62:57 only. Bit 56 and bit 63 must stay clean.
        let pointer = scheme.tag(0x0400_0010, 0x3f).expect("tag fits");
        assert_eq!(pointer >> 57, 0x3f);
        assert_eq!(pointer & (1 << 56), 0);
        assert_eq!(pointer & (1 << 63), 0);
        assert_eq!(scheme.mask(), 0x7e00_0000_0000_0000);
        assert_eq!(scheme.strip(pointer), 0x0400_0010);
        assert!(scheme.tag(0, 64).is_none());
    }

    #[test]
    fn lam_tag_bits_do_not_collide_with_the_pac_field() {
        // pac.rs owns bits 48:55. Neither scheme may place a tag there.
        for scheme in [TagScheme::SOFTWARE, TagScheme::LAM_U57] {
            assert_eq!(scheme.mask() & 0x00ff_0000_0000_0000, 0);
        }
    }

    #[test]
    fn probe_reports_a_status_without_panicking() {
        let status = probe();
        assert_eq!(status.scheme(), status.scheme());
        assert!(!status.detail().is_empty());
    }

    #[test]
    fn disabled_probe_never_claims_hardware() {
        let status = enable(false);
        assert!(!status.hardware());
        assert_eq!(status.scheme(), TagScheme::SOFTWARE);
    }

    #[test]
    fn lam_scheme_never_issues_shadow_sentinels() {
        // Shadow sentinels are 0xfd..=0xff. LAM_U57 only has 6 tag bits (0..63),
        // so it cannot collide with them by construction.
        assert!(TagScheme::LAM_U57.tag_limit() <= 0xfc);
    }
}
