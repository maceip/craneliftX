//! Native x86-64 block ABI and checked memory helpers.
//!
//! Native blocks do not touch guest memory with raw loads and stores. Every
//! access goes through the same `ProcessBus` path the interpreter uses, so
//! heap shadow tags and pointer authentication apply on the native backend
//! exactly as they do on RV64. Syscalls are not emitted as host `syscall`
//! instructions: a guest `syscall` ends the block and the runtime serves it
//! from the register file, so the guest never reaches the host kernel.
//!
//! The context begins with the complete supported CPU register state. Helper
//! offsets are derived from the Rust ABI layout so extending the register file
//! cannot silently overwrite a callback or reset SIMD state at block boundaries.

use super::runtime::Runtime;
use std::ffi::c_void;

pub const NATIVE_FS_BASE_OFFSET: i32 = 144;
pub const NATIVE_PROVENANCE_OFFSET: i32 = std::mem::offset_of!(NativeGuest, provenance) as i32;
pub const NATIVE_LAST_LOAD_PROV_OFFSET: i32 =
    std::mem::offset_of!(NativeGuest, last_load_prov) as i32;
pub const NATIVE_STACK_BOTTOM_OFFSET: i32 = std::mem::offset_of!(NativeGuest, stack_bottom) as i32;
pub const NATIVE_STACK_TOP_OFFSET: i32 = std::mem::offset_of!(NativeGuest, stack_top) as i32;
pub const NATIVE_COOKIE_OFFSET: i32 = std::mem::offset_of!(NativeGuest, cookie) as i32;
pub const NATIVE_LOAD_FN_OFFSET: i32 = std::mem::offset_of!(NativeGuest, load_fn) as i32;
pub const NATIVE_STORE_FN_OFFSET: i32 = std::mem::offset_of!(NativeGuest, store_fn) as i32;
pub const NATIVE_CPUID_FN_OFFSET: i32 = std::mem::offset_of!(NativeGuest, cpuid_fn) as i32;
pub const NATIVE_CPUID_OUT_OFFSET: i32 = std::mem::offset_of!(NativeGuest, cpuid_out) as i32;
pub const NATIVE_FAULT_PC_OFFSET: i32 = std::mem::offset_of!(NativeGuest, fault_pc) as i32;
pub const NATIVE_FAULT_KIND_OFFSET: i32 = std::mem::offset_of!(NativeGuest, fault_kind) as i32;

/// Fault kinds reported through [`NativeGuest::fault_kind`].
pub const FAULT_NONE: u64 = 0;
pub const FAULT_MEMORY: u64 = 3;
pub const FAULT_STACK: u64 = 2;

pub type NativeLoadFn = unsafe extern "C" fn(*mut c_void, u64, u64, u64) -> u64;
pub type NativeStoreFn = unsafe extern "C" fn(*mut c_void, u64, u64, u64, u64, u64);
pub type NativeCpuidFn = unsafe extern "C" fn(*mut c_void, u64, u64);

/// Context block passed to every native-compiled guest block.
#[repr(C)]
pub struct NativeGuest {
    pub regs: [u64; super::CONTEXT_WORDS],
    pub provenance: u64,
    pub last_load_prov: u64,
    pub stack_bottom: u64,
    pub stack_top: u64,
    pub cookie: *mut c_void,
    pub load_fn: NativeLoadFn,
    pub store_fn: NativeStoreFn,
    pub cpuid_fn: NativeCpuidFn,
    pub cpuid_out: [u64; 4],
    pub fault_pc: u64,
    pub fault_kind: u64,
    runtime: *mut Runtime,
}

impl NativeGuest {
    pub fn new(runtime: *mut Runtime) -> Self {
        Self {
            regs: [0; super::CONTEXT_WORDS],
            provenance: 0,
            last_load_prov: 0,
            stack_bottom: super::memory::STACK_BOTTOM,
            stack_top: super::memory::STACK_TOP,
            cookie: std::ptr::null_mut(),
            load_fn: native_load,
            store_fn: native_store,
            cpuid_fn: native_cpuid,
            cpuid_out: [0; 4],
            fault_pc: 0,
            fault_kind: FAULT_NONE,
            runtime,
        }
    }

