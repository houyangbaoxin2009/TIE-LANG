# tie 正式发行版打包脚本（Windows，0-Rust）
#
# 职责：构建 release 工具链（自举）→ 组装发行目录 → 生成 zip 压缩包。
# 产物：dist/tie-{发行版号}-win-x64.zip（如 tie-Harbor-2026.1-preview.2-win-x64.zip）
#
# 用法：
#   .\scripts\package.ps1 -ReleaseVersion Harbor-2026.1-preview.2   # 默认 2026.1
#   .\scripts\package.ps1 -ReleaseVersion 2026.2
#   .\scripts\package.ps1 -SkipReplBuild      # 跳过 repl.exe 自举（用现有产物）
#   .\scripts\package.ps1 -LlvmDir D:\LLVM    # 指定 LLVM 安装目录（默认 D:\LLVM）
#   .\scripts\package.ps1 -SkipLlvm           # 跳过 LLVM 精简工具链捆绑
#
# 流程（0-Rust，Rust 参考编译器已归档 tiec_rust）：
#   1. 自举验证：tiec.exe 编译 driver.tie → tiec2.exe（编译零错误）
#   2. repl.exe 自举（tiec 编译 repl/repl.tie）
#   3. 组装 dist/tie-{版本}/（bin/doc/examples/editor + bin/llvm/ 精简工具链）
#   4. Compress-Archive 打包为 zip

param(
    # 正式发行版号（年份.修订号），默认 2026.1
    [string]$ReleaseVersion = "2026.1",
    # 跳过 repl.exe 自举构建（复用现有 repl/repl.exe）
    [switch]$SkipReplBuild,
    # LLVM 安装目录（捆绑 clang/opt/llvm-ar/lld-link 与头文件的来源），默认 D:\LLVM
    [string]$LlvmDir = "D:\LLVM",
    # 跳过 LLVM 精简工具链捆绑（不打包 bin/llvm/）
    [switch]$SkipLlvm
)

# 错误即停
$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot
# 发行目录名与 zip 名（win-x64 平台后缀）
$DistName = "tie-$ReleaseVersion"
$ZipName = "tie-$ReleaseVersion-win-x64.zip"
$DistDir = Join-Path $Root "dist\$DistName"

Write-Host "[package] 发行版号: $ReleaseVersion" -ForegroundColor Cyan
Write-Host "[package] 仓库根目录: $Root" -ForegroundColor Cyan

# tiec（自举编译器，tie 语言自写，stage0 入库）：本脚本唯一编译器
$TiecExe = Join-Path $Root "compiler\tiec.exe"

# ---- 第 1 步：自举验证（tiec 编译自身 → tiec2.exe）----
Write-Host "`n[1/4] 自举验证: tiec 编译 driver.tie → tiec2.exe..." -ForegroundColor Yellow
$Tiec2Exe = Join-Path $Root "compiler\tiec2.exe"
Push-Location $Root
try {
    & $TiecExe "compiler\driver.tie" -o $Tiec2Exe
    if ($LASTEXITCODE -ne 0) {
        throw "自举编译失败（退出码 $LASTEXITCODE）"
    }
}
finally {
    Pop-Location
}
Write-Host "[1/4] 自举验证完成" -ForegroundColor Green

# ---- 第 2 步：repl.exe 自举（可选）----
Write-Host "`n[2/4] repl.exe 自举..." -ForegroundColor Yellow
$ReplExe = Join-Path $Root "repl\repl.exe"
if ($SkipReplBuild) {
    Write-Host "[2/4] 已跳过 repl 自举（-SkipReplBuild）" -ForegroundColor DarkYellow
}
else {
    if (Test-Path $ReplExe) {
        Write-Host "[2/4] 发现现有 repl.exe，覆盖构建..." -ForegroundColor DarkYellow
    }
    Push-Location $Root
    try {
        # 自举：tiec 编译 repl/repl.tie 并链接生成 repl.exe（0-Rust，
        # 运行时静态库 std/runtime.a 由 tiec 自动定位）
        & $TiecExe "repl\repl.tie"
        if ($LASTEXITCODE -ne 0) {
            throw "repl.exe 编译失败（退出码 $LASTEXITCODE）"
        }
    }
    finally {
        Pop-Location
    }
    if (-not (Test-Path $ReplExe)) {
        throw "repl.exe 未生成: $ReplExe"
    }
    Write-Host "[2/4] repl.exe 自举完成" -ForegroundColor Green
}

# ---- 第 3 步：组装发行目录 ----
Write-Host "`n[3/4] 组装发行目录 $DistDir ..." -ForegroundColor Yellow

