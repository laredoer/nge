#!/usr/bin/env python3
"""生成 nge 用的 glTF 测试 / 示例模型（.glb，贴图内嵌），只用标准库。

用法：python3 tools/gen_glb.py
输出：crates/nge-render3d/tests/data/{test,morph}.glb、examples/{model,platformer}/assets/crate.glb、examples/platformer/assets/{flag,jelly}.glb、
examples/platformer/assets/lab-*.glb（3D 试验场，见 docs/gameplay3d.md）
"""
import json
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def png(width, height, pixel):
    """pixel(x, y) -> (r, g, b, a)，返回 RGBA8 PNG 字节。"""
    rows = b"".join(b"\0" + b"".join(bytes(pixel(x, y)) for x in range(width)) for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


class Builder:
    def __init__(self):
        self.bin = bytearray()
        self.gltf = {"asset": {"version": "2.0", "generator": "nge tools/gen_glb.py"}, "bufferViews": [], "accessors": []}

    def view(self, data, target=None):
        while len(self.bin) % 4:
            self.bin += b"\0"
        view = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target:
            view["target"] = target
        self.bin += data
        self.gltf["bufferViews"].append(view)
        return len(self.gltf["bufferViews"]) - 1

    def accessor(self, values, kind, component=5126, vertex=True):
        """vertex=False：不是顶点 / 索引数据（逆绑定矩阵、动画），bufferView 不写 target。"""
        fmt = {5126: "f", 5125: "I", 5123: "H"}[component]
        flat = [c for v in values for c in (v if isinstance(v, (list, tuple)) else [v])]
        target = (34963 if component == 5125 else 34962) if vertex else None
        acc = {"bufferView": self.view(struct.pack(f"<{len(flat)}{fmt}", *flat), target), "componentType": component, "count": len(values), "type": kind}
        if kind == "VEC3" and component == 5126:
            acc["min"] = [min(v[i] for v in values) for i in range(3)]
            acc["max"] = [max(v[i] for v in values) for i in range(3)]
        if kind == "SCALAR" and not vertex:
            # 动画的时间轴要写 min / max
            acc["min"], acc["max"] = [min(values)], [max(values)]
        self.gltf["accessors"].append(acc)
        return len(self.gltf["accessors"]) - 1

    def image(self, data):
        self.gltf.setdefault("images", []).append({"bufferView": self.view(data), "mimeType": "image/png"})
        self.gltf.setdefault("textures", []).append({"source": len(self.gltf["images"]) - 1})
        return len(self.gltf["textures"]) - 1

    def write(self, path):
        while len(self.bin) % 4:
            self.bin += b"\0"
        self.gltf["buffers"] = [{"byteLength": len(self.bin)}]
        text = json.dumps(self.gltf, separators=(",", ":")).encode()
        text += b" " * (-len(text) % 4)
        body = struct.pack("<I4s", len(text), b"JSON") + text + struct.pack("<I4s", len(self.bin), b"BIN\0") + bytes(self.bin)
        path.write_bytes(struct.pack("<4sII", b"glTF", 2, 12 + len(body)) + body)
        print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")


def cube(size):
    """每面 4 个顶点、逆时针朝外，uv 铺满整张图（v 朝下）。"""
    h = size / 2
    faces = [((1, 0, 0), (0, 0, -1), (0, 1, 0)), ((-1, 0, 0), (0, 0, 1), (0, 1, 0)), ((0, 1, 0), (1, 0, 0), (0, 0, -1)),
             ((0, -1, 0), (1, 0, 0), (0, 0, 1)), ((0, 0, 1), (1, 0, 0), (0, 1, 0)), ((0, 0, -1), (-1, 0, 0), (0, 1, 0))]
    pos, nor, uv, idx = [], [], [], []
    for n, u, v in faces:
        base = len(pos)
        for du, dv in [(-1, -1), (1, -1), (1, 1), (-1, 1)]:
            pos.append(tuple(h * (n[i] + u[i] * du + v[i] * dv) for i in range(3)))
            nor.append(n)
            uv.append(((du + 1) / 2, (1 - dv) / 2))
        idx += [base, base + 1, base + 2, base, base + 2, base + 3]
    return pos, nor, uv, idx


def test_model():
    b = Builder()
    # 2×2 贴图：左上红、右上绿、左下蓝、右下白
    quadrants = {(0, 0): (255, 0, 0, 255), (1, 0): (0, 255, 0, 255), (0, 1): (0, 0, 255, 255), (1, 1): (255, 255, 255, 255)}
    tex = b.image(png(2, 2, lambda x, y: quadrants[(x, y)]))
    quad = {
        "POSITION": b.accessor([(-1, -1, 0), (1, -1, 0), (1, 1, 0), (-1, 1, 0)], "VEC3"),
        "NORMAL": b.accessor([(0, 0, 1)] * 4, "VEC3"),
        "TEXCOORD_0": b.accessor([(0, 1), (1, 1), (1, 0), (0, 0)], "VEC2"),
    }
    quad_idx = b.accessor([0, 1, 2, 0, 2, 3], "SCALAR", 5125)
    # 没有法线、没有索引的三角形，外加一个应被跳过的线段图元
    tri = {"POSITION": b.accessor([(0, 0, 0), (1, 0, 0), (0, 1, 0)], "VEC3")}
    b.gltf["meshes"] = [
        {"name": "quad", "primitives": [{"attributes": quad, "indices": quad_idx, "material": 0}]},
        {"name": "tri", "primitives": [{"attributes": tri, "material": 1}, {"attributes": tri, "mode": 1}]},
    ]
    b.gltf["materials"] = [
        {"pbrMetallicRoughness": {"baseColorTexture": {"index": tex}, "metallicFactor": 0.2, "roughnessFactor": 0.7},
         "alphaMode": "MASK", "alphaCutoff": 0.3, "doubleSided": True},
        {"pbrMetallicRoughness": {"baseColorFactor": [0.5, 0.5, 0.5, 1.0]}, "emissiveFactor": [1.0, 0.0, 0.0]},
    ]
    s = math.sqrt(0.5)
    b.gltf["nodes"] = [
        {"name": "root", "translation": [1, 0, 0], "children": [1]},
        {"name": "quad", "mesh": 0, "rotation": [0, s, 0, s], "scale": [2, 2, 2]},
        {"name": "tri", "mesh": 1, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 3, 0, 1]},
        {"name": "unused", "mesh": 1},
    ]
    b.gltf["scenes"] = [{"nodes": [3]}, {"nodes": [0, 2]}]
    b.gltf["scene"] = 1
    b.write(ROOT / "crates/nge-render3d/tests/data/test.glb")


