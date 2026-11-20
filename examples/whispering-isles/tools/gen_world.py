#!/usr/bin/env python3
"""Generate rich, organic, hand-painted-style procedural 3D glTF (.glb) assets
with embedded PBR textures and animations for Whispering Isles.
Uses Python standard library only.
"""
import json
import math
import random
import struct
import zlib
from pathlib import Path

BASE = Path(__file__).resolve().parent.parent
ASSETS = BASE / "assets"
WORLD_HALF = 1400.0


# ---------------------------------------------------------------------------
# Deterministic 2D Value Noise & FBM for organic textures & sculpted meshes
# ---------------------------------------------------------------------------
def hash2(ix: int, iy: int, seed: int = 0) -> float:
    n = (ix * 374761393 + iy * 668265263 + seed * 1442695041) & 0xFFFFFFFF
    n = (n ^ (n >> 13)) * 1274126177 & 0xFFFFFFFF
    return ((n ^ (n >> 16)) & 0xFFFF) / 65535.0


def noise2(x: float, y: float, seed: int = 0) -> float:
    ix, iy = math.floor(x), math.floor(y)
    fx, fy = x - ix, y - iy
    ux, uy = fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy)
    a = hash2(ix, iy, seed)
    b = hash2(ix + 1, iy, seed)
    c = hash2(ix, iy + 1, seed)
    d = hash2(ix + 1, iy + 1, seed)
    return (a * (1 - ux) + b * ux) * (1 - uy) + (c * (1 - ux) + d * ux) * uy


def fbm2(x: float, y: float, octaves: int = 4, seed: int = 0) -> float:
    val, amp, freq, norm = 0.0, 0.5, 1.0, 0.0
    for i in range(octaves):
        val += amp * noise2(x * freq, y * freq, seed + i * 31)
        norm += amp
        amp *= 0.5
        freq *= 2.03
    return val / norm


def terrain_height(x: float, z: float) -> float:
    r = math.hypot(x, z)
    falloff = max(0.0, 1.0 - (r / 1280.0) ** 2.4)
    base = -42.0 + 62.0 * falloff
    rolling = (
        18.0 * math.sin(x * 0.0038 + 0.4) * math.cos(z * 0.0036 - 0.3)
        + 10.0 * math.sin((x + z) * 0.0055)
    ) * falloff
    d_peak = math.hypot(x - 0.0, z + 760.0)
    peak = 210.0 * math.exp(-(d_peak / 340.0) ** 2.0)
    d_ruins = math.hypot(x - 540.0, z + 320.0)
    ruins = 58.0 * math.exp(-(d_ruins / 250.0) ** 2.6)
    d_forest = math.hypot(x + 580.0, z + 260.0)
    forest = 42.0 * math.exp(-(d_forest / 280.0) ** 2.0)
    d_cove = math.hypot(x + 460.0, z - 520.0)
    cove = -52.0 * math.exp(-(d_cove / 230.0) ** 2.2)
    d_camp = math.hypot(x - 0.0, z - 220.0)
    camp_blend = math.exp(-(d_camp / 150.0) ** 2.0)
    h = base + rolling + peak + ruins + forest + cove
    return h * (1.0 - camp_blend * 0.65) + 28.0 * (camp_blend * 0.65)


# ---------------------------------------------------------------------------
# PNG & glTF 2.0 Binary Builder
# ---------------------------------------------------------------------------
def png_bytes(width: int, height: int, pixel_fn) -> bytes:
    rows = b"".join(b"\0" + b"".join(bytes(pixel_fn(x, y)) for x in range(width)) for y in range(height))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b"")


def quat(axis, angle: float):
    s = math.sin(angle * 0.5)
    return (axis[0] * s, axis[1] * s, axis[2] * s, math.cos(angle * 0.5))


class Builder:
    def __init__(self) -> None:
        self.bin = bytearray()
        self.gltf = {"asset": {"version": "2.0", "generator": "whispering-isles gen_world.py v2"}, "bufferViews": [], "accessors": []}

    def view(self, data: bytes, target=None) -> int:
        while len(self.bin) % 4:
            self.bin += b"\0"
        v = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target:
            v["target"] = target
        self.bin += data
        self.gltf["bufferViews"].append(v)
        return len(self.gltf["bufferViews"]) - 1

    def accessor(self, values, kind: str, component: int = 5126, vertex: bool = True) -> int:
        fmt = {5126: "f", 5125: "I", 5123: "H"}[component]
        flat = [c for v in values for c in (v if isinstance(v, (list, tuple)) else [v])]
        target = (34963 if component == 5125 else 34962) if vertex else None
        acc = {"bufferView": self.view(struct.pack(f"<{len(flat)}{fmt}", *flat), target), "componentType": component, "count": len(values), "type": kind}
        if kind == "VEC3" and component == 5126:
            acc["min"] = [min(v[i] for v in values) for i in range(3)]
            acc["max"] = [max(v[i] for v in values) for i in range(3)]
        if kind == "SCALAR" and not vertex:
            acc["min"], acc["max"] = [min(values)], [max(values)]
        self.gltf["accessors"].append(acc)
        return len(self.gltf["accessors"]) - 1

    def image(self, data: bytes) -> int:
        self.gltf.setdefault("images", []).append({"bufferView": self.view(data), "mimeType": "image/png"})
        self.gltf.setdefault("textures", []).append({"source": len(self.gltf["images"]) - 1})
        return len(self.gltf["textures"]) - 1

    def add_material(self, color=(1, 1, 1, 1), metallic=0.0, roughness=0.82, tex=None, normal_tex=None, normal_scale=1.0, emissive=None, alpha=None, double_sided=False) -> int:
        pbr = {"baseColorFactor": list(color), "metallicFactor": metallic, "roughnessFactor": roughness}
        if tex is not None:
            pbr["baseColorTexture"] = {"index": tex}
        m = {"pbrMetallicRoughness": pbr}
        if normal_tex is not None:
            m["normalTexture"] = {"index": normal_tex, "scale": normal_scale}
        if emissive:
            m["emissiveFactor"] = list(emissive)
        if alpha:
            m["alphaMode"] = alpha
        if double_sided:
            m["doubleSided"] = True
        self.gltf.setdefault("materials", []).append(m)
        return len(self.gltf["materials"]) - 1

    def add_mesh(self, name: str, prims) -> int:
        """prims: list of (pos, nor, uv_or_None, idx, mat_idx)."""
        gl_prims = []
        for pos, nor, uv, idx, mat in prims:
            attrs = {"POSITION": self.accessor(pos, "VEC3"), "NORMAL": self.accessor(nor, "VEC3")}
            if uv is not None:
                attrs["TEXCOORD_0"] = self.accessor(uv, "VEC2")
            gl_prims.append({"attributes": attrs, "indices": self.accessor(idx, "SCALAR", 5125), "material": mat})
        self.gltf.setdefault("meshes", []).append({"name": name, "primitives": gl_prims})
        return len(self.gltf["meshes"]) - 1

    def write(self, name: str) -> None:
        ASSETS.mkdir(parents=True, exist_ok=True)
        path = ASSETS / name
        while len(self.bin) % 4:
            self.bin += b"\0"
        self.gltf["buffers"] = [{"byteLength": len(self.bin)}]
        text = json.dumps(self.gltf, separators=(",", ":")).encode()
        text += b" " * (-len(text) % 4)
        body = struct.pack("<I4s", len(text), b"JSON") + text + struct.pack("<I4s", len(self.bin), b"BIN\0") + bytes(self.bin)
        path.write_bytes(struct.pack("<4sII", b"glTF", 2, 12 + len(body)) + body)


# ---------------------------------------------------------------------------
# Organic Sculpted Geometry Helpers (smooth normals, lathe, tubes, organic spheres)
# ---------------------------------------------------------------------------
def merge_meshes(meshes):
    pos, nor, uv, idx = [], [], [], []
    has_uv = all(m[2] is not None for m in meshes)
    for p, n, u, i in meshes:
        base = len(pos)
        pos.extend(p)
        nor.extend(n)
        if has_uv:
            uv.extend(u)
        idx.extend(k + base for k in i)
    return pos, nor, (uv if has_uv else None), idx