    pub fn take_fault(&mut self) -> Option<(u64, u64)> {
        if self.fault_kind == FAULT_NONE {
            None
        } else {
            let fault = (self.fault_kind, self.fault_pc);
            self.fault_kind = FAULT_NONE;
            self.fault_pc = 0;
            Some(fault)
        }
    }
}

/// Checked guest load. `guest_ptr` may carry a heap tag in the pointer metadata
/// bits. On a mitigation fault the fault slot is filled and 0 is returned; the
/// lowered code checks the slot and leaves the block.
pub unsafe extern "C" fn native_load(
    cookie: *mut c_void,
    guest_ptr: u64,
    size: u64,
    pc: u64,
) -> u64 {
    // SAFETY: execute_x86_64 installs the address of its live NativeGuest
    // after constructing it. That guest and its Runtime outlive this call.
    let guest = unsafe { &mut *(cookie as *mut NativeGuest) };
    let rt = unsafe { &mut *guest.runtime };
    let (value, prov) = rt.native_load(guest_ptr, size, pc);
    guest.last_load_prov = u64::from(prov);
    if rt.native_fault.is_some() {
        guest.fault_kind = FAULT_MEMORY;
        guest.fault_pc = pc;
    }
    value
}

/// Checked guest store. `constant_provenance` is 1 when the value came from an
/// immediate or lea and may be PAC-signed into a pointer slot.
pub unsafe extern "C" fn native_store(
    cookie: *mut c_void,
    guest_ptr: u64,
    size: u64,
    value: u64,
    constant_provenance: u64,
    pc: u64,
) {
    // SAFETY: same callback lifetime and ownership as native_load.
    let guest = unsafe { &mut *(cookie as *mut NativeGuest) };
    let rt = unsafe { &mut *guest.runtime };
    rt.native_store(guest_ptr, size, value, constant_provenance != 0, pc);
    if rt.native_fault.is_some() {
        guest.fault_kind = FAULT_MEMORY;
        guest.fault_pc = pc;
    }
}

/// CPUID emulation modeled on Justine Tunney's Blink (vendor/blink/blink/cpuid.c).
pub unsafe extern "C" fn native_cpuid(ctx: *mut c_void, leaf: u64, subleaf: u64) {
    let guest = unsafe { &mut *(ctx as *mut NativeGuest) };
    let intel = b"GenuineIntel";
    let intel_ebx = u32::from_le_bytes(intel[0..4].try_into().unwrap());
    let intel_edx = u32::from_le_bytes(intel[4..8].try_into().unwrap());
    let intel_ecx = u32::from_le_bytes(intel[8..12].try_into().unwrap());
    let (eax, ebx, ecx, edx) = match leaf as u32 {
        0 => (7, intel_ebx, intel_ecx, intel_edx),
        1 => {
            let cx =
                (1 << 0) | (1 << 1) | (1 << 9) | (1 << 13) | (1 << 23) | (1 << 30) | (1u32 << 31);
            let dx = (1 << 0)
                | (1 << 4)
                | (1 << 6)
                | (1 << 8)
                | (1 << 15)
                | (1 << 19)
                | (1 << 23)
                | (1 << 24)
                | (1 << 25)
                | (1 << 26);
            (0x000806ea, 0x01080800, cx, dx)
        }
        2 => (0x76035a01, 0x00f0b0ff, 0x00000000, 0x00ca0000),
        7 => {
            if subleaf as u32 == 0 {
                let bx = (1 << 0) | (1 << 9) | (1 << 18);
                let cx = 1 << 22;
                (0, bx, cx, 0)
            } else {
                (0, 0, 0, 0)
            }
        }
        0x80000000 => (0x80000001u32, intel_ebx, intel_ecx, intel_edx),
        0x80000001 => {
            let cx = 1 << 0;
            let dx = (1 << 0)
                | (1 << 8)
                | (1 << 11)
                | (1 << 15)
                | (1 << 20)
                | (1 << 23)
                | (1 << 24)
                | (1 << 27)
                | (1 << 29);
            (0, 0, cx, dx)
        }
        _ => (0, 0, 0, 0),
    };
    guest.cpuid_out[0] = eax as u64;
    guest.cpuid_out[1] = ebx as u64;
    guest.cpuid_out[2] = ecx as u64;
    guest.cpuid_out[3] = edx as u64;
}