# 清空旧目录，重建
if (Test-Path $DistDir) {
    Remove-Item $DistDir -Recurse -Force
}
New-Item -ItemType Directory -Path $DistDir -Force | Out-Null

# bin/：自举产物（tiec/tiec2 编译器 + repl 外壳 + pkg 包管理器；0-Rust）
$BinTarget = Join-Path $DistDir "bin"
New-Item -ItemType Directory -Path $BinTarget -Force | Out-Null

# repl.exe（自举外壳）
if (Test-Path $ReplExe) {
    Copy-Item $ReplExe $BinTarget
    Write-Host "  bin/repl.exe ✔" -ForegroundColor DarkGray
}
else {
    Write-Host "  警告: 缺失 repl.exe（跳过）" -ForegroundColor DarkYellow
}

# pkg.exe（包管理器自举产物，tie.exe 子命令转发依赖；非必需，缺失仅警告）
$PkgExe = Join-Path $Root "pkg\pkg.exe"
if (Test-Path $PkgExe) {
    Copy-Item $PkgExe $BinTarget
    Write-Host "  bin/pkg.exe ✔" -ForegroundColor DarkGray
}
else {
    Write-Host "  警告: 缺失 pkg.exe（跳过）" -ForegroundColor DarkYellow
}

# 自举 v2 新编译器 tiec.exe（compiler/ 编译产物，不随 compiler/ 源码进发行版，单列 bin/；
# tiec2.exe 是自举验证的临时产物，与 tiec.exe 同源码同尺寸，不进发行版）
$TiecSrc = Join-Path $Root "compiler\tiec.exe"
if (Test-Path $TiecSrc) {
    Copy-Item $TiecSrc $BinTarget
    Write-Host "  bin/tiec.exe ✔" -ForegroundColor DarkGray
}
else {
    Write-Host "  警告: 缺失 tiec.exe（跳过）" -ForegroundColor DarkYellow
}

# ---- LLVM 精简工具链（bin/llvm/，[3/4] 步骤的子步骤）----
# tiec/tie 编译链路后端调用 clang/opt/llvm-ar/lld-link，将本机 LLVM 安装的
# 工具与 clang 头文件捆绑进发行版 bin/llvm/，实现开箱即用（用户无需另装 LLVM）。
# 布局匹配 clang 默认资源目录查找（../lib/clang/<ver>/include 相对 clang.exe），
# 同时符合 TIE_LLVM_HOME 约定（<home>\bin\<tool>.exe；捆绑时 TIE_LLVM_HOME=bin/llvm）。
Write-Host "`n[3.5] LLVM 精简工具链..." -ForegroundColor Yellow

