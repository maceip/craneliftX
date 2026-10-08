; Minimal remill runtime stub for PURE-COMPUTE lifted functions.
;
; remill-lifted code calls a set of __remill_* intrinsics to model CPU
; state, memory, and flags. For a function that performs only integer
; arithmetic (no real loads/stores, no control-flow that depends on
; flags) -- e.g. ceremony_op -- these intrinsics have no effect on the
; final result. We therefore provide trivial definitions so the lifted
; module links and runs on wasm/Pulley.
;
; For general binaries, Anvill (fed a Ghidra/Binary Ninja spec) is the
; correct tool: it substitutes the FULL remill runtime semantics and
; recovers a clean C ABI. This stub is the minimal stand-in that proves
; the native -> remill -> Cranelift/Pulley chain end-to-end.

define ptr @__remill_write_memory_64(ptr %mem, i64 %addr, i64 %val) {
  ret ptr null
}

define i64 @__remill_read_memory_64(ptr %mem, i64 %addr) {
  ret i64 0
}

define zeroext i1 @__remill_flag_computation_overflow(i1 zeroext %u, ...) {
  ret i1 false
}

define zeroext i8 @__remill_undefined_8() {
  ret i8 0
}

define i64 @__remill_symbolic_STACK() {
  ret i64 0
}

define i64 @__remill_symbolic_GSBASE() {
  ret i64 0
}

define i64 @__remill_symbolic_FSBASE() {
  ret i64 0
}

define ptr @__remill_function_return(ptr %state, i64 %pc, ptr %mem) {
  ret ptr null
}

; compiler-rt 128-bit multiply, required because wasm has no native i128 mul.
; The lifted code only consumes the low 64 bits (trunc to i64), so a plain
; i64 multiply (native on wasm) is sufficient and avoids recursion into __multi3.
define i128 @__multi3(i128 %a, i128 %b) {
  %a_lo = trunc i128 %a to i64
  %b_lo = trunc i128 %b to i64
  %lo = mul i64 %a_lo, %b_lo
  %result = zext i64 %lo to i128
  ret i128 %result
}
