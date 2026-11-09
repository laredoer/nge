#!/usr/bin/env python3
"""生成长坂坡的像素精灵图集（只用标准库）：

    python3 examples/sanguo/tools/make_sprites.py

输出 assets/{player,troops,boss,fx}.png 和 ../sheets.json（每张图集的格子、尺寸、脚底位置、各动作的帧号，TS 直接读）。
人物按逻辑像素画（骨架 + 两节 IK 的四肢），描一圈深色轮廓，再放大 SCALE 倍：引擎的贴图是线性过滤，
放大后只有色块边上一点点糊，看着还是像素画。全部朝右画，朝左时 TS 把宽度写成负的（镜像）。
"""

import json
import math
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXAMPLE = HERE.parent
OUT = EXAMPLE / "assets"
SCALE = 3
OUTLINE = (20, 16, 26, 255)


def hexc(s, a=255):
    s = s.lstrip("#")
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16), a)


class Canvas:
    """逻辑像素画布，y 朝下。`xf` 把画的坐标变一下（整只人物前后倾、马立起来）。"""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.px = [[None] * w for _ in range(h)]
        self.xf = lambda p: p

    def set(self, x, y, c):
        x, y = math.floor(x), math.floor(y)
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[y][x] = c

    def dot(self, p, c):
        self.set(*self.xf(p), c)

    def disc(self, center, r, c):
        cx, cy = self.xf(center)
        for y in range(math.floor(cy - r) - 1, math.floor(cy + r) + 2):
            for x in range(math.floor(cx - r) - 1, math.floor(cx + r) + 2):
                if (x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2 <= r * r:
                    self.set(x, y, c)

    def line(self, a, b, c, width=1.0):
        """粗线：沿线按半径 width / 2 盖圆点；width ≤ 1 时一个像素宽。"""
        (ax, ay), (bx, by) = self.xf(a), self.xf(b)
        steps = int(max(abs(bx - ax), abs(by - ay)) * 2) + 1
        for i in range(steps + 1):
            t = i / steps
            x, y = ax + (bx - ax) * t, ay + (by - ay) * t
            if width <= 1:
                self.set(x, y, c)
            else:
                r = width / 2
                for py in range(math.floor(y - r) - 1, math.floor(y + r) + 2):
                    for qx in range(math.floor(x - r) - 1, math.floor(x + r) + 2):
                        if (qx + 0.5 - x) ** 2 + (py + 0.5 - y) ** 2 <= r * r:
                            self.set(qx, py, c)

    def poly(self, pts, c):
        """扫描线填多边形（按像素中心判断）。"""
        pts = [self.xf(p) for p in pts]
        ys = [p[1] for p in pts]
        for y in range(math.floor(min(ys)), math.ceil(max(ys)) + 1):
            sy = y + 0.5
            xs = []
            for i in range(len(pts)):
                (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % len(pts)]
                if (y0 <= sy < y1) or (y1 <= sy < y0):
                    xs.append(x0 + (sy - y0) * (x1 - x0) / (y1 - y0))
            xs.sort()
            for i in range(0, len(xs) - 1, 2):
                for x in range(math.ceil(xs[i] - 0.5), math.floor(xs[i + 1] - 0.5) + 1):
                    self.set(x, y, c)

    def ellipse(self, center, rx, ry, c):
        cx, cy = center
        self.poly([(cx + rx * math.cos(a * math.pi / 12), cy + ry * math.sin(a * math.pi / 12)) for a in range(24)], c)

    def outline(self, c=OUTLINE):
        """透明像素挨着（上下左右）不透明的，描成轮廓色；半透明的特效不描。"""
        solid = [[p is not None and p[3] == 255 for p in row] for row in self.px]
        for y in range(self.h):
            for x in range(self.w):
                if self.px[y][x] is not None:
                    continue
                if any(0 <= x + dx < self.w and 0 <= y + dy < self.h and solid[y + dy][x + dx] for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))):
                    self.px[y][x] = c

    def rotated(self, pivot, degrees):
        """绕 pivot 转（顺时针为正，图片坐标），按最近邻取样，返回新画布。"""
        out = Canvas(self.w, self.h)
        a = math.radians(degrees)
        ca, sa = math.cos(a), math.sin(a)
        px, py = pivot
        for y in range(self.h):
            for x in range(self.w):
                # 反着转回原图找像素
                dx, dy = x + 0.5 - px, y + 0.5 - py
                sx, sy = px + dx * ca + dy * sa, py - dx * sa + dy * ca
                ix, iy = math.floor(sx), math.floor(sy)
                if 0 <= ix < self.w and 0 <= iy < self.h:
                    out.px[y][x] = self.px[iy][ix]
        return out

    def shifted(self, dx, dy):
        out = Canvas(self.w, self.h)
        for y in range(self.h):
            for x in range(self.w):
                if 0 <= x - dx < self.w and 0 <= y - dy < self.h:
                    out.px[y][x] = self.px[y - dy][x - dx]
        return out


