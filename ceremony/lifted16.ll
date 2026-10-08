; ModuleID = 'lifted_code'
source_filename = "lifted_code"
target datalayout = "e-m:e-p:32:32-i64:64-n32:64-S128"
target triple = "wasm32-unknown-unknown"

%struct.State = type { %struct.X86State }
%struct.X86State = type { %struct.ArchState, [32 x %union.VectorReg], %struct.ArithFlags, %union.anon, %struct.Segments, %struct.AddressSpace, %struct.GPR, %struct.X87Stack, %struct.MMX, %struct.FPUStatusFlags, [8 x i8], %union.FPU, %struct.SegmentCaches, %struct.K_REG }
%struct.ArchState = type { i32, i32, %union.anon }
%union.VectorReg = type { %union.vec512_t }
%union.vec512_t = type { %struct.uint64v8_t }
%struct.uint64v8_t = type { [8 x i64] }
%struct.ArithFlags = type { i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8 }
%union.anon = type { i64 }
%struct.Segments = type { i16, %union.SegmentSelector, i16, %union.SegmentSelector, i16, %union.SegmentSelector, i16, %union.SegmentSelector, i16, %union.SegmentSelector, i16, %union.SegmentSelector }
%union.SegmentSelector = type { i16 }
%struct.AddressSpace = type { i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg }
%struct.Reg = type { %union.anon }
%struct.GPR = type { i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg, i64, %struct.Reg }
%struct.X87Stack = type { [8 x %struct.anon.3] }
%struct.anon.3 = type { [6 x i8], %struct.float80_t }
%struct.float80_t = type { [10 x i8] }
%struct.MMX = type { [8 x %struct.anon.4] }
%struct.anon.4 = type { i64, %union.vec64_t }
%union.vec64_t = type { %struct.uint64v1_t }
%struct.uint64v1_t = type { [1 x i64] }
%struct.FPUStatusFlags = type { i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, i8, [2 x i8] }
%union.FPU = type { %struct.anon.11 }
%struct.anon.11 = type { %struct.FpuFXSAVE, [96 x i8] }
%struct.FpuFXSAVE = type { %union.SegmentSelector, %union.SegmentSelector, %union.FPUAbridgedTagWord, i8, i16, i32, %union.SegmentSelector, i16, i32, %union.SegmentSelector, i16, %union.FPUControlStatus, %union.FPUControlStatus, [8 x %struct.FPUStackElem], [16 x %union.vec128_t] }
%union.FPUAbridgedTagWord = type { i8 }
%union.FPUControlStatus = type { i32 }
%struct.FPUStackElem = type { %union.anon.9, [6 x i8] }
%union.anon.9 = type { %struct.float80_t }
%union.vec128_t = type { %struct.uint128v1_t }
%struct.uint128v1_t = type { [1 x i128] }
%struct.SegmentCaches = type { %struct.SegmentShadow, %struct.SegmentShadow, %struct.SegmentShadow, %struct.SegmentShadow, %struct.SegmentShadow, %struct.SegmentShadow }
%struct.SegmentShadow = type { %union.anon, i32, i32 }
%struct.K_REG = type { [8 x %struct.anon.16] }
%struct.anon.16 = type { i64, i64 }

; Function Attrs: noduplicate noinline nounwind optnone
declare dso_local ptr @__remill_write_memory_64(ptr noundef, i64 noundef, i64 noundef) #0

; Function Attrs: noduplicate noinline nounwind optnone
declare dso_local zeroext i1 @__remill_flag_computation_overflow(i1 noundef zeroext, ...) #0

; Function Attrs: noduplicate noinline nounwind optnone
declare dso_local zeroext i8 @__remill_undefined_8() #0

; Function Attrs: noduplicate noinline nounwind optnone
declare dso_local i64 @__remill_read_memory_64(ptr noundef, i64 noundef) #0

; Function Attrs: noduplicate noinline nounwind optnone
declare dso_local ptr @__remill_function_return(ptr noundef nonnull align 1, i64 noundef, ptr noundef) #0

