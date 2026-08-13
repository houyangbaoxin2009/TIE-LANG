# -*- coding: utf-8 -*-
"""
tie 语言 logo 生成器 v2.1 —— 修复版
- 用"胶囊链"（采样点画圆）替代粗 line，消除锯齿
- 精修：三态连笔 / 四段环 / 穿环结 / 动态T
"""
import os, math
from PIL import Image, ImageDraw, ImageFont

S = 2048
OUT_S = 512
C = 1024
ROOT = os.path.dirname(os.path.abspath(__file__))
VAR = os.path.join(ROOT, "v2")
os.makedirs(VAR, exist_ok=True)
BG = "#F8FAFC"

INK    = "#0F172A"
INDIGO = "#1E40AF"
BLUE   = "#2563EB"
CYAN   = "#0EA5E9"
SKY    = "#38BDF8"

def bezier_pts(ctrl, n=120):
    """三次贝塞尔曲线采样"""
    pts = []
    for i in range(0, len(ctrl) - 3, 3):
        p0, p1, p2, p3 = ctrl[i:i+4]
        for k in range(n + 1):
            t = k / n
            mt = 1 - t
            x = mt**3*p0[0] + 3*mt*mt*t*p1[0] + 3*mt*t*t*p2[0] + t**3*p3[0]
            y = mt**3*p0[1] + 3*mt*mt*t*p1[1] + 3*mt*t*t*p2[1] + t**3*p3[1]
            pts.append((x, y))
    return pts

def capsule(d, ctrl, width, color, n=160):
    """胶囊链曲线：采样点连线 + 端点画圆，宽线圆润无锯齿"""
    pts = bezier_pts(ctrl, n)
    r = width // 2
    d.line(pts, fill=color, width=width, joint="curve")
    for x, y in pts[::8]:
        d.ellipse([x-r, y-r, x+r, y+r], fill=color)
    for x, y in (pts[0], pts[-1]):
        d.ellipse([x-r, y-r, x+r, y+r], fill=color)

# ═══════════════ 方案 1：穿环结（Loop-Knot）═══
def v_loopknot(d):
    ring_c = (C, 300)
    ring_r = 430
    ring_w = 180
    # 绳：从左上穿入、右下穿出（经过环心下方一点）
    capsule(d, [(240, 200), (520, 320), (1500, 700), (1780, 940)], 180, INK)
    # 环：盖住绳中段 → 结的层次
    d.ellipse([ring_c[0]-ring_r, ring_c[1]-ring_r,
               ring_c[0]+ring_r, ring_c[1]+ring_r],
              outline=INK, width=ring_w)
    # 环内绳上叠一小节"结眼"高光环（细线，强化穿过感）
    d.ellipse([C-130, ring_c[1]+240-130, C+130, ring_c[1]+240+130],
              outline=BG, width=26)

