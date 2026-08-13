# 渲染 4 个概念 logo 对比图（浅色背景，单张 PNG）
# 使用 Edge headless 截图（与 render-preview.ps1 同法）

$ErrorActionPreference = "Stop"
$edge = "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
if (-not (Test-Path $edge)) { $edge = "C:\Program Files\Microsoft\Edge\Application\msedge.exe" }

$assets = "F:\Projects\tie\assets"
$concepts = Join-Path $assets "concepts"
$preview = Join-Path $concepts "preview"
$tmp = Join-Path $concepts ".render"
New-Item -ItemType Directory -Force -Path $preview, $tmp | Out-Null

function Render-Svg([string]$svgFile, [string]$outFile, [int]$w, [int]$h, [string]$bg, [int]$pad) {
  $absSvg = (Join-Path $concepts $svgFile) -replace '\\','/'
  $html = @"
<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><style>
  html,body{margin:0;padding:0;background:$bg;}
  .box{width:$(($w + 2*$pad))px;height:$(($h + 2*$pad))px;display:flex;align-items:center;justify-content:center;}
  img{display:block;}
</style></head>
<body><div class="box"><img src="file:///$absSvg" width="$w" height="$h"></div></body></html>
"@
  $htmlPath = Join-Path $tmp ("t_" + [IO.Path]::GetFileNameWithoutExtension($outFile) + ".html")
  Set-Content -Path $htmlPath -Value $html -Encoding utf8
  $winW = $w + 2*$pad; $winH = $h + 2*$pad
  $userData = Join-Path $tmp ("profile_" + [IO.Path]::GetFileNameWithoutExtension($outFile))
  & $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 `
    --user-data-dir=$userData --window-size="$winW,$winH" `
    "--screenshot=$outFile" ("file:///" + ($htmlPath -replace '\\','/')) 2>$null
  Start-Sleep -Milliseconds 400
  Write-Host "rendered: $outFile"
}

Render-Svg "c1-necktie.svg"   (Join-Path $preview "c1-necktie.png")   512 512 "#F8FAFC" 40
Render-Svg "c2-trit.svg"      (Join-Path $preview "c2-trit.png")      512 512 "#F8FAFC" 40
Render-Svg "c3-pipeline.svg"  (Join-Path $preview "c3-pipeline.png")  512 512 "#F8FAFC" 40
Render-Svg "c4-loop.svg"      (Join-Path $preview "c4-loop.png")      512 512 "#F8FAFC" 40

Write-Host "done."
