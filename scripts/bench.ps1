# tie 编译器性能基准脚本（自举 v2 计划 T0.1）
#
# 职责：为 "tie 新编译器（tie 自写）vs 老 Rust 编译器" 建立可重复的计时基准。
#       语料为 scripts/bench/corpus.txt 冻结的 101 个 .tie 文件（pass/fail 标记），
#       两条计时通道：
#         a) 前端通道（frontend）：tie-frontend <file> --check  （仅词法/语法/语义）
#         b) 前端+IR 通道（frontend_ir）：tie-llvm <file> --emit-ir （前端 + IR 生成）
#       每文件预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0。
#
# 用法：
#   .\scripts\bench.ps1 --help                 # 打印用法
#   .\scripts\bench.ps1 baseline               # 生成 docs/bench/baseline-rust.json/.md
#   .\scripts\bench.ps1 gate1                  # 阶段 1 前端对照（tiec-proto vs tie-frontend，框架）
#   .\scripts\bench.ps1 gate4                  # 阶段 5 全链路对照（tiec vs tie-llvm，框架）
#
# 退出码：0 = 成功 / 1 = 失败（参数错误、exe 缺失、语料错误等）
#
# 前置：target\release\tie-frontend.exe 与 tie-llvm.exe（先 cargo build --release -p tie-frontend -p tie-llvm）

param(
    # 子命令：baseline / gate1 / gate4（--help/-h 由 $Help 开关捕获）
    [Parameter(Position = 0)]
    [string]$Command = "",
    # 帮助开关：--help / -h / -? / /? 均会绑定到此
    [switch]$Help
)

# 错误即停
$ErrorActionPreference = "Stop"

# ---- 常量 ----
# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot
# 语料清单
$CorpusFile = Join-Path $Root "scripts\bench\corpus.txt"
# 临时目录（Start-Process 输出重定向文件，避免污染终端）
$BenchTmp = Join-Path $env:TEMP "tie-bench"
# 老编译器（Rust）通道可执行文件——当前基线 = release 构建的 tie-frontend / tie-llvm
$FrontendExe = Join-Path $Root "target\release\tie-frontend.exe"
$LlvmExe     = Join-Path $Root "target\release\tie-llvm.exe"
# tie 新编译器（阶段 1 原型 / 阶段 5 完整编译器，未构建时跳过）
$ProtoExe    = Join-Path $Root "compiler\proto\tiec-proto.exe"
$TiecExe     = Join-Path $Root "compiler\tiec.exe"
# 测量参数
$RunsPerFile = 5   # 每文件计时运行次数（取中位数）
$WarmupRuns  = 1   # 预热次数（不计时，排除启动噪声）
# 子命令 → 输出文件前缀（docs/bench/<前缀>.json / <前缀>.md）
$OutPrefixMap = @{
    baseline = "baseline-rust"
    gate1    = "phase1"
    gate4    = "phase5"
}

# 确保临时目录存在（Start-Process 输出重定向要求父目录已存在）
New-Item -ItemType Directory -Path $BenchTmp -Force | Out-Null

# ---- 内部函数 ----

# 固定当前进程（以及所有子进程）到核心 0。
# 说明：PowerShell 7 的 Start-Process 已移除 -Affinity 参数（5.1 才有），
# 故改用 kernel32 SetProcessAffinityMask 将整个基准进程钉在核心 0，
# 通过 & / Start-Process 启动的编译器子进程自动继承该亲和性，等效于取核心 0 固定。
function Set-Core0Affinity {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class TieBenchAffinity {
    [DllImport("kernel32.dll")]
    public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")]
    public static extern bool SetProcessAffinityMask(IntPtr hProcess, IntPtr dwProcessAffinityMask);
}
"@ -ErrorAction SilentlyContinue
    # 掩码 0x1 = 仅核心 0；核心数 >1 时固定，单核机器上掩码仍是 1（无副作用）
    return [TieBenchAffinity]::SetProcessAffinityMask(
        [TieBenchAffinity]::GetCurrentProcess(), [IntPtr]1)
}

# 获取 CPU 型号（报告与 JSON 记录用）
function Get-CpuModel {
    return (Get-CimInstance Win32_Processor).Name
}

# 获取当前机器名
function Get-MachineName {
    return $env:COMPUTERNAME
}

