"""生成 assets/hills.png：白色的连绵山丘剪影，左右能无缝接上（横向重复用），颜色由精灵的 color 染。

    python3 tools/make_hills.py
"""

import math
import struct
import zlib
from pathlib import Path

W, H = 256, 160


def height(x: float) -> float:
    # 周期都是 W 的整数分之一，左右两头一样高
    t = 2 * math.pi * x / W
    # 最低处也高过地面（放大 1.5 倍后高出地面瓦片），最高处接近图片顶边
    return 95 + 30 * math.sin(t) + 18 * math.sin(2 * t + 1.3) + 8 * math.sin(5 * t + 0.4)


def png(width: int, height_: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height_, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main() -> None:
    rows = []
    for y in range(H):
        row = bytearray()
        for x in range(W):
            # 图片 y 朝下：离底边 H - y 低于山高的是山；边上半个像素做抗锯齿
            cover = max(0.0, min(1.0, height(x + 0.5) - (H - y - 0.5) + 0.5))
            row += bytes((255, 255, 255, round(cover * 255)))
        rows.append(bytes(row))
    out = Path(__file__).resolve().parent.parent / "assets" / "hills.png"
    out.write_bytes(png(W, H, rows))
    print(out)


if __name__ == "__main__":
    main()