def transform_mesh(mesh, offset=(0, 0, 0), rot_y=0.0, rot_x=0.0, scale=(1, 1, 1)):
    pos, nor, uv, idx = mesh
    cy, sy = math.cos(rot_y), math.sin(rot_y)
    cx, sx = math.cos(rot_x), math.sin(rot_x)
    ox, oy, oz = offset
    scx, scy, scz = scale

    def xform(v, translate=True):
        x, y, z = v[0] * scx, v[1] * scy, v[2] * scz
        # rot X
        y, z = y * cx - z * sx, y * sx + z * cx
        # rot Y
        x, z = x * cy + z * sy, -x * sy + z * cy
        return (x + ox, y + oy, z + oz) if translate else (x, y, z)

    npos = [xform(p, True) for p in pos]
    nnor = []
    for n in nor:
        v = xform(n, False)
        l = math.sqrt(v[0] ** 2 + v[1] ** 2 + v[2] ** 2) or 1.0
        nnor.append((v[0] / l, v[1] / l, v[2] / l))
    return npos, nnor, uv, list(idx)


def compute_smooth_normals(pos, idx):
    acc = [[0.0, 0.0, 0.0] for _ in pos]
    for i in range(0, len(idx), 3):
        a, b, c = idx[i], idx[i + 1], idx[i + 2]
        pa, pb, pc = pos[a], pos[b], pos[c]
        u = (pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2])
        v = (pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2])
        nx = u[1] * v[2] - u[2] * v[1]
        ny = u[2] * v[0] - u[0] * v[2]
        nz = u[0] * v[1] - u[1] * v[0]
        for k in (a, b, c):
            acc[k][0] += nx
            acc[k][1] += ny
            acc[k][2] += nz
    nor = []
    for x, y, z in acc:
        l = math.sqrt(x * x + y * y + z * z) or 1.0
        nor.append((x / l, y / l, z / l))
    return nor


def flat_faces(faces, uv_scale=0.02):
    pos, nor, uv, idx = [], [], [], []
    for face in faces:
        a, b, c = face[0], face[1], face[2]
        u = [b[i] - a[i] for i in range(3)]
        v = [c[i] - a[i] for i in range(3)]
        n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        l = math.sqrt(sum(x * x for x in n)) or 1.0
        n = tuple(x / l for x in n)
        base = len(pos)
        for p in face:
            pos.append(tuple(float(x) for x in p))
            nor.append(n)
            # Triplanar-ish UV projection
            if abs(n[1]) > 0.7:
                uv.append((p[0] * uv_scale, p[2] * uv_scale))
            elif abs(n[0]) > 0.7:
                uv.append((p[2] * uv_scale, p[1] * uv_scale))
            else:
                uv.append((p[0] * uv_scale, p[1] * uv_scale))
        for i in range(1, len(face) - 1):
            idx += [base, base + i, base + i + 1]
    return pos, nor, uv, idx


def lathe(profile, segs=14, wobble=0.0, seed=0, cap_top=True, cap_bottom=True):
    """profile: list of (radius, y). Builds a smooth surface of revolution with UVs."""
    pos, uv, idx = [], [], []
    rings = len(profile)
    for r_idx, (rad, y) in enumerate(profile):
        v_coord = r_idx / max(1, rings - 1)
        for s in range(segs + 1):
            u_coord = s / segs
            ang = 2.0 * math.pi * u_coord
            w = 1.0 + wobble * (noise2(math.cos(ang) * 2.0 + r_idx * 0.7, math.sin(ang) * 2.0 + seed, seed) - 0.5) * 2.0
            rr = max(0.001, rad * w)
            pos.append((math.cos(ang) * rr, y, -math.sin(ang) * rr))
            uv.append((u_coord, v_coord))
    stride = segs + 1
    for r_idx in range(rings - 1):
        for s in range(segs):
            a = r_idx * stride + s
            b = a + 1
            c = a + stride
            d = c + 1
            idx += [a, b, d, a, d, c]
    if cap_bottom and profile[0][0] > 0.05:
        center = len(pos)
        pos.append((0.0, profile[0][1], 0.0))
        uv.append((0.5, 0.0))
        for s in range(segs):
            idx += [center, s + 1, s]
    if cap_top and profile[-1][0] > 0.05:
        center = len(pos)
        top_start = (rings - 1) * stride
        pos.append((0.0, profile[-1][1], 0.0))
        uv.append((0.5, 1.0))
        for s in range(segs):
            idx += [center, top_start + s, top_start + s + 1]
    return pos, compute_smooth_normals(pos, idx), uv, idx


def organic_sphere(rx, ry, rz, rings=10, segs=14, rough=0.14, seed=1):
    """Sculpted organic ellipsoid (for tree canopies, boulders, stylized heads, bushes)."""
    pos, uv, idx = [], [], []
    for i in range(rings + 1):
        phi = math.pi * i / rings
        sp, cp = math.sin(phi), math.cos(phi)
        for j in range(segs + 1):
            theta = 2.0 * math.pi * j / segs
            dx, dy, dz = sp * math.cos(theta), cp, -sp * math.sin(theta)
            n = 1.0 + rough * (fbm2(dx * 2.2 + dz * 1.4, dy * 2.2 + seed, 3, seed) - 0.5) * 2.0
            pos.append((dx * rx * n, dy * ry * n, dz * rz * n))
            uv.append((j / segs, i / rings))
    w = segs + 1
    for i in range(rings):
        for j in range(segs):
            a = i * w + j
            b = a + w
            idx += [a, b, b + 1, a, b + 1, a + 1]
    return pos, compute_smooth_normals(pos, idx), uv, idx


def curved_tube(points, radii, segs=10):
    """Extrude a smooth organic tube along 3D spine points (for curved trunks, branches, handles)."""
    pos, uv, idx = [], [], []
    n_pts = len(points)
    for k in range(n_pts):
        p = points[k]
        p_prev = points[max(0, k - 1)]
        p_next = points[min(n_pts - 1, k + 1)]
        tx, ty, tz = p_next[0] - p_prev[0], p_next[1] - p_prev[1], p_next[2] - p_prev[2]
        tl = math.sqrt(tx * tx + ty * ty + tz * tz) or 1.0
        tx, ty, tz = tx / tl, ty / tl, tz / tl
        # Stable frame
        up = (0.0, 0.0, 1.0) if abs(ty) > 0.92 else (0.0, 1.0, 0.0)
        ux = up[1] * tz - up[2] * ty
        uy = up[2] * tx - up[0] * tz
        uz = up[0] * ty - up[1] * tx
        ul = math.sqrt(ux * ux + uy * uy + uz * uz) or 1.0
        ux, uy, uz = ux / ul, uy / ul, uz / ul
        vx = ty * uz - tz * uy
        vy = tz * ux - tx * uz
        vz = tx * uy - ty * ux
        rad = radii[k]
        for s in range(segs + 1):
            ang = 2.0 * math.pi * s / segs
            ca, sa = math.cos(ang), math.sin(ang)
            pos.append((p[0] + rad * (ca * ux + sa * vx), p[1] + rad * (ca * uy + sa * vy), p[2] + rad * (ca * uz + sa * vz)))
            uv.append((s / segs, k / max(1, n_pts - 1)))
    stride = segs + 1
    for k in range(n_pts - 1):
        for s in range(segs):
            a = k * stride + s
            b = a + 1
            c = a + stride
            d = c + 1
            idx += [a, b, d, a, d, c]
    return pos, compute_smooth_normals(pos, idx), uv, idx