# 读取语料清单，返回 [{ Path, Role }]；跳过空行与 '#' 注释行
function Read-Corpus {
    $items = @()
    foreach ($line in [System.IO.File]::ReadAllLines($CorpusFile)) {
        $trimmed = $line.Trim()
        if ($trimmed -eq "" -or $trimmed.StartsWith("#")) { continue }
        $parts = $trimmed -split '\s+'
        if ($parts.Count -lt 2) {
            throw "语料行格式错误: '$line'（应为 '<路径> <pass|fail>'）"
        }
        if ($parts[1] -notin @("pass", "fail")) {
            throw "语料行标记错误: '$line'（标记只能为 pass 或 fail）"
        }
        $items += [PSCustomObject]@{ Path = $parts[0]; Role = $parts[1] }
    }
    return $items
}

# 取中位数：排序后取中间值（本脚本固定 5 次采样）
function Get-Median {
    param([double[]]$Values)
    $sorted = @($Values | Sort-Object)
    return $sorted[[math]::Floor($sorted.Count / 2)]
}

# 单次运行一条通道：隐藏窗口 + 输出重定向到临时文件（排除终端渲染/启动噪声），
# 用 Measure-Command 计时，返回 { ExitCode, Ms }。
# 注意：Start-Process 每次以覆盖模式打开重定向文件，串行运行无冲突。
function Invoke-LaneOnce {
    param(
        [string]$Exe,
        [string[]]$ArgsList
    )
    $outFile = Join-Path $BenchTmp "out.txt"
    $errFile = Join-Path $BenchTmp "err.txt"
    # 清理上次残留，避免句柄占用
    Remove-Item $outFile, $errFile -Force -ErrorAction SilentlyContinue
    $script:LaneExitCode = $null
    $m = Measure-Command {
        $proc = Start-Process -FilePath $Exe -ArgumentList $ArgsList `
            -WindowStyle Hidden `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile -PassThru
        $proc.WaitForExit()
        $script:LaneExitCode = $proc.ExitCode
    }
    return [PSCustomObject]@{ ExitCode = $script:LaneExitCode; Ms = $m.TotalMilliseconds }
}

# 单文件单通道测量：预热 1 次（不计时）+ 5 次计时取中位数，
# 返回 { ExitCode, MedianMs }。ExitCode 取最后一次运行的退出码。
function Measure-Lane {
    param(
        [string]$Exe,
        [string[]]$ArgsList
    )
    # 预热：先跑 1 次，加载编译器/缓存，不计时
    for ($i = 0; $i -lt $WarmupRuns; $i++) {
        $null = Invoke-LaneOnce -Exe $Exe -ArgsList $ArgsList
    }
    $samples = @()
    $exitCode = $null
    for ($i = 0; $i -lt $RunsPerFile; $i++) {
        $r = Invoke-LaneOnce -Exe $Exe -ArgsList $ArgsList
        $samples += $r.Ms
        $exitCode = $r.ExitCode
    }
    return [PSCustomObject]@{ ExitCode = $exitCode; MedianMs = Get-Median -Values $samples }
}