define i64 @call_sub_0(i64 %arg_RDI, i64 %arg_RSI) #1 {
  %MONITOR.i = alloca i64, align 8
  %STATE.i = alloca ptr, align 8
  %MEMORY.i = alloca ptr, align 8
  %NEXT_PC.i = alloca i64, align 8
  %CSBASE.i = alloca i64, align 8
  %SSBASE.i = alloca i64, align 8
  %ESBASE.i = alloca i64, align 8
  %DSBASE.i = alloca i64, align 8
  %1 = alloca %struct.State, align 8
  %RIP = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 33, i32 0, i32 0, !remill_register !0
  store i64 0, ptr %RIP, align 8
  %symbolic_STACK = call i64 @__remill_symbolic_STACK()
  %RSP = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 13, i32 0, i32 0, !remill_register !1
  store i64 %symbolic_STACK, ptr %RSP, align 8
  %symbolic_GSBASE = call i64 @__remill_symbolic_GSBASE()
  %GSBASE = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 5, i32 5, i32 0, i32 0, !remill_register !2
  store i64 %symbolic_GSBASE, ptr %GSBASE, align 8
  %symbolic_FSBASE = call i64 @__remill_symbolic_FSBASE()
  %FSBASE = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 5, i32 7, i32 0, i32 0, !remill_register !3
  store i64 %symbolic_FSBASE, ptr %FSBASE, align 8
  %RDI = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 11, i32 0, i32 0, !remill_register !4
  store i64 %arg_RDI, ptr %RDI, align 8
  %RSI = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 9, i32 0, i32 0, !remill_register !5
  store i64 %arg_RSI, ptr %RSI, align 8
  call void @llvm.experimental.noalias.scope.decl(metadata !6)
  call void @llvm.experimental.noalias.scope.decl(metadata !9)
  call void @llvm.lifetime.start.p0(i64 8, ptr %MONITOR.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %STATE.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %MEMORY.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %NEXT_PC.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %CSBASE.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %SSBASE.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %ESBASE.i)
  call void @llvm.lifetime.start.p0(i64 8, ptr %DSBASE.i)
  %RDI.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 11, i32 0, i32 0
  %RSI.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 9, i32 0, i32 0
  %RAX.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 1, i32 0, i32 0
  %RSP.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 13, i32 0, i32 0
  %RBP.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 15, i32 0, i32 0
  store i64 0, ptr %MONITOR.i, align 8, !noalias !11
  store ptr %1, ptr %STATE.i, align 8, !noalias !11
  store ptr undef, ptr %MEMORY.i, align 8, !noalias !11
  store i64 0, ptr %NEXT_PC.i, align 8, !noalias !11
  %PC.i = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 33, i32 0, i32 0
  store i64 0, ptr %CSBASE.i, align 8, !noalias !11
  store i64 0, ptr %SSBASE.i, align 8, !noalias !11
  store i64 0, ptr %ESBASE.i, align 8, !noalias !11
  store i64 0, ptr %DSBASE.i, align 8, !noalias !11
  store i64 0, ptr %NEXT_PC.i, align 8, !noalias !11
  %2 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %2, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %3 = add i64 %2, 1
  store i64 %3, ptr %NEXT_PC.i, align 8, !noalias !11
  %4 = load i64, ptr %RBP.i, align 8, !alias.scope !6, !noalias !9
  %5 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  %6 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 6, i32 13
  %7 = load i64, ptr %6, align 8, !alias.scope !6, !noalias !9
  %8 = add i64 %7, -8
  %9 = call ptr @__remill_write_memory_64(ptr noundef %5, i64 noundef %8, i64 noundef %4) #5
  store i64 %8, ptr %6, align 8, !alias.scope !6, !noalias !9
  store ptr %9, ptr %MEMORY.i, align 8, !noalias !11
  %10 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %10, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %11 = add i64 %10, 3
  store i64 %11, ptr %NEXT_PC.i, align 8, !noalias !11
  %12 = load i64, ptr %RSP.i, align 8, !alias.scope !6, !noalias !9
  %13 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  store i64 %12, ptr %RBP.i, align 8, !alias.scope !6, !noalias !9
  store ptr %13, ptr %MEMORY.i, align 8, !noalias !11
  %14 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %14, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %15 = add i64 %14, 4
  store i64 %15, ptr %NEXT_PC.i, align 8, !noalias !11
  %16 = load i64, ptr %RSI.i, align 8, !alias.scope !6, !noalias !9
  %17 = add i64 %16, 1
  %18 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  store i64 %17, ptr %RAX.i, align 8, !alias.scope !6, !noalias !9
  store ptr %18, ptr %MEMORY.i, align 8, !noalias !11
  %19 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %19, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %20 = add i64 %19, 4
  store i64 %20, ptr %NEXT_PC.i, align 8, !noalias !11
  %21 = load i64, ptr %RAX.i, align 8, !alias.scope !6, !noalias !9
  %22 = load i64, ptr %RDI.i, align 8, !alias.scope !6, !noalias !9
  %23 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  %24 = ashr i64 %21, 63
  %25 = ashr i64 %22, 63
  %26 = zext i64 %24 to i128
  %27 = shl nuw i128 %26, 64
  %28 = zext i64 %21 to i128
  %29 = or i128 %27, %28
  %30 = zext i64 %25 to i128
  %31 = shl nuw i128 %30, 64
  %32 = zext i64 %22 to i128
  %33 = or i128 %31, %32
  %34 = mul nsw i128 %33, %29
  %35 = trunc i128 %34 to i64
  %36 = lshr i128 %34, 64
  %37 = trunc i128 %36 to i64
  store i64 %35, ptr %RAX.i, align 8, !alias.scope !6, !noalias !9
  %38 = sext i64 %35 to i128
  %39 = icmp ne i128 %34, %38
  %40 = call zeroext i1 (i1, ...) @__remill_flag_computation_overflow(i1 noundef zeroext %39, i64 noundef %21, i64 noundef %22, i64 noundef %35, i64 noundef %37) #5
  %41 = zext i1 %40 to i8
  %42 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 1
  store i8 %41, ptr %42, align 1, !alias.scope !6, !noalias !9
  %43 = call zeroext i8 @__remill_undefined_8() #5
  %44 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 3
  store i8 %43, ptr %44, align 1, !alias.scope !6, !noalias !9
  %45 = call zeroext i8 @__remill_undefined_8() #5
  %46 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 5
  store i8 %45, ptr %46, align 1, !alias.scope !6, !noalias !9
  %47 = call zeroext i8 @__remill_undefined_8() #5
  %48 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 7
  store i8 %47, ptr %48, align 1, !alias.scope !6, !noalias !9
  %49 = call zeroext i8 @__remill_undefined_8() #5
  %50 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 9
  store i8 %49, ptr %50, align 1, !alias.scope !6, !noalias !9
  %51 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 2, i32 13
  store i8 %41, ptr %51, align 1, !alias.scope !6, !noalias !9
  store ptr %23, ptr %MEMORY.i, align 8, !noalias !11
  %52 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %52, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %53 = add i64 %52, 1
  store i64 %53, ptr %NEXT_PC.i, align 8, !noalias !11
  %54 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  %55 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 6, i32 13
  %56 = load i64, ptr %55, align 8, !alias.scope !6, !noalias !9
  %57 = add i64 %56, 8
  store i64 %57, ptr %55, align 8, !alias.scope !6, !noalias !9
  %58 = call i64 @__remill_read_memory_64(ptr noundef %54, i64 noundef %56) #5
  store i64 %58, ptr %RBP.i, align 8, !alias.scope !6, !noalias !9
  store ptr %54, ptr %MEMORY.i, align 8, !noalias !11
  %59 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %59, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %60 = add i64 %59, 1
  store i64 %60, ptr %NEXT_PC.i, align 8, !noalias !11
  %61 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  %62 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 6, i32 13
  %63 = load i64, ptr %62, align 8, !alias.scope !6, !noalias !9
  %64 = call i64 @__remill_read_memory_64(ptr noundef %61, i64 noundef %63) #5
  %65 = getelementptr inbounds %struct.X86State, ptr %1, i64 0, i32 6, i32 33
  store i64 %64, ptr %65, align 8, !alias.scope !6, !noalias !9
  store i64 %64, ptr %NEXT_PC.i, align 8, !noalias !11
  %66 = load i64, ptr %62, align 8, !alias.scope !6, !noalias !9
  %67 = add i64 %66, 8
  store i64 %67, ptr %62, align 8, !alias.scope !6, !noalias !9
  store ptr %61, ptr %MEMORY.i, align 8, !noalias !11
  %68 = load i64, ptr %NEXT_PC.i, align 8, !noalias !11
  store i64 %68, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %69 = load ptr, ptr %MEMORY.i, align 8, !noalias !11
  %70 = load i64, ptr %PC.i, align 8, !alias.scope !6, !noalias !9
  %71 = call ptr @__remill_function_return(ptr %1, i64 %70, ptr %69)
  call void @llvm.lifetime.end.p0(i64 8, ptr %MONITOR.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %STATE.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %MEMORY.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %NEXT_PC.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %CSBASE.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %SSBASE.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %ESBASE.i)
  call void @llvm.lifetime.end.p0(i64 8, ptr %DSBASE.i)
  %RAX = getelementptr inbounds %struct.State, ptr %1, i32 0, i32 0, i32 6, i32 1, i32 0, i32 0, !remill_register !12
  %72 = load i64, ptr %RAX, align 8
  ret i64 %72
}