# ═══════════════ 方案 2：三态连笔（Constellation）═══
def v_constellation(d):
    pts = [(C, 420), (C-540, 1450), (C+540, 1450)]
    sizes = [150, 115, 80]
    # 连笔曲线：大点 → 中点 → 小点（连续 S）
    capsule(d, [(C, 570), (C-300, 750), (C-540, 1000), (C-540, 1330),
                (C-540, 1480), (C, 1720), (C+540, 1480), (C+540, 1330)], 56, CYAN)
    for (x, y), r in zip(pts, sizes):
        d.ellipse([x-r, y-r, x+r, y+r], fill=INDIGO)
        d.ellipse([x-r//3, y-r//3, x+r//3, y+r//3], fill=SKY)

# ═══════════════ 方案 3：四段环（Quadrant Ring）═══
def v_quadrant(d):
    r, w = 460, 150
    colors = ["#1E3A8A", "#2563EB", "#3B82F6", "#0EA5E9"]
    gap = 16
    seg = 90 - gap
    for i, col in enumerate(colors):
        a0 = -45 + i * 90 + gap / 2
        a1 = a0 + seg
        d.arc([C-r, C-r, C+r, C+r], start=a0, end=a1, fill=col, width=w)
    d.ellipse([C-90, C-90, C+90, C+90], fill=INK)
    d.ellipse([C-30, C-30, C+30, C+30], fill=SKY)

# ═══════════════ 方案 4：动态 T（Slanted T，修复拼接）═══
def v_slanted_t(d):
    ang = math.radians(14)
    # 横笔：斜平行四边形
    hw, hh = 600, 230
    x0, y0 = 340, 660
    dx, dy = hw * math.cos(ang), -hw * math.sin(ang)
    sx, sy = hh * math.sin(ang), hh * math.cos(ang)
    d.polygon([(x0, y0), (x0+dx, y0+dy), (x0+dx+sx, y0+dy+sy), (x0+sx, y0+sy)], fill=INDIGO)
    # 竖笔：从横笔中心偏右，向下
    vw = 150
    vx = x0 + hw * 0.52 + sx * 0.5
    vy = y0 + sy * 0.5
    vlen = 720
    d.polygon([(vx, vy), (vx+vw, vy), (vx+vw, vy+vlen), (vx, vy+vlen)], fill=INDIGO)
    # 竖笔末端圆弧钩（与竖笔等宽，圆心角 90°，半径 = 竖笔宽 → 自然衔接）
    hk = vw  # 钩半径 = 竖笔宽，接缝无缝
    cx0, cy0 = vx, vy + vlen
    d.arc([cx0-hk, cy0-hk, cx0+vw+hk, cy0+hk], start=90, end=180, fill=INDIGO, width=vw)
    d.arc([cx0-hk, cy0-hk, cx0+vw+hk, cy0+hk], start=90, end=180, fill=INDIGO, width=vw)

# ═══════════════ 方案 5：缺口环（精简版）═══
def v_slit_ring(d):
    r, w = 470, 190
    # 用两段弧画环：左右两段 + 顶部缺口（负空间，无尖角）
    gap = 34  # 缺口半角（度）
    d.arc([C-r, C-r, C+r, C+r], start=90+gap, end=270-gap, fill=INK, width=w)   # 左大半
    d.arc([C-r, C-r, C+r, C+r], start=270+gap, end=450-gap, fill=INK, width=w)  # 右大半
    # 缺口处青色点（悬浮在缺口内，位于环外缘内侧）
    rr = r - w // 2
    py = C - rr - 40
    d.ellipse([C-70, py-70, C+70, py+70], fill=CYAN)

# ═══════════════ 方案 6：蝴蝶结（柔化版）═══
def v_bowtie(d):
    col = INDIGO
    # 左右翼：用弧线三角（圆角翼）——两段弧围成
    for sgn in (-1, 1):
        # 翼：外弧（半圆）+ 两条切线
        tip_x = C + sgn * 560
        # 上弧
        d.arc([tip_x-480, C-520, tip_x+480, C+40], start=0, end=180 if sgn < 0 else 180, fill=col, width=170)
        d.arc([tip_x-480, C-40, tip_x+480, C+520], start=180, end=360, fill=col, width=170)
        d.polygon([(C+sgn*40, C-220), (tip_x, C-40), (tip_x, C+40), (C+sgn*40, C+220)], fill=col)
    # 中心结
    d.rounded_rectangle([C-130, C-160, C+130, C+160], radius=70, fill=col)
    # 尾带（圆角收尾：两端画圆）
    for sgn in (-1, 1):
        tail = [(C+sgn*230, C+80), (C+sgn*320, C+80), (C+sgn*480, C+940), (C+sgn*390, C+940)]
        d.polygon(tail, fill=col)
        d.ellipse([C+sgn*480-60, C+880, C+sgn*480+60, C+1000], fill=col)

V2 = [
    ("v2-loopknot",      "穿环结", v_loopknot),
    ("v2-constellation", "三态连笔", v_constellation),
    ("v2-quadrant",      "四段环", v_quadrant),
    ("v2-slanted-t",     "动态T", v_slanted_t),
    ("v2-slit-ring",     "缺口环", v_slit_ring),
    ("v2-bowtie",        "蝴蝶结", v_bowtie),
]

def render(fn):
    img = Image.new("RGB", (S, S), BG)
    fn(ImageDraw.Draw(img))
    return img.resize((OUT_S, OUT_S), Image.Resampling.LANCZOS)

for name, _, fn in V2:
    render(fn).save(os.path.join(VAR, name + ".png"))
print("v2 fixed done:", len(V2))

try:
    font = ImageFont.truetype("C:/Windows/Fonts/msyh.ttc", 34)
except OSError:
    font = ImageFont.load_default()
CELL = OUT_S + 90
GAP = 30
cols, rows = 3, 2
W = cols * CELL + (cols + 1) * GAP
H = rows * CELL + (rows + 1) * GAP
grid = Image.new("RGB", (W, H), "#FFFFFF")
gd = ImageDraw.Draw(grid)
for i, (name, desc, fn) in enumerate(V2):
    r, c = divmod(i, cols)
    x0 = GAP + c * (CELL + GAP)
    y0 = GAP + r * (CELL + GAP)
    grid.paste(render(fn), (x0 + 45, y0 + 45))
    gd.text((x0 + 45, y0 + OUT_S + 62), desc, fill="#334155", font=font)
grid.save(os.path.join(ROOT, "grid2.png"))
print("grid2 saved")
