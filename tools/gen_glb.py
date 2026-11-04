#!/usr/bin/env python3
"""生成 nge 用的 glTF 测试 / 示例模型（.glb，贴图内嵌），只用标准库。

用法：python3 tools/gen_glb.py
输出：crates/nge-render3d/tests/data/test.glb、examples/model/assets/crate.glb
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

    def accessor(self, values, kind, component=5126):
        fmt = {5126: "f", 5125: "I"}[component]
        flat = [c for v in values for c in (v if isinstance(v, (list, tuple)) else [v])]
        target = 34963 if component == 5125 else 34962
        acc = {"bufferView": self.view(struct.pack(f"<{len(flat)}{fmt}", *flat), target), "componentType": component, "count": len(values), "type": kind}
        if kind == "VEC3" and component == 5126:
            acc["min"] = [min(v[i] for v in values) for i in range(3)]
            acc["max"] = [max(v[i] for v in values) for i in range(3)]
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
    b.write(ROOT / "examples/model/assets/crate.glb")


if __name__ == "__main__":
    test_model()
    crate_model()
