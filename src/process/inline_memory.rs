//! Inline only cases that are semantically identical to ProcessBus. Failed or
//! ambiguous checks take the reference path, which supplies the precise fault,
//! PAC history/provenance updates and counters. No metadata is guest-addressable.
use super::*;
use crate::process::{
    fast_memory::{FastPage, CACHE_ENTRIES, PAGE_SHIFT},
    heap::{GRANULE, HEAP_BASE, HEAP_SIZE},
    native,
    pac::PAC_MASK,
};

impl Lower<'_> {
    fn native_field(&mut self, offset: i32) -> Value {
        self.b
            .ins()
            .load(types::I64, MemFlags::new(), self.ctx, offset)
    }
    fn slow_if(&mut self, condition: Value, slow: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(condition, slow, &[], next, &[]);
        self.b.switch_to_block(next);
        self.b.seal_block(next);
    }
    fn slow_if_stack(&mut self, plain: Value, slow: Block) {
        let bottom = self.native_field(native::NATIVE_STACK_BOTTOM_OFFSET);
        let top = self.native_field(native::NATIVE_STACK_TOP_OFFSET);
        let ge = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, plain, bottom);
        let lt = self.b.ins().icmp(IntCC::UnsignedLessThan, plain, top);
        let dynamic = self.b.ins().band(ge, lt);
        let offset = self.b.ins().iadd_imm(plain, -(STACK_BOTTOM as i64));
        let fixed = self.b.ins().icmp_imm(
            IntCC::UnsignedLessThan,
            offset,
            (STACK_TOP - STACK_BOTTOM) as i64,
        );
        let stack = self.b.ins().bor(dynamic, fixed);
        self.slow_if(stack, slow);
    }
    fn pac_page(&mut self, plain: Value) -> Value {
        let pages = self.native_field(native::PAC_PAGES);
        let index = self.b.ins().ushr_imm(plain, PAGE_SHIFT as i64);
        let at = self.b.ins().iadd(pages, index);
        self.b.ins().load(types::I8, MemFlags::new(), at, 0)
    }
    /// Return a pinned host address only after mapping, permission, complete
    /// span and all touched tag granules have been checked.
    fn fast_address(&mut self, p: Value, w: u16, write: bool, slow: Block) -> (Value, Value) {
        let enabled = self.native_field(native::FAST_ENABLED);
        let disabled = self.b.ins().icmp_imm(IntCC::Equal, enabled, 0);
        self.slow_if(disabled, slow);
        let mask = self.native_field(native::TAG_MASK);
        let inv = self.b.ins().bnot(mask);
        let plain = self.b.ins().band(p, inv);
        let invalid = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThan,
            plain,
            (ADDRESS_LIMIT - u64::from(w / 8)) as i64,
        );
        self.slow_if(invalid, slow);
        let end = self.b.ins().iadd_imm(plain, i64::from(w / 8));
        let page = self.b.ins().ushr_imm(plain, PAGE_SHIFT as i64);
        let index = self.b.ins().band_imm(page, (CACHE_ENTRIES - 1) as i64);
        let offset = self
            .b
            .ins()
            .imul_imm(index, std::mem::size_of::<FastPage>() as i64);
        let table = self.native_field(native::FAST_PAGES);
        let entry = self.b.ins().iadd(table, offset);
        let cached = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            entry,
            std::mem::offset_of!(FastPage, page) as i32,
        );
        let miss = self.b.ins().icmp(IntCC::NotEqual, page, cached);
        self.slow_if(miss, slow);
        let start = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            entry,
            std::mem::offset_of!(FastPage, start) as i32,
        );
        let limit = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            entry,
            std::mem::offset_of!(FastPage, end) as i32,
        );
        let below = self.b.ins().icmp(IntCC::UnsignedLessThan, plain, start);
        let above = self.b.ins().icmp(IntCC::UnsignedGreaterThan, end, limit);
        let crosses = self.b.ins().bor(below, above);
        self.slow_if(crosses, slow);
        if write {
            let writable = self.b.ins().load(
                types::I64,
                MemFlags::new(),
                entry,
                std::mem::offset_of!(FastPage, writable) as i32,
            );
            let forbidden = self.b.ins().icmp_imm(IntCC::Equal, writable, 0);
            self.slow_if(forbidden, slow);
        }
        let tagged = self.b.create_block();
        let checked = self.b.create_block();
        let tags = self.native_field(native::TAGS_ENABLED);
        self.b.ins().brif(tags, tagged, &[], checked, &[]);
        self.b.switch_to_block(tagged);
        self.b.seal_block(tagged);
        let shift = self.native_field(native::TAG_SHIFT);
        let tag_bits = self.b.ins().band(p, mask);
        let tag = self.b.ins().ushr(tag_bits, shift);
        let offset = self.b.ins().iadd_imm(plain, -(HEAP_BASE as i64));
        let in_heap = self
            .b
            .ins()
            .icmp_imm(IntCC::UnsignedLessThan, offset, HEAP_SIZE as i64);
        let heap = self.b.create_block();
        let outside = self.b.create_block();
        self.b.ins().brif(in_heap, heap, &[], outside, &[]);
        self.b.switch_to_block(outside);
        self.b.seal_block(outside);
        let bad_tag = self.b.ins().icmp_imm(IntCC::NotEqual, tag, 0);
        self.slow_if(bad_tag, slow);
        self.b.ins().jump(checked, &[]);
        self.b.switch_to_block(heap);
        self.b.seal_block(heap);
        let beyond_heap = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThan,
            end,
            (HEAP_BASE + HEAP_SIZE) as i64,
        );
        self.slow_if(beyond_heap, slow);
        let first = self
            .b
            .ins()
            .ushr_imm(offset, GRANULE.trailing_zeros() as i64);
        let last_byte = self.b.ins().iadd_imm(offset, i64::from(w / 8) - 1);
        let last = self
            .b
            .ins()
            .ushr_imm(last_byte, GRANULE.trailing_zeros() as i64);
        let shadow = self.native_field(native::SHADOW);
        // Scalar widths are <= 8 bytes, so at most two 16-byte granules.
        for index in [first, last] {
            let at = self.b.ins().iadd(shadow, index);
            let expected = self.b.ins().load(types::I8, MemFlags::new(), at, 0);
            let expected = self.b.ins().uextend(types::I64, expected);
            let mismatch = self.b.ins().icmp(IntCC::NotEqual, tag, expected);
            self.slow_if(mismatch, slow);
        }
        self.b.ins().jump(checked, &[]);
        self.b.switch_to_block(checked);
        self.b.seal_block(checked);
        let bias = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            entry,
            std::mem::offset_of!(FastPage, bias) as i32,
        );
        (self.b.ins().iadd(plain, bias), plain)
    }
    pub(super) fn native_load(&mut self, p: Value, w: u16) -> Result<Value, EngineError> {
        self.bounds_check(p, w);
        let slow = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b.append_block_param(done, types::I64);
        let (host, plain) = self.fast_address(p, w, false, slow);
        if w == 64 {
            self.slow_if_stack(plain, slow);
        }
        // ProcessBus also returns literal/trusted-slot provenance for narrow
        // loads. Keep that behavior, including when authentication is disabled.
        let sensitive = self.pac_page(plain);
        self.slow_if(sensitive, slow);
        let raw = self.b.ins().load(ty(w), MemFlags::new(), host, 0);
        let raw = if w == 64 {
            raw
        } else {
            self.b.ins().uextend(types::I64, raw)
        };
        if w == 64 {
            let code = self.b.ins().band_imm(raw, PAC_MASK as i64);
            let has_code = self.b.ins().icmp_imm(IntCC::NotEqual, code, 0);
            let unsigned = self.b.ins().band_imm(raw, !PAC_MASK as i64);
            let offset = self.b.ins().iadd_imm(unsigned, -0x10000);
            let possible_code = self.b.ins().icmp_imm(
                IntCC::UnsignedLessThan,
                offset,
                (ADDRESS_LIMIT - 0x10000) as i64,
            );
            let has_code = self.b.ins().band(has_code, possible_code);
            let pac = self.native_field(native::PAC_ENABLED);
            let pac = self.b.ins().icmp_imm(IntCC::NotEqual, pac, 0);
            let needs_auth = self.b.ins().band(pac, has_code);
            self.slow_if(needs_auth, slow);
        }
        let zero = self.c(0);
        self.b.ins().jump(done, &[raw, zero]);
        self.b.switch_to_block(slow);
        self.b.seal_block(slow);
        let value = self.native_load_slow(p, w)?;
        self.b.ins().jump(done, &[value, self.last_load_prov]);
        self.b.switch_to_block(done);
        self.b.seal_block(done);
        self.last_load_prov = self.b.block_params(done)[1];
        Ok(self.b.block_params(done)[0])
    }
    pub(super) fn native_store(&mut self, p: Value, w: u16, v: Value, provenance: Value) {
        self.bounds_check(p, w);
        let slow = self.b.create_block();
        let done = self.b.create_block();
        let (host, plain) = self.fast_address(p, w, true, slow);
        // Stack provenance is updated even with PAC disabled and on narrow writes.
        self.slow_if_stack(plain, slow);
        if w == 64 {
            let sensitive = self.pac_page(plain);
            let sensitive = self.b.ins().icmp_imm(IntCC::NotEqual, sensitive, 0);
            let offset = self.b.ins().iadd_imm(v, -0x10000);
            let possible_code = self.b.ins().icmp_imm(
                IntCC::UnsignedLessThan,
                offset,
                (ADDRESS_LIMIT - 0x10000) as i64,
            );
            let special = self.b.ins().bor(sensitive, possible_code);
            let pac = self.native_field(native::PAC_ENABLED);
            let pac = self.b.ins().icmp_imm(IntCC::NotEqual, pac, 0);
            let special = self.b.ins().band(pac, special);
            self.slow_if(special, slow);
        }
        let value = if w == 64 {
            v
        } else {
            self.b.ins().ireduce(ty(w), v)
        };
        self.b.ins().store(MemFlags::new(), value, host, 0);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(slow);
        self.b.seal_block(slow);
        self.native_store_slow(p, w, v, provenance);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        self.b.seal_block(done);
    }

    pub(super) fn native_rep(&mut self, copy: bool, w: u16, source: Value) {
        let rcx = self.div.variable(2) as usize;
        let rdi = self.div.variable(5) as usize;
        let rsi = self.div.variable(4) as usize;
        let count = self.var(rcx);
        let run = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(count, run, &[], done, &[]);
        self.b.switch_to_block(run);
        self.b.seal_block(run);
        let destination = self.var(rdi);
        let mut sig = Signature::new(CallConv::SystemV);
        for _ in 0..7 {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));
        let sig = self.b.import_signature(sig);
        let cookie = self.native_field(native::NATIVE_COOKIE_OFFSET);
        let function = self.native_field(native::BULK_FN);
        let kind = self.c(u64::from(copy));
        let size = self.c(u64::from(w / 8));
        let pc = self.c(self.rip);
        let call = self.b.ins().call_indirect(
            sig,
            function,
            &[cookie, kind, destination, source, count, size, pc],
        );
        let completed = self.b.inst_results(call)[0];
        let advance = self.b.create_block();
        let checked = self.b.create_block();
        // A zero-count REP and a fault on the first element retain provenance.
        self.b.ins().brif(completed, advance, &[], checked, &[]);
        self.b.switch_to_block(advance);
        self.b.seal_block(advance);
        let bytes = self.b.ins().imul_imm(completed, i64::from(w / 8));
        let next_dst = self.b.ins().iadd(destination, bytes);
        self.set(rdi, next_dst);
        if copy {
            let next_src = self.b.ins().iadd(source, bytes);
            self.set(rsi, next_src);
        }
        let remaining = self.b.ins().isub(count, completed);
        self.set(rcx, remaining);
        self.b.ins().jump(checked, &[]);
        self.b.switch_to_block(checked);
        self.b.seal_block(checked);
        self.check_native_fault();
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        self.b.seal_block(done);
    }
}
