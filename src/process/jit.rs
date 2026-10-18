//! JIT memory management: allocate W^X pages for compiled native code.
//!
//! Compiled blocks are copied into freshly allocated pages, then the pages are
//! transitioned from RW to RX via mprotect. This prevents an attacker from
//! overwriting JIT code with shellcode after compilation.
use std::alloc::Layout;

/// A region of executable memory holding one compiled block.
pub struct JitPage {
    ptr: *mut u8,
    layout: Layout,
    size: usize,
}

impl JitPage {
    /// Allocate a new page with at least `code_size` bytes of capacity.
    /// The page is initially read-write.
    pub fn allocate(code_size: usize) -> Result<Self, String> {
        let page_size = page_size();
        let aligned_size = ((code_size + page_size - 1) / page_size) * page_size;
        if aligned_size == 0 {
            return Err("empty code".into());
        }

        #[cfg(unix)]
        {
            // SAFETY: mmap with valid size and alignment.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    aligned_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(format!(
                    "mmap failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let layout = Layout::from_size_align(aligned_size, page_size)
                .map_err(|e| format!("{e}"))?;
            Ok(Self {
                ptr: ptr as *mut u8,
                layout,
                size: code_size,
            })
        }
        #[cfg(windows)]
        {
            // SAFETY: VirtualAlloc with valid size.
            let ptr = unsafe {
                windows_sys::Win32::System::Memory::VirtualAlloc(
                    std::ptr::null_mut(),
                    aligned_size,
                    windows_sys::Win32::System::Memory::MEM_COMMIT | windows_sys::Win32::System::Memory::MEM_RESERVE,
                    windows_sys::Win32::System::Memory::PAGE_READWRITE,
                )
            };
            if ptr.is_null() {
                return Err("VirtualAlloc failed".into());
            }
            let layout = Layout::from_size_align(aligned_size, page_size)
                .map_err(|e| format!("{e}"))?;
            Ok(Self {
                ptr: ptr as *mut u8,
                layout,
                size: code_size,
            })
        }
    }

    /// Copy compiled code into the page.
    ///
    /// # Safety
    /// `code` must not be larger than the allocated capacity.
    pub unsafe fn copy_code(&mut self, code: &[u8]) {
        assert!(
            code.len() <= self.layout.size(),
            "code {} bytes > capacity {} bytes",
            code.len(),
            self.layout.size()
        );
        // SAFETY: ptr is valid for layout.size() bytes, code fits.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), self.ptr, code.len());
        }
        self.size = code.len();
    }

    /// Transition the page from read-write to read-execute.
    pub fn make_executable(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            // SAFETY: ptr is a valid allocation of at least layout.size() bytes.
            let ret = unsafe {
                libc::mprotect(
                    self.ptr as *mut libc::c_void,
                    self.layout.size(),
                    libc::PROT_READ | libc::PROT_EXEC,
                )
            };
            if ret != 0 {
                return Err(format!(
                    "mprotect(PROT_READ | PROT_EXEC) failed: errno={}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        #[cfg(windows)]
        {
            let mut old = 0;
            // SAFETY: ptr is a valid allocation.
            let ret = unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    self.ptr as *mut std::ffi::c_void,
                    self.layout.size(),
                    windows_sys::Win32::System::Memory::PAGE_EXECUTE_READ,
                    &mut old,
                )
            };
            if ret == 0 {
                return Err("VirtualProtect(PAGE_EXECUTE_READ) failed".into());
            }
        }
        Ok(())
    }

    /// Raw pointer to the executable memory.
    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Size of the compiled code (not the allocation).
    pub fn code_size(&self) -> usize {
        self.size
    }
}

impl Drop for JitPage {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.layout.size());
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                self.ptr as *mut std::ffi::c_void,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            );
        }
    }
}

fn page_size() -> usize {
    // Cache the page size since it never changes.
    use std::sync::OnceLock;
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| {
        #[cfg(unix)]
        {
            unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
        }
        #[cfg(windows)]
        {
            let mut info = std::mem::MaybeUninit::uninit();
            // SAFETY: SystemInfo is a plain struct.
            unsafe {
                windows_sys::Win32::System::SystemInformation::GetSystemInfo(info.as_mut_ptr());
                info.assume_init_ref().dwPageSize as usize
            }
        }
    })
}

/// Type of a compiled native block function.
///
/// Arguments:
///   ctx: pointer to a NativeGuest (register file + checked memory helpers)
///   cfi_bitmap: pointer to EntryBitmap (may be null if CFI disabled)
///
/// Returns:
///   number of steps executed, or a sentinel on fault.
pub type NativeBlockFn = unsafe extern "C" fn(
    *mut crate::process::native::NativeGuest,
    *const crate::process::cfi::EntryBitmap,
) -> u64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_and_protect() {
        let mut page = JitPage::allocate(64).unwrap();
        let code = &[0x90u8; 64]; // NOP sled
        // SAFETY: code fits in allocated page.
        unsafe { page.copy_code(code) };
        page.make_executable().unwrap();
        assert_eq!(page.code_size(), 64);
    }
}
