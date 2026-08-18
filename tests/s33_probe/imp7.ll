; ModuleID = 'tie'
source_filename = "input.tie"

declare i32 @printf(ptr, ...)

declare i64 @strlen(ptr)
declare i32 @strcmp(ptr, ptr)
declare ptr @malloc(i64)
declare void @free(ptr)
declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)
declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)

declare ptr @fopen(ptr, ptr)
declare i64 @fwrite(ptr, i64, i64, ptr)
declare i32 @fclose(ptr)
declare i32 @fflush(ptr)
declare void @exit(i32)
declare i32 @remove(ptr)
declare double @sqrt(double)
declare double @sin(double)
declare double @cos(double)
declare double @tan(double)
declare double @exp(double)
declare double @log(double)
declare double @pow(double, double)
declare double @floor(double)
declare double @ceil(double)
declare double @round(double)

define i64 @seven() {
bb0:
%0 = add i64 0, 7
ret i64 %0
}


