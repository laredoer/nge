"""生成 assets/tiles.png：一行 16 格、每格 32×32 的瓦片图集（见 docs/gameplay2d.md）。
前 3 格（石块、草地、木板）照抄现有 tiles.png 的前 96 像素，后面的画出来：
3 单向木板，4 / 5 是 45° 上坡、下坡，6 / 7 是 22.5° 上坡的低、高半段，8 / 9 是 22.5° 下坡的高、低半段，
10 … 12 水（三帧），13 / 14 火把（两帧），15 空着。

    python3 tools/make_tiles.py
"""

import math
import struct
import zlib
from pathlib import Path

T = 32
COLS = 16
OUT = Path(__file__).resolve().parent.parent / "assets" / "tiles.png"


def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def read_png(data: bytes) -> tuple[int, int, list[bytearray]]:
    """读 8 位 RGBA、不隔行的 PNG。"""
    width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", data[16:29])
    assert (depth, color, interlace) == (8, 6, 0), "只认 8 位 RGBA"
    pos, idat = 8, b""
    while pos < len(data):
        (n,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        if kind == b"IDAT":
            idat += data[pos + 8 : pos + 8 + n]
        pos += 12 + n
    raw = zlib.decompress(idat)
    stride = width * 4
    rows: list[bytearray] = []
    prev = bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1 : (y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - 4] if i >= 4 else 0
            b = prev[i]
            c = prev[i - 4] if i >= 4 else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    return width, height, rows


Pixel = tuple[int, int, int, int]
STONE = (125, 127, 134, 255)
DIRT = (122, 86, 56, 255)
GRASS = (95, 174, 79, 255)
WOOD = (176, 128, 74, 255)
DARK_WOOD = (120, 84, 44, 255)
CLEAR = (0, 0, 0, 0)


def shade(c: Pixel, k: float) -> Pixel:
    return (min(255, int(c[0] * k)), min(255, int(c[1] * k)), min(255, int(c[2] * k)), c[3])


def one_way(x: int, y: int) -> Pixel:
    # 上面一条木板（8 像素厚），下面两根细支架
    if y < 8:
        return shade(WOOD, 1.15 if y < 2 else 0.85 if y > 5 else 1.0)
    if x in (5, 6, 25, 26) and y < 14:
        return DARK_WOOD
    return CLEAR


def slope(left: float, right: float):
    """顶边从左边的高 `left` 到右边的高 `right`（格高的几分之几）：线以下是土、贴着线的一层草。"""

    def draw(x: int, y: int) -> Pixel:
        f = (x + 0.5) / T
        top = T * (1 - (left + (right - left) * f))
        depth = y + 0.5 - top
        if depth < 0:
            return CLEAR
        if depth < 4:
            return GRASS
        return shade(DIRT, 1.0 - 0.15 * ((x // 8 + y // 8) % 2))

    return draw


def water(phase: int):
    def draw(x: int, y: int) -> Pixel:
        wave = 3 + 2 * math.sin((x / T + phase / 3) * 2 * math.pi)
        if y < wave:
            return CLEAR
        k = 1.25 if y < wave + 2 else 1.0 - 0.2 * y / T
        return shade((60, 130, 210, 200), k)

    return draw


def torch(phase: int):
    def draw(x: int, y: int) -> Pixel:
        cx = 16
        if 18 <= y < 30 and 14 <= x < 18:
            return DARK_WOOD
        # 火苗：两帧左右晃一下
        sway = 1 if phase else -1
        dx = x + 0.5 - (cx + sway * (18 - y) / 8)
        r = (y - 6) / 12
        if 6 <= y < 18 and abs(dx) < 4.5 * (1 - abs(r - 0.6)):
            inner = abs(dx) < 2 * (1 - abs(r - 0.6))
            return (255, 230, 120, 255) if inner else (240, 120, 40, 230)
        return CLEAR

    return draw


def blank(x: int, y: int) -> Pixel:
    return CLEAR


TILES = [
    one_way,
    slope(0, 1),
    slope(1, 0),
    slope(0, 0.5),
    slope(0.5, 1),
    slope(1, 0.5),
    slope(0.5, 0),
    water(0),
    water(1),
    water(2),
    torch(0),
    torch(1),
    blank,
]


def main() -> None:
    width, height, old = read_png(OUT.read_bytes())
    assert height == T and width >= 3 * T, "现有 tiles.png 至少要有前 3 格"
    rows = []
    for y in range(T):
        row = bytearray(old[y][: 3 * T * 4])
        for draw in TILES:
            for x in range(T):
                row += bytes(draw(x, y))
        rows.append(bytes(row))
    assert len(rows[0]) == COLS * T * 4
    OUT.write_bytes(png(COLS * T, T, rows))


if __name__ == "__main__":
    main()
