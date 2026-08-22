/* examples/lib_math_dyn/main.c —— M5 动态库 C 冒烟测试（dev33 批次 12）
 * 用 LoadLibrary + GetProcAddress 加载 tie 编译的 .dll，调用导出函数。
 * 符号名约定：命名空间全名转 $（mathdyn::add → "mathdyn$add"）。
 *
 * 构建（由 run.ps1 调用）：
 *   clang main.c -o main.exe          # 或用 cl / clang 任意 C 编译器
 * 运行：main.exe lib_math_dyn.dll
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

typedef long long int64;

typedef int64 (*Fn2)(int64, int64);
typedef int64 (*Fn1)(int64);

static int failures = 0;

static void expect_eq(const char *what, int64 got, int64 want) {
    if (got != want) {
        printf("FAIL %s: got %lld want %lld\n", what, got, want);
        failures++;
    } else {
        printf("PASS %s = %lld\n", what, got);
    }
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "用法: main.exe <lib_math_dyn.dll>\n");
        return 2;
    }
    HMODULE h = LoadLibraryA(argv[1]);
    if (!h) {
        fprintf(stderr, "LoadLibrary 失败: %lu\n", (unsigned long)GetLastError());
        return 1;
    }
    printf("LoadLibrary OK: %s\n", argv[1]);

    /* 导出面 = 命名空间 pub func（符号 mathdyn$add / mathdyn$mul / ...） */
    Fn2 add = (Fn2)GetProcAddress(h, "mathdyn$add");
    Fn2 mul = (Fn2)GetProcAddress(h, "mathdyn$mul");
    Fn2 sub = (Fn2)GetProcAddress(h, "mathdyn$sub");
    Fn2 max2 = (Fn2)GetProcAddress(h, "mathdyn$max2");
    Fn1 neg = (Fn1)GetProcAddress(h, "mathdyn$neg");
    Fn1 use_private = (Fn1)GetProcAddress(h, "mathdyn$use_private");
    /* 私有函数不导出——GetProcAddress 应失败 */
    Fn1 private_helper = (Fn1)GetProcAddress(h, "mathdyn$private_helper");

    if (!add || !mul || !sub || !max2 || !neg || !use_private) {
        fprintf(stderr, "GetProcAddress 失败（导出面不完整）\n");
        FreeLibrary(h);
        return 1;
    }
    printf("GetProcAddress OK（6 个 pub 导出符号全部解析）\n");

    expect_eq("add(2,3)", add(2, 3), 5);
    expect_eq("mul(6,7)", mul(6, 7), 42);
    expect_eq("sub(10,4)", sub(10, 4), 6);
    expect_eq("max2(9,4)", max2(9, 4), 9);
    expect_eq("max2(-3,5)", max2(-3, 5), 5);
    expect_eq("neg(7)", neg(7), -7);
    expect_eq("use_private(3)", use_private(3), 301);

    if (private_helper != NULL) {
        printf("FAIL 私有函数 mathdyn$private_helper 不应导出\n");
        failures++;
    } else {
        printf("PASS 私有函数未导出（GetProcAddress 返回 NULL）\n");
    }

    FreeLibrary(h);
    if (failures == 0) {
        printf("=== C 冒烟全部通过 ===\n");
        return 0;
    }
    printf("=== %d 项失败 ===\n", failures);
    return 1;
}
