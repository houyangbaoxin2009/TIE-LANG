# 渲染 tie 品牌 logo 预览图：浅色 / 深色背景 × 主图标 / 组合 logo = 4 张 PNG
# 使用 Edge headless 打开包裹 SVG 的临时 HTML 并截图（无需安装 playwright）

$ErrorActionPreference = "Stop"
$edge = "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
if (-not (Test-Path $edge)) { $edge = "C:\Program Files\Microsoft\Edge\Application\msedge.exe" }

$assets = "F:\Projects\tie\assets"
$preview = Join-Path $assets "preview"
$tmp = Join-Path $assets ".render"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Render-Svg([string]$svgFile, [string]$outFile, [int]$w, [int]$h, [string]$bg, [int]$pad, [string]$colorScheme = "light") {
  # img 使用绝对 file:// 路径，避免临时 HTML 位于子目录时相对路径解析失败
  $absSvg = (Join-Path $assets $svgFile) -replace '\\','/'
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
  $schemeArg = ""
  if ($colorScheme -eq "dark") { $schemeArg = "--force-prefers-color-scheme=dark" }
  # 透明背景：--default-background-color=00000000 让截图带 alpha 通道
  & $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 $schemeArg `
    "--default-background-color=00000000" `
    --user-data-dir=$userData --window-size="$winW,$winH" `
    "--screenshot=$outFile" ("file:///" + ($htmlPath -replace '\\','/')) 2>$null
  Start-Sleep -Milliseconds 400
  Write-Host "rendered: $outFile"
}

# 全部透明背景 PNG：浅色模式（石墨黑环）+ 深色模式（浅色环，CSS 自适应）
Render-Svg "tie-logo.svg"       (Join-Path $preview "tie-logo-light.png")  512 512 "transparent" 40 "light"
Render-Svg "tie-logo.svg"       (Join-Path $preview "tie-logo-dark.png")   512 512 "transparent" 40 "dark"
# 组合 logo 560×240，四周留 40px
Render-Svg "tie-logo-full.svg"  (Join-Path $preview "tie-logo-full-light.png")  560 240 "transparent" 40 "light"
Render-Svg "tie-logo-full.svg"  (Join-Path $preview "tie-logo-full-dark.png")   560 240 "transparent" 40 "dark"

Write-Host "done."