if ($SkipLlvm) {
    Write-Host "[3.5] 已跳过 LLVM 工具链打包（-SkipLlvm）" -ForegroundColor DarkYellow
}
else {
    # 主工具 clang.exe 缺失 → 整个 LLVM 块跳过（zip 仍正常打包）
    if (-not (Test-Path (Join-Path $LlvmDir "bin\clang.exe"))) {
        Write-Host "  LLVM 工具链打包跳过（未找到 $LlvmDir\bin\clang.exe）" -ForegroundColor DarkYellow
    }
    else {
        # bin/llvm/bin/ 与 bin/llvm/lib/：目标目录
        $LlvmBin = Join-Path $DistDir "bin\llvm\bin"
        $LlvmLib = Join-Path $DistDir "bin\llvm\lib"
        New-Item -ItemType Directory -Path $LlvmBin -Force | Out-Null
        New-Item -ItemType Directory -Path $LlvmLib -Force | Out-Null

        # 工具逐个复制（clang.exe 必需——缺失视为本块致命错误；其余可选——缺失仅警告）
        $LlvmTools = @(
            @{ Name = "clang.exe";    Required = $true },
            @{ Name = "opt.exe";      Required = $false },
            @{ Name = "llvm-ar.exe";  Required = $false },
            @{ Name = "lld-link.exe"; Required = $false }
        )
        foreach ($lt in $LlvmTools) {
            $src = Join-Path $LlvmDir "bin\$($lt.Name)"
            if (Test-Path $src) {
                Copy-Item $src $LlvmBin
                Write-Host "  bin/llvm/bin/$($lt.Name) ✔" -ForegroundColor DarkGray
            }
            elseif ($lt.Required) {
                throw "clang.exe 缺失（$LlvmDir\bin\clang.exe），LLVM 工具链打包失败"
            }
            else {
                Write-Host "  警告: 缺失 $($lt.Name)（跳过）" -ForegroundColor DarkYellow
            }
        }

        # 头文件资源目录：lib/clang/<ver>/include 整体复制（保留版本目录名，
        # 匹配 clang 默认资源目录查找 ../lib/clang/<ver>/include 相对 clang.exe）
        $verDir = Get-ChildItem (Join-Path $LlvmDir "lib\clang") -Directory | Select-Object -First 1
        if ($verDir) {
            $IncludeDest = Join-Path $LlvmLib ("clang\" + $verDir.Name + "\include")
            Copy-Item (Join-Path $verDir.FullName "include") $IncludeDest -Recurse
            Write-Host "  bin/llvm/lib/clang/$($verDir.Name)/include ✔" -ForegroundColor DarkGray
        }
        else {
            Write-Host "  警告: 未找到 $LlvmDir\lib\clang\<ver>\include（跳过头文件）" -ForegroundColor DarkYellow
        }

        # 许可文件：优先 LLVM 安装目录自带（官方安装包），缺失时回退仓库内置
        # third_party/llvm/LICENSE.TXT（LLVM 官方 Apache-2.0 with LLVM Exceptions，
        # 随发行版捆绑二进制必须附许可证）
        $LlvmLicense = Join-Path $LlvmDir "LICENSE.txt"
        $BundledLicense = Join-Path $Root "third_party\llvm\LICENSE.TXT"
        if (-not (Test-Path $LlvmLicense)) {
            $LlvmLicense = $BundledLicense
        }
        if (Test-Path $LlvmLicense) {
            Copy-Item $LlvmLicense (Join-Path $DistDir "bin\llvm\LICENSE.txt")
            Write-Host "  bin/llvm/LICENSE.txt ✔" -ForegroundColor DarkGray
        }
        else {
            Write-Host "  警告: 缺失 LLVM 许可文件（$BundledLicense 也不存在，跳过）" -ForegroundColor DarkYellow
        }

        # 大小汇总（MB，统计 bin/llvm/ 下全部复制文件）
        $LlvmTotal = (Get-ChildItem (Join-Path $DistDir "bin\llvm") -Recurse -File |
            Measure-Object -Property Length -Sum).Sum
        Write-Host "  LLVM 工具链合计: $([math]::Round($LlvmTotal / 1MB, 2)) MB" -ForegroundColor DarkGray
        Write-Host "[3.5] LLVM 精简工具链完成（TIE_LLVM_HOME=$DistDir\bin\llvm）" -ForegroundColor Green
    }
}

# doc/：文档与许可
$DocTarget = Join-Path $DistDir "doc"
New-Item -ItemType Directory -Path $DocTarget -Force | Out-Null
foreach ($doc in @("README.md", "CHANGELOG.md", "LICENSE")) {
    $src = Join-Path $Root $doc
    if (Test-Path $src) {
        Copy-Item $src $DocTarget
    }
}
# docs/ 子目录（language.md / ai-guide.md / prompt-pack.md / release.md）
$DocsSub = Join-Path $Root "docs"
if (Test-Path $DocsSub) {
    New-Item -ItemType Directory -Path (Join-Path $DocTarget "docs") -Force | Out-Null
    Get-ChildItem $DocsSub -Filter "*.md" | ForEach-Object {
        Copy-Item $_.FullName (Join-Path $DocTarget "docs")
    }
}
Write-Host "  doc/ ✔" -ForegroundColor DarkGray

# examples/：示例源码
$ExamplesSrc = Join-Path $Root "examples"
$ExamplesTarget = Join-Path $DistDir "examples"
if (Test-Path $ExamplesSrc) {
    New-Item -ItemType Directory -Path $ExamplesTarget -Force | Out-Null
    # 只复制 .tie 源码，不复制编译产物（.exe / .a / .o / .ll）
    Get-ChildItem $ExamplesSrc -Filter "*.tie" | ForEach-Object {
        Copy-Item $_.FullName $ExamplesTarget
    }
    Write-Host "  examples/ ✔" -ForegroundColor DarkGray
}

# std/、ext/ 与 rdu/：标准库、扩展库与嵌入式基础层（tie 语言自写，随发行版内置；
# 用户程序 import "../std/..." 或 "../ext/..." 或 "../rdu/..." 依赖本地库目录。
# rdu 为嵌入式基础层（Rudimentary），独立于 std/ext（不 import 任何东西），
# 无栈纪律（零原语/零动态内存/无递归/无全局状态），随发行版内置）
foreach ($lib in @("std", "ext", "rdu")) {
    $LibSrc = Join-Path $Root $lib
    if (Test-Path $LibSrc) {
        $LibTarget = Join-Path $DistDir $lib
        New-Item -ItemType Directory -Path $LibTarget -Force | Out-Null
        Get-ChildItem $LibSrc -Filter "*.tie" | ForEach-Object {
            Copy-Item $_.FullName $LibTarget
        }
        # std/runtime.a（仅 std 特有）：tie 自写运行时静态库（tiec 链接用户程序必需，
        # tie 语言产物；rdu/ext 不产生 runtime.a——rdu 为 freestanding 零运行时依赖）
        $RuntimeLib = Join-Path $LibSrc "runtime.a"
        if (Test-Path $RuntimeLib) {
            Copy-Item $RuntimeLib $LibTarget
            Write-Host "  $lib/runtime.a ✔" -ForegroundColor DarkGray
        }
        Write-Host "  $lib/ ✔" -ForegroundColor DarkGray
    }
}

# skills/tie-dev/：tie 开发技能（SKILL.md，面向「用 tie 写软件」的开发者与 AI 助手；
# 随发行版分发，用户可复制到自己的 AI 技能目录——如 ~/.dsh/skills/、.opencode/skills/）
$SkillSrc = Join-Path $Root "skills"
if (Test-Path $SkillSrc) {
    $SkillTarget = Join-Path $DistDir "skills"
    New-Item -ItemType Directory -Path $SkillTarget -Force | Out-Null
    Get-ChildItem $SkillSrc -Recurse -File | ForEach-Object {
        $rel = $_.FullName.Substring($SkillSrc.Length).TrimStart("\")
        $dest = Join-Path $SkillTarget $rel
        New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null
        Copy-Item $_.FullName $dest
    }
    Write-Host "  skills/ ✔（tie-dev 技能）" -ForegroundColor DarkGray
}

# editor/vscode-tie/：VSCode 扩展（净化复制——排除 node_modules/ 与 out/ 构建产物，
# 源码分发：用户 npm install + npm run compile 或 vsce package 后安装）
$EditorSrc = Join-Path $Root "editor\vscode-tie"
if (Test-Path $EditorSrc) {
    $EditorTarget = Join-Path $DistDir "editor\vscode-tie"
    New-Item -ItemType Directory -Path $EditorTarget -Force | Out-Null
    Get-ChildItem $EditorSrc -Force | Where-Object {
        $_.Name -notin @("node_modules", "out")
    } | ForEach-Object {
        Copy-Item $_.FullName $EditorTarget -Recurse
    }
    Write-Host "  editor/vscode-tie/ ✔（已排除 node_modules/、out/）" -ForegroundColor DarkGray
}

# compiler/：自举 v2 编译器源码（tie 语言自写，T2–T5 阶段产物）。
# 递归复制全部 .tie 源码（含子目录）与 README.tie；排除编译产物 tiec.exe/tiec2.exe
# （已在 bin/ 段处理，不进 compiler/ 目录），随发行版内置便于检视与二次开发
$CompilerSrc = Join-Path $Root "compiler"
$CompilerTarget = Join-Path $DistDir "compiler"
if (Test-Path $CompilerSrc) {
    New-Item -ItemType Directory -Path $CompilerTarget -Force | Out-Null
    # 递归复制全部 .tie 源码（含子目录）+ README.tie，剔除 .exe 编译产物
    Get-ChildItem $CompilerSrc -Recurse -File | Where-Object {
        $_.Extension -eq ".tie" -or $_.Name -eq "README.tie"
    } | ForEach-Object {
        $rel = $_.FullName.Substring($CompilerSrc.Length).TrimStart("\")
        $dest = Join-Path $CompilerTarget $rel
        New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null
        Copy-Item $_.FullName $dest
    }
    Write-Host "  compiler/ ✔（tiec 源码）" -ForegroundColor DarkGray
}

Write-Host "[3/4] 发行目录组装完成" -ForegroundColor Green

# ---- 第 4 步：打包 zip ----
Write-Host "`n[4/4] 打包 zip..." -ForegroundColor Yellow
$ZipPath = Join-Path $Root "dist\$ZipName"
if (Test-Path $ZipPath) {
    Remove-Item $ZipPath -Force
}
Compress-Archive -Path $DistDir -DestinationPath $ZipPath -CompressionLevel Optimal
if (-not (Test-Path $ZipPath)) {
    throw "zip 打包失败: $ZipPath"
}

# ---- 汇总 ----
$ZipSizeMB = [math]::Round((Get-Item $ZipPath).Length / 1MB, 2)
Write-Host "`n打包完成 ✔" -ForegroundColor Green
Write-Host "  zip: $ZipPath ($ZipSizeMB MB)"
Write-Host "  目录: $DistDir"