def box_mesh(lo, hi, uv_scale=0.02):
    (x0, y0, z0), (x1, y1, z1) = lo, hi
    faces = [
        [(x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1)],
        [(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)],
        [(x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (x0, y1, z0)],
        [(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)],
        [(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)],
        [(x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0)],
    ]
    return flat_faces(faces, uv_scale)


# ---------------------------------------------------------------------------
# Hand-Painted Procedural Textures (embedded PNGs)
# ---------------------------------------------------------------------------
def normal_map_from_height(size: int, h_fn, strength: float = 2.0) -> bytes:
    grid = [[h_fn(x / size, y / size) for x in range(size)] for y in range(size)]

    def px(x, y):
        xl, xr = (x - 1) % size, (x + 1) % size
        yu, yd = (y - 1) % size, (y + 1) % size
        dx = (grid[y][xr] - grid[y][xl]) * strength
        dy = (grid[yd][x] - grid[yu][x]) * strength
        nx, ny, nz = -dx, -dy, 1.0
        l = math.sqrt(nx * nx + ny * ny + nz * nz) or 1.0
        return (
            int(max(0, min(255, (nx / l * 0.5 + 0.5) * 255))),
            int(max(0, min(255, (ny / l * 0.5 + 0.5) * 255))),
            int(max(0, min(255, (nz / l * 0.5 + 0.5) * 255))),
            255,
        )

    return png_bytes(size, size, px)


def tex_stone_brick(size=128) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        row = int(v * 6)
        col_shift = 0.5 if row % 2 else 0.0
        bx = (u * 3 + col_shift) % 1.0
        by = (v * 6) % 1.0
        mortar = (bx < 0.06 or by < 0.08)
        n = fbm2(u * 8.0, v * 8.0, 3, 17)
        block_var = (hash2(int(u * 3 + col_shift), row, 5) - 0.5) * 26
        moss = max(0.0, (0.38 - v) * 2.2 + (fbm2(u * 6.0, v * 6.0, 3, 91) - 0.52) * 1.4)
        moss = min(1.0, moss)
        r = 176 + block_var + (n - 0.5) * 32
        g = 166 + block_var + (n - 0.5) * 28
        b = 148 + block_var + (n - 0.5) * 24
        if mortar:
            r, g, b = r * 0.62, g * 0.62, b * 0.60
        r = r * (1 - moss) + 74 * moss
        g = g * (1 - moss) + 122 * moss
        b = b * (1 - moss) + 64 * moss
        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, b))), 255)

    return png_bytes(size, size, px)


def tex_stone_brick_normal(size=128) -> bytes:
    def h(u, v):
        row = int(v * 6)
        col_shift = 0.5 if row % 2 else 0.0
        bx = (u * 3 + col_shift) % 1.0
        by = (v * 6) % 1.0
        mortar = 0.0 if (bx < 0.06 or by < 0.08) else 0.72
        return mortar + fbm2(u * 12.0, v * 12.0, 3, 17) * 0.28

    return normal_map_from_height(size, h, strength=2.6)


def tex_natural_rock(size=128) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        n1 = fbm2(u * 5.0, v * 5.0, 4, 71)
        n2 = fbm2(u * 18.0, v * 18.0, 3, 83)
        cracks = abs(fbm2(u * 7.5, v * 7.5, 3, 47) - 0.5) * 2.0
        crevice = 0.68 if cracks < 0.08 else 1.0
        moss = max(0.0, (0.42 - v) * 2.0 + (n1 - 0.5) * 1.1)
        moss = min(0.85, moss)
        r = (132 + n1 * 48 + (n2 - 0.5) * 26) * crevice
        g = (128 + n1 * 44 + (n2 - 0.5) * 24) * crevice
        b = (120 + n1 * 38 + (n2 - 0.5) * 20) * crevice
        r = r * (1 - moss) + 76 * moss
        g = g * (1 - moss) + 126 * moss
        b = b * (1 - moss) + 64 * moss
        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, b))), 255)

    return png_bytes(size, size, px)


def tex_natural_rock_normal(size=128) -> bytes:
    def h(u, v):
        cracks = abs(fbm2(u * 7.5, v * 7.5, 3, 47) - 0.5) * 2.0
        base = 0.35 if cracks < 0.08 else 0.7
        return base + fbm2(u * 16.0, v * 16.0, 4, 83) * 0.4

    return normal_map_from_height(size, h, strength=3.0)


def tex_wood_grain(size=128) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        plank = int(u * 4)
        seam = ((u * 4) % 1.0) < 0.05
        grain = math.sin((u * 26.0 + fbm2(u * 5.0, v * 12.0, 3, 23) * 3.5) * math.pi) * 0.5 + 0.5
        shade = (hash2(plank, 0, 12) - 0.5) * 22
        r = 128 + grain * 34 + shade
        g = 82 + grain * 24 + shade * 0.7
        b = 48 + grain * 14 + shade * 0.5
        if seam:
            r, g, b = r * 0.5, g * 0.5, b * 0.5
        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, b))), 255)

    return png_bytes(size, size, px)


def tex_bark(size=96) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        ridge = fbm2(u * 14.0, v * 4.0, 3, 41)
        r = 86 + ridge * 46
        g = 58 + ridge * 32
        b = 38 + ridge * 22
        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, b))), 255)

    return png_bytes(size, size, px)


def tex_foliage(base_rgb, hi_rgb, size=96, seed=7) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        dapple = fbm2(u * 7.5, v * 7.5, 4, seed)
        sun = max(0.0, 1.0 - v * 0.85) * 0.45 + dapple * 0.55
        r = base_rgb[0] * (1 - sun) + hi_rgb[0] * sun
        g = base_rgb[1] * (1 - sun) + hi_rgb[1] * sun
        b = base_rgb[2] * (1 - sun) + hi_rgb[2] * sun
        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, b))), 255)

    return png_bytes(size, size, px)


def tex_water(size=128) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        w1 = fbm2(u * 8.0, v * 8.0, 3, 61)
        w2 = math.sin((u * 6.0 + w1 * 1.6) * math.tau) * 0.5 + 0.5
        foam = max(0.0, (w1 * 0.7 + w2 * 0.3) - 0.68) * 3.0
        r = 26 + foam * 180
        g = 148 + w1 * 34 + foam * 95
        b = 186 + w2 * 30 + foam * 60
        return (int(min(255, r)), int(min(255, g)), int(min(255, b)), 208)

    return png_bytes(size, size, px)


def tex_cloth(base_rgb, trim_rgb, size=96) -> bytes:
    def px(x, y):
        u, v = x / size, y / size
        weave = fbm2(u * 14.0, v * 14.0, 2, 19) * 18 - 9
        is_trim = (v > 0.84 and v < 0.94)
        c = trim_rgb if is_trim else base_rgb
        return (int(max(0, min(255, c[0] + weave))), int(max(0, min(255, c[1] + weave))), int(max(0, min(255, c[2] + weave))), 255)

    return png_bytes(size, size, px)


