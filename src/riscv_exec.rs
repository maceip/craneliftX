use crate::{context::GuestContext, error::EngineError};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiscvCpu {
    pub regs: [u64; 32],
    pub pc: u64,
    /// One bit per register: set when the register holds a value that came
    /// straight from an immediate, a rip-relative address or the code image,
    /// and was therefore not derived from guest data. Stores of code pointers
    /// are only signed when this is set.
    pub constant: u32,
    /// Require an indirect branch target to be an authenticated pointer.
    pub authenticate_calls: bool,
}

const STACK_BYTES: usize = 64 * 1024;
const STEP_LIMIT: usize = 100_000;
pub(crate) const RETURN_ADDRESS: u64 = u64::MAX - 1;

fn sext(value: u64, bits: u32) -> u64 {
    ((value << (64 - bits)) as i64 >> (64 - bits)) as u64
}

fn failure(pc: u64, reason: &'static str) -> EngineError {
    EngineError::Execute { pc, reason }
}

/// A flat code array at guest address zero, plus direct host data pointers.
/// Code-relative loads serve Cranelift literal pools; code stores are forbidden.
struct Memory<'a> {
    code: &'a [u8],
    stack_start: u64,
    stack_end: u64,
    context: std::ops::Range<u64>,
    bounded: bool,
}

pub(crate) trait MemoryAccess {
    fn code(&self) -> &[u8];
    fn stack_bounds(&self) -> (u64, u64);
    unsafe fn load(&mut self, address: u64, size: usize, pc: u64) -> Result<u64, EngineError>;
    unsafe fn store(
        &mut self,
        address: u64,
        size: usize,
        value: u64,
        pc: u64,
    ) -> Result<(), EngineError>;
    /// Re-authenticate a loaded word. Returns the value the register sees and
    /// whether that value was an authenticated code pointer. The trace-mode bus
    /// has no heap and returns the word unchanged and unauthenticated.
    fn authenticate_load(
        &mut self,
        _address: u64,
        _size: usize,
        raw: u64,
        _pc: u64,
    ) -> Result<(u64, bool), EngineError> {
        Ok((raw, false))
    }
    /// Sign a stored word when it is a code pointer. `constant` is the storing
    /// register's provenance; see `RiscvCpu::constant`.
    fn authenticate_store(
        &mut self,
        _address: u64,
        _size: usize,
        value: u64,
        _constant: bool,
        _pc: u64,
    ) -> Result<u64, EngineError> {
        Ok(value)
    }
}

impl Memory<'_> {
    fn check_host_range(&self, address: u64, end: u64, pc: u64) -> Result<(), EngineError> {
        if self.bounded
            && !(address >= self.stack_start && end <= self.stack_end)
            && !(address >= self.context.start && end <= self.context.end)
        {
            return Err(failure(pc, "unmapped guest memory access"));
        }
        Ok(())
    }
}

impl MemoryAccess for Memory<'_> {
    fn code(&self) -> &[u8] {
        self.code
    }
    fn stack_bounds(&self) -> (u64, u64) {
        (self.stack_start, self.stack_end)
    }
    unsafe fn load(&mut self, address: u64, size: usize, pc: u64) -> Result<u64, EngineError> {
        let end = address
            .checked_add(size as u64)
            .ok_or_else(|| failure(pc, "load address overflow"))?;
        let mut bytes = [0; 8];
        if address < self.code.len() as u64 {
            let source = self
                .code()
                .get(address as usize..end as usize)
                .ok_or_else(|| failure(pc, "code-relative load out of bounds"))?;
            bytes[..size].copy_from_slice(source);
        } else {
            self.check_host_range(address, end, pc)?;
            if address < 4096 {
                return Err(failure(pc, "null or low host data pointer"));
            }
            // SAFETY: execute_riscv's caller guarantees the generated accesses.
            unsafe {
                std::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), size);
            }
        }
        Ok(u64::from_le_bytes(bytes))
    }

    unsafe fn store(
        &mut self,
        address: u64,
        size: usize,
        value: u64,
        pc: u64,
    ) -> Result<(), EngineError> {
        let end = address
            .checked_add(size as u64)
            .ok_or_else(|| failure(pc, "store address overflow"))?;
        if address < self.code.len() as u64 || address < 4096 {
            return Err(failure(pc, "store to code or low host memory"));
        }
        self.check_host_range(address, end, pc)?;
        // SAFETY: execute_riscv's caller guarantees exclusive writable memory.
        unsafe {
            std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), address as *mut u8, size);
        }
        Ok(())
    }
}

