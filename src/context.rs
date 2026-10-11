/// The x86 integer state. RFLAGS and vector/segment state are intentionally absent.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GuestContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
}

pub const REGISTER_COUNT: usize = 16;
pub const RIP_OFFSET: i32 = std::mem::offset_of!(GuestContext, rip) as i32;
