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

def draw_wordmark(d, pal):
    """组合版 tie 文字：三字母总高 140px、笔画 30px、视觉空隙 36px"""
    w = 30
    # t：横笔（y 50-80，中心 65）+ 竖笔（y 65-190，圆头视觉顶不超横笔顶）
    d.line([(270, 65), (370, 65)], fill=pal["letter"], width=w)
    d.line([(320, 65), (320, 190)], fill=pal["letter"], width=w)
    # i：点（青）+ 竖笔（y 102-190）
    d.ellipse([421-15, 63-15, 421+15, 63+15], fill=pal["dot"])
    d.line([(421, 102), (421, 190)], fill=pal["letter"], width=w)
    # e：圆环 + 中横（不超圆：472 → 612）
    d.ellipse([542-70, 120-70, 542+70, 120+70], outline=pal["letter"], width=w)
    d.line([(472, 120), (612, 120)], fill=pal["letter"], width=w)

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
    # 文字：整体 ×4
    dd = lambda p1, p2: d.line([(p1[0]*k, p1[1]*k), (p2[0]*k, p2[1]*k)],
                               fill=pal["letter"], width=int(30*k))
    dd((270, 65), (370, 65)); dd((320, 50), (320, 190))
    d.ellipse([(421-15)*k, (63-15)*k, (421+15)*k, (63+15)*k], fill=pal["dot"])
    dd((421, 102), (421, 190))
    d.ellipse([(542-70)*k, (120-70)*k, (542+70)*k, (120+70)*k], outline=pal["letter"], width=int(30*k))
    dd((472, 120), (620, 120))
    img.resize((640, 240), Image.Resampling.LANCZOS).save(out)
    print("saved:", out)

render_icon(LIGHT, os.path.join(PREVIEW, "tie-logo-light.png"))
render_icon(DARK,  os.path.join(PREVIEW, "tie-logo-dark.png"))
render_full(LIGHT, os.path.join(PREVIEW, "tie-logo-full-light.png"))
render_full(DARK,  os.path.join(PREVIEW, "tie-logo-full-dark.png"))
print("done.")
