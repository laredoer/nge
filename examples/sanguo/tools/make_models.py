#!/usr/bin/env python3
"""生成长坂坡的 3D 物件（.glb，贴图内嵌，只用标准库），复用 tools/gen_glb.py 的 Builder：

    python3 examples/sanguo/tools/make_models.py

输出 assets/ 下：ground（一段地面）、wall（城墙残段）、cart / cart_fire（粮车、着火的粮车）、banner（会飘的旌旗）、
barricade（拒马）、bridge（长坂桥和桥下的河）。

纵深的换算（和 belt.ts 一致）：物件按「带子坐标」(x, 高 h, 纵深 d) 建，d 越大离镜头越远。写进模型时换成
(x, h + d, -d)：往里走一格，画面上抬高一格、离相机远一格。这样物件放在 (x, 地面 y + d0, z0 - d0)
时，每个顶点的屏幕高度和深度都和站在同一纵深的精灵对得上，物件挡住后面的人、被前面的人挡住。
这个换算是个剪切（行列式 -1，镜像），法线按逆转置换算，三角形的绕向按换算后的法线重新排。
"""

import math
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / "tools"))
from gen_glb import Builder, png, qmul, quat  # noqa: E402

OUT = Path(__file__).resolve().parent.parent / "assets"


def belt(p):
    x, h, d = p
    return (x, h + d, -d)


def belt_normal(n):
    # 剪切 M 的逆转置：(nx, nh, nd) → (nx, nh, nh - nd)
    x, h, d = n
    v = (x, h, h - d)
    length = math.sqrt(sum(c * c for c in v)) or 1.0
    return tuple(c / length for c in v)


def sub(a, b):
    return tuple(a[i] - b[i] for i in range(3))


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return sum(a[i] * b[i] for i in range(3))


def norm(a):
    length = math.sqrt(dot(a, a)) or 1.0
    return tuple(c / length for c in a)