def write_png(path, width, height, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    raw = b"".join(b"\0" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
    print(f"wrote {path.relative_to(EXAMPLE)} ({path.stat().st_size} bytes)")


def atlas(name, cells, cols, cw, ch):
    """把逻辑像素的格子按 SCALE 放大，一行 cols 个拼成一张图。"""
    rows_n = math.ceil(len(cells) / cols)
    W, H = cols * cw * SCALE, rows_n * ch * SCALE
    rows = []
    for y in range(H):
        row = bytearray()
        r, ly = divmod(y // SCALE, ch)
        for x in range(W):
            col, lx = divmod(x // SCALE, cw)
            i = r * cols + col
            p = cells[i].px[ly][lx] if i < len(cells) else None
            row += bytes(p if p is not None else (0, 0, 0, 0))
        rows.append(bytes(row))
    write_png(OUT / f"{name}.png", W, H, rows)
    return [cols, rows_n]


# ---------- 人物骨架 ----------

def dirv(deg):
    """0° 朝下，90° 朝前（右），180° 朝上（图片坐标，y 朝下）。"""
    a = math.radians(deg)
    return (math.sin(a), math.cos(a))


def add(p, v, k=1.0):
    return (p[0] + v[0] * k, p[1] + v[1] * k)


def ik(root, target, l1, l2, bend):
    """两节肢体：从 root 够到 target（够不着就伸直），关节往 bend（+1 前 / -1 后）那边弯，返回关节和末端。"""
    dx, dy = target[0] - root[0], target[1] - root[1]
    d = max(1e-3, min(math.hypot(dx, dy), l1 + l2 - 1e-3))
    ux, uy = dx / math.hypot(dx, dy) if (dx or dy) else 0, dy / math.hypot(dx, dy) if (dx or dy) else 1
    a = (l1 * l1 + d * d - l2 * l2) / (2 * d)
    h = math.sqrt(max(0.0, l1 * l1 - a * a))
    # 垂直于 root→target 的方向，挑朝 bend 那边的
    nx, ny = -uy, ux
    if nx * bend < 0 or (nx == 0 and ny * bend > 0):
        nx, ny = -nx, -ny
    joint = (root[0] + ux * a + nx * h, root[1] + uy * a + ny * h)
    end = (root[0] + ux * d, root[1] + uy * d)
    return joint, end


class Look:
    def __init__(self, **kw):
        self.__dict__.update(kw)


def shade(c, k):
    return (int(c[0] * k), int(c[1] * k), int(c[2] * k), c[3])


def humanoid(cv, cx, ground, pose, look):
    """画一个朝右的人：pose 写胯的偏移、上身前倾、两只脚、两只手的位置和兵器的角度。"""
    hip = (cx + pose.get("dx", 0), ground - 12 + pose.get("dy", 0))
    lean = pose.get("lean", 0)
    shoulder = add(hip, dirv(180 - lean), 9)
    head = add(shoulder, dirv(180 - lean), 4.5)
    feet = [(cx + fx, ground - lift) for fx, lift in pose.get("feet", [(-3, 0), (3, 0)])]
    hands = [add(shoulder, h) for h in pose.get("hands", [(-3, 8), (3, 8)])]
    legs = [ik(hip, f, 6.5, 6.5, 1) for f in feet]
    arms = [ik(shoulder, h, 5, 5, -1) for h in hands]

    def leg(i, k):
        knee, foot = legs[i]
        cv.line(hip, knee, shade(look.pants, k), 3)
        cv.line(knee, foot, shade(look.pants, k), 3)
        cv.line(add(foot, (0.5, -0.5)), add(foot, (2.5, -0.5)), shade(look.boots, k), 2)
        cv.line(foot, add(knee, (foot[0] - knee[0], foot[1] - knee[1]), 0.6), shade(look.boots, k), 3)

    def arm(i, k):
        elbow, hand = arms[i]
        cv.line(shoulder, elbow, shade(look.sleeve, k), 3)
        cv.line(elbow, hand, shade(look.sleeve, k), 2.6)
        cv.disc(hand, 1.2, shade(look.skin, k))

    if look.back_item:
        look.back_item(cv, shoulder, hip, lean)
    arm(0, 0.72)
    if pose.get("weapon_back"):
        look.weapon(cv, arms[0][1], pose["weapon_back"])
    leg(0, 0.72)
    leg(1, 1.0)
    # 下摆：胯两边往两个膝盖张开
    kb, kf = legs[0][0], legs[1][0]
    skirt = [add(hip, (-3, -1)), add(hip, (3, -1)), add(kf, (2, look.skirt)), add(kb, (-2, look.skirt))]
    cv.poly(skirt, look.robe)
    cv.line(hip, shoulder, look.robe, 6)
    cv.line(add(hip, (0, -2)), add(shoulder, (0, 1)), look.armor, 4)
    cv.line(add(hip, (-3, -1.5)), add(hip, (3, -1.5)), look.belt, 1.5)
    look.head(cv, head, lean)
    if pose.get("weapon") is not None:
        look.weapon(cv, arms[1][1], pose["weapon"])
    arm(1, 1.0)


def head_with(helmet, crest=None, plume=None, band=None):
    def draw(cv, c, lean):
        cv.disc(c, 3.3, SKIN)
        if band:
            cv.poly([add(c, (-3.6, 0.5)), add(c, (-3, -2.6)), add(c, (0, -3.8)), add(c, (3, -2.8)), add(c, (3.4, -1.2)), add(c, (-1, -1.2)), add(c, (-1.5, 1.5))], HAIR)
        if helmet:
            cv.poly([add(c, (-3.6, 0)), add(c, (-3.4, -2.6)), add(c, (0, -4.2)), add(c, (3.4, -2.6)), add(c, (3.6, -0.4))], helmet)
        if band:
            cv.line(add(c, (-3.2, -1)), add(c, (3.2, -1)), band, 1.4)
            cv.line(add(c, (-3.5, -1)), add(c, (-6, 1.5)), band, 1)
        if crest:
            cv.line(add(c, (0, -4)), add(c, (0, -6.5)), crest, 1.6)
        if plume:
            cv.line(add(c, (-0.5, -4.5)), add(c, (-4, -7.5)), plume, 2)
            cv.line(add(c, (-4, -7.5)), add(c, (-6.5, -6)), plume, 1.5)
        # 眼睛
        cv.set(c[0] + 1.6, c[1] - 0.2, (30, 20, 20, 255))
    return draw


def spear(cv, hand, w):
    """枪：w = (角度：0 朝前、90 朝上，握在离枪尾多少)。"""
    angle, grip = w
    d = (math.cos(math.radians(angle)), -math.sin(math.radians(angle)))
    tail, tip = add(hand, d, -grip), add(hand, d, 30 - grip)
    cv.line(tail, tip, hexc("7a5230"), 1.6)
    cv.poly([add(tip, d, 5), add(tip, (-d[1], d[0]), 1.6), add(tip, (d[1], -d[0]), 1.6)], hexc("e8eef4"))
    cv.line(add(tip, d, -1), add(tip, d, -3.5), hexc("d23a30"), 2.2)


def saber(cv, hand, w):
    angle, _ = w
    d = (math.cos(math.radians(angle)), -math.sin(math.radians(angle)))
    n = (-d[1], d[0])
    cv.line(add(hand, d, -2), add(hand, d, 1), hexc("6b4a2a"), 1.6)
    base = add(hand, d, 1.5)
    # 刀身微弯，刀背一侧加一条暗线
    pts = [add(base, n, 1.2), add(add(base, d, 8), n, 1.6), add(add(base, d, 12), n, 0.2), add(add(base, d, 11), n, -1), add(base, n, -0.8)]
    cv.poly(pts, hexc("c4ccd4"))
    cv.line(add(hand, n, 2), add(hand, n, -2), hexc("8a7a40"), 1)


def bow(cv, hand, w):
    """弓：w = (拉开多少 0..1, _)。弓竖着，弦往后拉。"""
    pull, _ = w
    top, bottom = add(hand, (1.5, -8)), add(hand, (1.5, 8))
    mid = add(hand, (3.5, 0))
    cv.line(top, mid, hexc("8b5a2b"), 1.6)
    cv.line(mid, bottom, hexc("8b5a2b"), 1.6)
    nock = add(hand, (-1 - pull * 7, 0))
    cv.line(top, nock, hexc("e6e0d0"), 1)
    cv.line(nock, bottom, hexc("e6e0d0"), 1)
    if pull > 0.2:
        cv.line(nock, add(hand, (5, 0)), hexc("a07850"), 1)
        cv.set(hand[0] + 5, hand[1], hexc("cfd6dc"))


def quiver(cv, shoulder, hip, lean):
    cv.line(add(shoulder, (-3, -1)), add(hip, (-4, 0)), hexc("5a3a22"), 3)
    cv.line(add(shoulder, (-3, -1)), add(shoulder, (-5, -4)), hexc("d8d0c0"), 1)


SKIN = hexc("e8b48a")
HAIR = hexc("1c1418")

PLAYER = Look(
    pants=hexc("9aa3b4"), boots=hexc("3a3340"), sleeve=hexc("e4e8ef"), skin=SKIN, robe=hexc("f2f4f8"), armor=hexc("aab4c3"),
    belt=hexc("b0302a"), skirt=2, weapon=spear, back_item=None, head=head_with(hexc("c9d1dc"), plume=hexc("d23a30")),
)
SOLDIER = Look(
    pants=hexc("23272f"), boots=hexc("15161a"), sleeve=hexc("2a3a40"), skin=SKIN, robe=hexc("2b4e52"), armor=hexc("1d3337"),
    belt=hexc("6b5a2a"), skirt=1, weapon=saber, back_item=None, head=head_with(hexc("22343a"), crest=hexc("0f1a1d")),
)
ARCHER = Look(
    pants=hexc("2a2d30"), boots=hexc("17181b"), sleeve=hexc("3a5552"), skin=SKIN, robe=hexc("3d6a66"), armor=hexc("2c4a47"),
    belt=hexc("5a3a22"), skirt=0, weapon=bow, back_item=quiver, head=head_with(None, band=hexc("1a2226")),
)

# 步兵格子：64 × 48 逻辑像素，脚底在第 44 行（下面留一行给轮廓），人站在中间
FOOT_W, FOOT_H, FOOT_GROUND = 80, 48, 44


def frame(look, pose, rotate=None):
    cv = Canvas(FOOT_W, FOOT_H)
    humanoid(cv, FOOT_W // 2, FOOT_GROUND, pose, look)
    if rotate:
        # 倒地：绕脚底往后转，再挪回地面
        cv = cv.rotated((FOOT_W // 2, FOOT_GROUND), rotate[0]).shifted(*rotate[1])
    cv.outline()
    return cv


def walk(i, stride=4, arms=None):
    s = [1, 0, -1, 0][i] * stride
    return {"dy": -abs([0, 1, 0, 1][i]), "feet": [(-s, 0 if s <= 0 else 1), (s, 0 if s >= 0 else 1)], "lean": 4}


def player_frames():
    F = {}
    order = []

    def put(name, poses):
        F[name] = list(range(len(order), len(order) + len(poses)))
        order.extend(poses)

    hold = {"hands": [(4, 6), (8, 5)], "weapon": (12, 14)}
    put("idle", [dict(hold, feet=[(-4, 0), (4, 0)], dy=0), dict(hold, feet=[(-4, 0), (4, 0)], dy=1)])
    put("walk", [dict(walk(i), **hold) for i in range(4)])
    # 第一段：收枪、刺出去、收回
    put("atk1", [
        {"feet": [(-5, 0), (4, 0)], "lean": -4, "hands": [(-2, 6), (2, 6)], "weapon": (4, 20)},
        {"feet": [(-6, 0), (7, 0)], "lean": 14, "dy": 1, "hands": [(7, 3), (13, 2)], "weapon": (0, 12)},
        {"feet": [(-5, 0), (5, 0)], "lean": 6, "hands": [(4, 5), (9, 4)], "weapon": (6, 14)},
    ])
    # 第二段：从后上方横扫到前下方
    put("atk2", [
        {"feet": [(-5, 0), (4, 0)], "lean": -6, "hands": [(-4, -2), (0, -4)], "weapon": (150, 12)},
        {"feet": [(-6, 0), (6, 0)], "lean": 12, "dy": 1, "hands": [(6, 2), (11, 4)], "weapon": (-10, 10)},
        {"feet": [(-6, 0), (6, 0)], "lean": 16, "dy": 2, "hands": [(6, 6), (10, 8)], "weapon": (-30, 10)},
    ])
    # 第三段：压低、往上挑、举高、收势
    put("atk3", [
        {"feet": [(-7, 0), (6, 0)], "lean": 18, "dy": 3, "hands": [(5, 8), (9, 9)], "weapon": (-25, 12)},
        {"feet": [(-6, 0), (6, 0)], "lean": 6, "dy": 0, "hands": [(6, 0), (9, -2)], "weapon": (45, 12)},
        {"feet": [(-5, 0), (5, 2)], "lean": -6, "dy": -1, "hands": [(3, -6), (5, -9)], "weapon": (80, 10)},
        {"feet": [(-5, 0), (5, 0)], "lean": 2, "hands": [(4, 4), (8, 4)], "weapon": (20, 14)},
    ])
    # 突刺：压低身子，枪平着往前
    put("thrust", [
        {"feet": [(-9, 0), (7, 0)], "lean": 24, "dy": 3, "hands": [(9, 4), (14, 4)], "weapon": (2, 8)},
        {"feet": [(-11, 1), (9, 0)], "lean": 30, "dy": 4, "hands": [(11, 5), (16, 5)], "weapon": (0, 6)},
        {"feet": [(-6, 0), (6, 0)], "lean": 10, "dy": 1, "hands": [(6, 5), (10, 5)], "weapon": (6, 12)},
    ])
    put("jump", [
        {"feet": [(-4, 5), (4, 7)], "lean": 4, "dy": -4, "hands": [(3, 3), (8, 2)], "weapon": (30, 14)},
        {"feet": [(-5, 3), (3, 2)], "lean": 0, "dy": -3, "hands": [(3, 5), (8, 4)], "weapon": (10, 14)},
    ])
    # 下劈：枪尖朝下
    put("plunge", [
        {"feet": [(-4, 6), (3, 7)], "lean": 10, "dy": -4, "hands": [(4, -4), (6, -6)], "weapon": (100, 12)},
        {"feet": [(-4, 4), (4, 5)], "lean": 20, "dy": -3, "hands": [(7, 6), (8, 9)], "weapon": (-80, 8)},
    ])
    put("hurt", [{"feet": [(-6, 0), (3, 0)], "lean": -18, "dy": 1, "hands": [(-6, 2), (-1, 4)], "weapon": (40, 14)}])
    lying = {"feet": [(-4, 0), (4, 0)], "lean": 0, "hands": [(-3, 6), (3, 7)], "weapon": (90, 14)}
    cells = [frame(PLAYER, p) for p in order]
    F["down"] = [len(cells), len(cells) + 1]
    cells.append(frame(PLAYER, dict(lying, lean=-10), rotate=(-50, (-4, -2))))
    cells.append(frame(PLAYER, dict(lying, weapon=(100, 14)), rotate=(-88, (-4, -3))))
    return cells, F


def troop_frames():
    F = {}
    cells = []

    def put(name, look, poses):
        F[name] = list(range(len(cells), len(cells) + len(poses)))
        cells.extend(frame(look, p) if not isinstance(p, tuple) else frame(look, p[0], rotate=p[1]) for p in poses)

    hold = {"hands": [(-3, 7), (5, 5)], "weapon": (50, 0)}
    put("soldier.idle", SOLDIER, [dict(hold), dict(hold, dy=1)])
    put("soldier.walk", SOLDIER, [dict(walk(i), **hold) for i in range(4)])
    put("soldier.attack", SOLDIER, [
        {"feet": [(-5, 0), (4, 0)], "lean": -8, "hands": [(-4, 2), (1, -6)], "weapon": (110, 0)},
        {"feet": [(-6, 0), (7, 0)], "lean": 16, "dy": 2, "hands": [(4, 6), (9, 5)], "weapon": (-30, 0)},
        {"feet": [(-5, 0), (5, 0)], "lean": 8, "dy": 1, "hands": [(2, 7), (7, 7)], "weapon": (-60, 0)},
    ])
    put("soldier.hurt", SOLDIER, [{"feet": [(-6, 0), (3, 0)], "lean": -18, "hands": [(-6, 2), (0, 3)], "weapon": (80, 0)}])
    put("soldier.down", SOLDIER, [({**hold, "lean": -10}, (-50, (-4, -2))), ({**hold, "weapon": (100, 0)}, (-88, (-4, -3)))])

    bow_hold = {"hands": [(-3, 7), (6, 4)], "weapon": (0, 0)}
    draw_pose = {"feet": [(-5, 0), (4, 0)], "lean": -2, "hands": [(1, 1), (9, 0)]}
    put("archer.idle", ARCHER, [dict(bow_hold), dict(bow_hold, dy=1)])
    put("archer.walk", ARCHER, [dict(walk(i), **bow_hold) for i in range(4)])
    put("archer.attack", ARCHER, [dict(draw_pose, weapon=(0.5, 0)), dict(draw_pose, weapon=(1.0, 0), hands=[(-1, 1), (9, 0)]), dict(draw_pose, weapon=(0.0, 0), lean=2)])
    put("archer.hurt", ARCHER, [{"feet": [(-6, 0), (3, 0)], "lean": -18, "hands": [(-6, 2), (0, 3)], "weapon": (0, 0)}])
    put("archer.down", ARCHER, [({**bow_hold, "lean": -10}, (-50, (-4, -2))), (bow_hold, (-88, (-4, -3)))])
    return cells, F


# ---------- 骑将 ----------

BOSS_W, BOSS_H, BOSS_GROUND = 96, 64, 60
HORSE = hexc("3a2a24")
MANE = hexc("16100e")
CAPE = hexc("8a1f2e")
BOSS_ARMOR = hexc("1f3236")
GOLD = hexc("c8a040")


def glaive(cv, hand, w):
    angle, grip = w
    d = (math.cos(math.radians(angle)), -math.sin(math.radians(angle)))
    n = (-d[1], d[0])
    tail, tip = add(hand, d, -grip), add(hand, d, 34 - grip)
    cv.line(tail, tip, hexc("4a3020"), 1.8)
    cv.poly([add(tip, n, 1), add(add(tip, d, 9), n, 3), add(tip, d, 12), add(add(tip, d, 4), n, -2)], hexc("d0d8e0"))
    cv.line(add(tip, d, -1), add(tip, d, -3), hexc("c03030"), 2.4)


def horse_and_rider(pose):
    """骑将：马（四条腿按蹄子位置弯）+ 坐在上面的将领，披风往后飘。pose 的 pitch 让整匹马绕后蹄抬起来。"""
    cv = Canvas(BOSS_W, BOSS_H)
    cx, g = BOSS_W // 2 - 4, BOSS_GROUND
    pitch = pose.get("pitch", 0)
    drop = pose.get("drop", 0)
    pivot = (cx - 12, g)
    a = math.radians(-pitch)

    def rot(p):
        dx, dy = p[0] - pivot[0], p[1] - pivot[1]
        return (pivot[0] + dx * math.cos(a) - dy * math.sin(a), pivot[1] + dx * math.sin(a) + dy * math.cos(a))

    cv.xf = rot
    body = (cx, g - 20 + pose.get("bob", 0) + drop)
    # 腿：后两条暗一点
    hooves = pose.get("hooves", [(-12, 0), (-8, 0), (9, 0), (13, 0)])
    hips = [add(body, (-10, 2)), add(body, (-8, 2)), add(body, (9, 2)), add(body, (11, 2))]
    for i in (0, 2, 1, 3):
        k = 0.7 if i in (0, 2) else 1.0
        foot = (cx + hooves[i][0], g - hooves[i][1])
        knee, end = ik(hips[i], foot, 9, 9, -1 if i < 2 else 1)
        c = shade(HORSE, k)
        cv.line(hips[i], knee, c, 3)
        cv.line(knee, end, c, 2.2)
        cv.line(add(end, (-1, 0)), add(end, (1, 0)), shade(MANE, k), 2)
    cv.line(add(body, (-14, -2)), add(body, (-20, 6 + pose.get("tail", 0))), MANE, 2.4)
    cv.ellipse(body, 15, 6.5, HORSE)
    cv.ellipse(add(body, (2, 1)), 10, 3.5, shade(HORSE, 1.25))
    neck = add(body, (11, -3))
    head = add(body, (19, -13 + pose.get("nod", 0)))
    cv.poly([add(neck, (-3, 3)), add(neck, (4, 2)), add(head, (2, 1)), add(head, (-3, -1))], HORSE)
    cv.poly([add(head, (-2, -2)), add(head, (3, -2)), add(head, (8, 2)), add(head, (7, 4)), add(head, (0, 3))], HORSE)
    cv.line(add(neck, (-2, 0)), add(head, (-2, -2)), MANE, 2)
    cv.dot(add(head, (3, -0.5)), (230, 200, 120, 255))
    # 鞍和马甲
    cv.poly([add(body, (-6, -6)), add(body, (5, -6)), add(body, (6, 0)), add(body, (-7, 0))], hexc("5a1a22"))
    cv.line(add(body, (-7, -1)), add(body, (6, -1)), GOLD, 1)
    # 将领
    hip = add(body, (-1, -8))
    lean = pose.get("lean", 4)
    shoulder = add(hip, dirv(180 - lean), 10)
    hd = add(shoulder, dirv(180 - lean), 4.8)
    flutter = pose.get("cape", 0)
    cv.poly([add(shoulder, (-2, -1)), add(shoulder, (1, 0)), add(hip, (-6 - flutter, 2)), add(hip, (-14 - flutter, 1 + flutter / 2)), add(hip, (-10 - flutter, -4))], CAPE)
    cv.line(hip, add(hip, (5, 5)), hexc("23272f"), 3)
    cv.line(add(hip, (5, 5)), add(hip, (5, 10)), hexc("15161a"), 2.6)
    cv.line(hip, shoulder, BOSS_ARMOR, 7)
    cv.line(add(hip, (-2, -3)), add(shoulder, (2, 2)), GOLD, 1)
    cv.disc(add(shoulder, (-1, 0)), 2.5, shade(BOSS_ARMOR, 1.3))
    cv.disc(hd, 3.5, SKIN)
    cv.poly([add(hd, (-3.8, 0.5)), add(hd, (-3.6, -2.8)), add(hd, (0, -4.5)), add(hd, (3.6, -2.8)), add(hd, (3.8, -0.4))], BOSS_ARMOR)
    cv.line(add(hd, (-1, -4)), add(hd, (-3, -9)), GOLD, 1.4)
    cv.line(add(hd, (1, -4)), add(hd, (3, -9)), GOLD, 1.4)
    cv.line(add(hd, (1.5, 1.5)), add(hd, (2.5, 3.5)), HAIR, 1.4)
    cv.dot(add(hd, (1.8, -0.4)), (30, 20, 20, 255))
    hand = add(shoulder, pose.get("hand", (7, 4)))
    elbow, hand = ik(shoulder, hand, 5.5, 5.5, -1)
    glaive(cv, hand, pose.get("weapon", (40, 14)))
    cv.line(shoulder, elbow, BOSS_ARMOR, 3.2)
    cv.line(elbow, hand, BOSS_ARMOR, 2.6)
    cv.disc(hand, 1.3, SKIN)
    cv.xf = lambda p: p
    cv.outline()
    return cv


def boss_frames():
    F = {}
    cells = []

    def put(name, poses):
        F[name] = list(range(len(cells), len(cells) + len(poses)))
        cells.extend(horse_and_rider(p) for p in poses)

    put("idle", [{}, {"bob": 1, "nod": 1, "tail": 1}])
    gait = [[(-14, 0), (-6, 2), (7, 2), (15, 0)], [(-10, 3), (-9, 0), (11, 0), (10, 3)], [(-6, 0), (-13, 2), (14, 2), (6, 0)], [(-9, 0), (-10, 3), (10, 3), (11, 0)]]
    put("walk", [{"hooves": h, "bob": i % 2, "tail": i % 2} for i, h in enumerate(gait)])
    # 冲锋前摇：马立起来，将领举刀
    put("windup", [
        {"pitch": 10, "hooves": [(-12, 0), (-9, 0), (10, 4), (14, 6)], "lean": -6, "hand": (2, -6), "weapon": (110, 12), "cape": 2},
        {"pitch": 18, "hooves": [(-12, 0), (-10, 0), (11, 8), (15, 10)], "lean": -10, "hand": (1, -8), "weapon": (125, 12), "cape": 3},
    ])
    put("charge", [
        {"hooves": [(-17, 1), (-14, 3), (15, 3), (19, 1)], "lean": 18, "hand": (9, 3), "weapon": (5, 16), "cape": 6, "tail": -3},
        {"hooves": [(-8, 3), (-5, 1), (6, 1), (9, 3)], "lean": 18, "bob": -1, "hand": (9, 3), "weapon": (5, 16), "cape": 8, "tail": -4},
    ])
    put("attack", [
        {"lean": -6, "hand": (1, -7), "weapon": (130, 12), "cape": 1},
        {"lean": 14, "hand": (9, 5), "weapon": (-20, 12), "cape": 3},
        {"lean": 8, "hand": (7, 6), "weapon": (-40, 12), "cape": 2},
    ])
    put("hurt", [{"lean": -16, "hand": (-2, 2), "weapon": (60, 12), "pitch": 4, "nod": -2}])
    put("down", [
        {"lean": -24, "hand": (-4, 3), "weapon": (150, 10), "drop": 4, "hooves": [(-12, 0), (-8, 0), (9, 3), (13, 3)], "nod": 3},
        {"lean": -60, "hand": (-6, 6), "weapon": (170, 10), "drop": 9, "hooves": [(-16, 0), (-12, 0), (13, 0), (17, 0)], "nod": 6, "tail": 3},
    ])
    return cells, F


# ---------- 特效 ----------

FX_W = FX_H = 48


def fx_frames():
    F = {}
    cells = []

    def put(name, cvs):
        F[name] = list(range(len(cells), len(cells) + len(cvs)))
        cells.extend(cvs)

    c = 24
    arrow = Canvas(FX_W, FX_H)
    arrow.line((10, c), (36, c), hexc("a07850"), 1)
    arrow.poly([(40, c + 0.5), (35, c - 2), (35, c + 3)], hexc("cfd6dc"))
    arrow.line((10, c - 1.5), (14, c), hexc("e8e0d0"), 1)
    arrow.line((10, c + 2), (14, c + 1), hexc("e8e0d0"), 1)
    arrow.outline()
    put("arrow", [arrow])

    def arc(start, end, r, width, color, cx=16, cy=c, squash=1.0, alpha=255):
        cv = Canvas(FX_W, FX_H)
        steps = 24
        for i in range(steps + 1):
            t = i / steps
            a = math.radians(start + (end - start) * t)
            w = width * math.sin(math.pi * t) + 0.6
            p = (cx + r * math.cos(a), cy - r * math.sin(a) * squash)
            core = (255, 255, 255, alpha)
            cv.line(p, p, color + (int(alpha * 0.8),), w + 1.5)
            cv.line(p, p, core, w * 0.6)
        return cv

    blue = (190, 225, 255)
    def jab(length, alpha):
        # 第一段的刺：一道往前收尖的光
        cv = Canvas(FX_W, FX_H)
        for i in range(length):
            t = i / length
            x = 46 - length + i
            cv.line((x, c), (x, c), blue + (int(alpha * (0.3 + 0.7 * t)),), 1 + 3.5 * math.sin(math.pi * t))
        cv.line((46 - length * 0.6, c), (46, c), (255, 255, 255, alpha), 1)
        return cv

    put("slash1", [jab(30, 255), jab(36, 140)])
    put("slash2", [arc(80, -40, 20, 3.5, blue), arc(70, -50, 21, 2.2, blue, alpha=150)])
    put("slash3", [arc(-60, 70, 19, 3.8, (255, 220, 160)), arc(-50, 80, 20, 2.4, (255, 220, 160), alpha=150)])

    def streak(length, alpha):
        cv = Canvas(FX_W, FX_H)
        for k, dy in enumerate((-4, 0, 3)):
            cv.line((44 - length + k * 5, c + dy), (44, c + dy), blue + (int(alpha * (0.5 if dy else 1)),), 1.5 if dy else 2.5)
        cv.line((44 - length, c), (46, c), (255, 255, 255, alpha), 1)
        return cv

    put("thrust", [streak(40, 255), streak(30, 140)])

    def ring(rx, alpha):
        cv = Canvas(FX_W, FX_H)
        for i in range(32):
            a = 2 * math.pi * i / 32
            p = (c + rx * math.cos(a), 34 + rx * 0.3 * math.sin(a))
            cv.line(p, p, (255, 230, 180, alpha), 2)
        return cv

    put("shock", [ring(12, 255), ring(20, 140)])

    def spark(r, alpha):
        cv = Canvas(FX_W, FX_H)
        for i in range(8):
            a = 2 * math.pi * i / 8 + 0.2
            length = r if i % 2 == 0 else r * 0.55
            cv.line((c, c), (c + length * math.cos(a), c + length * math.sin(a)), (255, 240, 200, alpha), 1.6)
        cv.disc((c, c), r * 0.3, (255, 255, 255, alpha))
        return cv

    put("spark", [spark(6, 255), spark(10, 220), spark(13, 120)])

    def smoke(k, alpha):
        cv = Canvas(FX_W, FX_H)
        for i, (dx, dy, r) in enumerate([(-6, 4, 6), (5, 3, 6), (0, -2, 7), (-3, -7, 5), (6, -6, 4)]):
            g = 150 + i * 8
            cv.disc((c + dx * (1 + k * 0.35), 30 + dy * (1 + k * 0.3) - k * 3), r * (1 + k * 0.25), (g, g - 6, g - 14, alpha))
        return cv

    put("smoke", [smoke(k, a) for k, a in enumerate((230, 190, 130, 70))])
    return cells, F


def main():
    OUT.mkdir(exist_ok=True)
    sheets = {}
    for name, (cells, frames), cols, (cw, ch, ground) in [
        ("player", player_frames(), 8, (FOOT_W, FOOT_H, FOOT_GROUND)),
        ("troops", troop_frames(), 8, (FOOT_W, FOOT_H, FOOT_GROUND)),
        ("boss", boss_frames(), 4, (BOSS_W, BOSS_H, BOSS_GROUND)),
        ("fx", fx_frames(), 6, (FX_W, FX_H, FX_H // 2)),
    ]:
        grid = atlas(name, cells, cols, cw, ch)
        sheets[name] = {
            "texture": f"asset:/@sanguo/{name}.png",
            "grid": grid,
            # 一格在世界里画多大（point，和放大后的像素一样）
            "size": [cw * SCALE, ch * SCALE],
            # 脚底（逻辑像素第 ground 行的底边）离格子中心多远，往下为正
            "foot": (ground + 1) * SCALE - ch * SCALE // 2,
            "frames": frames,
        }
    path = EXAMPLE / "sheets.json"
    path.write_text(json.dumps(sheets, ensure_ascii=False, indent=1) + "\n")
    print(f"wrote {path.relative_to(EXAMPLE)}")


if __name__ == "__main__":
    main()