# ---------------------------------------------------------------------------
# Model Builders
# ---------------------------------------------------------------------------
def build_terrain():
    b = Builder()
    grid = 76
    pos, nor, uv, idx = [], [], [], []
    step = (WORLD_HALF * 2.0) / grid

    for gz in range(grid + 1):
        z = -WORLD_HALF + gz * step
        for gx in range(grid + 1):
            x = -WORLD_HALF + gx * step
            y = terrain_height(x, z)
            eps = 8.0
            hx = terrain_height(x + eps, z) - terrain_height(x - eps, z)
            hz = terrain_height(x, z + eps) - terrain_height(x, z - eps)
            nx, ny, nz = -hx, 2.0 * eps, -hz
            nl = math.sqrt(nx * nx + ny * ny + nz * nz) or 1.0
            pos.append((x, y, z))
            nor.append((nx / nl, ny / nl, nz / nl))
            uv.append((gx / grid, gz / grid))

    w = grid + 1
    for gz in range(grid):
        for gx in range(grid):
            a = gz * w + gx
            b0 = a + 1
            c = a + w
            d = c + 1
            idx += [a, c, b0, b0, c, d]

    # Crisp 640x640 hand-painted FBM multi-biome terrain texture + tangent-space normal map
    tex_sz = 640

    def biome_pixel(px, py):
        u, v = px / (tex_sz - 1), py / (tex_sz - 1)
        x = -WORLD_HALF + u * (WORLD_HALF * 2.0)
        z = -WORLD_HALF + v * (WORLD_HALF * 2.0)
        y = terrain_height(x, z)
        eps = 12.0
        slope = math.hypot(terrain_height(x + eps, z) - terrain_height(x - eps, z), terrain_height(x, z + eps) - terrain_height(x, z - eps)) / (2.0 * eps)

        n_macro = fbm2(u * 10.0, v * 10.0, 4, 3)
        n_micro = fbm2(u * 36.0, v * 36.0, 3, 19)

        # Base sunlit lush meadow grass
        r = 84 + n_macro * 42 + (n_micro - 0.5) * 26
        g = 152 + n_macro * 38 + (n_micro - 0.5) * 24
        bl = 70 + n_macro * 22 + (n_micro - 0.5) * 18

        # Emerald pine forest floor (deeper mossy green)
        d_forest = math.hypot(x + 580.0, z + 260.0)
        f_w = max(0.0, min(1.0, 1.0 - (d_forest - 160.0) / 260.0))
        r = r * (1 - f_w) + (56 + n_macro * 28) * f_w
        g = g * (1 - f_w) + (112 + n_macro * 32) * f_w
        bl = bl * (1 - f_w) + (66 + n_macro * 18) * f_w

        # Wind Ruins sandstone plaza tint
        d_ruins = math.hypot(x - 540.0, z + 320.0)
        r_w = max(0.0, min(1.0, 1.0 - (d_ruins - 120.0) / 180.0))
        r = r * (1 - r_w) + (164 + n_micro * 26) * r_w
        g = g * (1 - r_w) + (152 + n_micro * 24) * r_w
        bl = bl * (1 - r_w) + (116 + n_micro * 20) * r_w

        # Cliff rock strata on steeper slopes
        s_w = max(0.0, min(1.0, (slope - 0.28) / 0.26))
        strata = math.sin(y * 0.22 + n_macro * 4.0) * 14
        r = r * (1 - s_w) + (126 + strata + n_micro * 20) * s_w
        g = g * (1 - s_w) + (120 + strata + n_micro * 18) * s_w
        bl = bl * (1 - s_w) + (112 + strata + n_micro * 16) * s_w

        # High alpine peak snow-dusted rock
        a_w = max(0.0, min(1.0, (y - 125.0) / 65.0))
        r = r * (1 - a_w) + (214 + n_micro * 20) * a_w
        g = g * (1 - a_w) + (220 + n_micro * 18) * a_w
        bl = bl * (1 - a_w) + (228 + n_micro * 16) * a_w

        # Golden sandy beach near water level
        sand_w = max(0.0, min(1.0, (2.0 - y) / 16.0))
        r = r * (1 - sand_w) + (218 + n_micro * 20) * sand_w
        g = g * (1 - sand_w) + (196 + n_micro * 18) * sand_w
        bl = bl * (1 - sand_w) + (142 + n_micro * 14) * sand_w

        # Warm dirt trails radiating from camp
        wiggle = (n_macro - 0.5) * 48.0
        path_d = min(
            abs(x + wiggle) if -680 < z < 260 else 999,
            abs((x + 580) * 0.45 - (z + 260) * 0.55 + wiggle * 0.5) if -580 < x < 20 and -260 < z < 230 else 999,
            abs((x - 540) * 0.5 + (z + 320) * 0.5 + wiggle * 0.5) if 0 < x < 540 and -320 < z < 230 else 999,
        )
        if path_d < 32.0 and y > -2.0 and y < 128.0:
            pw = max(0.0, min(1.0, 1.0 - path_d / 32.0)) * 0.72
            r = r * (1 - pw) + (182 + n_micro * 22) * pw
            g = g * (1 - pw) + (156 + n_micro * 20) * pw
            bl = bl * (1 - pw) + (112 + n_micro * 16) * pw

        return (int(max(0, min(255, r))), int(max(0, min(255, g))), int(max(0, min(255, bl))), 255)

    tex = b.image(png_bytes(tex_sz, tex_sz, biome_pixel))
    n_tex = b.image(normal_map_from_height(384, lambda u, v: fbm2(u * 64.0, v * 64.0, 3, 29) * 0.6 + fbm2(u * 22.0, v * 22.0, 3, 13) * 0.4, strength=2.2))
    mat = b.add_material((1, 1, 1, 1), 0.0, 0.90, tex=tex, normal_tex=n_tex, normal_scale=0.85)
    b.add_mesh("terrain", [(pos, nor, uv, idx, mat)])
    b.gltf["nodes"] = [{"name": "terrain", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("terrain.glb")


def build_water():
    b = Builder()
    tex = b.image(tex_water(128))
    mat = b.add_material((1, 1, 1, 0.84), metallic=0.18, roughness=0.12, tex=tex, alpha="BLEND", double_sided=True)
    pos = [(-3800, 0, -3800), (-3800, 0, 3800), (3800, 0, 3800), (3800, 0, -3800)]
    nor = [(0, 1, 0)] * 4
    uv = [(0, 0), (0, 28), (28, 28), (28, 0)]
    idx = [0, 1, 2, 0, 2, 3]
    b.add_mesh("water", [(pos, nor, uv, idx, mat)])
    b.gltf["nodes"] = [{"name": "water", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("water.glb")


def build_explorer():
    """Sculpted animated traveler with rounded head, curved wide-brim hat, pleated cloak,
    leather backpack, and animated limbs (`idle` and `walk` glTF clips!)."""
    b = Builder()
    cloak_tex = b.image(tex_cloth((42, 108, 178), (235, 198, 92)))
    hat_tex = b.image(tex_cloth((118, 72, 42), (195, 68, 54)))
    wood_tex = b.image(tex_wood_grain(64))

    m_cloak = b.add_material((1, 1, 1, 1), 0.0, 0.82, tex=cloak_tex)
    m_skin = b.add_material((0.96, 0.80, 0.66, 1.0), 0.0, 0.68)
    m_hat = b.add_material((1, 1, 1, 1), 0.0, 0.84, tex=hat_tex)
    m_scarf = b.add_material((0.88, 0.28, 0.24, 1.0), 0.0, 0.78)
    m_pack = b.add_material((0.85, 0.72, 0.58, 1.0), 0.0, 0.85, tex=wood_tex)
    m_boot = b.add_material((0.26, 0.18, 0.14, 1.0), 0.05, 0.75)

    # Torso + pleated bell cloak (origin at y=0, feet at -24)
    cloak = lathe([(12.5, -14), (11.2, -6), (9.2, 1), (7.6, 7), (5.0, 9.5)], segs=16, wobble=0.04, seed=5)
    scarf = lathe([(7.6, 6.2), (8.6, 8.2), (7.2, 10.2)], segs=14)
    head = transform_mesh(organic_sphere(6.2, 6.6, 6.2, 10, 14, rough=0.02, seed=2), offset=(0, 14.5, 0))
    hat = lathe([(14.5, 18.0), (13.8, 19.6), (7.5, 20.6), (6.0, 26.5), (2.2, 31.5), (0.3, 33.0)], segs=16, wobble=0.03, seed=9)
    pack = transform_mesh(organic_sphere(6.4, 7.6, 4.4, 8, 12, rough=0.05, seed=3), offset=(0, 2.0, 9.2))
    bedroll = transform_mesh(lathe([(3.0, -6.5), (3.2, 0), (3.0, 6.5)], segs=10), offset=(0, 9.8, 9.4), rot_x=math.pi / 2)
    bedroll = transform_mesh(bedroll, rot_y=math.pi / 2)
    body_mesh = b.add_mesh("body", [
        (*cloak, m_cloak),
        (*scarf, m_scarf),
        (*head, m_skin),
        (*hat, m_hat),
        (*pack, m_pack),
        (*bedroll, m_scarf),
    ])

    # Rounded limbs (pivoted at hip/shoulder)
    leg = lathe([(2.8, -11.5), (3.2, -9.0), (2.6, -5.0), (3.0, 0.0)], segs=10)
    leg_mesh = b.add_mesh("leg", [(*leg, m_boot)])
    arm = lathe([(2.2, -9.5), (2.6, -5.0), (2.8, 0.0)], segs=10)
    arm_mesh = b.add_mesh("arm", [(*arm, m_cloak)])

    # Node hierarchy: 0=root, 1=body, 2=legL, 3=legR, 4=armL, 5=armR
    b.gltf["nodes"] = [
        {"name": "root", "children": [1, 2, 3, 4, 5]},
        {"name": "body", "mesh": body_mesh, "translation": [0, 0, 0]},
        {"name": "legL", "mesh": leg_mesh, "translation": [-4.2, -12.5, 0]},
        {"name": "legR", "mesh": leg_mesh, "translation": [4.2, -12.5, 0]},
        {"name": "armL", "mesh": arm_mesh, "translation": [-8.8, 6.5, 0]},
        {"name": "armR", "mesh": arm_mesh, "translation": [8.8, 6.5, 0]},
    ]

    # Animations: "idle" (gentle breath) & "walk" (lively stride + arm swing + bob)
    t_idle = b.accessor([0.0, 0.8, 1.6], "SCALAR", vertex=False)
    bob_idle = b.accessor([(0, 0.0, 0), (0, 1.4, 0), (0, 0.0, 0)], "VEC3", vertex=False)
    idle_anim = {
        "name": "idle",
        "samplers": [{"input": t_idle, "output": bob_idle}],
        "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}],
    }

    keys = 12
    times = [i / keys * 0.6 for i in range(keys + 1)]
    t_walk = b.accessor(times, "SCALAR", vertex=False)
    samplers, channels = [], []

    def add_rot(node_idx, amp, phase):
        vals = [quat((1, 0, 0), amp * math.sin(2 * math.pi * (t / 0.6) + phase)) for t in times]
        s_idx = len(samplers)
        samplers.append({"input": t_walk, "output": b.accessor(vals, "VEC4", vertex=False)})
        channels.append({"sampler": s_idx, "target": {"node": node_idx, "path": "rotation"}})

    add_rot(2, 0.62, 0.0)
    add_rot(3, 0.62, math.pi)
    add_rot(4, 0.48, math.pi)
    add_rot(5, 0.48, 0.0)
    bob_vals = [(0.0, abs(math.sin(2 * math.pi * (t / 0.6))) * 2.4, 0.0) for t in times]
    s_bob = len(samplers)
    samplers.append({"input": t_walk, "output": b.accessor(bob_vals, "VEC3", vertex=False)})
    channels.append({"sampler": s_bob, "target": {"node": 1, "path": "translation"}})

    b.gltf["animations"] = [idle_anim, {"name": "walk", "samplers": samplers, "channels": channels}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("explorer.glb")


def build_staff():
    """Sculpted glowing Wind Staff for Bone Socket attachment (origin at hand grip)."""
    b = Builder()
    wood_tex = b.image(tex_wood_grain(64))
    m_wood = b.add_material((0.92, 0.78, 0.62, 1.0), 0.05, 0.78, tex=wood_tex)
    m_gold = b.add_material((0.92, 0.76, 0.34, 1.0), 0.78, 0.28)
    m_orb = b.add_material((0.42, 0.96, 1.0, 1.0), 0.15, 0.12, emissive=(0.45, 1.15, 1.35))

    shaft = curved_tube([(0, -14, 0), (0.3, -4, 0.2), (0, 6, 0), (-0.4, 16, -0.3), (0, 23, 0)], [1.1, 1.25, 1.35, 1.45, 1.2], 8)
    collar = lathe([(1.5, 20.5), (2.4, 22.0), (2.8, 23.8), (1.8, 24.6)], segs=10)
    pommel = lathe([(0.5, -15.8), (1.6, -14.2), (1.3, -13.0)], segs=8)
    orb = transform_mesh(lathe([(0.1, -4.2), (3.4, -0.5), (3.4, 0.8), (0.1, 4.8)], segs=6), offset=(0, 27.6, 0))
    ring = transform_mesh(lathe([(4.4, -0.5), (4.9, 0.0), (4.4, 0.5)], segs=12, cap_top=False, cap_bottom=False), offset=(0, 27.6, 0), rot_x=0.45)
    b.add_mesh("staff", [(*shaft, m_wood), (*merge_meshes([collar, pommel, ring]), m_gold), (*orb, m_orb)])
    b.gltf["nodes"] = [{"name": "staff", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("staff.glb")


def build_wisp():
    """Luminous Wind Spirit companion & patrol sentinel with gentle bobbing animation."""
    b = Builder()
    m_core = b.add_material((0.68, 0.96, 1.0, 1.0), 0.1, 0.18, emissive=(0.35, 0.95, 1.15))
    m_ring = b.add_material((1.0, 0.86, 0.38, 1.0), 0.7, 0.25, emissive=(0.45, 0.35, 0.1))
    m_eye = b.add_material((0.08, 0.16, 0.24, 1.0), 0.1, 0.2)

    body = lathe([(0.2, -9.5), (4.8, -5.5), (7.2, 0.0), (6.4, 5.2), (3.2, 8.2), (0.2, 9.2)], segs=14)
    halo = transform_mesh(lathe([(9.5, -0.7), (10.3, 0.0), (9.5, 0.7)], segs=16, cap_top=False, cap_bottom=False), offset=(0, 3.0, 0), rot_x=0.35)
    wing_l = transform_mesh(organic_sphere(5.5, 2.2, 1.4, 6, 8, 0.1, 1), offset=(-8.0, 2.5, 2.0), rot_y=0.4)
    wing_r = transform_mesh(organic_sphere(5.5, 2.2, 1.4, 6, 8, 0.1, 2), offset=(8.0, 2.5, 2.0), rot_y=-0.4)
    eye_l = transform_mesh(organic_sphere(1.1, 1.4, 1.0, 6, 8, 0.0, 3), offset=(-2.4, 1.2, -6.4))
    eye_r = transform_mesh(organic_sphere(1.1, 1.4, 1.0, 6, 8, 0.0, 4), offset=(2.4, 1.2, -6.4))
    mesh_id = b.add_mesh("wisp", [(*merge_meshes([body, wing_l, wing_r]), m_core), (*halo, m_ring), (*merge_meshes([eye_l, eye_r]), m_eye)])
    b.gltf["nodes"] = [{"name": "wisp", "mesh": mesh_id}]
    keys = 12
    times = [i / keys * 2.0 for i in range(keys + 1)]
    t_acc = b.accessor(times, "SCALAR", vertex=False)
    tr_vals = [(0.0, math.sin(2 * math.pi * i / keys) * 3.0, 0.0) for i in range(keys + 1)]
    b.gltf["animations"] = [{
        "name": "float",
        "samplers": [{"input": t_acc, "output": b.accessor(tr_vals, "VEC3", vertex=False)}],
        "channels": [{"sampler": 0, "target": {"node": 0, "path": "translation"}}],
    }]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("wisp.glb")


def build_glider():
    """Curved airfoil wind glider with arched ribs and luminous cloth canopy."""
    b = Builder()
    cloth_tex = b.image(tex_cloth((242, 214, 128), (68, 224, 245)))
    m_sail = b.add_material((1, 1, 1, 0.94), 0.0, 0.55, tex=cloth_tex, emissive=(0.14, 0.24, 0.28), alpha="BLEND", double_sided=True)
    m_frame = b.add_material((0.46, 0.30, 0.18, 1.0), 0.08, 0.72)

    # Multi-segment curved airfoil canopy
    pos, uv, idx = [], [], []
    nx, nz = 12, 4
    for iz in range(nz + 1):
        vz = iz / nz
        for ix in range(nx + 1):
            vx = (ix / nx) * 2.0 - 1.0  # -1..1
            x = vx * 58.0
            sweep = abs(vx) * 14.0
            z = -16.0 + vz * 28.0 + sweep
            arch = math.cos(vx * math.pi * 0.5) * 11.0 + math.sin(vz * math.pi) * 4.5
            pos.append((x, arch, z))
            uv.append((ix / nx, vz))
    w = nx + 1
    for iz in range(nz):
        for ix in range(nx):
            a = iz * w + ix
            c = a + w
            idx += [a, c, a + 1, a + 1, c, c + 1]
    sail = (pos, compute_smooth_normals(pos, idx), uv, idx)

    spar = curved_tube([(-52, 2, -2), (-26, 9, -8), (0, 12, -10), (26, 9, -8), (52, 2, -2)], [1.4, 1.8, 2.0, 1.8, 1.4], 8)
    spine = curved_tube([(0, 11, -14), (0, 12, 0), (0, 9, 18)], [1.6, 1.8, 1.2], 8)
    b.add_mesh("glider", [(*sail, m_sail), (*merge_meshes([spar, spine]), m_frame)])
    b.gltf["nodes"] = [{"name": "glider", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("glider.glb")


def build_trees():
    bark_tex = tex_bark(96)

    # 1. Sculpted Pine Tree (organic curved trunk + drooping irregular foliage boughs)
    b = Builder()
    m_bark = b.add_material((1, 1, 1, 1), 0.0, 0.92, tex=b.image(bark_tex))
    m_leaf = b.add_material((1, 1, 1, 1), 0.0, 0.85, tex=b.image(tex_foliage((26, 94, 54), (72, 168, 92), seed=11)))
    trunk = curved_tube([(0, 0, 0), (2, 18, -1), (-1, 38, 1), (0, 62, 0)], [9.5, 7.5, 5.2, 2.5], 10)
    boughs = [
        lathe([(48, 26), (42, 34), (24, 56), (12, 72)], segs=12, wobble=0.18, seed=1),
        lathe([(38, 52), (32, 60), (18, 82), (8, 96)], segs=12, wobble=0.18, seed=2),
        lathe([(27, 78), (22, 86), (10, 108), (1.2, 124)], segs=12, wobble=0.16, seed=3),
    ]
    b.add_mesh("pine", [(*trunk, m_bark), (*merge_meshes(boughs), m_leaf)])
    b.gltf["nodes"] = [{"name": "pine", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("pine.glb")

    # 2. Broadleaf Oak Tree (gnarled branching trunk + 6 overlapping lush foliage clouds)
    b = Builder()
    m_bark = b.add_material((1, 1, 1, 1), 0.0, 0.92, tex=b.image(bark_tex))
    m_leaf = b.add_material((1, 1, 1, 1), 0.0, 0.82, tex=b.image(tex_foliage((58, 132, 52), (144, 206, 82), seed=21)))
    t_main = curved_tube([(0, 0, 0), (4, 20, -2), (-2, 42, 3), (0, 62, 0)], [12, 9.5, 7.5, 5.0], 10)
    br1 = curved_tube([(-1, 38, 2), (-18, 56, 10), (-30, 68, 14)], [5.5, 3.8, 2.0], 8)
    br2 = curved_tube([(1, 44, -1), (20, 62, -12), (32, 74, -16)], [5.2, 3.5, 2.0], 8)
    br3 = curved_tube([(0, 48, 0), (4, 66, 20), (8, 76, 30)], [4.8, 3.2, 1.8], 8)
    clusters = [
        transform_mesh(organic_sphere(36, 26, 34, 9, 12, 0.18, 10), offset=(0, 82, 0)),
        transform_mesh(organic_sphere(28, 22, 26, 8, 12, 0.2, 11), offset=(-30, 72, 14)),
        transform_mesh(organic_sphere(28, 22, 26, 8, 12, 0.2, 12), offset=(32, 76, -14)),
        transform_mesh(organic_sphere(25, 20, 24, 8, 12, 0.18, 13), offset=(8, 78, 28)),
        transform_mesh(organic_sphere(24, 19, 24, 8, 12, 0.18, 14), offset=(-18, 74, -22)),
    ]
    b.add_mesh("oak", [(*merge_meshes([t_main, br1, br2, br3]), m_bark), (*merge_meshes(clusters), m_leaf)])
    b.gltf["nodes"] = [{"name": "oak", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("oak.glb")

    # 3. Blossom / Golden Spirit Tree (graceful sweeping trunk + pastel cherry-blossom canopy)
    b = Builder()
    m_bark = b.add_material((0.88, 0.80, 0.82, 1.0), 0.0, 0.90, tex=b.image(bark_tex))
    m_leaf = b.add_material((1, 1, 1, 1), 0.0, 0.78, tex=b.image(tex_foliage((225, 118, 146), (255, 206, 218), seed=33)))
    t_main = curved_tube([(0, 0, 0), (-6, 22, 4), (4, 46, -4), (10, 68, 0)], [10.5, 8.2, 6.2, 4.2], 10)
    br1 = curved_tube([(2, 42, -2), (-22, 60, -14), (-34, 70, -18)], [5.0, 3.2, 1.8], 8)
    br2 = curved_tube([(6, 52, 0), (26, 66, 14), (36, 74, 20)], [4.8, 3.0, 1.6], 8)
    clusters = [
        transform_mesh(organic_sphere(34, 22, 32, 9, 12, 0.18, 21), offset=(10, 82, 0)),
        transform_mesh(organic_sphere(28, 19, 26, 8, 12, 0.2, 22), offset=(-30, 72, -16)),
        transform_mesh(organic_sphere(26, 18, 25, 8, 12, 0.2, 23), offset=(34, 76, 18)),
        transform_mesh(organic_sphere(22, 16, 22, 8, 12, 0.18, 24), offset=(-8, 88, 18)),
    ]
    b.add_mesh("blossom", [(*merge_meshes([t_main, br1, br2]), m_bark), (*merge_meshes(clusters), m_leaf)])
    b.gltf["nodes"] = [{"name": "blossom", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("blossom.glb")


def build_ruins_and_props():
    stone_tex = tex_stone_brick(128)
    stone_norm = tex_stone_brick_normal(128)
    rock_tex = tex_natural_rock(128)
    rock_norm = tex_natural_rock_normal(128)
    wood_tex = tex_wood_grain(128)

    # 1. Vaulted Curved Stone Ruin Arch (real semicircular stone voussoirs + sculpted moss!)
    b = Builder()
    m_stone = b.add_material((1, 1, 1, 1), 0.0, 0.88, tex=b.image(stone_tex), normal_tex=b.image(stone_norm), normal_scale=1.0)
    m_moss = b.add_material((1, 1, 1, 1), 0.0, 0.92, tex=b.image(tex_foliage((58, 114, 58), (112, 174, 82), seed=55)))
    parts = [
        # Left & right stepped piers
        box_mesh((-74, 0, -24), (-44, 16, 24)),
        box_mesh((-68, 16, -20), (-48, 92, 20)),
        box_mesh((44, 0, -24), (74, 16, 24)),
        box_mesh((48, 16, -20), (68, 92, 20)),
    ]
    # 11 radial curved arch voussoir stones
    arch_r = 58.0
    for i in range(11):
        a = math.pi * (i + 0.5) / 11.0
        cx, cy = math.cos(a) * arch_r, 92.0 + math.sin(a) * arch_r
        block = box_mesh((-10.5, -12, -21), (10.5, 12, 21))
        # Rotate around Z by (a - pi/2)
        cz, sz = math.cos(a - math.pi / 2), math.sin(a - math.pi / 2)
        p, n, u, idx = block
        rp = [(x * cz - y * sz + cx, x * sz + y * cz + cy, z) for x, y, z in p]
        rn = [(x * cz - y * sz, x * sz + y * cz, z) for x, y, z in n]
        parts.append((rp, rn, u, idx))
    moss_pads = [
        transform_mesh(organic_sphere(22, 8, 18, 7, 10, 0.22, 3), offset=(-18, 152, 0)),
        transform_mesh(organic_sphere(16, 7, 15, 7, 10, 0.22, 4), offset=(26, 146, 2)),
    ]
    b.add_mesh("ruin_arch", [(*merge_meshes(parts), m_stone), (*merge_meshes(moss_pads), m_moss)])
    b.gltf["nodes"] = [{"name": "ruin_arch", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("ruin_arch.glb")

    # 2. Fluted Ruin Pillar (stepped base + tapered fluted shaft + carved capital)
    b = Builder()
    m_stone = b.add_material((1, 1, 1, 1), 0.0, 0.88, tex=b.image(stone_tex))
    shaft = lathe([(22, 0), (21, 10), (16, 14), (14, 18), (13.5, 58), (12.0, 92), (17.5, 98), (19.5, 108)], segs=12, wobble=0.04, seed=8)
    b.add_mesh("ruin_pillar", [(*shaft, m_stone)])
    b.gltf["nodes"] = [{"name": "ruin_pillar", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("ruin_pillar.glb")

    # 3. Arched Wooden Ruin Gate (textured wood planks + bronze straps & ring handle)
    b = Builder()
    m_wood = b.add_material((1, 1, 1, 1), 0.0, 0.82, tex=b.image(wood_tex))
    m_bronze = b.add_material((0.82, 0.66, 0.30, 1.0), 0.72, 0.34)
    door = box_mesh((0, 0, -5), (84, 114, 5))
    bands = merge_meshes([
        box_mesh((2, 22, -6.5), (82, 31, 6.5)),
        box_mesh((2, 82, -6.5), (82, 91, 6.5)),
        transform_mesh(organic_sphere(6, 6, 3, 7, 10, 0.05, 1), offset=(68, 56, 7)),
        transform_mesh(organic_sphere(6, 6, 3, 7, 10, 0.05, 1), offset=(68, 56, -7)),
    ])
    b.add_mesh("ruin_gate", [(*door, m_wood), (*bands, m_bronze)])
    b.gltf["nodes"] = [{"name": "ruin_gate", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("ruin_gate.glb")

    # 4. Ornate Ancient Shrine with Animated Floating Crystal (`float` glTF animation!)
    for variant, c_col, c_emis in [
        ("shrine_dormant.glb", (1.0, 0.78, 0.28, 1.0), (0.72, 0.48, 0.12)),
        ("shrine_awake.glb", (0.28, 0.94, 1.0, 1.0), (0.25, 0.88, 1.0)),
    ]:
        b = Builder()
        m_stone = b.add_material((1, 1, 1, 1), 0.05, 0.84, tex=b.image(stone_tex))
        m_crystal = b.add_material(c_col, 0.25, 0.16, emissive=c_emis)
        dais = lathe([(42, 0), (40, 8), (32, 10), (30, 20), (22, 24), (16, 48), (22, 54), (18, 58)], segs=8)
        pillars = []
        for k in range(4):
            a = k * math.pi * 0.5 + math.pi * 0.25
            px, pz = math.cos(a) * 28.0, math.sin(a) * 28.0
            pillars.append(transform_mesh(lathe([(6, 0), (5, 18), (3.5, 42), (1.5, 50)], segs=6), offset=(px, 10, pz)))
        base_mesh = b.add_mesh("dais", [(*merge_meshes([dais] + pillars), m_stone)])

        # Multi-faceted elongated crystal spire
        crystal = lathe([(0.1, -26), (14.5, -4), (16.0, 4), (0.1, 30)], segs=6)
        crys_mesh = b.add_mesh("crystal", [(*crystal, m_crystal)])

        b.gltf["nodes"] = [
            {"name": "shrine", "mesh": base_mesh, "children": [1]},
            {"name": "crystal", "mesh": crys_mesh, "translation": [0, 86, 0]},
        ]
        # Built-in glTF float + spin animation
        keys = 16
        times = [i / keys * 3.2 for i in range(keys + 1)]
        t_acc = b.accessor(times, "SCALAR", vertex=False)
        tr_vals = [(0.0, 86.0 + math.sin(2 * math.pi * i / keys) * 7.5, 0.0) for i in range(keys + 1)]
        rot_vals = [quat((0, 1, 0), 2 * math.pi * i / keys) for i in range(keys + 1)]
        b.gltf["animations"] = [{
            "name": "float",
            "samplers": [
                {"input": t_acc, "output": b.accessor(tr_vals, "VEC3", vertex=False)},
                {"input": t_acc, "output": b.accessor(rot_vals, "VEC4", vertex=False)},
            ],
            "channels": [
                {"sampler": 0, "target": {"node": 1, "path": "translation"}},
                {"sampler": 1, "target": {"node": 1, "path": "rotation"}},
            ],
        }]
        b.gltf["scenes"] = [{"nodes": [0]}]
        b.gltf["scene"] = 0
        b.write(variant)

    # 5. Floating Wind Relic (faceted glowing star crystal)
    b = Builder()
    m_rel = b.add_material((1.0, 0.94, 0.48, 1.0), 0.3, 0.16, emissive=(0.95, 0.78, 0.22))
    gem = lathe([(0.1, -18), (11, -3), (12, 3), (0.1, 20)], segs=6)
    ring = lathe([(16, -2), (18, 0), (16, 2)], segs=12, cap_top=False, cap_bottom=False)
    b.add_mesh("relic", [(*merge_meshes([gem, ring]), m_rel)])
    b.gltf["nodes"] = [{"name": "relic", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("relic.glb")

    # 6. Campfire (sculpted river stones ring + organic firewood logs)
    b = Builder()
    m_stone = b.add_material((0.56, 0.54, 0.50, 1.0), 0.0, 0.92, tex=b.image(stone_tex))
    m_log = b.add_material((0.88, 0.72, 0.58, 1.0), 0.0, 0.88, tex=b.image(wood_tex), emissive=(0.18, 0.05, 0.0))
    stones = [
        transform_mesh(organic_sphere(7.5, 5.2, 6.8, 6, 8, 0.22, i + 1), offset=(math.cos(i * math.tau / 9) * 23, 3.5, math.sin(i * math.tau / 9) * 23))
        for i in range(9)
    ]
    logs = [
        curved_tube([(-16, 2, -4), (0, 8, 0), (16, 14, 4)], [3.6, 3.4, 3.0], 8),
        curved_tube([(14, 2, -12), (0, 9, 0), (-14, 15, 12)], [3.6, 3.4, 3.0], 8),
        curved_tube([(-10, 2, 14), (0, 10, 0), (10, 16, -14)], [3.4, 3.2, 2.8], 8),
    ]
    b.add_mesh("campfire", [(*merge_meshes(stones), m_stone), (*merge_meshes(logs), m_log)])
    b.gltf["nodes"] = [{"name": "campfire", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("campfire.glb")

    # 7. Floating Sky-Island Platform (rugged tapered rock underside + carved rune stone deck!)
    b = Builder()
    m_stone = b.add_material((1, 1, 1, 1), 0.05, 0.86, tex=b.image(stone_tex))
    m_rune = b.add_material((0.30, 0.90, 1.0, 1.0), 0.2, 0.28, emissive=(0.24, 0.75, 0.95))
    rock_base = lathe([(2, -52), (24, -36), (48, -18), (68, -4), (66, 8), (60, 10)], segs=10, wobble=0.14, seed=44)
    rune_ring = lathe([(54, 10.2), (54, 12.0), (44, 12.0), (44, 10.2)], segs=10)
    b.add_mesh("platform", [(*rock_base, m_stone), (*rune_ring, m_rune)])
    b.gltf["nodes"] = [{"name": "platform", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("platform.glb")

    # 8. Sculpted Mossy Boulder (natural granite rock texture + tangent-space normal map)
    b = Builder()
    m_rock = b.add_material((1, 1, 1, 1), 0.0, 0.90, tex=b.image(rock_tex), normal_tex=b.image(rock_norm), normal_scale=1.1)
    rock = organic_sphere(28, 20, 24, 10, 14, 0.24, seed=77)
    b.add_mesh("boulder", [(*rock, m_rock)])
    b.gltf["nodes"] = [{"name": "boulder", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("boulder.glb")

    # 9. Wooden Supply Barrel (rounded lathe barrel with bronze hoops, replaces boxy crates!)
    b = Builder()
    m_wood = b.add_material((1, 1, 1, 1), 0.0, 0.82, tex=b.image(wood_tex))
    m_iron = b.add_material((0.35, 0.36, 0.40, 1.0), 0.75, 0.4)
    body = lathe([(14, -18), (17.5, -8), (18.5, 0), (17.5, 8), (14, 18)], segs=12)
    hoop1 = lathe([(17.8, -11), (18.2, -9), (17.8, -7)], segs=12, cap_top=False, cap_bottom=False)
    hoop2 = lathe([(17.8, 7), (18.2, 9), (17.8, 11)], segs=12, cap_top=False, cap_bottom=False)
    b.add_mesh("crate", [(*body, m_wood), (*merge_meshes([hoop1, hoop2]), m_iron)])
    b.gltf["nodes"] = [{"name": "crate", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("crate.glb")

    # 10. Wildflower & Grass Cluster (adds lush ground detail everywhere!)
    b = Builder()
    m_grass = b.add_material((0.36, 0.68, 0.28, 1.0), 0.0, 0.85, double_sided=True)
    m_flower = b.add_material((1.0, 0.88, 0.35, 1.0), 0.0, 0.65, emissive=(0.15, 0.12, 0.02))
    blades = []
    flowers = []
    rng = random.Random(101)
    for _ in range(9):
        bx, bz = rng.uniform(-14, 14), rng.uniform(-14, 14)
        h = rng.uniform(11, 19)
        ang = rng.uniform(0, math.tau)
        blades.append(curved_tube([(bx, 0, bz), (bx + math.cos(ang) * 3, h * 0.55, bz + math.sin(ang) * 3), (bx + math.cos(ang) * 7, h, bz + math.sin(ang) * 7)], [1.4, 1.0, 0.2], 4))
    for _ in range(4):
        fx, fz = rng.uniform(-11, 11), rng.uniform(-11, 11)
        fh = rng.uniform(13, 18)
        flowers.append(transform_mesh(organic_sphere(2.8, 1.8, 2.8, 5, 7, 0.15, 2), offset=(fx, fh, fz)))
    b.add_mesh("flowers", [(*merge_meshes(blades), m_grass), (*merge_meshes(flowers), m_flower)])
    b.gltf["nodes"] = [{"name": "flowers", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("flowers.glb")

    # 11. Warm Glowing Trail Lantern Post
    b = Builder()
    m_wood = b.add_material((1, 1, 1, 1), 0.0, 0.86, tex=b.image(wood_tex))
    m_glow = b.add_material((1.0, 0.88, 0.52, 1.0), 0.0, 0.3, emissive=(1.0, 0.78, 0.35))
    post = curved_tube([(0, 0, 0), (0, 34, 0), (3, 56, 0), (14, 62, 0)], [3.6, 3.0, 2.4, 1.8], 8)
    lamp = transform_mesh(lathe([(0.5, -7), (5.5, -4), (6.0, 2), (3.5, 6), (0.5, 8)], segs=6), offset=(14, 52, 0))
    b.add_mesh("lantern", [(*post, m_wood), (*lamp, m_glow)])
    b.gltf["nodes"] = [{"name": "lantern", "mesh": 0}]
    b.gltf["scenes"] = [{"nodes": [0]}]
    b.gltf["scene"] = 0
    b.write("lantern.glb")


def build_world_json():
    rng = random.Random(2026)

    shrines = [
        {"id": "shrine-meadow", "name": "晨曦瞭望坛", "region": "晨曦草坡", "x": 115, "z": 120,
         "lore": "旅人的第一座风语石碑。登上北方星穹峰，乘风滑翔可俯瞰整片群岛。"},
        {"id": "shrine-forest", "name": "翡翠林圣坛", "region": "翡翠松林", "x": -580, "z": -260,
         "lore": "古松深处的萤火从未熄灭，风穿过林梢时常带着远古歌谣。"},
        {"id": "shrine-ruins", "name": "风蚀古迹坛", "region": "风蚀石林遗迹", "x": 540, "z": -320,
         "lore": "昔日浮空城邦的石门至今仍可推开，守望着海面上的第一缕晨光。"},
        {"id": "shrine-cove", "name": "晶蓝湖畔坛", "region": "晶蓝湖湾", "x": -290, "z": 310,
         "lore": "潮水退去时，湖底的晶石会映出与夜空一模一样的星图。"},
        {"id": "shrine-summit", "name": "星穹峰顶坛", "region": "星穹峰顶", "x": 0, "z": -760,
         "lore": "群岛最高处的风之信标。跃入云海展开滑翔伞，世界尽在脚下。"},
    ]
    for s in shrines:
        s["y"] = round(terrain_height(s["x"], s["z"]), 1)

    relic_coords = [
        (-160, 140), (-460, -140), (-680, -340), (360, -180),
        (610, -400), (-360, 440), (-120, -520), (0, -640),
    ]
    relics = [{"id": f"relic-{i}", "x": rx, "y": round(terrain_height(rx, rz) + 28.0, 1), "z": rz} for i, (rx, rz) in enumerate(relic_coords)]

    trees = []
    for _ in range(52):
        if len(trees) < 22:
            tx, tz = rng.uniform(-820, -320), rng.uniform(-520, 60)
            kind = "pine" if rng.random() < 0.72 else "oak"
        elif len(trees) < 34:
            tx, tz = rng.uniform(-340, 380), rng.uniform(-220, 460)
            kind = "oak" if rng.random() < 0.65 else "blossom"
        else:
            tx, tz = rng.uniform(-880, 880), rng.uniform(-650, 740)
            kind = rng.choice(["pine", "oak", "blossom"])
        ty = terrain_height(tx, tz)
        if ty < 4.0 or ty > 135.0:
            continue
        if math.hypot(tx, tz - 220) < 115:
            continue
        if any(math.hypot(tx - s["x"], tz - s["z"]) < 90 for s in shrines):
            continue
        if math.hypot(tx - 540, tz + 320) < 160:
            continue
        trees.append({
            "id": f"tree-{len(trees)}",
            "kind": kind,
            "x": round(tx, 1),
            "y": round(ty - 3.0, 1),
            "z": round(tz, 1),
            "scale": round(rng.uniform(0.85, 1.32), 2),
            "rot": round(rng.uniform(0, 360), 1),
        })

    flowers = []
    for _ in range(42):
        fx, fz = rng.uniform(-780, 760), rng.uniform(-560, 620)
        fy = terrain_height(fx, fz)
        if fy < 5.0 or fy > 115.0:
            continue
        if math.hypot(fx, fz - 220) < 55:
            continue
        flowers.append({
            "id": f"flw-{len(flowers)}",
            "x": round(fx, 1),
            "y": round(fy - 1.0, 1),
            "z": round(fz, 1),
            "scale": round(rng.uniform(0.8, 1.4), 2),
            "rot": round(rng.uniform(0, 360), 1),
        })

    lanterns = [
        {"id": "lantern-0", "x": 42, "z": 185, "rot": 210},
        {"id": "lantern-1", "x": -110, "z": 90, "rot": 45},
        {"id": "lantern-2", "x": 380, "z": -150, "rot": 120},
        {"id": "lantern-3", "x": -220, "z": 250, "rot": 300},
    ]
    for l in lanterns:
        l["y"] = round(terrain_height(l["x"], l["z"]) - 1.0, 1)

    boulders = []
    for i in range(18):
        bx, bz = rng.uniform(-850, 850), rng.uniform(-850, 700)
        by = terrain_height(bx, bz)
        if by < -4.0 or math.hypot(bx, bz - 220) < 110:
            continue
        if any(math.hypot(bx - s["x"], bz - s["z"]) < 80 for s in shrines):
            continue
        boulders.append({
            "id": f"boulder-{i}",
            "x": round(bx, 1),
            "y": round(by + 4.0, 1),
            "z": round(bz, 1),
            "scale": round(rng.uniform(0.8, 1.45), 2),
            "rot": round(rng.uniform(0, 360), 1),
        })

    ruin_y = round(terrain_height(540, -320), 1)
    ruins = {
        "center": [540, ruin_y, -320],
        "arches": [
            {"id": "ruin-arch-1", "x": 540, "y": round(terrain_height(540, -210) - 4, 1), "z": -210, "rot": 0},
            {"id": "ruin-arch-2", "x": 650, "y": round(terrain_height(650, -320) - 4, 1), "z": -320, "rot": 90},
        ],
        "pillars": [
            {"id": "ruin-p1", "x": 455, "y": round(terrain_height(455, -260) - 2, 1), "z": -260},
            {"id": "ruin-p2", "x": 455, "y": round(terrain_height(455, -380) - 2, 1), "z": -380},
            {"id": "ruin-p3", "x": 625, "y": round(terrain_height(625, -260) - 2, 1), "z": -260},
            {"id": "ruin-p4", "x": 625, "y": round(terrain_height(625, -380) - 2, 1), "z": -380},
        ],
        "gate": {"id": "ruin-gate", "x": 498, "y": round(terrain_height(540, -210), 1), "z": -210},
        "crates": [
            {"id": "crate-0", "x": 485, "y": round(terrain_height(485, -295) + 22, 1), "z": -295},
            {"id": "crate-1", "x": 524, "y": round(terrain_height(524, -295) + 22, 1), "z": -295},
            {"id": "crate-2", "x": 504, "y": round(terrain_height(504, -295) + 60, 1), "z": -295},
        ],
    }

    camp = {"x": 0, "y": round(terrain_height(0, 220), 1), "z": 220}
    spawn = {"x": 0, "y": round(terrain_height(0, 285) + 28, 1), "z": 285}
    platforms = [
        {"id": "sky-plat-1", "x": 0, "y": round(terrain_height(0, -410) + 86, 1), "z": -410},
        {"id": "sky-plat-2", "x": 0, "y": round(terrain_height(0, -560) + 112, 1), "z": -560},
    ]

    data = {
        "spawn": spawn,
        "camp": camp,
        "shrines": shrines,
        "relics": relics,
        "trees": trees,
        "flowers": flowers,
        "lanterns": lanterns,
        "boulders": boulders,
        "ruins": ruins,
        "platforms": platforms,
    }
    (BASE / "world.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Generated world.json ({len(trees)} trees, {len(flowers)} flowers, {len(lanterns)} lanterns, {len(boulders)} boulders)")


if __name__ == "__main__":
    build_terrain()
    build_water()
    build_explorer()
    build_staff()
    build_wisp()
    build_glider()
    build_trees()
    build_ruins_and_props()
    build_world_json()
    print("All sculpted textured 3D GLB assets generated.")
