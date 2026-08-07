# tie 正式发行版打包脚本（Windows）
#
# 职责：构建 release 工具链 → 组装发行目录 → 生成 zip 压缩包。
# 产物：dist/tie-{发行版号}-win-x64.zip（如 tie-2026.1-win-x64.zip）
#
# 用法：
#   .\scripts\package.ps1                     # 默认发行版号 2026.1
#   .\scripts\package.ps1 -ReleaseVersion 2026.2
#   .\scripts\package.ps1 -SkipReplBuild      # 跳过 repl.exe 自举（用现有产物）
#
# 流程：
#   1. cargo build --release（全 workspace，验证 0 错误）
#   2. repl.exe 自举（tie-interp staticlib + tie-llvm 编译 repl/repl.tie）
#   3. 组装 dist/tie-{版本}/（bin/doc/examples/editor）
#   4. Compress-Archive 打包为 zip

param(
    # 正式发行版号（年份.修订号），默认 2026.1
    [string]$ReleaseVersion = "2026.1",
    # 跳过 repl.exe 自举构建（复用现有 repl/repl.exe）
    [switch]$SkipReplBuild
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

# ---- 第 1 步：release 构建（全 workspace）----
Write-Host "`n[1/4] cargo build --release (全 workspace)..." -ForegroundColor Yellow
Push-Location $Root
try {
    cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) {
        throw "release 构建失败（退出码 $LASTEXITCODE）"
    }
}
finally {
    Pop-Location
}
Write-Host "[1/4] release 构建完成" -ForegroundColor Green

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
        # 自举：先构建 tie-interp staticlib（tie_interp.lib），
        # 再经 tie-llvm 编译 repl/repl.tie 并链接生成 repl.exe
        cargo build --release -p tie-interp
        if ($LASTEXITCODE -ne 0) {
            throw "tie-interp staticlib 构建失败（退出码 $LASTEXITCODE）"
        }
        & (Join-Path $Root "target\release\tie-llvm.exe") "repl\repl.tie"
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

# bin/：7 个二进制（6 个工具 + repl 外壳）
$BinDir = Join-Path $Root "target\release"
$BinTarget = Join-Path $DistDir "bin"
New-Item -ItemType Directory -Path $BinTarget -Force | Out-Null

# 工具链二进制清单（源文件名 → 是否必须）
$Tools = @(
    @{ Name = "tie.exe";          Required = $true },
    @{ Name = "tie-prep.exe";     Required = $true },
    @{ Name = "tie-frontend.exe"; Required = $true },
    @{ Name = "tie-llvm.exe";     Required = $true },
    @{ Name = "tie-lsp.exe";      Required = $true },
    @{ Name = "tie-interp.exe";   Required = $true }  # 实际为库，无独立 exe 时跳过
)
foreach ($tool in $Tools) {
    $src = Join-Path $BinDir $tool.Name
    if (Test-Path $src) {
        Copy-Item $src $BinTarget
        Write-Host "  bin/$($tool.Name) ✔" -ForegroundColor DarkGray
    }
    elseif ($tool.Required) {
        Write-Host "  警告: 缺失 $($tool.Name)（跳过）" -ForegroundColor DarkYellow
    }
}
# repl.exe（自举外壳）
if (Test-Path $ReplExe) {
    Copy-Item $ReplExe $BinTarget
    Write-Host "  bin/repl.exe ✔" -ForegroundColor DarkGray
}
else {
    Write-Host "  警告: 缺失 repl.exe（跳过）" -ForegroundColor DarkYellow
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

# editor/vscode-tie/：VSCode 扩展
$EditorSrc = Join-Path $Root "editor\vscode-tie"
if (Test-Path $EditorSrc) {
    $EditorTarget = Join-Path $DistDir "editor\vscode-tie"
    New-Item -ItemType Directory -Path $EditorTarget -Force | Out-Null
    Copy-Item (Join-Path $EditorSrc "*") $EditorTarget -Recurse
    Write-Host "  editor/vscode-tie/ ✔" -ForegroundColor DarkGray
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
