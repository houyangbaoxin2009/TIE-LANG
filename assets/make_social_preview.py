# -*- coding: utf-8 -*-
"""
tie 品牌 GitHub 社交预览图（social-preview.png，1280×640）
- 深靛蓝渐变背景 + 组合 logo（深色模式浅色版）+ 中英双语标语
- 复用 render_png.py 的 DARK 配色与几何
"""
import os
from PIL import Image, ImageDraw, ImageFont
from render_png import DARK  # 复用深色模式配色（浅色环/文字 + 亮青点）

ROOT = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(ROOT, "social-preview.png")
W, H = 1280, 640

# ── 背景：垂直渐变 深石墨 #0B1220 → 深靛 #1E1B4B ──
bg = Image.new("RGB", (W, H))
bd = ImageDraw.Draw(bg)
c1 = (11, 18, 32)   # #0B1220
c2 = (30, 27, 75)   # #1E1B4B
for y in range(H):
    t = y / H
    col = tuple(round(c1[i] + (c2[i] - c1[i]) * t) for i in range(3))
    bd.line([(0, y), (W, y)], fill=col)

# 融合 logo（透明 PNG 叠加）
logo = Image.open(os.path.join(ROOT, "preview", "tie-logo-full-dark.png")).convert("RGBA")
# 放大 1.5× → 960×360，水平居中、纵向偏上（留出下方标语空间）
scale = 1.5
lw, lh = int(logo.width * scale), int(logo.height * scale)
logo = logo.resize((lw, lh), Image.Resampling.LANCZOS)
lx, ly = (W - lw) // 2, 96
bg.paste(logo, (lx, ly), logo)

# ── 标语 ──
def font(size, bold=False):
    path = "C:/Windows/Fonts/msyhbd.ttc" if bold else "C:/Windows/Fonts/msyh.ttc"
    try:
        return ImageFont.truetype(path, size)
    except OSError:
        return ImageFont.load_default()

d = ImageDraw.Draw(bg)
# 主标语（中文，粗体）
f_main = font(44, bold=True)
tag_cn = "一门语言，写逻辑 · 写界面 · 写数据库 · 当数据交换格式"
bbox = d.textbbox((0, 0), tag_cn, font=f_main)
d.text(((W - (bbox[2] - bbox[0])) // 2, 470), tag_cn, fill="#E2E8F0", font=f_main)
# 副标语（英文）
f_sub = font(26)
tag_en = "One language for logic, UI, database & data exchange"
bbox2 = d.textbbox((0, 0), tag_en, font=f_sub)
d.text(((W - (bbox2[2] - bbox2[0])) // 2, 535), tag_en, fill="#7DD3FC", font=f_sub)

bg.save(OUT)
print("saved:", OUT, f"{W}x{H}")