impl RiscvCpu {
    pub(crate) unsafe fn run(
        &mut self,
        memory: &mut impl MemoryAccess,
        step_limit: usize,
    ) -> Result<u64, EngineError> {
        self.regs[0] = 0;
        for step in 0..step_limit {
            let pc = self.pc;
            if pc & 3 != 0 {
                return Err(failure(pc, "unaligned instruction pointer"));
            }
            let start =
                usize::try_from(pc).map_err(|_| failure(pc, "instruction pointer overflow"))?;
            let end = start
                .checked_add(4)
                .ok_or_else(|| failure(pc, "instruction pointer overflow"))?;
            let bytes = memory
                .code()
                .get(start..end)
                .ok_or_else(|| failure(pc, "instruction fetch out of bounds"))?;
            let word = u32::from_le_bytes(bytes.try_into().expect("four-byte instruction"));
            if word & 3 != 3 {
                return Err(failure(pc, "compressed instructions are disabled"));
            }
            if word == 0x0000_8067 && self.regs[1] == RETURN_ADDRESS {
                return Ok(step as u64 + 1);
            }
            let opcode = word & 0x7f;
            let rd = ((word >> 7) & 31) as usize;
            let funct3 = (word >> 12) & 7;
            let rs1 = ((word >> 15) & 31) as usize;
            let rs2 = ((word >> 20) & 31) as usize;
            let funct7 = word >> 25;
            let a = self.regs[rs1];
            let b = self.regs[rs2];
            let imm_i = sext((word >> 20) as u64, 12);
            let mut next_pc = pc.wrapping_add(4);
            let mut result = None;
            // Provenance of the value about to be written to rd.
            let mut from_constant = false;
            match opcode {
                0x37 => {
                    result = Some(sext((word & 0xffff_f000) as u64, 32)); // LUI
                    from_constant = true;
                }
                0x17 => {
                    result = Some(pc.wrapping_add(sext((word & 0xffff_f000) as u64, 32))); // AUIPC
                    from_constant = true;
                }
                0x6f => {
                    // JAL
                    let immediate = ((word >> 31) << 20)
                        | (((word >> 12) & 0xff) << 12)
                        | (((word >> 20) & 1) << 11)
                        | (((word >> 21) & 0x3ff) << 1);
                    result = Some(next_pc);
                    from_constant = true;
                    next_pc = pc.wrapping_add(sext(immediate as u64, 21));
                }
                0x67 if funct3 == 0 => {
                    // JALR: only an authenticated pointer may be branched to.
                    if self.authenticate_calls && self.constant & (1 << rs1) == 0 {
                        return Err(failure(
                            pc,
                            "indirect branch through an unauthenticated pointer",
                        ));
                    }
                    result = Some(next_pc);
                    next_pc = a.wrapping_add(imm_i) & !1;
                }
                0x63 => {
                    let take = match funct3 {
                        0 => a == b,
                        1 => a != b,
                        4 => (a as i64) < (b as i64),
                        5 => (a as i64) >= (b as i64),
                        6 => a < b,
                        7 => a >= b,
                        _ => return Err(failure(pc, "illegal branch encoding")),
                    };
                    if take {
                        let immediate = ((word >> 31) << 12)
                            | (((word >> 7) & 1) << 11)
                            | (((word >> 25) & 0x3f) << 5)
                            | (((word >> 8) & 0xf) << 1);
                        next_pc = pc.wrapping_add(sext(immediate as u64, 13));
                    }
                }
                0x03 => {
                    let (size, signed) = match funct3 {
                        0 => (1, true),
                        1 => (2, true),
                        2 => (4, true),
                        3 => (8, false),
                        4 => (1, false),
                        5 => (2, false),
                        6 => (4, false),
                        _ => return Err(failure(pc, "illegal load encoding")),
                    };
                    let address = a.wrapping_add(imm_i);
                    // A load out of the code image is a literal-pool read: the
                    // value is a constant of the program, not guest data.
                    from_constant = address < memory.code().len() as u64;
                    // SAFETY: forwarded caller contract; offsets are normal wrapping RV64 arithmetic.
                    let raw = unsafe { memory.load(address, size, pc)? };
                    let (value, authenticated) =
                        memory.authenticate_load(address, size, raw, pc)?;
                    // `authenticated` is true only for trusted static-image
                    // constants, stack spills, or CONTEXT_BASE register slots
                    // that already carried constant provenance — never for heap
                    // slots (`pointer_substitution`).
                    from_constant = from_constant || authenticated;
                    result = Some(if signed {
                        sext(value, (size * 8) as u32)
                    } else {
                        value
                    });
                }
                0x23 => {
                    let size = match funct3 {
                        0 => 1,
                        1 => 2,
                        2 => 4,
                        3 => 8,
                        _ => return Err(failure(pc, "illegal store encoding")),
                    };
                    let immediate = ((word >> 25) << 5) | ((word >> 7) & 31);
                    let address = a.wrapping_add(sext(immediate as u64, 12));
                    // Only a value that came from a constant may sign a pointer
                    // into an authenticated slot; anything else is data.
                    let value = memory.authenticate_store(
                        address,
                        size,
                        b,
                        self.constant & (1 << rs2) != 0,
                        pc,
                    )?;
                    // SAFETY: forwarded caller contract.
                    unsafe {
                        memory.store(address, size, value, pc)?;
                    }
                }
                0x13 => {
                    let shamt = (word >> 20) & 63;
                    // lui/addi pairs build one constant across two writes, and
                    // `addi rd,rs,0` is a move: provenance has to survive both,
                    // or a compiler-materialized address would look like data.
                    from_constant = self.constant & (1 << rs1) != 0
                        && (rs1 == rd || (funct3 == 0 && imm_i == 0));
                    result = Some(match funct3 {
                        0 => a.wrapping_add(imm_i),
                        2 => u64::from((a as i64) < (imm_i as i64)),
                        3 => u64::from(a < imm_i),
                        4 => a ^ imm_i,
                        6 => a | imm_i,
                        7 => a & imm_i,
                        1 if word >> 26 == 0 => a << shamt,
                        5 if word >> 26 == 0 => a >> shamt,
                        5 if word >> 26 == 0x10 => ((a as i64) >> shamt) as u64,
                        _ => return Err(failure(pc, "illegal immediate arithmetic encoding")),
                    });
                }
                0x1b => {
                    let shamt = (word >> 20) & 31;
                    let value = match (funct3, funct7) {
                        (0, _) => a.wrapping_add(imm_i) as u32,
                        (1, 0) => (a as u32) << shamt,
                        (5, 0) => (a as u32) >> shamt,
                        (5, 0x20) => ((a as i32) >> shamt) as u32,
                        _ => return Err(failure(pc, "illegal word immediate encoding")),
                    };
                    result = Some(sext(value as u64, 32));
                }
                0x33 => {
                    from_constant = match (funct3, funct7) {
                        (0, 0) | (6, 0) if rs2 == 0 => self.constant & (1 << rs1) != 0,
                        (0, 0) | (6, 0) if rs1 == 0 => self.constant & (1 << rs2) != 0,
                        _ => false,
                    };
                    result = Some(match (funct3, funct7) {
                        (0, 1) => a.wrapping_mul(b),
                        (1, 1) => (((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64,
                        (2, 1) => (((a as i64 as i128) * (b as i128)) >> 64) as u64,
                        (3, 1) => (((a as u128) * (b as u128)) >> 64) as u64,
                        (0, 0) => a.wrapping_add(b),
                        (0, 0x20) => a.wrapping_sub(b),
                        (1, 0) => a << (b & 63),
                        (2, 0) => u64::from((a as i64) < (b as i64)),
                        (3, 0) => u64::from(a < b),
                        (4, 0) => a ^ b,
                        (5, 0) => a >> (b & 63),
                        (5, 0x20) => ((a as i64) >> (b & 63)) as u64,
                        (6, 0) => a | b,
                        (7, 0) => a & b,
                        _ => return Err(failure(pc, "unsupported register arithmetic encoding")),
                    });
                }
                0x3b => {
                    let value = match (funct3, funct7) {
                        (0, 1) => (a as u32).wrapping_mul(b as u32),
                        (0, 0) => (a as u32).wrapping_add(b as u32),
                        (0, 0x20) => (a as u32).wrapping_sub(b as u32),
                        (1, 0) => (a as u32) << (b & 31),
                        (5, 0) => (a as u32) >> (b & 31),
                        (5, 0x20) => ((a as i32) >> (b & 31)) as u32,
                        _ => return Err(failure(pc, "unsupported word register encoding")),
                    };
                    result = Some(sext(value as u64, 32));
                }
                // The supported x86 subset is non-atomic. FENCE is sufficient as
                // a sequential no-op; CSR, ECALL/EBREAK, F/D/A and unsupported M forms fail closed.
                0x0f if funct3 == 0 => {}
                _ => return Err(failure(pc, "unsupported RISC-V instruction")),
            }
            if let Some(value) = result {
                if rd != 0 {
                    self.regs[rd] = value;
                    // Provenance follows the value, not the register: writing a
                    // register with data clears its constant bit.
                    self.constant = (self.constant & !(1 << rd)) | (u32::from(from_constant) << rd);
                }
            }
            self.regs[0] = 0;
            if self.regs[2] < memory.stack_bounds().0 || self.regs[2] > memory.stack_bounds().1 {
                return Err(failure(pc, "interpreter stack overflow"));
            }
            self.pc = next_pc;
        }
        Err(failure(self.pc, "instruction budget exhausted"))
    }
}

/// Interpret the generated RV64I bytes, with a0 = host ctx pointer and pc = 0.
/// A private, aligned stack accommodates Cranelift's ABI frame and spills.
///
/// # Safety
/// Code must come from this engine's audited/lowered trace. Every resulting
/// host data load/store must reference live initialized memory of sufficient
/// size; writes require exclusive access. Guest pointers must not alias code,
/// the interpreter's internals, or ctx. The filter is not a memory sandbox.
/// Errors may leave earlier host stores applied; execution is not transactional.
pub unsafe fn execute_riscv(code: &[u8], ctx: &mut GuestContext) -> Result<(), EngineError> {
    // SAFETY: forwarded caller contract.
    unsafe { RiscvExecutor::default().execute(code, ctx).map(|_| ()) }
}

/// Reuses one private interpreter stack. Use one executor per concurrent caller.
pub struct RiscvExecutor {
    stack: Vec<u8>,
}

impl Default for RiscvExecutor {
    fn default() -> Self {
        Self {
            stack: vec![0; STACK_BYTES + 16],
        }
    }
}

impl RiscvExecutor {
    /// Interpret with bounds checks on every memory access. Only the generated
    /// code (read-only), private interpreter stack, and context are mapped.
    /// File virtual addresses and arbitrary register values cannot escape into
    /// host memory. Errors may leave earlier context/stack stores applied.
    pub fn execute_bounded(
        &mut self,
        code: &[u8],
        ctx: &mut GuestContext,
    ) -> Result<u64, EngineError> {
        // SAFETY: all host accesses are checked against these live owned regions.
        unsafe { self.execute_inner(code, ctx, true) }
    }
    /// Executes a prepared trace, returning actual fetched RV64 instruction count.
    ///
    /// # Safety
    /// The code and host pointers must satisfy `execute_riscv`'s contract.
    pub unsafe fn execute(
        &mut self,
        code: &[u8],
        ctx: &mut GuestContext,
    ) -> Result<u64, EngineError> {
        // SAFETY: forwarded caller contract.
        unsafe { self.execute_inner(code, ctx, false) }
    }

    unsafe fn execute_inner(
        &mut self,
        code: &[u8],
        ctx: &mut GuestContext,
        bounded: bool,
    ) -> Result<u64, EngineError> {
        let stack_start = self.stack.as_mut_ptr() as u64;
        let stack_end = (stack_start + self.stack.len() as u64) & !15;
        let mut cpu = RiscvCpu::default();
        cpu.regs[1] = RETURN_ADDRESS;
        cpu.regs[2] = stack_end;
        cpu.regs[10] = ctx as *mut GuestContext as u64;
        let mut memory = Memory {
            code,
            stack_start,
            stack_end,
            // Exclude the repr(align(16)) tail padding: it is not initialized data.
            context: cpu.regs[10]..cpu.regs[10] + crate::context::RIP_OFFSET as u64 + 8,
            bounded,
        };
        // SAFETY: caller covers external data; context and private stack are live.
        unsafe { cpu.run(&mut memory, STEP_LIMIT) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_memory_rejects_host_pointers_and_uninitialized_context_padding() {
        let mut outside = 123u64;
        let mut ctx = GuestContext {
            rdi: &mut outside as *mut u64 as u64,
            ..Default::default()
        };
        // ld t0,40(a0) loads ctx.rdi. Subsequent loads/stores cannot use that
        // live but unmapped host pointer. Padding also must not be readable.
        for words in [
            vec![0x0285_3283u32, 0x0002_b303, 0x0000_8067],
            vec![0x0285_3283u32, 0x0002_b023, 0x0000_8067],
            vec![0x0885_3283u32, 0x0000_8067],
        ] {
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let error = RiscvExecutor::default()
                .execute_bounded(&code, &mut ctx)
                .unwrap_err();
            assert!(error.to_string().contains("unmapped guest memory"));
            assert_eq!(outside, 123);
        }
    }

    fn run_words(words: &[u32], registers: &[(usize, u64)]) -> Result<RiscvCpu, EngineError> {
        let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        let mut memory = Memory {
            code: &code,
            stack_start: 0,
            stack_end: u64::MAX,
            context: 0..0,
            bounded: false,
        };
        let mut cpu = RiscvCpu::default();
        cpu.regs[1] = RETURN_ADDRESS;
        for &(reg, value) in registers {
            cpu.regs[reg] = value;
        }
        // SAFETY: these fixtures contain no host-memory operations.
        unsafe {
            cpu.run(&mut memory, 32)?;
        }
        Ok(cpu)
    }

    #[test]
    fn x0_and_word_sign_extension() {
        let cpu = run_words(&[0x0010_0013, 0xfff0_051b, 0x0015_059b, 0x0000_8067], &[]).unwrap();
        assert_eq!(cpu.regs[0], 0);
        assert_eq!(cpu.regs[10], u64::MAX);
        assert_eq!(cpu.regs[11], 0);
    }

    #[test]
    fn backward_branch_and_jalr_read_before_write() {
        // addi a0,3; addi a0,-1; bne a0,x0,-4; jalr t0,0(t0); skipped ebreak; ret
        let cpu = run_words(
            &[
                0x0030_0513,
                0xfff5_0513,
                0xfe05_1ee3,
                0x0002_82e7,
                0x0010_0073,
                0x0000_8067,
            ],
            &[(5, 20)],
        )
        .unwrap();
        assert_eq!(cpu.regs[10], 0);
        assert_eq!(cpu.regs[5], 16);
    }

    #[test]
    fn rejects_unknown_compressed_truncated_and_looping_code() {
        for words in [
            &[0x0000_0073][..],
            &[0][..],
            &[0x0000_006f][..],
            &[0xffff_ffff][..],
        ] {
            assert!(run_words(words, &[]).is_err());
        }
        assert!(run_words(&[], &[]).is_err());
    }
}