; Function Attrs: nounwind willreturn memory(none)
declare i64 @__remill_symbolic_STACK() #2

; Function Attrs: nounwind willreturn memory(none)
declare i64 @__remill_symbolic_GSBASE() #2

; Function Attrs: nounwind willreturn memory(none)
declare i64 @__remill_symbolic_FSBASE() #2

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: readwrite)
declare void @llvm.experimental.noalias.scope.decl(metadata) #3

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.start.p0(i64 immarg, ptr nocapture) #4

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.end.p0(i64 immarg, ptr nocapture) #4

attributes #0 = { noduplicate noinline nounwind optnone "frame-pointer"="all" "no-builtins" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "tune-cpu"="generic" }
attributes #1 = { "disable-tail-calls"="true" }
attributes #2 = { nounwind willreturn memory(none) }
attributes #3 = { nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: readwrite) }
attributes #4 = { nocallback nofree nosync nounwind willreturn memory(argmem: readwrite) }
attributes #5 = { nobuiltin nounwind "no-builtins" }

!0 = !{[4 x i8] c"RIP\00"}
!1 = !{[4 x i8] c"RSP\00"}
!2 = !{[7 x i8] c"GSBASE\00"}
!3 = !{[7 x i8] c"FSBASE\00"}
!4 = !{[4 x i8] c"RDI\00"}
!5 = !{[4 x i8] c"RSI\00"}
!6 = !{!7}
!7 = distinct !{!7, !8, !"sub_0: %state"}
!8 = distinct !{!8, !"sub_0"}
!9 = !{!10}
!10 = distinct !{!10, !8, !"sub_0: %memory"}
!11 = !{!7, !10}
!12 = !{[4 x i8] c"RAX\00"}
