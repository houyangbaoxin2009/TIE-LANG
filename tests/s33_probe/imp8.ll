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

declare i64 @tie_table_len(ptr)
declare ptr @tie_table_at_string(ptr, i64, ptr)
declare i64 @tie_table_at_i64(ptr, i64, ptr)
declare void @tie_table_push_string(ptr, ptr)
declare void @tie_table_push_i64(ptr, i64)
declare void @tie_table_set_string(ptr, i64, ptr, ptr)
declare void @tie_table_set_i64(ptr, i64, i64, ptr)

@intern_pool = global ptr null
@intern_keys = global ptr null
@intern_vals = global ptr null

define i64 @intern$binsearch(ptr %0) {
bb0:
%1 = alloca ptr
%2 = alloca i64
%3 = alloca i64
%4 = alloca i64
%5 = alloca i8
%6 = alloca i8
store ptr %0, ptr %1
%7 = add i64 0, 0
store i64 %7, ptr %2
%8 = load ptr, ptr @intern_keys
%9 = call i64 @tie_table_len(ptr %8)
%10 = add i64 0, 1
%11 = sub i64 %9, %10
store i64 %11, ptr %3
br label %bb1
bb1:
%12 = load i64, ptr %2
%13 = load i64, ptr %3
%14 = icmp sle i64 %12, %13
br i1 %14, label %bb2, label %bb3
bb2:
%15 = load i64, ptr %2
%16 = load i64, ptr %3
%17 = add i64 %15, %16
%18 = add i64 0, 2
%19 = sdiv i64 %17, %18
store i64 %19, ptr %4
%20 = load ptr, ptr @intern_keys
%21 = load i64, ptr %4
%22 = call ptr @tie_table_at_string(ptr %20, i64 %21, ptr %5)
%23 = load ptr, ptr %1
%24 = call i32 @strcmp(ptr %22, ptr %23)
%25 = icmp eq i32 %24, 0
br i1 %25, label %bb4, label %bb5
bb4:
%26 = load i64, ptr %4
ret i64 %26
bb5:
br label %bb6
bb6:
%27 = load ptr, ptr @intern_keys
%28 = load i64, ptr %4
%29 = call ptr @tie_table_at_string(ptr %27, i64 %28, ptr %6)
%30 = load ptr, ptr %1
%31 = call i32 @strcmp(ptr %29, ptr %30)
%32 = icmp slt i32 %31, 0
br i1 %32, label %bb7, label %bb8
bb7:
%33 = load i64, ptr %4
%34 = add i64 0, 1
%35 = add i64 %33, %34
store i64 %35, ptr %2
br label %bb9
bb8:
%36 = load i64, ptr %4
%37 = add i64 0, 1
%38 = sub i64 %36, %37
store i64 %38, ptr %3
br label %bb9
bb9:
br label %bb1
bb3:
%39 = add i64 0, 1
%40 = sub i64 0, %39
ret i64 %40
}

