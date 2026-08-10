# compare_tokens.ps1 —— T1.1 验收：tie 词法原型 vs Rust tie-frontend --tokens 全流对比
# ============================================================
# 用法：pwsh ./compiler/proto/compare_tokens.ps1 [文件...]
#   无参数：对内置语料（examples/std/tests 下 16 文件）逐一对比
#   带参数：只对比指定的文件
# 对比内容：
#   1) token 总数（首行「共 N 个 token（含 Eof）」一致）
#   2) 逐 token 的 (行, 列, 种类) 三元组一致（tag 映射回 Rust TokenKind 名）
#     ——lexeme 文本不参与对比（Rust 存解码值，原型存原始切片，语义等价）
# 输出：每文件 PASS/FAIL + 首个分歧行；末尾汇总。
# 退出码：全部 PASS 为 0，任一 FAIL 为 1。
# 注意：用 $args 接收文件参数（-File 方式下 param 数组绑定有兼容问题）。

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'

# lexer_test.exe 未构建时自动编译（自包含运行）
$exePath = "$PSScriptRoot/lexer_test.exe"
if (-not (Test-Path $exePath)) {
    & "$PSScriptRoot/../../target/release/tie-llvm.exe" "$PSScriptRoot/lexer_test.tie" -o $exePath 2>&1 | Out-Null
    if (-not (Test-Path $exePath)) {
        Write-Error "lexer_test.exe 构建失败，请先运行 target/release/tie-llvm.exe compiler/proto/lexer_test.tie"
    }
}

# tag 编号 → Rust TokenKind 名称（与 lexer.tie 文件头 tag 表一致）
$tagNames = @{
    0 = '整数'; 1 = '浮点'; 2 = '字符串'; 3 = '字符'; 4 = '标识符'
    5 = 'Func'; 6 = 'Var'; 7 = 'Const'; 8 = 'If'; 9 = 'Else'; 10 = 'While'
    11 = 'For'; 12 = 'In'; 13 = 'Return'; 14 = 'Break'; 15 = 'Continue'
    16 = 'Switch'; 17 = 'Case'; 18 = 'Default'; 19 = 'When'; 20 = 'Import'
    21 = 'As'; 22 = 'Struct'; 23 = 'Extends'; 24 = 'Namespace'; 25 = 'Pub'
    26 = 'Using'; 27 = 'Extern'; 28 = 'True'; 29 = 'False'; 30 = 'Zero'
    31 = 'Ref'; 32 = '类型'; 33 = 'LParen'; 34 = 'RParen'; 35 = 'LBrace'
    36 = 'RBrace'; 37 = 'LBracket'; 38 = 'RBracket'; 39 = 'Comma'
    40 = 'Colon'; 41 = 'DoubleColon'; 42 = 'Semi'; 43 = 'Dot'; 44 = 'DotDot'
    45 = 'Arrow'; 46 = 'Plus'; 47 = 'Minus'; 48 = 'Star'; 49 = 'Slash'
    50 = 'Percent'; 51 = 'Eq'; 52 = 'EqEq'; 53 = 'NotEq'; 54 = 'Lt'
    55 = 'Gt'; 56 = 'Le'; 57 = 'Ge'; 58 = 'AndAnd'; 59 = 'OrOr'; 60 = 'Bang'
    61 = 'PlusEq'; 62 = 'MinusEq'; 63 = 'StarEq'; 64 = 'SlashEq'
    65 = 'PercentEq'; 66 = 'AmpEq'; 67 = 'PipeEq'; 68 = 'CaretEq'
    69 = 'ShlEq'; 70 = 'ShrEq'; 71 = 'Amp'; 72 = 'Pipe'; 73 = 'Caret'
    74 = 'Shl'; 75 = 'Shr'; 76 = 'Inc'; 77 = 'Dec'; 78 = 'Question'
    79 = 'Eof'
}

if ($args.Count -eq 0) {
    $Files = @('examples/hello.tie', 'examples/table.tie', 'examples/switch.tie',
        'examples/oop.tie', 'examples/wide.tie', 'examples/char.tie',
        'examples/m4_ops.tie', 'std/string.tie', 'std/assert.tie',
        'std/csv.tie', 'std/sort.tie', 'std/math.tie', 'std/intern.tie',
        'tests/language/byref_table.tie', 'tests/language/global_table.tie',
        'tests/language/intern.tie')
} else {
    $Files = @($args)
}

# 解析 Rust --tokens 输出 → 行列表：@{line, col, kind}
function Parse-RustTokens([string]$File) {
    $raw = & "$PSScriptRoot/../../target/release/tie-frontend.exe" $File --tokens 2>&1 | Out-String
    $lines = $raw -split "`r?`n" | Where-Object { $_ -match '^\s+\d+:\d+' }
    $toks = foreach ($l in $lines) {
        if ($l -match '^\s+(\d+):(\d+)\s+(.+?)\s*$') {
            [pscustomobject]@{ line = [int]$matches[1]; col = [int]$matches[2]; kind = $matches[3].Trim() }
        }
    }
    return @($toks)
}

# 解析原型输出 → 行列表：@{line, col, kind}
function Parse-ProtoTokens([string]$File) {
    $raw = & "$PSScriptRoot/lexer_test.exe" $File 2>&1 | Out-String
    $lines = $raw -split "`r?`n" | Where-Object { $_ -match '^\s+\d+:\d+' }
    $toks = foreach ($l in $lines) {
        if ($l -match '^\s+(\d+):(\d+)\s+tag=(\d+)\s+') {
            $t = [int]$matches[3]
            $kind = if ($tagNames.ContainsKey($t)) { $tagNames[$t] } else { "未知$t" }
            [pscustomobject]@{ line = [int]$matches[1]; col = [int]$matches[2]; kind = $kind }
        }
    }
    return @($toks)
}

# Rust 种类名的规范化：去掉值部分（如「整数 42」「类型 'i64'」→ 只看种类词）
function Normalize-Kind([string]$k) {
    if ($k -match '^(整数|浮点|字符串|字符|标识符|类型)\b') { return $matches[1] }
    return $k
}

$pass = 0; $fail = 0
foreach ($f in $Files) {
    $rust = Parse-RustTokens $f
    $proto = Parse-ProtoTokens $f
    # 校正：原型 lexer_test 输出的行号列号与 Rust 一致；比较总数与三元组
    if ($rust.Count -ne $proto.Count) {
        Write-Host "FAIL $f : token 数不一致 Rust=$($rust.Count) 原型=$($proto.Count)"
        $fail++
        continue
    }
    $diff = $null
    for ($i = 0; $i -lt $rust.Count; $i++) {
        $r = Normalize-Kind $rust[$i].kind
        $p = $proto[$i].kind
        if ($rust[$i].line -ne $proto[$i].line -or $rust[$i].col -ne $proto[$i].col -or $r -ne $p) {
            $diff = "第 $i 个 token: Rust=[$($rust[$i].line):$($rust[$i].col) $r] 原型=[$($proto[$i].line):$($proto[$i].col) $p]"
            break
        }
    }
    if ($null -eq $diff) {
        Write-Host "PASS $f : $($rust.Count) 个 token 全流一致"
        $pass++
    } else {
        Write-Host "FAIL $f : $diff"
        $fail++
    }
}
Write-Host ""
Write-Host "汇总：PASS=$pass FAIL=$fail"
if ($fail -gt 0) { exit 1 }
exit 0