class Mesh:
    """按材质分组攒三角形；点都是带子坐标，写出去时再换算。"""

    def __init__(self):
        self.groups = {}

    def poly(self, pts, n, mat, uvs=None):
        """凸多边形（扇形三角化），n 是带子坐标里朝外的法线。"""
        g = self.groups.setdefault(mat, ([], [], [], []))
        pos, nor, uv, idx = g
        world = [belt(p) for p in pts]
        wn = belt_normal(n)
        base = len(pos)
        for i, p in enumerate(world):
            pos.append(p)
            nor.append(wn)
            uv.append(uvs[i] if uvs else (0.0, 0.0))
        for i in range(1, len(pts) - 1):
            a, b, c = base, base + i, base + i + 1
            # 换算后按法线摆正绕向（逆时针朝外）
            if dot(cross(sub(world[i], world[0]), sub(world[i + 1], world[0])), wn) < 0:
                b, c = c, b
            idx += [a, b, c]

    def box(self, x0, x1, h0, h1, d0, d1, mat, tile=64.0, skip=()):
        """长方体；贴图按面在世界里的大小平铺（每 tile 个单位一张）。"""
        faces = {
            "front": ([(x0, h0, d0), (x1, h0, d0), (x1, h1, d0), (x0, h1, d0)], (0, 0, -1), lambda p: (p[0], -p[1])),
            "back": ([(x1, h0, d1), (x0, h0, d1), (x0, h1, d1), (x1, h1, d1)], (0, 0, 1), lambda p: (-p[0], -p[1])),
            "top": ([(x0, h1, d0), (x1, h1, d0), (x1, h1, d1), (x0, h1, d1)], (0, 1, 0), lambda p: (p[0], -p[2])),
            "bottom": ([(x0, h0, d1), (x1, h0, d1), (x1, h0, d0), (x0, h0, d0)], (0, -1, 0), lambda p: (p[0], p[2])),
            "left": ([(x0, h0, d1), (x0, h0, d0), (x0, h1, d0), (x0, h1, d1)], (-1, 0, 0), lambda p: (-p[2], -p[1])),
            "right": ([(x1, h0, d0), (x1, h0, d1), (x1, h1, d1), (x1, h1, d0)], (1, 0, 0), lambda p: (p[2], -p[1])),
        }
        for name, (pts, n, uvf) in faces.items():
            if name not in skip:
                self.poly(pts, n, mat, [(uvf(p)[0] / tile, uvf(p)[1] / tile) for p in pts])

    def beam(self, a, b, r, mat):
        """方截面的木头，从 a 到 b（带子坐标），半宽 r。"""
        axis = norm(sub(b, a))
        up = (0, 1, 0) if abs(axis[1]) < 0.9 else (1, 0, 0)
        u = norm(cross(axis, up))
        v = cross(u, axis)
        corners = [(su, sv) for su, sv in ((-1, -1), (1, -1), (1, 1), (-1, 1))]
        ring = lambda p: [tuple(p[i] + r * (su * u[i] + sv * v[i]) for i in range(3)) for su, sv in corners]
        ra, rb = ring(a), ring(b)
        for k in range(4):
            j = (k + 1) % 4
            n = norm(tuple(corners[k][0] * u[i] + corners[k][1] * v[i] + corners[j][0] * u[i] + corners[j][1] * v[i] for i in range(3)))
            self.poly([ra[k], ra[j], rb[j], rb[k]], n, mat)
        self.poly(list(reversed(ra)), tuple(-c for c in axis), mat)
        self.poly(rb, axis, mat)

    def wheel(self, x, h, d0, d1, r, mat, sides=12):
        """轴沿纵深的轮子。"""
        ring = [(x + r * math.cos(2 * math.pi * k / sides), h + r * math.sin(2 * math.pi * k / sides)) for k in range(sides)]
        self.poly([(px, ph, d0) for px, ph in ring], (0, 0, -1), mat)
        self.poly([(px, ph, d1) for px, ph in reversed(ring)], (0, 0, 1), mat)
        for k in range(sides):
            (ax, ah), (bx, bh) = ring[k], ring[(k + 1) % sides]
            n = (math.cos(2 * math.pi * (k + 0.5) / sides), math.sin(2 * math.pi * (k + 0.5) / sides), 0)
            self.poly([(ax, ah, d0), (bx, bh, d0), (bx, bh, d1), (ax, ah, d1)], n, mat)

    def write(self, b, materials):
        """写成一个网格（每种材质一个图元），返回网格编号。"""
        prims = []
        for mat, (pos, nor, uv, idx) in self.groups.items():
            prims.append({
                "attributes": {"POSITION": b.accessor(pos, "VEC3"), "NORMAL": b.accessor(nor, "VEC3"), "TEXCOORD_0": b.accessor(uv, "VEC2")},
                "indices": b.accessor(idx, "SCALAR", 5125),
                "material": materials[mat],
            })
        b.gltf.setdefault("meshes", []).append({"primitives": prims})
        return len(b.gltf["meshes"]) - 1


def material(b, name, color, rough=0.85, texture=None, emissive=None, double=False):
    m = {"name": name, "pbrMetallicRoughness": {"baseColorFactor": [*color, 1.0], "metallicFactor": 0.0, "roughnessFactor": rough}}
    if texture is not None:
        m["pbrMetallicRoughness"]["baseColorTexture"] = {"index": b.image(texture)}
    if emissive:
        m["emissiveFactor"] = emissive
    if double:
        m["doubleSided"] = True
    b.gltf.setdefault("materials", []).append(m)
    return len(b.gltf["materials"]) - 1