define i64 @intern$intern(ptr %0) {
bb10:
%1 = alloca ptr
%2 = alloca i64
%3 = alloca i64
%4 = alloca i64
%5 = alloca i8
%6 = alloca i8
%7 = alloca ptr
%8 = alloca i64
%9 = alloca i8
%10 = alloca i64
%11 = alloca i64
%12 = alloca i64
%13 = alloca i64
%14 = alloca i64
%15 = alloca i8
%16 = alloca i64
store ptr %0, ptr %7
%17 = load ptr, ptr %7
%18 = call i64 @intern$binsearch(ptr %17)
store i64 %18, ptr %8
%19 = load i64, ptr %8
%20 = add i64 0, 0
%21 = icmp sge i64 %19, %20
br i1 %21, label %bb11, label %bb12
bb11:
%22 = load ptr, ptr @intern_vals
%23 = load i64, ptr %8
%24 = call i64 @tie_table_at_i64(ptr %22, i64 %23, ptr %9)
ret i64 %24
bb12:
br label %bb13
bb13:
%25 = load ptr, ptr @intern_pool
%26 = call i64 @tie_table_len(ptr %25)
store i64 %26, ptr %10
%27 = load ptr, ptr @intern_pool
%28 = load ptr, ptr %7
call void @tie_table_push_string(ptr %27, ptr %28)
%29 = add i64 0, 0
%30 = load ptr, ptr @intern_keys
%31 = call i64 @tie_table_len(ptr %30)
store i64 %31, ptr %11
%32 = add i64 0, 0
store i64 %32, ptr %12
%33 = load ptr, ptr @intern_keys
%34 = call i64 @tie_table_len(ptr %33)
%35 = add i64 0, 1
%36 = sub i64 %34, %35
store i64 %36, ptr %13
br label %bb14
bb14:
%37 = load i64, ptr %12
%38 = load i64, ptr %13
%39 = icmp sle i64 %37, %38
br i1 %39, label %bb15, label %bb16
bb15:
%40 = load i64, ptr %12
%41 = load i64, ptr %13
%42 = add i64 %40, %41
%43 = add i64 0, 2
%44 = sdiv i64 %42, %43
store i64 %44, ptr %14
%45 = load ptr, ptr @intern_keys
%46 = load i64, ptr %14
%47 = call ptr @tie_table_at_string(ptr %45, i64 %46, ptr %15)
%48 = load ptr, ptr %7
%49 = call i32 @strcmp(ptr %47, ptr %48)
%50 = icmp slt i32 %49, 0
br i1 %50, label %bb17, label %bb18
bb17:
%51 = load i64, ptr %14
%52 = add i64 0, 1
%53 = add i64 %51, %52
store i64 %53, ptr %12
br label %bb19
bb18:
%54 = load i64, ptr %14
store i64 %54, ptr %11
%55 = load i64, ptr %14
%56 = add i64 0, 1
%57 = sub i64 %55, %56
store i64 %57, ptr %13
br label %bb19
bb19:
br label %bb14
bb16:
%58 = load ptr, ptr @intern_keys
%59 = load ptr, ptr %7
call void @tie_table_push_string(ptr %58, ptr %59)
%60 = add i64 0, 0
%61 = load ptr, ptr @intern_vals
%62 = load i64, ptr %10
call void @tie_table_push_i64(ptr %61, i64 %62)
%63 = add i64 0, 0
%64 = load ptr, ptr @intern_keys
%65 = call i64 @tie_table_len(ptr %64)
%66 = add i64 0, 1
%67 = sub i64 %65, %66
store i64 %67, ptr %16
br label %bb20
bb20:
%68 = load i64, ptr %16
%69 = load i64, ptr %11
%70 = icmp sgt i64 %68, %69
br i1 %70, label %bb21, label %bb22
bb21:
%71 = load ptr, ptr @intern_keys
%72 = load i64, ptr %16
%73 = load ptr, ptr @intern_keys
%74 = load i64, ptr %16
%75 = add i64 0, 1
%76 = sub i64 %74, %75
%77 = alloca i8
%78 = call ptr @tie_table_at_string(ptr %73, i64 %76, ptr %77)
%79 = alloca i8
call void @tie_table_set_string(ptr %71, i64 %72, ptr %78, ptr %79)
%80 = load ptr, ptr @intern_vals
%81 = load i64, ptr %16
%82 = load ptr, ptr @intern_vals
%83 = load i64, ptr %16
%84 = add i64 0, 1
%85 = sub i64 %83, %84
%86 = alloca i8
%87 = call i64 @tie_table_at_i64(ptr %82, i64 %85, ptr %86)
%88 = alloca i8
call void @tie_table_set_i64(ptr %80, i64 %81, i64 %87, ptr %88)
%89 = load i64, ptr %16
%90 = add i64 0, 1
%91 = sub i64 %89, %90
store i64 %91, ptr %16
br label %bb20
bb22:
%92 = load ptr, ptr @intern_keys
%93 = load i64, ptr %11
%94 = load ptr, ptr %7
%95 = alloca i8
call void @tie_table_set_string(ptr %92, i64 %93, ptr %94, ptr %95)
%96 = load ptr, ptr @intern_vals
%97 = load i64, ptr %11
%98 = load i64, ptr %10
%99 = alloca i8
call void @tie_table_set_i64(ptr %96, i64 %97, i64 %98, ptr %99)
%100 = load i64, ptr %10
ret i64 %100
}

define ptr @intern$lookup(i64 %0) {
bb23:
%1 = alloca ptr
%2 = alloca i64
%3 = alloca i64
store i64 %0, ptr %3
%4 = load i64, ptr %3
%5 = add i64 0, 0
%6 = icmp slt i64 %4, %5
br i1 %6, label %bb24, label %bb25
bb24:
%7 = ptrtoint ptr @.str.0 to i64
ret ptr @.str.0
bb25:
br label %bb26
bb26:
%8 = load i64, ptr %3
%9 = load ptr, ptr @intern_pool
%10 = call i64 @tie_table_len(ptr %9)
%11 = icmp sge i64 %8, %10
br i1 %11, label %bb27, label %bb28
bb27:
%12 = ptrtoint ptr @.str.0 to i64
ret ptr @.str.0
bb28:
br label %bb29
bb29:
%13 = load ptr, ptr @intern_pool
%14 = load i64, ptr %3
%15 = alloca i8
%16 = call ptr @tie_table_at_string(ptr %13, i64 %14, ptr %15)
ret ptr %16
}

define i64 @intern$interned_len() {
bb30:
%0 = load ptr, ptr @intern_pool
%1 = call i64 @tie_table_len(ptr %0)
ret i64 %1
}


@.str.0 = private unnamed_addr constant [1 x i8] c"\00"
