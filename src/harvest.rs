use iced_x86::{Decoder, DecoderError, DecoderOptions, Instruction};

use crate::{error::EngineError, x86::boundary};

pub const MAX_INSNS: usize = 32;

/// Decode a bounded byte slice. A boundary instruction is retained for auditing,
/// but next_rip points AT it: it has not executed. Otherwise next_rip is the end.
pub fn harvest_bytes(
    code: &[u8],
    start_rip: u64,
    max_insns: usize,
) -> Result<(Vec<Instruction>, u64), EngineError> {
    let mut decoder = Decoder::with_ip(64, code, start_rip, DecoderOptions::NONE);
    let mut trace = Vec::with_capacity(max_insns.min(MAX_INSNS));
    while decoder.can_decode() && trace.len() < max_insns.min(MAX_INSNS) {
        let rip = decoder.ip();
        let insn = decoder.decode();
        if insn.is_invalid() {
            return Err(EngineError::Harvest {
                rip,
                reason: "invalid or truncated x86 instruction",
            });
        }
        let stop = boundary(&insn);
        trace.push(insn);
        if stop {
            return Ok((trace, rip));
        }
    }
    Ok((trace, decoder.ip()))
}

/// Harvest live x86 code, reading only bytes needed for each instruction.
///
/// # Safety
/// start_rip must identify readable, initialized, immutable memory through the
/// first boundary instruction or min(max_insns, 32) complete instructions.
/// This routine cannot discover mapping lengths from an integer address.
pub unsafe fn harvest_trace(
    start_rip: u64,
    max_insns: usize,
) -> Result<(Vec<Instruction>, u64), EngineError> {
    if start_rip == 0 {
        return Err(EngineError::Harvest {
            rip: 0,
            reason: "null code pointer",
        });
    }
    let mut trace = Vec::with_capacity(max_insns.min(MAX_INSNS));
    let mut rip = start_rip;
    for _ in 0..max_insns.min(MAX_INSNS) {
        let mut bytes = [0; 15];
        let mut decoded = None;
        for count in 1..=15 {
            let address = rip
                .checked_add((count - 1) as u64)
                .ok_or(EngineError::Harvest {
                    rip,
                    reason: "instruction address overflow",
                })?;
            // SAFETY: the caller guarantees that all reached instruction bytes are readable.
            bytes[count - 1] = unsafe { (address as *const u8).read() };
            let mut decoder = Decoder::with_ip(64, &bytes[..count], rip, DecoderOptions::NONE);
            let insn = decoder.decode();
            match decoder.last_error() {
                DecoderError::None => {
                    decoded = Some(insn);
                    break;
                }
                DecoderError::NoMoreBytes => continue,
                _ => {
                    return Err(EngineError::Harvest {
                        rip,
                        reason: "invalid x86 instruction",
                    })
                }
            }
        }
        let insn = decoded.ok_or(EngineError::Harvest {
            rip,
            reason: "x86 instruction exceeds 15 bytes",
        })?;
        let stop = boundary(&insn);
        trace.push(insn);
        if stop {
            return Ok((trace, rip));
        }
        rip = insn.next_ip();
    }
    Ok((trace, rip))
}
