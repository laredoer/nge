"""生成 assets/stone_n.png：石碑的法线图（32×64）。四边往外倒角，中间两道横向的凹缝，表面有一点起伏。
RGB = 法线 × 0.5 + 0.5，绿色朝上、蓝色朝外（见 docs/render2d-more.md）。

    python3 tools/make_stone.py
"""

import math
import struct
import zlib
from pathlib import Path

W, H = 32, 64
BEVEL = 5
GROOVES = (21, 42)


def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def normal(x: int, y: int) -> tuple[float, float, float]:
    # y 是图片的行（往下），法线的 y 朝上
    nx = ny = 0.0
    if x < BEVEL:
        nx -= 0.7
    if x >= W - BEVEL:
        nx += 0.7
    if y < BEVEL:
        ny += 0.7
    if y >= H - BEVEL:
        ny -= 0.7
    for g in GROOVES:
        # 凹缝的上沿朝下、下沿朝上
        if y == g - 1:
            ny -= 0.8
        if y == g + 1:
            ny += 0.8
    # 一点起伏
    nx += 0.12 * math.sin(x * 1.7 + y * 0.6)
    ny += 0.12 * math.sin(y * 1.3 - x * 0.4)
    length = math.sqrt(nx * nx + ny * ny + 1.0)
    return nx / length, ny / length, 1.0 / length


def main() -> None:
    rows = []
    for y in range(H):
        row = bytearray()
        for x in range(W):
            n = normal(x, y)
            row += bytes(round((c * 0.5 + 0.5) * 255) for c in n) + b"\xff"
        rows.append(bytes(row))
    out = Path(__file__).resolve().parent.parent / "assets" / "stone_n.png"
    out.write_bytes(png(W, H, rows))
    print(f"写好 {out}")


if __name__ == "__main__":
    main()
