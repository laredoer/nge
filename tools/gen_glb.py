#!/usr/bin/env python3
"""生成 nge 用的 glTF 测试 / 示例模型（.glb，贴图内嵌），只用标准库。

用法：python3 tools/gen_glb.py
输出：crates/nge-render3d/tests/data/test.glb、examples/{model,platformer}/assets/crate.glb、examples/platformer/assets/flag.glb
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


if __name__ == "__main__":
    test_model()
    crate_model()
    flag_model()
