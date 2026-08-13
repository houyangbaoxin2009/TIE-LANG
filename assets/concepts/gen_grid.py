# -*- coding: utf-8 -*-
"""
tie 语言 logo 程序化生成器（方案探索网格）
- 6 种形状 × 3 种配色 = 18 个变体
- 输出：concepts/variants/ 单个 PNG + concepts/grid.png 总览对比图
- 超采样 4x 抗锯齿：画在 2048 画布，缩到 512
"""
import os
from PIL import Image, ImageDraw, ImageFont

S = 2048                      # 超采样画布边长
OUT_S = 512                   # 输出边长
ROOT = os.path.dirname(os.path.abspath(__file__))
VAR = os.path.join(ROOT, "variants")
os.makedirs(VAR, exist_ok=True)

# ── 配色方案（每套 3 档：深/主/浅）──
PALETTES = {
    "mono":  dict(deep="#0F172A", main="#334155", light="#94A3B8", name="石墨黑"),
    "cool":  dict(deep="#1E40AF", main="#3B82F6", light="#7DD3FC", name="靛蓝青"),
    "warm":  dict(deep="#C2410C", main="#F97316", light="#FDBA74", name="橙珊瑚"),
    "forest":dict(deep="#065F46", main="#10B981", light="#6EE7B7", name="翡翠绿"),
}

def lerp(c1, c2, t):
    """两色线性插值（#RRGGBB）"""
    a = [int(c1[i:i+2], 16) for i in (1, 3, 5)]
    b = [int(c2[i:i+2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(a[i]+(b[i]-a[i])*t):02X}" for i in range(3))

def fill_vgrad(draw, points, c1, c2, step=4):
    """多边形垂直渐变填充（逐水平线插值）"""
    ys = [p[1] for p in points]
    ymin, ymax = min(ys), max(ys)
    for y in range(ymin, ymax + 1, step):
        t = (y - ymin) / (ymax - ymin)
        col = lerp(c1, c2, t)
        xs = []
        n = len(points)
        for i in range(n):
            x1, y1 = points[i]
            x2, y2 = points[(i + 1) % n]
            if y2 == y1:
                continue
            if (y1 <= y <= y2) or (y2 <= y <= y1):
                xs.append(x1 + (x2 - x1) * (y - y1) / (y2 - y1))
        if xs:
            draw.line([(min(xs), y), (max(xs), y)], fill=col, width=step + 1)

# ═══════════════ 6 种形状 ═══════════════
C = 1024  # 画布中心

def sh_necktie(d, p):
    """具象领带：圆角结 + 渐变领身 + 底部尖角"""
    d.rounded_rectangle([784, 384, 1264, 800], radius=128, fill=p["deep"])
    fill_vgrad(d, [(880, 800), (1168, 800), (1472, 1650), (1024, 1900), (576, 1650)], p["deep"], p["light"])
    d.line([(1024, 800), (1024, 1830)], fill=p["deep"], width=36)

def sh_trit(d, p):
    """三态分子：三节点 + 连线 + 中心核心"""
    pts = [(1024, 560), (380, 1470), (1668, 1470)]
    for i in range(3):
        a, b = pts[i], pts[(i + 1) % 3]
        d.line([a, b], fill=p["light"], width=44)
    for (x, y) in pts:
        d.ellipse([x-200, y-200, x+200, y+200], fill=p["deep"])
        d.ellipse([x-78, y-78, x+78, y+78], fill=p["main"])
    d.ellipse([C-140, C-140, C+140, C+140], fill=p["main"])

def sh_infinity(d, p):
    """自举环：∞ 双环 + 中心点"""
    for cx in (560, 1488):
        d.ellipse([cx-430, C-430, cx+430, C+430], outline=p["main"], width=190)
    d.ellipse([C-120, C-120, C+120, C+120], fill=p["deep"])

def sh_pipeline(d, p):
    """四段流水线：Z 形折线 + 4 节点 + 出口箭头"""
    d.line([(480, 1560), (480, 1100), (1024, 1100), (1024, 640), (1568, 640)],
           fill=p["main"], width=104, joint="curve")
    d.line([(1568, 640), (1820, 640)], fill=p["light"], width=104)
    d.polygon([(1720, 500), (1930, 640), (1720, 780)], fill=p["light"])
    for (x, y) in [(480, 1560), (480, 1100), (1024, 1100), (1024, 640)]:
        d.ellipse([x-170, y-170, x+170, y+170], fill=p["deep"])

def sh_knot(d, p):
    """绳结：竖线穿过圆环（结眼）"""
    d.line([(C, 420), (C, 1300)], fill=p["deep"], width=170)
    d.ellipse([C-460, 1420-460, C+460, 1420+460], outline=p["main"], width=180)
    d.ellipse([C-120, 1420-120, C+120, 1420+120], fill=p["light"])

def sh_triangle(d, p):
    """三角结：圆角三角形 + 三节点 + 中心"""
    pts = [(1024, 430), (330, 1560), (1718, 1560)]
    d.line([pts[0], pts[1], pts[2], pts[0]], fill=p["main"], width=120, joint="curve")
    for (x, y) in pts:
        d.ellipse([x-150, y-150, x+150, y+150], fill=p["deep"])
    d.ellipse([C-120, C-120, C+120, C+120], fill=p["deep"])

SHAPES = {
    "necktie":   ("领带", sh_necktie),
    "trit":      ("三态", sh_trit),
    "infinity":  ("自举环", sh_infinity),
    "pipeline":  ("流水线", sh_pipeline),
    "knot":      ("绳结", sh_knot),
    "triangle":  ("三角结", sh_triangle),
}

# ═══════════════ 渲染 ═══════════════
def render(shape_fn, pal):
    img = Image.new("RGB", (S, S), "#F8FAFC")
    d = ImageDraw.Draw(img)
    shape_fn(d, pal)
    return img.resize((OUT_S, OUT_S), Image.Resampling.LANCZOS)

# 单个变体
for sname, (_, fn) in SHAPES.items():
    for pname, pal in PALETTES.items():
        img = render(fn, pal)
        img.save(os.path.join(VAR, f"v_{sname}_{pname}.png"))
print("variants done:", 6 * len(PALETTES))

# 网格总览：6 行 × 4 列，每格 512 + 标签
try:
    font = ImageFont.truetype("C:/Windows/Fonts/msyh.ttc", 34)
    font_s = ImageFont.truetype("C:/Windows/Fonts/msyh.ttc", 26)
except OSError:
    font = ImageFont.load_default()
    font_s = font

CELL = OUT_S + 90
GAP = 30
cols, rows = len(PALETTES), len(SHAPES)
W = cols * CELL + (cols + 1) * GAP
H = rows * CELL + (rows + 1) * GAP
grid = Image.new("RGB", (W, H), "#FFFFFF")
gd = ImageDraw.Draw(grid)

for r, (sname, (sdesc, fn)) in enumerate(SHAPES.items()):
    for c, (pname, pal) in enumerate(PALETTES.items()):
        x0 = GAP + c * (CELL + GAP)
        y0 = GAP + r * (CELL + GAP)
        img = render(fn, pal)
        grid.paste(img, (x0 + 45, y0 + 45))
        label = f"{sdesc} · {pal['name']}"
        gd.text((x0 + 45, y0 + OUT_S + 62), label, fill="#334155", font=font)

grid.save(os.path.join(ROOT, "grid.png"))
print("grid saved:", os.path.join(ROOT, "grid.png"), f"{W}x{H}")
