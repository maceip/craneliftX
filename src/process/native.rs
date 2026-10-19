//! Native x86-64 block ABI and checked memory helpers.
//!
//! Native blocks do not touch guest memory with raw loads and stores. Every
//! access goes through the same `ProcessBus` path the interpreter uses, so
//! heap shadow tags and pointer authentication apply on the native backend
//! exactly as they do on RV64. Syscalls are not emitted as host `syscall`
//! instructions: a guest `syscall` ends the block and the runtime serves it
//! from the register file, so the guest never reaches the host kernel.
//!
//! Layout of [`NativeGuest`] (the `ctx` pointer passed to a compiled block):
//!
//! ```text
//!  0 .. 144   guest registers rax..r15, rip, rflags  (18 × u64)
//! 144         cookie: *mut Runtime
//! 152         load_fn
//! 160         store_fn
//! 168         fault_pc
//! 176         fault_kind   (0 = ok)
//! ```
//!
//! Offsets 0..144 stay register-file compatible with the previous ABI so the
//! lowerer's `ctx[i*8]` addressing is unchanged.

use super::runtime::Runtime;
use std::ffi::c_void;

pub const NATIVE_FS_BASE_OFFSET: i32 = 144;
pub const NATIVE_COOKIE_OFFSET: i32 = 152;
pub const NATIVE_LOAD_FN_OFFSET: i32 = 160;
pub const NATIVE_STORE_FN_OFFSET: i32 = 168;
pub const NATIVE_FAULT_PC_OFFSET: i32 = 176;
pub const NATIVE_FAULT_KIND_OFFSET: i32 = 184;

/// Fault kinds reported through [`NativeGuest::fault_kind`].
pub const FAULT_NONE: u64 = 0;
pub const FAULT_MEMORY: u64 = 3;
pub const FAULT_STACK: u64 = 2;

pub type NativeLoadFn = unsafe extern "C" fn(*mut c_void, u64, u64, u64) -> u64;
pub type NativeStoreFn = unsafe extern "C" fn(*mut c_void, u64, u64, u64, u64, u64);

/// Context block passed to every native-compiled guest block.
#[repr(C)]
pub struct NativeGuest {
    pub regs: [u64; 19],
    pub cookie: *mut c_void,
    pub load_fn: NativeLoadFn,
    pub store_fn: NativeStoreFn,
    pub fault_pc: u64,
    pub fault_kind: u64,
}

impl NativeGuest {
    pub fn new(runtime: *mut Runtime) -> Self {
        Self {
            regs: [0; 19],
            cookie: runtime as *mut c_void,
            load_fn: native_load,
            store_fn: native_store,
            fault_pc: 0,
            fault_kind: FAULT_NONE,
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
    // SAFETY: cookie is a *mut Runtime installed by execute_x86_64 for the
    // duration of the native block call and is not otherwise aliased.
    let rt = unsafe { &mut *(cookie as *mut Runtime) };
    rt.native_load(guest_ptr, size, pc)
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
    // SAFETY: see native_load.
    let rt = unsafe { &mut *(cookie as *mut Runtime) };
    rt.native_store(guest_ptr, size, value, constant_provenance != 0, pc)
}