# 以 UTF-8 无 BOM 写文本文件（Windows 默认 UTF-16，需显式指定）
function Write-Utf8NoBom {
    param([string]$Path, [string]$Content)
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

# ---- 子命令实现 ----

# baseline：对全部语料跑两条通道，生成 docs/bench/baseline-rust.json/.md（Rust 基线快照）
function Invoke-Baseline {
    Write-Host "[bench] 子命令: baseline（Rust 编译器基线快照）" -ForegroundColor Cyan

    # 前置检查：两个通道 exe 必须存在，否则提示先构建
    if (-not (Test-Path $FrontendExe)) {
        Write-Host "[bench] 错误: 未找到 tie-frontend: $FrontendExe" -ForegroundColor Red
        Write-Host "[bench] 请先运行: cargo build --release -p tie-frontend" -ForegroundColor Yellow
        exit 1
    }
    if (-not (Test-Path $LlvmExe)) {
        Write-Host "[bench] 错误: 未找到 tie-llvm: $LlvmExe" -ForegroundColor Red
        Write-Host "[bench] 请先运行: cargo build --release -p tie-llvm" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "[bench] 前端通道: $FrontendExe" -ForegroundColor DarkGray
    Write-Host "[bench] 前端+IR 通道: $LlvmExe" -ForegroundColor DarkGray

    # CPU 固定到核心 0
    $affinityOk = Set-Core0Affinity
    Write-Host "[bench] CPU 固定核心 0: $affinityOk" -ForegroundColor DarkGray

    # 读取语料
    $corpus = Read-Corpus
    $total = $corpus.Count
    Write-Host "[bench] 语料: $CorpusFile（$total 个文件）" -ForegroundColor DarkGray

    # 逐文件测量
    $files = @()
    # 记录 --emit-ir 通道在输入同目录生成的 .ll（driver 按输入名写 .ll），
    # 函数结束时统一删除，避免基准产物散落源目录（见下方 Cleanup-GeneratedLl）
    $generatedLl = @()
    $index = 0
    foreach ($item in $corpus) {
        $index++
        $relPath = $item.Path
        $absPath = Join-Path $Root $relPath
        if (-not (Test-Path $absPath)) {
            throw "语料文件不存在: $absPath（请检查 corpus.txt 与仓库状态一致）"
        }

        $role = $item.Role
        if ($role -eq "pass") {
            # pass 文件：两条通道各预热 1 + 计时 5 取中位数
            Write-Host ("[{0}/{1}] {2} ..." -f $index, $total, $relPath) -ForegroundColor DarkGray
            $fe = Measure-Lane -Exe $FrontendExe -ArgsList @($absPath, "--check")
            $ir = Measure-Lane -Exe $LlvmExe -ArgsList @($absPath, "--emit-ir")
            $generatedLl += [System.IO.Path]::ChangeExtension($absPath, '.ll')
            $files += [PSCustomObject]@{
                path                  = $relPath
                role                  = "pass"
                frontend_ms           = [math]::Round($fe.MedianMs, 1)
                frontend_ir_ms        = [math]::Round($ir.MedianMs, 1)
                exit_code             = $fe.ExitCode
                frontend_ir_exit_code = $ir.ExitCode
                skipped_for_time      = $false
            }
        }
        else {
            # fail 文件（oop_neg_* 等负例）：只跑 1 次记录退出码，不参与耗时对比
            Write-Host ("[{0}/{1}] {2} (fail，仅退出码) ..." -f $index, $total, $relPath) -ForegroundColor DarkGray
            $fe = Invoke-LaneOnce -Exe $FrontendExe -ArgsList @($absPath, "--check")
            $ir = Invoke-LaneOnce -Exe $LlvmExe -ArgsList @($absPath, "--emit-ir")
            $generatedLl += [System.IO.Path]::ChangeExtension($absPath, '.ll')
            $files += [PSCustomObject]@{
                path                  = $relPath
                role                  = "fail"
                frontend_ms           = $null
                frontend_ir_ms        = $null
                exit_code             = $fe.ExitCode
                frontend_ir_exit_code = $ir.ExitCode
                skipped_for_time      = $true
            }
        }
    }

    # 汇总：仅 pass 文件计入耗时；fail 文件只统计数量
    $passFiles = @($files | Where-Object { $_.role -eq "pass" })
    $failFiles = @($files | Where-Object { $_.role -eq "fail" })
    $feSum  = ($passFiles | Measure-Object -Property frontend_ms -Sum).Sum
    $irSum  = ($passFiles | Measure-Object -Property frontend_ir_ms -Sum).Sum
    $feMax  = ($passFiles | Measure-Object -Property frontend_ms -Maximum).Maximum
    $irMax  = ($passFiles | Measure-Object -Property frontend_ir_ms -Maximum).Maximum
    $unexpected = @($passFiles | Where-Object { $_.frontend_ir_exit_code -ne 0 })
    # --check 通道对带 import 的文件不适用（tie-frontend --check 不做 import 展开，
    # 34 个 import 依赖文件预期 exit≠0），单独统计供 G1 判定参考可对比样本
    $checkOk = @($passFiles | Where-Object { $_.exit_code -eq 0 })
    $totals = [PSCustomObject]@{
        pass_count             = $passFiles.Count
        fail_count             = $failFiles.Count
        timed_count            = $passFiles.Count
        check_success_count    = $checkOk.Count
        frontend_total_ms      = [math]::Round($feSum, 1)
        frontend_ir_total_ms   = [math]::Round($irSum, 1)
        frontend_median_ms     = [math]::Round((Get-Median -Values @($passFiles | ForEach-Object { $_.frontend_ms })), 1)
        frontend_ir_median_ms  = [math]::Round((Get-Median -Values @($passFiles | ForEach-Object { $_.frontend_ir_ms })), 1)
        frontend_max_ms        = [math]::Round($feMax, 1)
        frontend_ir_max_ms     = [math]::Round($irMax, 1)
        unexpected_exit_count  = $unexpected.Count
    }

    # 组装 JSON（PSCustomObject 保证字段顺序稳定）
    $now = Get-Date
    $report = [PSCustomObject]@{
        generated_at       = $now.ToString("o")
        cpu                = Get-CpuModel
        machine            = Get-MachineName
        corpus_total       = $total
        corpus_file        = "scripts/bench/corpus.txt"
        frontend_exe       = $FrontendExe
        frontend_ir_exe    = $LlvmExe
        runs_per_file      = $RunsPerFile
        warmup_runs        = $WarmupRuns
        files              = $files
        totals             = $totals
    }
    $prefix = $OutPrefixMap["baseline"]
    $jsonPath = Join-Path $Root "docs\bench\$prefix.json"
    $mdPath   = Join-Path $Root "docs\bench\$prefix.md"
    Write-Utf8NoBom -Path $jsonPath -Content ($report | ConvertTo-Json -Depth 5)
    Write-Host "[bench] 已写: $jsonPath" -ForegroundColor Green

    # 生成 Markdown 报告
    $lines = @(
        "# tie 编译器 Rust 基线基准（$prefix）",
        "",
        "- 生成时间: $($now.ToString('yyyy-MM-dd HH:mm:ss'))",
        "- CPU: $($report.cpu)",
        "- 机器: $($report.machine)",
        "- 语料文件数: $total（pass $($passFiles.Count) / fail $($failFiles.Count)）",
        "- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数，CPU 固定核心 0",
        '- 前端通道: `tie-frontend <file> --check`',
        '- 前端+IR 通道: `tie-llvm <file> --emit-ir`',
        "",
        "## 汇总",
        "",
        "| 指标 | 前端 --check | 前端+IR --emit-ir |",
        "| --- | ---: | ---: |",
        ("| 总耗时 (ms) | {0} | {1} |" -f $totals.frontend_total_ms, $totals.frontend_ir_total_ms),
        ("| 中位数 (ms) | {0} | {1} |" -f $totals.frontend_median_ms, $totals.frontend_ir_median_ms),
        ("| 最大单文件 (ms) | {0} | {1} |" -f $totals.frontend_max_ms, $totals.frontend_ir_max_ms),
        ("| 意外失败 (pass 文件 --emit-ir 退出码≠0) | {0} |" -f $totals.unexpected_exit_count),
        ("| --check 可成功样本 (import 无关) | {0} / {1} |" -f $totals.check_success_count, $totals.pass_count),
        "",
        "## 逐文件（耗时单位 ms）",
        "",
        "| 文件 | 角色 | 前端 | 前端+IR | 退出码 |",
        "| --- | --- | ---: | ---: | ---: |"
    )
    foreach ($f in $files) {
        $feMs  = if ($null -eq $f.frontend_ms)  { "-" } else { $f.frontend_ms }
        $irMs  = if ($null -eq $f.frontend_ir_ms) { "-" } else { $f.frontend_ir_ms }
        $flag  = if ($f.exit_code -ne 0 -and $f.role -eq "pass") { " ⚠" } else { "" }
        $lines += ("| {0} | {1} | {2} | {3} | {4}{5} |" -f $f.path, $f.role, $feMs, $irMs, $f.exit_code, $flag)
    }
    $lines += ""
    Write-Utf8NoBom -Path $mdPath -Content ($lines -join "`r`n")
    Write-Host "[bench] 已写: $mdPath" -ForegroundColor Green

    # 清理 --emit-ir 通道在输入同目录生成的 .ll（基准副产物，不落源目录）
    foreach ($llPath in $generatedLl) {
        Remove-Item $llPath -Force -ErrorAction SilentlyContinue
    }
    Write-Host "[bench] 已清理 $($generatedLl.Count) 个 --emit-ir 产物 .ll" -ForegroundColor DarkGray

    # 控制台汇总
    Write-Host "`n[bench] 完成" -ForegroundColor Green
    Write-Host "  corpus_total: $total（pass $($passFiles.Count) / fail $($failFiles.Count)）"
    Write-Host ("  前端 --check 总耗时: {0} ms" -f $totals.frontend_total_ms)
    Write-Host ("  前端+IR 总耗时:     {0} ms" -f $totals.frontend_ir_total_ms)
    if ($unexpected.Count -gt 0) {
        Write-Host "  警告: $($unexpected.Count) 个 pass 文件 --emit-ir 意外退出非 0：" -ForegroundColor DarkYellow
        foreach ($u in $unexpected) {
            Write-Host "    $($u.path) (exit=$($u.frontend_ir_exit_code))" -ForegroundColor DarkYellow
        }
    }
    exit 0
}

# gate1：阶段 1（T1.5 G1 闸门）tiec-proto --check vs tie-frontend --check 前端对照。
# 当前仅框架：tiec-proto 未构建时打印提示并跳过（退出 0），已构建时待阶段 1 填充。
function Invoke-Gate1 {
    Write-Host "[gate1] 阶段 1 前端对照（tiec-proto --check vs tie-frontend --check）" -ForegroundColor Cyan
    if (-not (Test-Path $ProtoExe)) {
        Write-Host "[gate1] 未找到 tiec-proto: $ProtoExe" -ForegroundColor DarkYellow
        Write-Host "[gate1] 阶段 1 原型尚未构建，跳过 gate1（退出 0）。" -ForegroundColor DarkGray
        exit 0
    }
    # TODO(阶段1 T1.5): 复用 Measure-Lane 分别测量 tiec-proto --check 与 tie-frontend --check，
    #   输出 docs/bench/phase1.json/.md；断言 total 比值 < 1.0（G1 硬闸门，目标 0.5–0.83）。
    Write-Host "[gate1] 检测到 tiec-proto，对照逻辑待阶段 1（T1.5）填充。" -ForegroundColor Yellow
    exit 0
}

# gate4：阶段 5（T5.1 G4 闸门）tiec --emit-ir vs tie-llvm --emit-ir 全链路对照。
# 当前仅框架：tiec 未构建时打印提示并跳过（退出 0），已构建时待阶段 5 填充。
function Invoke-Gate4 {
    Write-Host "[gate4] 阶段 5 全链路对照（tiec --emit-ir vs tie-llvm --emit-ir）" -ForegroundColor Cyan
    if (-not (Test-Path $TiecExe)) {
        Write-Host "[gate4] 未找到 tiec: $TiecExe" -ForegroundColor DarkYellow
        Write-Host "[gate4] 阶段 5 编译器尚未构建，跳过 gate4（退出 0）。" -ForegroundColor DarkGray
        exit 0
    }
    # TODO(阶段5 T5.1): 复用 Measure-Lane 分别测量 tiec --emit-ir 与 tie-llvm --emit-ir，
    #   对冻结语料输出 docs/bench/phase5.json/.md；断言 total 比值 < 1.0（G4 硬闸门，目标 0.5–0.83）。
    Write-Host "[gate4] 检测到 tiec，对照逻辑待阶段 5（T5.1）填充。" -ForegroundColor Yellow
    exit 0
}

# 打印用法
function Show-Help {
    Write-Host @"
tie 编译器性能基准脚本（自举 v2 计划 T0.1）

用法:
  .\scripts\bench.ps1 --help
  .\scripts\bench.ps1 baseline
  .\scripts\bench.ps1 gate1
  .\scripts\bench.ps1 gate4

子命令:
  baseline  对 scripts/bench/corpus.txt 中全部语料跑两条计时通道，生成
            docs/bench/baseline-rust.json 与 docs/bench/baseline-rust.md
            （Rust 编译器基线快照，供阶段 1/5 性能闸门对照）
            - 前端通道:   tie-frontend <file> --check
            - 前端+IR 通道: tie-llvm <file> --emit-ir
            - 计时: 每文件预热 1 次 + 5 次热运行取中位数（median-of-5），
              CPU 固定核心 0，Measure-Command 计时
            - fail 标记文件（oop_neg_* 等）只记录退出码，不参与耗时对比
  gate1     阶段 1 前端对照（tiec-proto --check vs tie-frontend --check）。
            tiec-proto 未构建时自动跳过（框架，逻辑待阶段 1 填充）
  gate4     阶段 5 全链路对照（tiec --emit-ir vs tie-llvm --emit-ir）。
            tiec 未构建时自动跳过（框架，逻辑待阶段 5 填充）
  --help    打印本帮助

退出码: 0 = 成功 / 1 = 失败（参数错误、编译器 exe 缺失、语料格式错误等）

前置: target\release\tie-frontend.exe 与 tie-llvm.exe 必须存在，否则先运行
  cargo build --release -p tie-frontend -p tie-llvm
"@
}

# ---- 主入口：参数分发 ----
# --help / -h / -? / /? 绑定到 $Help；无参数时 $Command 为空，同样打印帮助
if ($Help -or $Command -in @("", "--help", "-h", "-?", "/?")) {
    Show-Help
    exit 0
}

switch ($Command) {
    "baseline" { Invoke-Baseline }
    "gate1"    { Invoke-Gate1 }
    "gate4"    { Invoke-Gate4 }
    default {
        # 未知子命令 → 打印用法，退出 1
        Write-Host "[bench] 错误: 未知子命令 '$Command'" -ForegroundColor Red
        Show-Help
        exit 1
    }
}
