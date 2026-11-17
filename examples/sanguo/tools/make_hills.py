#!/usr/bin/env python3
"""生成 assets/hills.png：白色的远山剪影（尖一点的山峰，叠两层），左右能无缝接上，颜色由精灵的 color 染：

    python3 examples/sanguo/tools/make_hills.py
"""

import math
import struct
import zlib
from pathlib import Path

W, H = 512, 200
OUT = Path(__file__).resolve().parent.parent / "assets" / "hills.png"


def ridge(x: float, peaks: list[tuple[float, float, float]]) -> float:
    """几座山峰（位置、高、宽）的最高处；按周期 W 绕回，左右两头一样高。"""
    best = 0.0
    for px, h, w in peaks:
        d = min(abs(x - px), W - abs(x - px))
        best = max(best, h * max(0.0, 1 - (d / w) ** 1.3))
    t = 2 * math.pi * x / W
    return best + 6 * math.sin(3 * t) + 3 * math.sin(11 * t + 0.7)


BACK = [(40, 170, 120), (190, 140, 110), (300, 185, 140), (440, 150, 100)]
FRONT = [(110, 110, 90), (250, 95, 80), (380, 120, 110), (500, 90, 70)]


def png(rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main() -> None:
    rows = []
    for y in range(H):
        row = bytearray()
        up = H - y - 0.5
        for x in range(W):
            back = max(0.0, min(1.0, ridge(x + 0.5, BACK) + 20 - up + 0.5))
            front = max(0.0, min(1.0, ridge(x + 0.5, FRONT) + 10 - up + 0.5))
            # 前面那层亮一点（白），后面那层暗一点（灰），染色后有远近
            alpha = max(back, front)
            grey = round(255 * (front + (1 - front) * 0.7)) if alpha else 0
            row += bytes((grey, grey, grey, round(alpha * 255)))
        rows.append(bytes(row))
    OUT.write_bytes(png(rows))
    print(OUT)


if __name__ == "__main__":
    main()
