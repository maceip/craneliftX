//! Intel Linear Address Masking: probe, select a tag placement, and report.
//!
//! What LAM actually buys, and where it does not
//! ---------------------------------------------
//! LAM makes the *hardware MMU* ignore a configurable set of upper address bits
//! on data accesses, so tagged pointers can be dereferenced by native
//! instructions without masking. That benefit only exists when the guest's own
//! machine code is executing on a real x86-64 core.
//!
//! In this engine it is not. Reached x86-64 is lifted to RV64 and executed by
//! the software interpreter in `riscv_exec`; the host core never translates a
//! guest virtual address, and there is no native fallback (`native_fallback:
//! false` in the run receipt). The interpreter already has to form its own host
//! address for every access, so masking a tag costs one `and` on a value it is
//! already computing. Enabling LAM here would change no code path and remove no
//! instruction.
//!
//! LAM becomes relevant the moment a native execution path exists -- the
//! "unlifted native x86-64 block" case. This module is the seam for that: it
//! probes the platform, selects a tag bit-field that the hardware will mask,
//! and records what happened in the ledger, so turning on native execution
//! later is a configuration change rather than a redesign.
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

// ActiveU57 is only constructed by the Linux x86-64 probe path.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LamStatus {
    /// Hardware masking is active; tags use `TagScheme::LAM_U57`.
    ActiveU57,
    /// No hardware masking. The interpreter masks; tags use 56:63.
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
            Self::ActiveU57 => "hardware masks bits 62:57 on data accesses".to_string(),
            Self::Software(reason) => {
                format!("no hardware masking: {reason}; interpreter masks on every access")
            }
        }
    }
}

/// Try to enable LAM_U57 for this process. Only meaningful on Linux x86-64 with
/// a LAM-capable core and kernel; everywhere else this reports Software with a
/// reason rather than pretending.
pub fn probe() -> LamStatus {
    probe_platform()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn probe_platform() -> LamStatus {
    const ARCH_SET_TAGGED_ADDR_CTRL: i64 = 0x4001;
    const ARCH_TAGGED_ADDR_ENABLE: i64 = 1 << 0;
    const ARCH_LAM_U57_BIT: i64 = 1 << 1;
    // SAFETY: arch_prctl with a request code and a plain integer argument has
    // no memory-safety contract; it either succeeds or returns a negative errno.
    let result: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 158i64 => result, // SYS_arch_prctl
            in("rdi") ARCH_SET_TAGGED_ADDR_CTRL,
            in("rsi") ARCH_TAGGED_ADDR_ENABLE | ARCH_LAM_U57_BIT,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    if result == 0 {
        LamStatus::ActiveU57
    } else {
        LamStatus::Software("arch_prctl(ARCH_SET_TAGGED_ADDR_CTRL) rejected LAM_U57")
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn probe_platform() -> LamStatus {
    LamStatus::Software("LAM is Intel x86-64 Linux only; this host cannot use it")
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
}
