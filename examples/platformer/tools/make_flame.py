"""生成 3D 粒子用的贴图（见 docs/particles3d-more.md）：

- assets/flame.png：4 × 4 格、每格 32 × 32 的火苗序列帧，按行从左上角数，一团火苗从窜高到收小、左右摆；
- assets/puff.png：32 × 32 的一团软边白烟（中心不透明、往外淡出，带一点起伏）；
- assets/ring.png：64 × 64 的白色圆环（冲击波），外沿亮、往里淡。

    python3 tools/make_flame.py
"""

import math
import struct
import zlib
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
CELL = 32
GRID = 4


def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def clamp(x: float) -> float:
    return max(0.0, min(1.0, x))


def flame(frame: int, x: float, y: float) -> tuple[int, int, int, int]:
    """第 frame 帧在格子里 (x, y)（0–1，y 往下）处的颜色。"""
    t = frame / (GRID * GRID)
    # 火苗：底部宽、往上收尖，随帧左右摆、先高后矮
    height = 0.85 - 0.35 * t
    sway = 0.12 * math.sin(t * math.tau * 2.0)
    up = clamp((1.0 - y) / height)  # 0 在底，1 在顶
    center = 0.5 + sway * up * up
    width = 0.32 * (1.0 - up) ** 0.6 * (1.0 - 0.3 * t)
    d = abs(x - center) / max(width, 1e-3)
    if up >= 1.0 or d >= 1.0:
        return (0, 0, 0, 0)
    heat = (1.0 - d * d) * (1.0 - up)
    r = clamp(0.6 + heat)
    g = clamp(0.15 + heat * 1.1)
    b = clamp(heat * heat * 0.8)
    a = clamp((1.0 - d) * 1.6) * clamp((1.0 - up) * 2.5)
    return (round(r * 255), round(g * 255), round(b * 255), round(a * 255))


def puff(x: float, y: float) -> tuple[int, int, int, int]:
    dx, dy = x - 0.5, y - 0.5
    r = math.hypot(dx, dy) * 2.0
    angle = math.atan2(dy, dx)
    edge = 0.85 + 0.1 * math.sin(angle * 5.0) + 0.05 * math.sin(angle * 9.0 + 1.0)
    a = clamp(1.0 - r / edge) ** 1.5
    return (255, 255, 255, round(a * 255))


def ring(x: float, y: float) -> tuple[int, int, int, int]:
    r = math.hypot(x - 0.5, y - 0.5) * 2.0
    a = clamp(1.0 - abs(r - 0.85) / 0.12) if r < 0.85 else clamp(1.0 - (r - 0.85) / 0.06)
    a = max(a, clamp(r - 0.3) * 0.25 if r < 0.85 else 0.0)
    return (255, 255, 255, round(a * 255))


def main() -> None:
    size = CELL * GRID
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            frame = (py // CELL) * GRID + px // CELL
            x, y = (px % CELL + 0.5) / CELL, (py % CELL + 0.5) / CELL
            row += bytes(flame(frame, x, y))
        rows.append(bytes(row))
    (ASSETS / "flame.png").write_bytes(png(size, size, rows))
    rows = [b"".join(bytes(puff((px + 0.5) / CELL, (py + 0.5) / CELL)) for px in range(CELL)) for py in range(CELL)]
    (ASSETS / "puff.png").write_bytes(png(CELL, CELL, rows))
    n = 64
    rows = [b"".join(bytes(ring((px + 0.5) / n, (py + 0.5) / n)) for px in range(n)) for py in range(n)]
    (ASSETS / "ring.png").write_bytes(png(n, n, rows))


if __name__ == "__main__":
    main()
