# -*- coding: utf-8 -*-
"""
tie 品牌 logo 透明 PNG 渲染器（Pillow）
- 直接按 tie-logo.svg / tie-logo-full.svg 的几何绘制，输出 RGBA 透明背景
- 4 张：主图标浅色/深色模式 + 组合版浅色/深色模式
- 4x 超采样抗锯齿
"""
import os
import math
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.abspath(__file__))
PREVIEW = os.path.join(ROOT, "preview")
os.makedirs(PREVIEW, exist_ok=True)

# ── 配色（浅色模式 = 石墨黑环；深色模式 = 浅色环，对应 SVG 媒体查询）──
LIGHT = dict(ring="#0F172A", dot="#0EA5E9", letter="#0F172A")
DARK  = dict(ring="#F8FAFC", dot="#38BDF8", letter="#F8FAFC")

def draw_icon(d, cx, cy, scale, pal):
    """缺口环主图标：两段弧 + 悬浮青点（坐标基于 512 画布）
    注意：PIL arc 的 bbox 是外缘、线宽向内扩展；SVG stroke 居中。
    故 PIL 外缘半径 = 117.5 + 23.75 = 141.25，描边 47.5 → 中线半径 117.5 与 SVG 一致"""
    r = 141.25 * scale   # 外缘半径
    w = 47.5 * scale
    box = [cx - r, cy - r, cx + r, cy + r]
    # 左大半弧：缺口右缘 (124°) → 缺口左缘 (236°)（PIL 角度 0°=3点钟，顺时针）
    d.arc(box, start=124, end=236, fill=pal["ring"], width=int(w))
    # 右大半弧：上缺口左缘 (304°) → 下缺口右缘 (416°)
    d.arc(box, start=304, end=416, fill=pal["ring"], width=int(w))
    # 悬浮青点（正上方缺口内：中线半径 117.5 → 中心 y = 256-117.5 = 138.5）
    dot_r = 17.5 * scale
    py = cy + (138.5 - 256) * scale
    d.ellipse([cx - dot_r, py - dot_r, cx + dot_r, py + dot_r], fill=pal["dot"])

def rline(d, p1, p2, w, color, cap=True):
    """圆头线（模拟 SVG stroke-linecap=round）：画线 + 两端画圆"""
    d.line([p1, p2], fill=color, width=w)
    if cap:
        r = w // 2
        for (x, y) in (p1, p2):
            d.ellipse([x - r, y - r, x + r, y + r], fill=color)

def draw_wordmark(d, pal, k=1.0):
    """组合版 tie 文字：三字母总高 140px、笔画 30px、视觉空隙 36px
    （几何与 tie-logo-full.svg 完全一致；e 中横为平头 butt）
    k：缩放系数（组合版 4x 超采样传 4.0）"""
    w = int(30 * k)
    # t：横笔（y 50-80，中心 65）+ 竖笔（y 65-190，圆头视觉顶 = 横笔顶 50 不超）
    rline(d, (270*k, 65*k), (370*k, 65*k), w, pal["letter"])
    rline(d, (320*k, 65*k), (320*k, 190*k), w, pal["letter"])
    # i：点（青）+ 竖笔（y 102-190，圆头）
    d.ellipse([(421-15)*k, (63-15)*k, (421+15)*k, (63+15)*k], fill=pal["dot"])
    rline(d, (421*k, 102*k), (421*k, 190*k), w, pal["letter"])
    # θ（希腊 theta，数学角度符号）：瘦椭圆环 + 中横（平头不超椭圆）
    # PIL ellipse outline 的 bbox 是外缘：外缘 rx=45+15=60、ry=62+15=77，
    # cy=128 → 底部 205 与 t/i 基线对齐，顶部 51≈50 齐平
    d.ellipse([(532-60)*k, (128-77)*k, (532+60)*k, (128+77)*k], outline=pal["letter"], width=w)
    rline(d, (497*k, 128*k), (569*k, 128*k), w, pal["letter"], cap=False)

def render_icon(pal, out):
    """主图标 512×512，4x 超采样"""
    S = 2048
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    draw_icon(d, 1024, 1024, 4.0, pal)
    img.resize((512, 512), Image.Resampling.LANCZOS).save(out)
    print("saved:", out)

def render_full(pal, out):
    """组合版 640×240，4x 超采样"""
    W, H = 2560, 960
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = 4.0
    # 图标组：translate(37.5,12.5) scale(0.42) → 中心 (145,120)，缩放 0.42
    draw_icon(d, 145 * k, 120 * k, 0.42 * k, pal)
    # 文字：与 SVG 完全一致（统一走 draw_wordmark，含外缘修正）
    draw_wordmark(d, pal, k)
    img.resize((640, 240), Image.Resampling.LANCZOS).save(out)
    print("saved:", out)

render_icon(LIGHT, os.path.join(PREVIEW, "tie-logo-light.png"))
render_icon(DARK,  os.path.join(PREVIEW, "tie-logo-dark.png"))
render_full(LIGHT, os.path.join(PREVIEW, "tie-logo-full-light.png"))
render_full(DARK,  os.path.join(PREVIEW, "tie-logo-full-dark.png"))
print("done.")
