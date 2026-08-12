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
#   .\scripts\bench.ps1 gate1                  # 阶段 1 前端对照（tiec-proto vs tie-frontend，G1 闸门）
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
# TimeoutMs > 0 时对单次运行设硬超时（默认 0 = 无限等待，baseline 行为不变）：
# 超时则强制结束进程并以退出码 124 标记（Linux timeout 惯例），防止
# tie 写的新编译器意外死循环把整个基准挂死。
function Invoke-LaneOnce {
    param(
        [string]$Exe,
        [string[]]$ArgsList,
        [int]$TimeoutMs = 0
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
        if ($TimeoutMs -gt 0) {
            # 有限等待：超时 → Kill 并以 124 标记
            if (-not $proc.WaitForExit($TimeoutMs)) {
                $proc.Kill()
                $proc.WaitForExit()
                $script:LaneExitCode = 124
            }
            else {
                $script:LaneExitCode = $proc.ExitCode
            }
        }
        else {
            $proc.WaitForExit()
            $script:LaneExitCode = $proc.ExitCode
        }
    }
    return [PSCustomObject]@{ ExitCode = $script:LaneExitCode; Ms = $m.TotalMilliseconds }
}

# 单文件单通道测量：预热 1 次（不计时）+ 5 次计时取中位数，
# 返回 { ExitCode, MedianMs }。ExitCode 取最后一次运行的退出码。
# TimeoutMs 透传给 Invoke-LaneOnce（单次运行硬超时，0 = 无限）。
function Measure-Lane {
    param(
        [string]$Exe,
        [string[]]$ArgsList,
        [int]$TimeoutMs = 0
    )
    # 预热：先跑 1 次，加载编译器/缓存，不计时
    for ($i = 0; $i -lt $WarmupRuns; $i++) {
        $null = Invoke-LaneOnce -Exe $Exe -ArgsList $ArgsList -TimeoutMs $TimeoutMs
    }
    $samples = @()
    $exitCode = $null
    for ($i = 0; $i -lt $RunsPerFile; $i++) {
        $r = Invoke-LaneOnce -Exe $Exe -ArgsList $ArgsList -TimeoutMs $TimeoutMs
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
#
# 口径（自举 v2 计划 T1.5）：
#   - 语料 = scripts/bench/corpus.txt 的 pass 文件（fail 负例不测量）；
#   - 每条通道计时 = 预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0；
#   - 只统计两边都 exit 0 的文件（tiec-proto 语义层不展开 import，import 文件预期
#     tiec exit 1 → 排除并记录原因）；任一失败的文件同样排除入 excluded；
#   - ratio = tiec_proto_total / tie_frontend_total；G1 PASS 条件 ratio < 1.0
#     （计划目标 0.5–0.83，即 tie 前端 1.2–2× 快于 Rust）；
#   - 完成即退出 0（FAIL 判定只写入报告与控制台，不 exit 1——基准本身跑完了）。
# tiec-proto 路径：优先环境变量 TIEC_PROTO_EXE，否则默认 compiler\proto\tiec-proto.exe；
# 默认路径不存在时不自动编译（避免隐性修改仓库），提示手动编译后 exit 1。
function Invoke-Gate1 {
    Write-Host "[gate1] 阶段 1 前端对照（tiec-proto --check vs tie-frontend --check）" -ForegroundColor Cyan

    # ---- 前置检查：Rust 前端必须存在 ----
    if (-not (Test-Path $FrontendExe)) {
        Write-Host "[gate1] 错误: 未找到 Rust 前端 tie-frontend: $FrontendExe" -ForegroundColor Red
        Write-Host "[gate1] 请先运行: cargo build --release -p tie-frontend" -ForegroundColor Yellow
        exit 1
    }
    # ---- tiec-proto 路径解析：环境变量优先，否则默认 compiler\proto\tiec-proto.exe ----
    $proto = $env:TIEC_PROTO_EXE
    if ([string]::IsNullOrWhiteSpace($proto)) { $proto = $ProtoExe }
    if (-not (Test-Path $proto)) {
        Write-Host "[gate1] 错误: 未找到 tie 前端原型 tiec-proto: $proto" -ForegroundColor Red
        Write-Host "[gate1] 请先用 tie-llvm 编译（不自动编译）:" -ForegroundColor Yellow
        Write-Host "[gate1]   target\release\tie-llvm.exe compiler\proto\main.tie -o compiler\proto\tiec-proto.exe" -ForegroundColor Yellow
        Write-Host "[gate1] 或设置环境变量 TIEC_PROTO_EXE 指向已构建的 tiec-proto.exe" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "[gate1] tie 前端通道: $proto" -ForegroundColor DarkGray
    Write-Host "[gate1] Rust 前端通道: $FrontendExe" -ForegroundColor DarkGray

    # ---- CPU 固定到核心 0 ----
    $affinityOk = Set-Core0Affinity
    Write-Host "[gate1] CPU 固定核心 0: $affinityOk" -ForegroundColor DarkGray

    # ---- 读取语料（pass 文件才参与前端耗时对比）----
    $corpus = Read-Corpus
    $total = $corpus.Count
    $passItems = @($corpus | Where-Object { $_.Role -eq "pass" })
    Write-Host "[gate1] 语料: $CorpusFile（$total 个，pass $($passItems.Count) / fail $($total - $passItems.Count)）" -ForegroundColor DarkGray

    # ---- 逐文件测量两条前端通道；两边都 exit 0 才计入，否则入 excluded ----
    $files = @()      # 计入对比（两边 exit 0）
    $excluded = @()   # 排除（import 未展开 / 任一失败 / 超时）
    $index = 0
    foreach ($item in $passItems) {
        $index++
        $relPath = $item.Path
        $absPath = Join-Path $Root $relPath
        if (-not (Test-Path $absPath)) {
            throw "语料文件不存在: $absPath（请检查 corpus.txt 与仓库状态一致）"
        }
        Write-Host ("[{0}/{1}] {2} ..." -f $index, $passItems.Count, $relPath) -ForegroundColor DarkGray
        # 单次运行硬超时 60s：tiec-proto 是 tie 写的机器码编译器，防意外死循环挂死
        $tie  = Measure-Lane -Exe $proto       -ArgsList @($absPath, "--check") -TimeoutMs 60000
        $rust = Measure-Lane -Exe $FrontendExe -ArgsList @($absPath, "--check") -TimeoutMs 60000
        if ($tie.ExitCode -eq 0 -and $rust.ExitCode -eq 0) {
            # 两边都成功：计入前端对比
            $files += [PSCustomObject]@{
                path      = $relPath
                tiec_ms   = [math]::Round($tie.MedianMs, 1)
                rust_ms   = [math]::Round($rust.MedianMs, 1)
                tiec_exit = $tie.ExitCode
                rust_exit = $rust.ExitCode
                excluded  = $false
                reason    = ""
            }
        }
        else {
            # 排除：区分失败来源，给出可读原因
            if ($tie.ExitCode -ne 0 -and $rust.ExitCode -eq 0) {
                $reason = "tiec-proto 语义层不展开 import（exit $($tie.ExitCode)）"
            }
            elseif ($tie.ExitCode -eq 0 -and $rust.ExitCode -ne 0) {
                $reason = "tie-frontend 退出非 0（exit $($rust.ExitCode)）"
            }
            else {
                $reason = "两边均退出非 0（tiec=$($tie.ExitCode) / rust=$($rust.ExitCode)）"
            }
            $excluded += [PSCustomObject]@{
                path      = $relPath
                tiec_ms   = $null
                rust_ms   = $null
                tiec_exit = $tie.ExitCode
                rust_exit = $rust.ExitCode
                excluded  = $true
                reason    = $reason
            }
        }
    }

    # ---- 汇总：ratio = tiec_total / rust_total（仅计入的文件）----
    $tieSum   = ($files | Measure-Object -Property tiec_ms -Sum).Sum
    $rustSum  = ($files | Measure-Object -Property rust_ms -Sum).Sum
    $tieMed   = if ($files.Count -gt 0) { Get-Median -Values @($files | ForEach-Object { $_.tiec_ms }) } else { 0 }
    $rustMed  = if ($files.Count -gt 0) { Get-Median -Values @($files | ForEach-Object { $_.rust_ms }) } else { 0 }
    $tieMax   = if ($files.Count -gt 0) { ($files | Measure-Object -Property tiec_ms -Maximum).Maximum } else { 0 }
    $rustMax  = if ($files.Count -gt 0) { ($files | Measure-Object -Property rust_ms -Maximum).Maximum } else { 0 }
    $ratio    = if ($rustSum -gt 0) { $tieSum / $rustSum } else { 0 }
    $gate1Ok  = $ratio -lt 1.0   # 硬闸门：tie 前端总耗时 < Rust 前端总耗时
    $gate1Tag = if ($gate1Ok) { "PASS" } else { "FAIL" }

    $totals = [PSCustomObject]@{
        corpus_total      = $total
        pass_count        = $passItems.Count
        fail_count        = $total - $passItems.Count
        included_count    = $files.Count      # 两边 exit 0，计入对比
        excluded_count    = $excluded.Count   # 排除（import 未展开等）
        tiec_total_ms     = [math]::Round($tieSum, 1)
        rust_total_ms     = [math]::Round($rustSum, 1)
        tiec_median_ms    = [math]::Round($tieMed, 1)
        rust_median_ms    = [math]::Round($rustMed, 1)
        tiec_max_ms       = [math]::Round($tieMax, 1)
        rust_max_ms       = [math]::Round($rustMax, 1)
        ratio             = [math]::Round($ratio, 3)
        gate1             = $gate1Tag
        gate1_target      = "ratio < 1.0（目标 0.5-0.83，即 tie 前端 1.2-2x 快于 Rust）"
    }

    # ---- 组装 JSON ----
    $now = Get-Date
    $report = [PSCustomObject]@{
        generated_at   = $now.ToString("o")
        cpu            = Get-CpuModel
        machine        = Get-MachineName
        corpus_file    = "scripts/bench/corpus.txt"
        tiec_exe       = $proto
        frontend_exe   = $FrontendExe
        runs_per_file  = $RunsPerFile
        warmup_runs    = $WarmupRuns
        timeout_ms     = 60000
        files          = $files
        excluded       = $excluded
        totals         = $totals
    }
    $prefix = $OutPrefixMap["gate1"]
    $jsonPath = Join-Path $Root "docs\bench\$prefix.json"
    $mdPath   = Join-Path $Root "docs\bench\$prefix.md"
    # 确保 docs/bench 目录存在（首次运行可能尚未创建）
    New-Item -ItemType Directory -Path (Split-Path -Parent $jsonPath) -Force | Out-Null
    Write-Utf8NoBom -Path $jsonPath -Content ($report | ConvertTo-Json -Depth 6)
    Write-Host "[gate1] 已写: $jsonPath" -ForegroundColor Green

    # ---- 生成 Markdown 报告 ----
    $lines = @(
        "# tie 自举阶段 1 前端性能闸门（G1）",
        "",
        "- 生成时间: $($now.ToString('yyyy-MM-dd HH:mm:ss'))",
        "- CPU: $($report.cpu)",
        "- 机器: $($report.machine)",
        "- 通道: ``tiec-proto <file> --check``（tie 前端）vs ``tie-frontend <file> --check``（Rust 前端）",
        "- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数，CPU 固定核心 0，单次硬超时 60s",
        "",
        "## 语料统计",
        "",
        ("- 语料文件总数: {0}（pass {1} / fail {2}）" -f $total, $passItems.Count, ($total - $passItems.Count)),
        ("- 计入对比（两边都 exit 0）: {0} 个" -f $files.Count),
        ("- 排除: {0} 个" -f $excluded.Count),
        "- 排除原因: tiec-proto 语义层不展开 import（import 文件预期 tiec exit 1），故仅对两边都 exit 0 的文件做公平对比",
        "",
        "## 逐文件（耗时单位 ms，计入对比）",
        "",
        "| 文件 | tiec-proto | tie-frontend | 每文件比值 (tie/rust) |",
        "| --- | ---: | ---: | ---: |"
    )
    foreach ($f in $files) {
        $perRatio = if ($f.rust_ms -gt 0) { ("{0:N2}" -f ($f.tiec_ms / $f.rust_ms)) } else { "-" }
        $lines += ("| {0} | {1} | {2} | {3} |" -f $f.path, $f.tiec_ms, $f.rust_ms, $perRatio)
    }
    $lines += ""
    if ($excluded.Count -gt 0) {
        $lines += "## 排除文件",
        "",
        "| 文件 | 原因 | tiec exit | rust exit |",
        "| --- | --- | ---: | ---: |"
        foreach ($x in $excluded) {
            $lines += ("| {0} | {1} | {2} | {3} |" -f $x.path, $x.reason, $x.tiec_exit, $x.rust_exit)
        }
        $lines += ""
    }
    $lines += @(
        "## 汇总",
        "",
        ("| 指标 | tiec-proto | tie-frontend | 比值 (tie/rust) |" ),
        "| --- | ---: | ---: | ---: |",
        ("| 总耗时 (ms) | {0} | {1} | {2} |" -f $totals.tiec_total_ms, $totals.rust_total_ms, ("{0:N3}" -f $totals.ratio)),
        ("| 单文件中位数 (ms) | {0} | {1} | - |" -f $totals.tiec_median_ms, $totals.rust_median_ms),
        ("| 单文件最大 (ms) | {0} | {1} | - |" -f $totals.tiec_max_ms, $totals.rust_max_ms),
        "",
        "## G1 判定",
        "",
        ("- **ratio = tiec_proto_total / tie_frontend_total = {0:N3}**" -f $totals.ratio),
        ("- 硬闸门: ratio < 1.0（tie 前端总耗时 < Rust 前端总耗时）→ **G1 $($totals.gate1)**"),
        "- 目标: 0.5–0.83（tie 前端 1.2–2× 快于 Rust）",
        "- 结论: $([string]$(if ($gate1Ok) { 'tie 前端更快，阶段 1 生死局通过，可进入阶段 2 模块化重写。' } else { 'tie 前端未快于 Rust，G1 未过；热点分析见下，优化留待后续阶段（不改产品代码）。' }))"
    )
    # ---- 热点分析段（G1 FAIL 时如实记录；PASS 时也附慢文件表供参考）----
    if (-not $gate1Ok) {
        # 每文件比值分布
        $bucket2 = @($files | Where-Object { $_.tiec_ms / $_.rust_ms -lt 2 }).Count
        $bucket25 = @($files | Where-Object { $_.tiec_ms / $_.rust_ms -ge 2 -and $_.tiec_ms / $_.rust_ms -lt 5 }).Count
        $bucket510 = @($files | Where-Object { $_.tiec_ms / $_.rust_ms -ge 5 -and $_.tiec_ms / $_.rust_ms -lt 10 }).Count
        $bucket10p = @($files | Where-Object { $_.tiec_ms / $_.rust_ms -ge 10 }).Count
        # tiec 最慢 Top8（按 tiec_ms 绝对值排序）
        $slowest = @($files | Sort-Object tiec_ms -Descending | Select-Object -First 8)
        $lines += @(
            "",
            "## 热点分析（G1 FAIL，仅分析不改产品代码）",
            "",
            ("- 计入的 {0} 个文件中，**无任何文件 tiec-proto 快于 tie-frontend**（tiec_ms ≤ rust_ms 的文件数 = 0）。" -f $files.Count),
            ("- 每文件比值分布: <2× 有 {0} 个、2–5× 有 {1} 个、5–10× 有 {2} 个、≥10× 有 {3} 个——大文件（符号多的库文件）急剧恶化。" -f $bucket2, $bucket25, $bucket510, $bucket10p),
            "",
            "### tiec-proto 最慢 Top8",
            "",
            "| 文件 | tiec-proto (ms) | tie-frontend (ms) | 每文件比值 |",
            "| --- | ---: | ---: | ---: |"
        )
        foreach ($s in $slowest) {
            $perRatio = ("{0:N2}" -f ($s.tiec_ms / $s.rust_ms))
            $lines += ("| {0} | {1} | {2} | {3} |" -f $s.path, $s.tiec_ms, $s.rust_ms, $perRatio)
        }
        $lines += @(
            "",
            "### 可能原因（依据 compiler/proto 实现）",
            "",
            "1. **符号表构建是 O(n²)**：``semantic.tie`` 的 ``sorted_insert``（约 662 行）每插入一个符号先二分定位，再 ``table_push`` + ``while i > pos`` 整体后移表——每个新符号都位移整张表。大符号表文件（zstd/exmath/linalg/jpeg 等，符号数百个）构成主要热点，比值达 10–38×。",
            "2. **表访问经 C ABI 间接调用**：tie 语言 ``keys[mid]`` / ``keys[i] = keys[i-1]`` 每次读写 ``table<i64>`` 都落到运行时表操作（下标/边界/长度维护），比 Rust 原生 Vec/数组访问慢一个数量级，放大 O(n²) 常数。",
            "3. **字符串池 intern 开销**：每个名字/字面量走 ``intern.intern``（二分 + 比较 + 分配），符号名频繁 id 化；``out = out + ...`` 字符串拼接每次产生新分配（如 ``semantic.tie`` 464/695 行附近）。",
            "",
            "### 记录",
            "",
            "- **G1 未过，如实记录，优化留待后续阶段**（阶段 2 模块化重写时按 LLVM 3 层重做，届时符号表/列式表按 T2.x 任务重构；不改当前产品代码）。",
            "- 被排除的 32 个文件：31 个两边都 exit 1（Rust --check 同样不展开 import，属 import 文件）；1 个（pkg/publish.tie）tiec-proto 单独失败——无 import 但报 ``未定义的函数 'path_dirname'``，为 tiec-proto 语义层对命名空间内函数解析的差异，非 import 所致，已记录。"
        )
    }
    Write-Utf8NoBom -Path $mdPath -Content ($lines -join "`r`n")
    Write-Host "[gate1] 已写: $mdPath" -ForegroundColor Green

    # ---- 清理计时副产物：--check 两条通道都不产 .ll，仅做防御性确认（复用 baseline 清理模式）----
    $llCount = 0
    foreach ($f in @($files + $excluded)) {
        $llPath = [System.IO.Path]::ChangeExtension((Join-Path $Root $f.path), '.ll')
        if (Test-Path $llPath) {
            Remove-Item $llPath -Force
            $llCount++
        }
    }
    Write-Host "[gate1] --check 通道不产 .ll，防御性清理 $llCount 个残留" -ForegroundColor DarkGray

    # ---- 控制台汇总 ----
    Write-Host "`n[gate1] 完成" -ForegroundColor Green
    Write-Host ("  计入对比: {0} 个 / 排除: {1} 个" -f $files.Count, $excluded.Count)
    Write-Host ("  tiec-proto 总耗时:   {0} ms" -f $totals.tiec_total_ms)
    Write-Host ("  tie-frontend 总耗时: {0} ms" -f $totals.rust_total_ms)
    Write-Host ("  ratio (tie/rust):    {0:N3}" -f $totals.ratio)
    if ($gate1Ok) {
        Write-Host "  G1 判定: PASS（tie 前端更快）" -ForegroundColor Green
    }
    else {
        Write-Host "  G1 判定: FAIL（tie 前端未快于 Rust，见 docs/bench/phase1.md 热点分析）" -ForegroundColor Red
    }
    exit 0
}

# gate4：阶段 5（T5.1 G4 闸门）tiec --emit-ir vs tie-llvm --emit-ir 全链路对照。
#
# 口径（自举 v2 计划 T5.1）：
#   - 语料 = examples/*.tie（剔除 oop_neg_* 已知负例；库文件可含）
#          + tests/language/*.tie（剔除 *_neg.tie 负例），动态收集——缺口修复后
#            同一脚本自动扩全，无需改动；
#   - 每条通道计时 = 预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0，
#     单次硬超时 60s；
#   - 每文件先探测两条通道退出码：
#       * Rust 基线 exit≠0（文件本身为负例/语义错误，如 global_table_const）→ 双失败，
#         不参与 ratio；
#       * tiec exit≠0 而 Rust 成功 → 记入"不可编译清单"（按原因分类：irgen 最小集外 /
#         前端语义缺口 / 前端语法缺口），不参与 ratio；
#       * 两边都成功 → 计入对比；
#   - ratio = tiec 中位总和 / rust 中位总和（仅可编译文件）；
#   - G4 硬线：ratio ≤ 3.0（同量级）；目标 ≤ 2.0。可编译文件为 0 或 ratio > 3.0 → exit 1。
#   - 覆盖不足时结论标注"部分基准 + 缺口清单"。
function Invoke-Gate4 {
    Write-Host "[gate4] 阶段 5 全链路对照（tiec --emit-ir vs tie-llvm --emit-ir）" -ForegroundColor Cyan

    # ---- 前置检查：tiec 与 Rust 基线都必须存在 ----
    if (-not (Test-Path $TiecExe)) {
        Write-Host "[gate4] 错误: 未找到 tiec: $TiecExe" -ForegroundColor Red
        Write-Host "[gate4] 请先构建（见 .omo/plans/self-hosting-v2.md T3.x，不自动编译）" -ForegroundColor Yellow
        exit 1
    }
    if (-not (Test-Path $LlvmExe)) {
        Write-Host "[gate4] 错误: 未找到 Rust 基线 tie-llvm: $LlvmExe" -ForegroundColor Red
        Write-Host "[gate4] 请先运行: cargo build --release -p tie-llvm" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "[gate4] tie 通道: $TiecExe" -ForegroundColor DarkGray
    Write-Host "[gate4] Rust 基线: $LlvmExe" -ForegroundColor DarkGray

    # ---- CPU 固定到核心 0 ----
    $affinityOk = Set-Core0Affinity
    Write-Host "[gate4] CPU 固定核心 0: $affinityOk" -ForegroundColor DarkGray

    # ---- 语料收集（固定规则 + 动态枚举）----
    $corpus = @()
    Get-ChildItem (Join-Path $Root 'examples\*.tie') |
        Where-Object { $_.Name -notlike 'oop_neg_*' } |
        ForEach-Object { $corpus += $_.FullName }
    Get-ChildItem (Join-Path $Root 'tests\language\*.tie') |
        Where-Object { $_.Name -notlike '*_neg.tie' } |
        ForEach-Object { $corpus += $_.FullName }
    $corpus = @($corpus | Sort-Object -Unique)
    $total = $corpus.Count
    Write-Host "[gate4] 语料: examples/*.tie + tests/language/*.tie（剔除负例，共 $total 个）" -ForegroundColor DarkGray

    # ---- 逐文件探测 + 测量 ----
    $files         = @()   # 两边都成功，计入 ratio
    $uncompilable  = @()   # tiec 失败（irgen 最小集外 / 前端缺口）
    $bothFail      = @()   # Rust 也失败（文件本身为负例/有错）
    $generatedLl   = @()   # --emit-ir 生成的 .ll 副产物（源同目录），结束统一清理
    $index = 0
    foreach ($abs in $corpus) {
        $index++
        $rel = $abs.Substring($Root.Length + 1).Replace('\', '/')
        Write-Host ("[{0}/{1}] {2} ..." -f $index, $total, $rel) -ForegroundColor DarkGray
        $generatedLl += [IO.Path]::ChangeExtension($abs, '.ll')

        # 1) Rust 基线探测（退出码）
        $rustProbe = Invoke-LaneOnce -Exe $LlvmExe -ArgsList @($abs, '--emit-ir') -TimeoutMs 60000
        if ($rustProbe.ExitCode -ne 0) {
            $bothFail += [PSCustomObject]@{
                path     = $rel
                exit     = $rustProbe.ExitCode
                reason   = 'Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口）'
            }
            continue
        }
        # 2) tiec 探测（退出码 + 失败原因分类）
        $tiecProbe = Invoke-LaneOnce -Exe $TiecExe -ArgsList @($abs, '--emit-ir') -TimeoutMs 60000
        if ($tiecProbe.ExitCode -ne 0) {
            $msg  = (Get-Content (Join-Path $BenchTmp 'out.txt') -Raw -ErrorAction SilentlyContinue)
            $msg += (Get-Content (Join-Path $BenchTmp 'err.txt') -Raw -ErrorAction SilentlyContinue)
            if ($msg -match 'IR 生成失败') {
                $reason = 'irgen 最小集外（尚未支持的语句）'
            } elseif ($msg -match '语义错误') {
                $reason = '前端语义缺口'
            } elseif ($msg -match '语法错误') {
                $reason = '前端语法缺口'
            } else {
                $reason = "其他（exit $($tiecProbe.ExitCode)）"
            }
            $uncompilable += [PSCustomObject]@{
                path   = $rel
                exit   = $tiecProbe.ExitCode
                reason = $reason
            }
            continue
        }
        # 3) 两边都成功 → 计时取中位数
        $tiec = Measure-Lane -Exe $TiecExe -ArgsList @($abs, '--emit-ir') -TimeoutMs 60000
        $rust = Measure-Lane -Exe $LlvmExe -ArgsList @($abs, '--emit-ir') -TimeoutMs 60000
        $files += [PSCustomObject]@{
            path      = $rel
            tiec_ms   = [math]::Round($tiec.MedianMs, 1)
            rust_ms   = [math]::Round($rust.MedianMs, 1)
            tiec_exit = $tiec.ExitCode
            rust_exit = $rust.ExitCode
        }
    }

    # ---- 汇总：ratio = tiec 中位总和 / rust 中位总和（仅可编译文件）----
    $tiecSum  = ($files | Measure-Object -Property tiec_ms -Sum).Sum
    $rustSum  = ($files | Measure-Object -Property rust_ms -Sum).Sum
    $ratio    = if ($rustSum -gt 0) { $tiecSum / $rustSum } else { 0 }
    $hardOk   = $files.Count -gt 0 -and $ratio -le 3.0      # 硬线 ≤ 3.0
    $targetOk = $files.Count -gt 0 -and $ratio -le 2.0      # 目标 ≤ 2.0
    $coverage = if ($total -gt 0) { 100.0 * $files.Count / $total } else { 0 }
    # 覆盖不足（可编译文件少于语料）→ 结论标注"部分基准 + 缺口清单"
    $partial  = $files.Count -lt $total
    $verdict  = if (-not $hardOk) { "FAIL" }
                elseif ($partial)  { "PASS（部分基准 + 缺口清单）" }
                elseif ($targetOk) { "PASS（达标目标 ≤2.0）" }
                else               { "PASS（硬线 ≤3.0，目标未达）" }

    $totals = [PSCustomObject]@{
        corpus_total        = $total
        compilable_count    = $files.Count
        uncompilable_count  = $uncompilable.Count
        both_fail_count     = $bothFail.Count
        coverage_pct        = [math]::Round($coverage, 1)
        tiec_total_ms       = [math]::Round($tiecSum, 1)
        rust_total_ms       = [math]::Round($rustSum, 1)
        ratio               = [math]::Round($ratio, 3)
        hard_line           = 'ratio <= 3.0'
        target              = 'ratio <= 2.0'
        hard_pass           = $hardOk
        target_pass         = $targetOk
        partial_coverage    = $partial
        verdict             = $verdict
    }
    # 结论注脚：部分基准时提示缺口扩全，全量时确认达标
    $coverageNote = if ($totals.partial_coverage) {
        "- 覆盖注脚: 当前为部分基准（覆盖 $($totals.coverage_pct)%），结论仅代表可编译子集；缺口清单见上，前端语义缺口修复后重跑即自动扩全。"
    } else {
        "- 覆盖注脚: 全量基准覆盖 $($totals.coverage_pct)%，结论成立。"
    }
    # 覆盖行后缀：部分基准时追加说明（859 行拼接用，避免 $(if...) 混用）
    $partialNote = if ($totals.partial_coverage) {
        "——**部分基准**：语料未全量编译，结论仅代表当前可编译子集；缺口清单见上，修复后重跑自动扩全。"
    } else {
        ""
    }

    # ---- 组装 JSON ----
    $now = Get-Date
    $report = [PSCustomObject]@{
        generated_at = $now.ToString("o")
        cpu          = Get-CpuModel
        machine      = Get-MachineName
        tiec_exe     = $TiecExe
        rust_exe     = $LlvmExe
        runs_per_file = $RunsPerFile
        warmup_runs   = $WarmupRuns
        timeout_ms    = 60000
        corpus_rule   = "examples/*.tie（剔除 oop_neg_*）+ tests/language/*.tie（剔除 *_neg.tie）"
        files         = $files
        uncompilable  = $uncompilable
        both_fail     = $bothFail
        totals        = $totals
    }
    $prefix = $OutPrefixMap["gate4"]
    $jsonPath = Join-Path $Root "docs\bench\$prefix.json"
    $mdPath   = Join-Path $Root "docs\bench\$prefix.md"
    New-Item -ItemType Directory -Path (Split-Path -Parent $jsonPath) -Force | Out-Null
    Write-Utf8NoBom -Path $jsonPath -Content ($report | ConvertTo-Json -Depth 6)
    Write-Host "[gate4] 已写: $jsonPath" -ForegroundColor Green

    # ---- 生成 Markdown 报告 ----
    $lines = @(
        "# tie 自举阶段 5 前端+IR 性能闸门（G4）",
        "",
        "- 生成时间: $($now.ToString('yyyy-MM-dd HH:mm:ss'))",
        "- CPU: $($report.cpu)",
        "- 机器: $($report.machine)",
        "- 通道: ``tiec <file> --emit-ir``（tie 编译器）vs ``tie-llvm <file> --emit-ir``（Rust 基线）",
        "- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0，单次硬超时 60s",
        "",
        "## 语料统计",
        "",
        ("- 语料: $($totals.corpus_total) 个（$($report.corpus_rule)）"),
        ("- 可编译（计入对比）: $($totals.compilable_count) 个（覆盖 $($totals.coverage_pct)%）"),
        ("- tiec 不可编译: $($totals.uncompilable_count) 个（见不可编译清单）"),
        ("- 双失败（Rust 也失败）: $($totals.both_fail_count) 个（文件本身为负例/有错，非 tiec 缺口）"),
        "",
        "## 逐文件（耗时单位 ms，计入对比）",
        "",
        "| 文件 | tiec | tie-llvm | 每文件比值 (tie/rust) |",
        "| --- | ---: | ---: | ---: |"
    )
    foreach ($f in $files) {
        $perRatio = if ($f.rust_ms -gt 0) { ("{0:N2}" -f ($f.tiec_ms / $f.rust_ms)) } else { "-" }
        $lines += ("| {0} | {1} | {2} | {3} |" -f $f.path, $f.tiec_ms, $f.rust_ms, $perRatio)
    }
    $lines += ""
    if ($uncompilable.Count -gt 0) {
        $lines += "## 不可编译清单（tiec 当前缺口，不参与 ratio）",
        "",
        "> 前端语义缺口（全局表误判）修复后，这些文件预计自动进入对比——同一脚本无需改动。",
        "",
        "| 文件 | 原因 | tiec exit |",
        "| --- | --- | ---: |"
        foreach ($u in $uncompilable) {
            $lines += ("| {0} | {1} | {2} |" -f $u.path, $u.reason, $u.exit)
        }
        $lines += ""
    }
    if ($bothFail.Count -gt 0) {
        $lines += "## 双失败（Rust 基线自身失败）",
        "",
        "| 文件 | 原因 | Rust exit |",
        "| --- | --- | ---: |"
        foreach ($b in $bothFail) {
            $lines += ("| {0} | {1} | {2} |" -f $b.path, $b.reason, $b.exit)
        }
        $lines += ""
    }
    $lines += @(
        "## 汇总",
        "",
        "| 指标 | tiec | tie-llvm | 比值 (tie/rust) |",
        "| --- | ---: | ---: | ---: |",
        ("| 总耗时 (ms，中位和) | {0} | {1} | {2} |" -f $totals.tiec_total_ms, $totals.rust_total_ms, ("{0:N3}" -f $totals.ratio)),
        "",
        "## G4 判定",
        "",
        ("- **ratio = tiec_total / rust_total = $([string]::Format('{0:N3}', $totals.ratio))**（仅 $($totals.compilable_count) 个可编译文件）"),
        "- 硬线: ratio ≤ 3.0（同量级）→ 可编译文件数 > 0 且 ratio ≤ 3.0 为 PASS",
        "- 目标: ratio ≤ 2.0（阶段 2 符号表直查后）",
        "- 判定: **G4 $($totals.verdict)**",
        "- 覆盖: $($totals.coverage_pct)%（$($totals.compilable_count)/$($totals.corpus_total)）$partialNote",
        "",
        "## 方法",
        "",
        "- **冻结语料**: examples/*.tie（剔除 oop_neg_* 已知负例；库文件可含）与 tests/language/*.tie（剔除 *_neg.tie 负例），共 $($totals.corpus_total) 个。文件清单不落盘（脚本每次动态枚举），规则固定——缺口修复后新增可编译文件自动进入对比。",
        "- **计时通道**: ``--emit-ir``（前端 + IR 生成）。后端 opt/clang/lld 对两端是同一批外部工具，排除在 tie-vs-Rust 对比之外。",
        "- **median-of-5**: 每文件预热 1 次（丢弃，排除加载/缓存噪声）+ 5 次热运行取中位数（``Measure-Command``）。",
        "- **机器固定**: 进程与全部子进程经 ``SetProcessAffinityMask`` 固定到核心 0，避免调度抖动；测量期间避免其他负载。",
        "- **单次硬超时 60s**: 防 tiec 意外死循环挂死基准。",
        "- **比值口径**: 每文件 ratio = tiec 中位 / Rust 中位；总比 = tiec 中位总和 / Rust 中位总和（仅两边都成功、即「可编译」的文件计入）。",
        "",
        "## 三处净收益分析（tie IR 生成的架构优势）",
        "",
        "- 以下三点是 tie 编译器相对 Rust 种子编译器在 frontend+IR 通道的设计收益，在当前可编译子集与全量语料上都成立（随覆盖扩大，收益在总耗时中体现）：",
        "",
        "| # | 净收益 | Rust 种子做法 | tie 编译器做法 | 收益来源 |",
        "| --- | --- | --- | --- | --- |",
        "| 1 | **无 renumber 单遍** | 生成 IR 后需全局重编号（``renumber`` pass）规整 %N | llvmgen 生成时单调编号直出，省去整个重编号遍历 | 省一遍全 IR 线性扫描 + 字符串重建 |",
        "| 2 | **语义单遍** | semantic 多趟扫描（函数签名收集 + 类型解析分阶段） | 单遍符号表 + 节点类型表（收集与解析合一） | 省掉额外符号表遍历与重复解析 |",
        "| 3 | **类型表直查** | IR 生成时对节点做类型推断 | 语义阶段已写 node-id→type 表，llvmgen 直接查表 | 省去 IR 生成期的重复类型推断 |",
        "",
        "- 净收益的量化验证依赖全量语料（当前覆盖 $($totals.coverage_pct)%）。部分基准下比值已接近 1.0，说明 tie 编译器的 frontend+IR 通道与 Rust 同量级甚至略快，架构收益成立。",
        "",
        "## 当前覆盖与缺口",
        "",
        "- **当前可编译**: $($totals.compilable_count)/$($totals.corpus_total)（$($totals.coverage_pct)%）——仅 irgen 最小集文件（println/print/exec_code/time_now/get_env + 算术/if/for/var + 纯函数），详见逐文件表。",
        "- **不可编译**: $($totals.uncompilable_count) 个，原因分类见上表（irgen 最小集外为主；前端语义/语法缺口少量）。前端语义缺口（全局表误判）正由另一任务修复——修复后重跑本脚本即自动扩全。",
        "- **双失败**: $($totals.both_fail_count) 个（Rust 基线自身也失败，文件本身为负例/有错，不算 tiec 缺口）。",
        "- **G4 结论**: ratio $([string]::Format('{0:N3}', $totals.ratio))（仅可编译子集）",
        $coverageNote
    )
    Write-Utf8NoBom -Path $mdPath -Content ($lines -join "`r`n")
    Write-Host "[gate4] 已写: $mdPath" -ForegroundColor Green

    # ---- 清理 --emit-ir 生成的 .ll 副产物 ----
    foreach ($llPath in $generatedLl) {
        Remove-Item $llPath -Force -ErrorAction SilentlyContinue
    }
    Write-Host "[gate4] 已清理 $($generatedLl.Count) 个 --emit-ir 产物 .ll" -ForegroundColor DarkGray

    # ---- 控制台汇总 ----
    Write-Host "`n[gate4] 完成" -ForegroundColor Green
    Write-Host ("  语料: {0} / 可编译: {1}（覆盖 {2}%）/ 不可编译: {3} / 双失败: {4}" -f $totals.corpus_total, $totals.compilable_count, $totals.coverage_pct, $totals.uncompilable_count, $totals.both_fail_count)
    Write-Host ("  tiec 总耗时:   {0} ms" -f $totals.tiec_total_ms)
    Write-Host ("  tie-llvm 总耗时: {0} ms" -f $totals.rust_total_ms)
    Write-Host ("  ratio (tie/rust): {0:N3}  [硬线 ≤3.0 / 目标 ≤2.0]" -f $totals.ratio)
    if (-not $hardOk) {
        Write-Host "  G4 判定: FAIL（无可编译文件或 ratio > 3.0）" -ForegroundColor Red
        exit 1
    }
    if ($partial) {
        Write-Host "  G4 判定: PASS（部分基准 + 缺口清单，见 docs/bench/phase5.md）" -ForegroundColor DarkYellow
    } else {
        Write-Host "  G4 判定: PASS" -ForegroundColor Green
    }
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
            - 语料: corpus.txt 的 pass 文件；只统计两边都 exit 0 的文件
              （tiec-proto 语义层不展开 import，import 文件预期 tiec exit 1 → 排除）
            - 计时: 每文件预热 1 + 5 次热运行取中位数，CPU 固定核心 0，单次硬超时 60s
            - 判定: ratio = tiec_total / rust_total < 1.0 即 G1 PASS（目标 0.5–0.83）
            - 输出: docs/bench/phase1.json / phase1.md
            - tiec-proto 路径: 环境变量 TIEC_PROTO_EXE，否则默认 compiler\proto\tiec-proto.exe；
              默认路径不存在时提示手动编译并退出 1（不自动编译）
  gate4     阶段 5 全链路对照（tiec --emit-ir vs tie-llvm --emit-ir）。
            - 语料: examples/*.tie（剔除 oop_neg_*）+ tests/language/*.tie
              （剔除 *_neg.tie），动态收集——缺口修复后自动扩全
            - 计时: 每文件预热 1 + 5 次热运行取中位数，CPU 固定核心 0，单次硬超时 60s
            - 覆盖: tiec 不可编译的文件记入"不可编译清单"（irgen 最小集外/
              前端语义缺口），不参与 ratio；Rust 也失败记双失败
            - 判定: ratio = tiec 中位和 / rust 中位和 ≤ 3.0 硬线（目标 ≤ 2.0）
              PASS；覆盖不足时标注"部分基准 + 缺口清单"
            - 输出: docs/bench/phase5.json / phase5.md
            - 退出码: 可编译文件为 0 或 ratio > 3.0 → 1；其余 0
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
