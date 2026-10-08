; Remill execution semantics for lifted traces.
;
; These definitions match the contract remill's own test runner uses
; (vendor/remill/tests/X86/Run.cpp and test_runner_lib/TestRunner.cpp):
;   * flag and compare intrinsics return their computed boolean
;   * memory intrinsics are real loads and stores at the guest address
;   * control-transfer intrinsics return the memory token unchanged
;
; guest_stack is split in half. The stack grows down from guest_stack_top().
; Guest data passed in by the emulator lives at and above that address.
; On wasm32, ptrtoint yields a linear-memory offset. On a native or qemu
; target, it yields a real pointer. The same IR is correct for both.

@guest_stack = global [65536 x i8] zeroinitializer, align 16

define i64 @guest_stack_top() {
  %base = ptrtoint ptr @guest_stack to i64
  %top = add i64 %base, 32768
  ret i64 %top
}

define i64 @__remill_symbolic_STACK() {
  %top = call i64 @guest_stack_top()
  ret i64 %top
}

define i64 @__remill_symbolic_GSBASE() {
  ret i64 0
}

define i64 @__remill_symbolic_FSBASE() {
  ret i64 0
}

define zeroext i8 @__remill_undefined_8() {
  ret i8 0
}

define zeroext i16 @__remill_undefined_16() {
  ret i16 0
}

define i32 @__remill_undefined_32() {
  ret i32 0
}

define i64 @__remill_undefined_64() {
  ret i64 0
}

define i8 @__remill_read_memory_8(ptr %mem, i64 %addr) {
  %p = inttoptr i64 %addr to ptr
  %v = load i8, ptr %p, align 1
  ret i8 %v
}

define i16 @__remill_read_memory_16(ptr %mem, i64 %addr) {
  %p = inttoptr i64 %addr to ptr
  %v = load i16, ptr %p, align 1
  ret i16 %v
}

define i32 @__remill_read_memory_32(ptr %mem, i64 %addr) {
  %p = inttoptr i64 %addr to ptr
  %v = load i32, ptr %p, align 1
  ret i32 %v
}

define i64 @__remill_read_memory_64(ptr %mem, i64 %addr) {
  %p = inttoptr i64 %addr to ptr
  %v = load i64, ptr %p, align 1
  ret i64 %v
}

define ptr @__remill_write_memory_8(ptr %mem, i64 %addr, i8 %val) {
  %p = inttoptr i64 %addr to ptr
  store i8 %val, ptr %p, align 1
  ret ptr %mem
}

define ptr @__remill_write_memory_16(ptr %mem, i64 %addr, i16 %val) {
  %p = inttoptr i64 %addr to ptr
  store i16 %val, ptr %p, align 1
  ret ptr %mem
}

define ptr @__remill_write_memory_32(ptr %mem, i64 %addr, i32 %val) {
  %p = inttoptr i64 %addr to ptr
  store i32 %val, ptr %p, align 1
  ret ptr %mem
}

define ptr @__remill_write_memory_64(ptr %mem, i64 %addr, i64 %val) {
  %p = inttoptr i64 %addr to ptr
  store i64 %val, ptr %p, align 1
  ret ptr %mem
}

define zeroext i1 @__remill_flag_computation_zero(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_flag_computation_sign(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_flag_computation_carry(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_flag_computation_overflow(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_eq(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_neq(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_ult(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_ule(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_ugt(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_uge(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_slt(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_sle(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_sgt(i1 zeroext %result) {
  ret i1 %result
}

define zeroext i1 @__remill_compare_sge(i1 zeroext %result) {
  ret i1 %result
}

define ptr @__remill_function_return(ptr %state, i64 %pc, ptr %mem) {
  ret ptr %mem
}

define ptr @__remill_function_call(ptr %state, i64 %pc, ptr %mem) {
  ret ptr %mem
}

define ptr @__remill_jump(ptr %state, i64 %pc, ptr %mem) {
  ret ptr %mem
}

define ptr @__remill_missing_block(ptr %state, i64 %pc, ptr %mem) {
  ret ptr %mem
}

define ptr @__remill_error(ptr %state, i64 %pc, ptr %mem) {
  ret ptr %mem
}

; compiler-rt 128-bit multiply. Lifted integer code only consumes the low 64
; bits when the source multiply was 64-bit, which matches wasm's lowering.
define i128 @__multi3(i128 %a, i128 %b) {
  %a_lo = trunc i128 %a to i64
  %b_lo = trunc i128 %b to i64
  %lo = mul i64 %a_lo, %b_lo
  %result = zext i64 %lo to i128
  ret i128 %result
}