def finish(b, mesh, name):
    b.gltf["nodes"] = [{"name": name, "mesh": mesh}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write(OUT / f"{name}.glb")


def clamp(v):
    return max(0, min(255, int(v)))


def noise_tex(size, base, spread, seed, extra=None):
    rng = random.Random(seed)
    cells = [[rng.uniform(-1, 1) for _ in range(size)] for _ in range(size)]

    def pixel(x, y):
        n = cells[y][x] * 0.6 + cells[y // 4 * 4][x // 4 * 4] * 0.4
        c = [clamp(base[i] + n * spread) for i in range(3)]
        if extra:
            c = extra(x, y, c, rng)
        return (*c, 255)

    return png(size, size, pixel)


# ---------- 物件 ----------

GROUND_LEN = 400
# 地面从镜头前很远一直铺到城墙后面；厚 60（桥边能看到河岸）
GROUND_NEAR, GROUND_FAR, GROUND_DEEP = -320, 230, -60


def ground():
    b = Builder()

    def dirt(x, y, c, rng):
        # 零星的草：深一点的绿点
        if (x * 7 + y * 13) % 37 == 0 or (x * 11 + y * 5) % 53 == 0:
            return [clamp(c[0] * 0.55), clamp(c[1] * 0.75), clamp(c[2] * 0.45)]
        return c

    top = material(b, "dirt", (1, 1, 1), texture=noise_tex(64, (150, 118, 84), 26, 7, dirt))
    side = material(b, "bank", (0.42, 0.33, 0.25))
    m = Mesh()
    m.box(0, GROUND_LEN, GROUND_DEEP, 0, GROUND_NEAR, GROUND_FAR, top, tile=96, skip=("left", "right", "bottom", "front", "back"))
    m.box(0, GROUND_LEN, GROUND_DEEP, 0, GROUND_NEAR, GROUND_FAR, side, skip=("top", "bottom", "back", "front"))
    finish(b, m.write(b, {top: top, side: side}), "ground")


def wall():
    """城墙残段：长 260、高约 120、厚 34，顶上有垛口，右边塌了一截，墙根堆着碎砖。"""
    b = Builder()

    def bricks(x, y, c, rng):
        row = y // 8
        if y % 8 == 0 or (x + (8 if row % 2 else 0)) % 16 == 0:
            return [clamp(v * 0.7) for v in c]
        return c

    brick = material(b, "brick", (1, 1, 1), texture=noise_tex(64, (128, 112, 98), 18, 3, bricks))
    dark = material(b, "rubble", (0.33, 0.29, 0.26))
    m = Mesh()
    heights = [120, 122, 121, 120, 119, 121, 118, 112, 104, 96, 80, 64, 70]
    step = 20
    for i, h in enumerate(heights):
        x0 = i * step
        m.box(x0, x0 + step, 0, h, 0, 34, brick)
        # 垛口：还站着的那段墙上每隔一格一个
        if h >= 115 and i % 2 == 0:
            m.box(x0 + 3, x0 + 15, h, h + 14, 0, 10, brick)
    rng = random.Random(5)
    for k in range(9):
        x = 190 + rng.uniform(0, 80)
        s = rng.uniform(6, 12)
        m.box(x, x + s, 0, s * 0.8, -12 + rng.uniform(0, 10), -12 + rng.uniform(0, 10) + s, dark)
    finish(b, m.write(b, {brick: brick, dark: dark}), "wall")


def cart(burning):
    """粮车：车板、四面挡板、两只轮子、车辕，上面摞着麻袋。着火的那辆麻袋烧黑了，带发光的火星（emissive，bloom 会晕开）。"""
    b = Builder()
    wood = material(b, "wood", (0.45, 0.30, 0.18) if not burning else (0.22, 0.15, 0.10))
    wheel = material(b, "wheel", (0.30, 0.20, 0.12) if not burning else (0.16, 0.11, 0.08))
    sack = material(b, "sack", (0.72, 0.62, 0.42) if not burning else (0.18, 0.14, 0.12))
    mats = {wood: wood, wheel: wheel, sack: sack}
    m = Mesh()
    m.box(-38, 38, 16, 22, -16, 16, wood)
    m.box(-38, 38, 22, 30, -16, -13, wood)
    m.box(-38, 38, 22, 30, 13, 16, wood)
    m.box(-38, -35, 22, 30, -13, 13, wood)
    m.box(35, 38, 22, 30, -13, 13, wood)
    for x, d, h in [(-24, -6, 0), (-2, -7, 0), (20, -6, 0), (-14, 6, 0), (10, 5, 0), (-5, 0, 12)]:
        m.box(x - 10, x + 10, 22 + h, 34 + h, d - 8, d + 8, sack)
    m.wheel(-6, 15, -23, -17, 15, wheel)
    m.wheel(-6, 15, 17, 23, 15, wheel)
    m.box(-9, -3, 12, 18, -24, 24, wood)
    m.beam((36, 20, -10), (74, 10, -10), 1.8, wood)
    m.beam((36, 20, 10), (74, 10, 10), 1.8, wood)
    if burning:
        ember = material(b, "ember", (0.9, 0.35, 0.1), emissive=[1.0, 0.38, 0.08])
        mats[ember] = ember
        for x, d, h in [(-20, -6, 34), (4, -7, 34), (-5, 0, 46), (14, 5, 34)]:
            m.box(x - 5, x + 5, h, h + 2, d - 4, d + 4, ember)
    finish(b, m.write(b, mats), "cart_fire" if burning else "cart")


def barricade():
    """拒马：一根顺着纵深的横木，三组削尖的木桩交叉成 X 穿过去（从正面看是一排 X）。"""
    b = Builder()
    wood = material(b, "stake", (0.42, 0.29, 0.17))
    tip = material(b, "tip", (0.62, 0.50, 0.34))
    m = Mesh()
    m.beam((0, 14, -22), (0, 14, 22), 3, wood)
    for d in (-16, 0, 16):
        for sx in (-1, 1):
            a, c = (-18 * sx, 0, d), (18 * sx, 32, d)
            m.beam(a, c, 1.8, wood)
            # 削尖的那头颜色浅一点
            m.beam(c, (21 * sx, 35.5, d), 1.2, tip)
    finish(b, m.write(b, {wood: wood, tip: tip}), "barricade")


BRIDGE_LEN = 600


def bridge():
    """长坂桥：木板桥面（顶面和地面一样高），两边栏杆，下面三个桥墩立在河里；河面是桥下一大片暗蓝。"""
    b = Builder()

    def planks(x, y, c, rng):
        return [clamp(v * 0.65) for v in c] if x % 16 == 0 else c

    deck = material(b, "planks", (1, 1, 1), texture=noise_tex(64, (132, 96, 62), 16, 11, planks))
    post = material(b, "post", (0.36, 0.24, 0.14))
    stone = material(b, "pier", (0.36, 0.34, 0.33))

    def ripples(x, y, c, rng):
        return [clamp(v + 40) for v in c] if (y % 9 == 0 and (x // 6 + y) % 3 == 0) else c

    water = material(b, "water", (1, 1, 1), rough=0.25, texture=noise_tex(64, (34, 58, 84), 8, 13, ripples), emissive=[0.02, 0.05, 0.08])
    m = Mesh()
    m.box(0, BRIDGE_LEN, -8, 0, -18, 148, deck, tile=128, skip=("back",))
    for k in range(BRIDGE_LEN // 50 + 1):
        x = k * 50
        for d0 in (-18, 143):
            m.box(x - 2.5, x + 2.5, 0, 30, d0, d0 + 5, post)
    for d0 in (-18, 143):
        m.box(0, BRIDGE_LEN, 24, 29, d0, d0 + 5, post, skip=("left", "right"))
    for x in (60, 300, 540):
        m.box(x - 10, x + 10, -80, -8, -12, 142, stone)
    m.box(-300, BRIDGE_LEN + 400, -60, -55, -420, 280, water, tile=160, skip=("bottom", "left", "right", "front", "back"))
    finish(b, m.write(b, {deck: deck, post: post, stone: stone, water: water}), "bridge")


def banner():
    """旌旗：木杆高 150，顶上一块宽 50、高 70 的旗子挂在杆的右边，4 节骨头连成一串；
    动画 wave 让每节错开相位绕竖轴摆、在画面里上下起伏（正交相机正着看，只绕竖轴摆看不出来）。"""
    b = Builder()
    pole = material(b, "pole", (0.40, 0.27, 0.16))
    base = material(b, "base", (0.35, 0.33, 0.31))

    def cloth_pixel(x, y):
        # 暗青底、金边、中间一个圆
        if x < 3 or y < 3 or y > 60 or x > 60:
            return (200, 160, 60, 255)
        if (x - 32) ** 2 + (y - 32) ** 2 < 140:
            return (200, 160, 60, 255) if (x - 32) ** 2 + (y - 32) ** 2 > 90 else (40, 26, 22, 255)
        return (30, 78, 80, 255)

    cloth = material(b, "cloth", (1, 1, 1), rough=0.95, texture=png(64, 64, cloth_pixel), double=True)
    m = Mesh()
    m.beam((0, 0, 0), (0, 156, 0), 2.2, pole)
    m.beam((0, 152, 0), (8, 152, 0), 1.5, pole)
    m.box(-8, 8, 0, 6, -8, 8, base)
    rigid = m.write(b, {pole: pole, base: base})

    segments, width, height, rows, top = 4, 50.0, 70.0, 4, 150.0
    step = width / segments
    pos, nor, uv, joints, weights, idx = [], [], [], [], [], []
    for r in range(rows):
        for c in range(segments + 1):
            # 旗子末端（右边）下摆剪成锯齿
            sag = 6 if (c == segments and r == rows - 1) else 0
            pos.append((2 + c * step, top - height * r / (rows - 1) + sag, 0.0))
            nor.append((0, 0, 1))
            uv.append((c / segments, r / (rows - 1)))
            joints.append((max(c - 1, 0), 0, 0, 0))
            weights.append((1.0, 0.0, 0.0, 0.0))
    for r in range(rows - 1):
        for c in range(segments):
            a, d = r * (segments + 1) + c, (r + 1) * (segments + 1) + c
            idx += [a, d, d + 1, a, d + 1, a + 1]
    prim = {
        "attributes": {
            "POSITION": b.accessor(pos, "VEC3"),
            "NORMAL": b.accessor(nor, "VEC3"),
            "TEXCOORD_0": b.accessor(uv, "VEC2"),
            "JOINTS_0": b.accessor(joints, "VEC4", 5123),
            "WEIGHTS_0": b.accessor(weights, "VEC4"),
        },
        "indices": b.accessor(idx, "SCALAR", 5125),
        "material": cloth,
    }
    b.gltf["meshes"].append({"name": "cloth", "primitives": [prim]})
    # 节点：0 旗杆，1 布（蒙皮），2–5 骨头（2 在杆顶，后面每节往右 step）
    bones = [2 + k for k in range(segments)]
    b.gltf["nodes"] = [{"name": "pole", "mesh": rigid}, {"name": "cloth", "mesh": len(b.gltf["meshes"]) - 1, "skin": 0}] + [
        {"name": f"bone{k}", "translation": [2, top, 0] if k == 0 else [step, 0, 0], **({"children": [bones[k + 1]]} if k + 1 < segments else {})}
        for k in range(segments)
    ]
    inverse_bind = [(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -(2 + k * step), -top, 0, 1) for k in range(segments)]
    b.gltf["skins"] = [{"joints": bones, "inverseBindMatrices": b.accessor(inverse_bind, "MAT4", vertex=False)}]
    keys = 16
    times = [1.2 * i / keys for i in range(keys + 1)]
    time_acc = b.accessor(times, "SCALAR", vertex=False)
    channels, samplers = [], []
    for k in range(segments):
        values = []
        for i in range(keys + 1):
            phase = 2 * math.pi * i / keys - k * 1.1
            swing = math.radians(16 + 6 * k) * math.sin(phase)
            ripple = math.radians(5 + 2 * k) * math.sin(phase + 0.8)
            values.append(qmul(quat((0, 1, 0), swing), quat((0, 0, 1), ripple)))
        samplers.append({"input": time_acc, "output": b.accessor(values, "VEC4", vertex=False)})
        channels.append({"sampler": k, "target": {"node": bones[k], "path": "rotation"}})
    b.gltf["animations"] = [{"name": "wave", "channels": channels, "samplers": samplers}]
    b.gltf["scenes"] = [{"nodes": [0, bones[0], 1]}]
    b.gltf["scene"] = 0
    b.write(OUT / "banner.glb")


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    ground()
    wall()
    cart(False)
    cart(True)
    barricade()
    bridge()
    banner()