def crate_model():
    b = Builder()

    def brick(x, y):
        # 砖墙：64×64 一块砖，灰色砖缝，每块砖颜色略有不同，右下角一块亮色标记方向
        row, col = y // 32, (x + (16 if (y // 32) % 2 else 0)) // 64
        if y % 32 < 3 or (x + (16 if row % 2 else 0)) % 64 < 3:
            return (170, 165, 155, 255)
        if row == 7 and col == 3:
            return (240, 200, 60, 255)
        shade = 30 * (((row * 7 + col * 13) % 5) - 2)
        return (max(0, min(255, 165 + shade)), max(0, min(255, 70 + shade // 2)), 45, 255)

    tex = b.image(png(256, 256, brick))
    pos, nor, uv, idx = cube(1.0)
    prim = {"attributes": {"POSITION": b.accessor(pos, "VEC3"), "NORMAL": b.accessor(nor, "VEC3"), "TEXCOORD_0": b.accessor(uv, "VEC2")},
            "indices": b.accessor(idx, "SCALAR", 5125), "material": 0}
    b.gltf["meshes"] = [{"name": "box", "primitives": [prim]}]
    b.gltf["materials"] = [{"name": "brick", "pbrMetallicRoughness": {"baseColorTexture": {"index": tex}, "metallicFactor": 0.0, "roughnessFactor": 0.8}}]
    # 底座一个大箱子，上面叠一个转了 45° 的小箱子（验证节点层级）
    r = math.sin(math.pi / 8), math.cos(math.pi / 8)
    b.gltf["nodes"] = [
        {"name": "crate", "mesh": 0, "children": [1]},
        {"name": "top", "mesh": 0, "translation": [0, 0.8, 0], "rotation": [0, r[0], 0, r[1]], "scale": [0.6, 0.6, 0.6]},
    ]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    for example in ["model", "platformer"]:
        b.write(ROOT / f"examples/{example}/assets/crate.glb")


def quat(axis, angle):
    """绕单位轴 axis 转 angle 弧度的四元数 (x, y, z, w)。"""
    s = math.sin(angle / 2)
    return (axis[0] * s, axis[1] * s, axis[2] * s, math.cos(angle / 2))


def qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def flag_model():
    """终点的旗子：宽 1、高 0.6 的布从原点往右、往下挂，4 节骨头连成一串（每节 0.25），动画 wave 让每节错开相位绕 y 摆、绕 z 微微下垂。"""
    b = Builder()
    segments, width, height, rows = 4, 1.0, 0.6, 3
    step = width / segments
    pos, nor, joints, weights, idx = [], [], [], [], []
    for r in range(rows):
        for c in range(segments + 1):
            pos.append((c * step, -height * r / (rows - 1), 0.0))
            nor.append((0, 0, 1))
            # 第 c 列跟着第 c - 1 节骨头（它的起点在左边一列）；第 0 列在旗杆上不动
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
            "JOINTS_0": b.accessor(joints, "VEC4", 5123),
            "WEIGHTS_0": b.accessor(weights, "VEC4"),
        },
        "indices": b.accessor(idx, "SCALAR", 5125),
        "material": 0,
    }
    b.gltf["meshes"] = [{"name": "cloth", "primitives": [prim]}]
    b.gltf["materials"] = [{"name": "cloth", "pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.18, 0.12, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.9}, "doubleSided": True}]
    # 节点 0 是布（挂蒙皮），1–4 是骨头：1 在原点，后面每节往右 step
    bones = list(range(1, segments + 1))
    b.gltf["nodes"] = [{"name": "cloth", "mesh": 0, "skin": 0}] + [
        {"name": f"bone{k}", **({"translation": [step, 0, 0]} if k else {}), **({"children": [k + 2]} if k + 1 < segments else {})} for k in range(segments)
    ]
    inverse_bind = [(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -k * step, 0, 0, 1) for k in range(segments)]
    b.gltf["skins"] = [{"joints": bones, "inverseBindMatrices": b.accessor(inverse_bind, "MAT4", vertex=False)}]
    keys = 16
    times = [i / keys for i in range(keys + 1)]
    time_acc = b.accessor(times, "SCALAR", vertex=False)
    channels, samplers = [], []
    for k in range(segments):
        values = []
        for t in times:
            phase = 2 * math.pi * t - k * 0.9
            swing = math.radians(14 + 6 * k) * math.sin(phase)
            droop = math.radians(-4) * (1 + math.sin(phase + 1.3)) * (1 if k else 0)
            values.append(qmul(quat((0, 1, 0), swing), quat((0, 0, 1), droop)))
        samplers.append({"input": time_acc, "output": b.accessor(values, "VEC4", vertex=False)})
        channels.append({"sampler": k, "target": {"node": bones[k], "path": "rotation"}})
    b.gltf["animations"] = [{"name": "wave", "channels": channels, "samplers": samplers}]
    b.gltf["scenes"] = [{"nodes": [1, 0]}]
    b.gltf["scene"] = 0
    b.write(ROOT / "examples/platformer/assets/flag.glb")


def morph_model():
    """变形测试模型：宽 1、高 1 的竖直方块（朝 +Z），目标 0 把顶边抬高 1，目标 1 整体右移 0.5。
    网格默认权重 [0.25, 0]；节点 0 自己写了 [0.5, 0]（节点优先），节点 1 用网格的；动画 squish 线性地 0 → (1, 0.5) → 0。"""
    b = Builder()
    pos = [(-0.5, 0, 0), (0.5, 0, 0), (0.5, 1, 0), (-0.5, 1, 0)]
    lift = [(0, 0, 0), (0, 0, 0), (0, 1, 0), (0, 1, 0)]
    prim = {
        "attributes": {"POSITION": b.accessor(pos, "VEC3"), "NORMAL": b.accessor([(0, 0, 1)] * 4, "VEC3")},
        "indices": b.accessor([0, 1, 2, 0, 2, 3], "SCALAR", 5125),
        "targets": [
            {"POSITION": b.accessor(lift, "VEC3"), "NORMAL": b.accessor([(0, 0.5, 0)] * 4, "VEC3")},
            {"POSITION": b.accessor([(0.5, 0, 0)] * 4, "VEC3")},
        ],
    }
    b.gltf["meshes"] = [{"name": "bump", "primitives": [prim], "weights": [0.25, 0.0]}]
    b.gltf["nodes"] = [{"name": "squish", "mesh": 0, "weights": [0.5, 0.0]}, {"name": "plain", "mesh": 0, "translation": [2, 0, 0]}]
    times = b.accessor([0.0, 0.5, 1.0], "SCALAR", vertex=False)
    weights = b.accessor([0.0, 0.0, 1.0, 0.5, 0.0, 0.0], "SCALAR", vertex=False)
    b.gltf["animations"] = [{"name": "squish", "channels": [{"sampler": 0, "target": {"node": 0, "path": "weights"}}], "samplers": [{"input": times, "output": weights}]}]
    b.gltf["scenes"] = [{"nodes": [0, 1]}]
    b.gltf["scene"] = 0
    b.write(ROOT / "crates/nge-render3d/tests/data/morph.glb")


def jelly_model():
    """平台跳跃的幽灵果冻：底面半径 0.5、高 0.6 的半透明圆顶（BLEND），底在 y = 0。
    一个变形目标把它压扁摊开；动画 idle 权重一直是 0，squish 0 → 1 → 0 压一下弹回来。"""
    b = Builder()
    rings, segments, radius, height = 6, 24, 0.5, 0.6
    pos, nor, idx = [], [], []
    for i in range(rings + 1):
        phi = math.pi / 2 * i / rings
        r, y = radius * math.cos(phi), height * math.sin(phi)
        for j in range(segments + 1):
            theta = 2 * math.pi * j / segments
            x, z = r * math.cos(theta), -r * math.sin(theta)
            # 椭球的梯度当法线；顶点处退化成朝上
            n = (x / radius ** 2, y / height ** 2, z / radius ** 2)
            length = math.sqrt(sum(c * c for c in n)) or 1.0
            pos.append((x, y, z))
            nor.append(tuple(c / length for c in n) if i < rings else (0.0, 1.0, 0.0))
    for i in range(rings):
        for j in range(segments):
            a, c = i * (segments + 1) + j, (i + 1) * (segments + 1) + j
            idx += [a, a + 1, c + 1, a, c + 1, c]
    squish = [(0.35 * x, -0.5 * y, 0.35 * z) for x, y, z in pos]
    prim = {
        "attributes": {"POSITION": b.accessor(pos, "VEC3"), "NORMAL": b.accessor(nor, "VEC3")},
        "indices": b.accessor(idx, "SCALAR", 5125),
        "material": 0,
        "targets": [{"POSITION": b.accessor(squish, "VEC3")}],
    }
    b.gltf["meshes"] = [{"name": "jelly", "primitives": [prim], "weights": [0.0]}]
    b.gltf["materials"] = [{
        "name": "jelly",
        "pbrMetallicRoughness": {"baseColorFactor": [0.55, 0.85, 1.0, 0.55], "metallicFactor": 0.0, "roughnessFactor": 0.25},
        "emissiveFactor": [0.08, 0.16, 0.24],
        "alphaMode": "BLEND",
    }]
    b.gltf["nodes"] = [{"name": "jelly", "mesh": 0}]

    def weights(name, times, values):
        sampler = {"input": b.accessor(times, "SCALAR", vertex=False), "output": b.accessor(values, "SCALAR", vertex=False)}
        return {"name": name, "channels": [{"sampler": 0, "target": {"node": 0, "path": "weights"}}], "samplers": [sampler]}

    b.gltf["animations"] = [weights("idle", [0.0, 1.0], [0.0, 0.0]), weights("squish", [0.0, 0.3, 0.6], [0.0, 1.0, 0.0])]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write(ROOT / "examples/platformer/assets/jelly.glb")


# ---- 3D 试验场（平台跳跃按 G）：单位是 point，场景里 scale 1（石头除外） ----

def flat_mesh(faces):
    """faces：每个面一串逆时针（朝外看）的顶点，法线按面算，扇形切成三角形。返回 pos, nor, idx。"""
    pos, nor, idx = [], [], []
    for face in faces:
        a, b, c = face[0], face[1], face[2]
        u = [b[i] - a[i] for i in range(3)]
        v = [c[i] - a[i] for i in range(3)]
        n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        length = math.sqrt(sum(x * x for x in n))
        n = tuple(x / length for x in n)
        base = len(pos)
        for p in face:
            pos.append(tuple(float(x) for x in p))
            nor.append(n)
        for i in range(1, len(face) - 1):
            idx += [base, base + i, base + i + 1]
    return pos, nor, idx


def box_faces(lo, hi):
    (x0, y0, z0), (x1, y1, z1) = lo, hi
    return [
        [(x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1)],
        [(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)],
        [(x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (x0, y1, z0)],
        [(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)],
        [(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)],
        [(x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0)],
    ]


def wedge_faces(x0, x1, rise, z0, z1):
    """从 x0 的地面升到 x1 高 rise 的斜坡（三棱柱）。"""
    return [
        [(x0, 0, z1), (x1, 0, z1), (x1, rise, z1)],
        [(x1, 0, z0), (x0, 0, z0), (x1, rise, z0)],
        [(x0, 0, z0), (x0, 0, z1), (x1, rise, z1), (x1, rise, z0)],
        [(x1, 0, z1), (x1, 0, z0), (x1, rise, z0), (x1, rise, z1)],
        [(x0, 0, z0), (x1, 0, z0), (x1, 0, z1), (x0, 0, z1)],
    ]


def smooth_mesh(rings, segments, point):
    """经纬网格：point(i, j) -> (位置, 法线)，i 从上到下 0..rings，j 绕一圈 0..segments。"""
    pos, nor, idx = [], [], []
    for i in range(rings + 1):
        for j in range(segments + 1):
            p, n = point(i, j)
            pos.append(p)
            nor.append(n)
    w = segments + 1
    for i in range(rings):
        for j in range(segments):
            a, b = i * w + j, (i + 1) * w + j
            idx += [a, b, b + 1, a, b + 1, a + 1]
    return pos, nor, idx


def sphere(radius, rings=10, segments=16):
    def point(i, j):
        t, f = math.pi * i / rings, 2 * math.pi * j / segments
        n = (math.sin(t) * math.cos(f), math.cos(t), -math.sin(t) * math.sin(f))
        return tuple(radius * x for x in n), n
    return smooth_mesh(rings, segments, point)


def capsule(half_height, radius, rings=12, segments=16):
    """竖着的胶囊：上半球、圆柱、下半球（rings 是偶数，中间两圈分别在圆柱的上下沿）。"""
    half = rings // 2

    def point(i, j):
        # 上半球 0..half，下半球 half+1..rings+1：多一圈把圆柱接上
        k = i if i <= half else i - 1
        t, f = math.pi * k / rings, 2 * math.pi * j / segments
        n = (math.sin(t) * math.cos(f), math.cos(t), -math.sin(t) * math.sin(f))
        y = half_height if i <= half else -half_height
        return (radius * n[0], radius * n[1] + y, radius * n[2]), n
    return smooth_mesh(rings + 1, segments, point)


def lab_model(name, parts, color, metallic=0.0, roughness=0.8):
    """parts：[(pos, nor, idx)]，每个一个图元（同一个网格），一个纯色材质。"""
    b = Builder()
    prims = []
    for pos, nor, idx in parts:
        attrs = {"POSITION": b.accessor(pos, "VEC3"), "NORMAL": b.accessor(nor, "VEC3")}
        prims.append({"attributes": attrs, "indices": b.accessor(idx, "SCALAR", 5125), "material": 0})
    b.gltf["meshes"] = [{"name": name, "primitives": prims}]
    b.gltf["materials"] = [{"pbrMetallicRoughness": {"baseColorFactor": list(color) + [1.0], "metallicFactor": metallic, "roughnessFactor": roughness}}]
    b.gltf["nodes"] = [{"name": name, "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write(ROOT / f"examples/platformer/assets/lab-{name}.glb")


def lab_models():
    lab_model("floor", [flat_mesh(box_faces((-800, -10, -800), (800, 10, 800)))], (0.32, 0.36, 0.4))  # 中心在原点，和场景里的盒子碰撞体一样
    # 20° 的缓坡（长 300）和 40° 的陡坡（长 150），并排，宽 160
    gentle, steep = math.tan(math.radians(20)), math.tan(math.radians(40))
    lab_model("ramps", [flat_mesh(wedge_faces(0, 300, 300 * gentle, -80, 80) + wedge_faces(0, 150, 150 * steep, 140, 300))], (0.55, 0.5, 0.38))
    # 四级台阶，每级高 16、深 50、宽 160：一级一个图元
    steps = [flat_mesh(box_faces((i * 50, 0, -80), (i * 50 + 50, 16 * (i + 1), 80))) for i in range(4)]
    lab_model("steps", steps, (0.6, 0.45, 0.35))
    lab_model("door", [flat_mesh(box_faces((0, 0, -4), (90, 130, 4)))], (0.62, 0.38, 0.2))
    lab_model("platform", [flat_mesh(box_faces((-60, -8, -60), (60, 8, 60)))], (0.75, 0.52, 0.32))
    lab_model("ball", [sphere(1.0)], (0.75, 0.75, 0.8), metallic=0.8, roughness=0.3)
    lab_model("hero", [capsule(12, 12)], (0.95, 0.55, 0.25))
    # 不规则的石头：二十面体的顶点各自拉长或压扁一点，每个面一份顶点（60 个），凸包只剩 12 个角
    t = (1 + math.sqrt(5)) / 2
    corners = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t), (0, 1, -t), (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    bumps = [1.0, 0.85, 1.1, 0.9, 1.15, 0.8, 1.05, 0.95, 0.9, 1.2, 0.85, 1.0]
    corners = [tuple(c * k * s / math.sqrt(1 + t * t) for c, s in zip(p, (1.3, 0.8, 1.0))) for p, k in zip(corners, bumps)]
    tris = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4), (11, 10, 2), (10, 7, 6), (7, 1, 8),
            (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8), (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    lab_model("rock", [flat_mesh([[corners[i] for i in tri] for tri in tris])], (0.5, 0.48, 0.45))


if __name__ == "__main__":
    test_model()
    morph_model()
    crate_model()
    flag_model()
    jelly_model()
    lab_models()
