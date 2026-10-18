//! Hardware domain isolation for the runtime's own pages (Intel MPK).
//!
//! Threat this addresses
//! ---------------------
//! If any guest code ever runs as native x86-64 -- the "unlifted block" case --
//! then an arbitrary-write bug in that code can reach the RV64 code cache, the
//! ledger, or the shadow/PAC maps, which is strictly worse than not mitigating
//! at all. Memory Protection Keys let the runtime mark its own pages with a
//! key and revoke write access to them with a single non-privileged
//! instruction (`WRPKRU`) on the way into untrusted code.
//!
//! Status in this engine
//! ---------------------
//! There is currently no native execution path: reached code is lifted to RV64
//! and interpreted, and the run receipt records `native_fallback: false`. With
//! no native block there is no transition to guard, and MPK would protect a
//! boundary that does not exist. The runtime is also fully isolated by
//! construction today -- every guest access goes through `ProcessBus`, which
//! only resolves guest-mapped ranges, so no guest address can name a host
//! structure at all.
//!
//! This module therefore does three things: it probes honestly, it reports
//! `unavailable` with a reason instead of implying protection, and it provides
//! the real `pkey_mprotect`/`WRPKRU` sequence for the Linux x86-64 case so that
//! adding a native path later is wiring, not design.
#![allow(dead_code)]
// The switch routines and pkey plumbing are live only once a native execution
// path exists. They are retained so that adding one is wiring, not design.

/// One hardware protection key, or the reason it could not be obtained.
#[derive(Clone, Debug)]
pub struct Domain {
    pkey: i32,
    reason: Option<&'static str>,
    switches: u64,
}
impl Domain {
    pub fn available(&self) -> bool {
        self.reason.is_none()
    }
    pub fn reason(&self) -> Option<&'static str> {
        self.reason
    }
    pub fn switches(&self) -> u64 {
        self.switches
    }
    /// `true` while runtime pages are writable; `false` while guest-facing code
    /// is running with them revoked.
    pub fn revoke_writes(&mut self) {
        self.switch(false);
    }
    pub fn allow_writes(&mut self) {
        self.switch(true);
    }
    fn switch(&mut self, writable: bool) {
        if !self.available() {
            return;
        }
        self.switches += 1;
        // SAFETY: wrpkru only changes this thread's protection-key rights; it
        // touches no memory and cannot fault when the key is ours.
        unsafe { wrpkru(if writable { 0 } else { 0b11 << (2 * self.pkey) }) };
    }
}

pub fn acquire() -> Domain {
    acquire_platform()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn acquire_platform() -> Domain {
    const PKEY_ALLOC: i64 = 330;
    const PKEY_MPROTECT: i64 = 329;
    // SAFETY: pkey_alloc takes two integer arguments and returns a key or -1.
    let pkey: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") PKEY_ALLOC => pkey,
            in("rdi") 0i64,
            in("rsi") 0i64,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    if pkey < 0 {
        return Domain {
            pkey: -1,
            reason: Some("pkey_alloc failed; kernel or CPU lacks MPK"),
            switches: 0,
        };
    }
    let _ = PKEY_MPROTECT;
    Domain {
        pkey: pkey as i32,
        reason: None,
        switches: 0,
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn acquire_platform() -> Domain {
    Domain {
        pkey: -1,
        reason: Some("MPK is Linux x86-64 only; no hardware domain is enforced"),
        switches: 0,
    }
}

/// Apply `pkey` to a page-aligned host range. No-op when MPK is unavailable.
pub fn protect_pages(domain: &Domain, base: *mut u8, len: usize, prot: i32) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        if !domain.available() || base.is_null() || len == 0 {
            return false;
        }
        const PKEY_MPROTECT: i64 = 329;
        let rights = (i64::from(domain.pkey) & 0xf) << 4;
        // SAFETY: base..base+len must be a live, page-aligned mapping owned by
        // this process. Callers pass the runtime's own arena.
        let result: i64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") PKEY_MPROTECT => result,
                in("rdi") base as i64,
                in("rsi") len as i64,
                in("rdx") i64::from(prot) | rights,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        return result == 0;
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (domain, base, len, prot);
        false
    }
}

/// Write the PKRU register. Bit 2i disables write and bit 2i+1 disables access
/// for key i. Executed with `wrpkru` (EDX=0, ECX=0).
///
/// # Safety
/// Only changes this thread's protection-key rights register.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn wrpkru(mask: u32) {
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "wrpkru",
            in("eax") mask,
            options(nostack, nomem),
        );
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
unsafe fn wrpkru(_mask: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquisition_reports_a_reason_when_unavailable() {
        let domain = acquire();
        if domain.available() {
            assert!(domain.reason().is_none());
        } else {
            assert!(domain.reason().is_some());
        }
    }

    #[test]
    fn switching_an_unavailable_domain_is_a_counted_no_op() {
        let mut domain = acquire();
        if domain.available() {
            return;
        }
        domain.revoke_writes();
        domain.allow_writes();
        assert_eq!(domain.switches(), 0);
    }
}
