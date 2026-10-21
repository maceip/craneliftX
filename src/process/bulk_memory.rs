//! REP operations with checked page/granule spans. The address-space mutex is
//! retained for the whole operation. Pointer-sensitive, overlapping and faulting
//! spans execute element by element through the reference helpers.
use super::*;
use crate::process::{
    fast_memory::PAGE_BYTES, lower::POINTER_ADDRESS_MASK, memory::ADDRESS_LIMIT, pac::PAC_MASK,
};

impl Runtime {
    fn bulk_chunk(&mut self, copy: bool, dst: u64, src: u64, count: u64, size: u64) -> u64 {
        #[cfg(test)]
        if !self.inline_memory {
            return 0;
        }
        // SAFETY: only called by a native helper while execute_x86_64 owns the
        // process address-space mutex. No region or metadata can move or change
        // on another thread until this operation has finished.
        let space = unsafe { &mut *self.active_space };
        let scheme = space.heap.scheme();
        let to = scheme.strip(dst);
        let from = scheme.strip(src);
        let mut bytes = count.saturating_mul(size).min(PAGE_BYTES - to % PAGE_BYTES);
        if copy {
            bytes = bytes.min(PAGE_BYTES - from % PAGE_BYTES);
        }
        bytes -= bytes % size;
        if bytes == 0 || to.checked_add(bytes).is_none_or(|end| end > ADDRESS_LIMIT) {
            return 0;
        }
        if copy
            && from
                .checked_add(bytes)
                .is_none_or(|end| end > ADDRESS_LIMIT)
        {
            return 0;
        }
        let overlaps_stack = |start: u64| {
            (start < STACK_TOP && start + bytes > STACK_BOTTOM)
                || (start < self.stack_top && start + bytes > self.stack_bottom)
        };
        if overlaps_stack(to) || (copy && size == 8 && overlaps_stack(from)) {
            return 0;
        }
        if !space.heap.span_is_valid(dst, bytes) || (copy && !space.heap.span_is_valid(src, bytes))
        {
            return 0;
        }
        if size == 8 && (!space.pac.ordinary_page(to) || (copy && !space.pac.ordinary_page(from))) {
            return 0;
        }
        let plain_value = |value: u64| !(0x10000..ADDRESS_LIMIT).contains(&value);
        if copy {
            let Ok(source) = space.memory.read(from, bytes as usize) else {
                return 0;
            };
            if size == 8
                && space.pac.enabled()
                && source.chunks_exact(8).any(|word| {
                    let value = u64::from_le_bytes(word.try_into().unwrap());
                    (value & PAC_MASK != 0 && !plain_value(value & !PAC_MASK))
                        || !plain_value(value)
                })
            {
                return 0;
            }
        } else {
            if size == 8 && space.pac.enabled() && !plain_value(src) {
                return 0;
            }
        }
        if copy {
            if space.memory.copy_bytes(from, to, bytes as usize).is_err() {
                return 0;
            }
        } else {
            let Ok(destination) = space.memory.read_mut(to, bytes as usize) else {
                return 0;
            };
            if size == 1 {
                destination.fill(src as u8);
            } else {
                let word = src.to_le_bytes();
                for chunk in destination.chunks_exact_mut(size as usize) {
                    chunk.copy_from_slice(&word[..size as usize]);
                }
            }
        }
        #[cfg(test)]
        {
            self.bulk_fast_bytes += bytes;
        }
        bytes / size
    }

    pub(in crate::process) fn native_bulk(
        &mut self,
        copy: bool,
        dst: u64,
        src: u64,
        count: u64,
        size: u64,
        pc: u64,
    ) -> u64 {
        self.with_native_space(|rt| rt.native_bulk_locked(copy, dst, src, count, size, pc))
    }

    fn native_bulk_locked(&mut self, copy: bool, dst: u64, src: u64, count: u64, size: u64, pc: u64) -> u64 {
        assert!(!self.active_space.is_null());
        assert!([1, 2, 4, 8].contains(&size));
        let scheme = unsafe { (&*self.active_space).heap.scheme() };
        let to = scheme.strip(dst);
        let from = scheme.strip(src);
        let disjoint = count
            .checked_mul(size)
            .and_then(|bytes| {
                Some(to.checked_add(bytes)? <= from || from.checked_add(bytes)? <= to)
            })
            .unwrap_or(false);
        let mut done = 0;
        while done < count {
            let offset = done.wrapping_mul(size);
            let destination = dst.wrapping_add(offset);
            let source = if copy { src.wrapping_add(offset) } else { src };
            if !copy || disjoint {
                let copied = self.bulk_chunk(copy, destination, source, count - done, size);
                if copied != 0 {
                    done += copied;
                    continue;
                }
            }
            // Preserve per-element ordering and the valid prefix on any fault.
            // The old scalar JIT guards these addresses before calling helpers.
            if destination & POINTER_ADDRESS_MASK > ADDRESS_LIMIT - size
                || (copy && source & POINTER_ADDRESS_MASK > ADDRESS_LIMIT - size)
            {
                self.native_fault =
                    Some((pc, memory::error("bulk access outside guest address space")));
                break;
            }
            let (value, provenance) = if copy {
                let loaded = self.native_load(source, size, pc);
                if self.native_fault.is_some() {
                    break;
                }
                loaded
            } else {
                (source, false)
            };
            self.native_store(destination, size, value, provenance, pc);
            if self.native_fault.is_some() {
                break;
            }
            done += 1;
        }
        done
    }
}
