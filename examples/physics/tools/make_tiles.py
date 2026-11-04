"""画示例用的瓦片图集：python3 tools/make_tiles.py（在 examples/physics 下跑），输出 assets/tiles.png。

一行 3 格，每格 32×32：0 石墙，1 顶上长草的地面，2 木平台。
"""

import struct
import zlib

TILE = 32


def hex_rgb(h):
    return tuple(int(h[i : i + 2], 16) for i in (1, 3, 5))


def stone(x, y):
    if x in (0, TILE - 1) or y in (0, TILE - 1):
        return hex_rgb("#262c3a")
    if y == 1 or x == 1:
        return hex_rgb("#4a5470")
    # 错开的砖缝
    if y == TILE // 2 or (x == TILE // 2 and y < TILE // 2) or (x == 4 and y > TILE // 2):
        return hex_rgb("#2e3546")
    return hex_rgb("#3a4256")


def grass(x, y):
    if y < 5:
        return hex_rgb("#7bd389") if y < 2 else hex_rgb("#4f9d5d")
    if y == 5 and x % 4 == 0:
        return hex_rgb("#4f9d5d")
    return stone(x, y)


def plank(x, y):
    if x in (0, TILE - 1) or y in (0, TILE - 1):
        return hex_rgb("#4a3424")
    if y in (10, 21):
        return hex_rgb("#5e4330")
    return hex_rgb("#a07850") if y < 3 else hex_rgb("#8a6a4a")


def png(path, width, height, pixel):
    raw = b"".join(b"\x00" + b"".join(bytes(pixel(x, y)) + b"\xff" for x in range(width)) for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


TILES = [stone, grass, plank]
png("assets/tiles.png", TILE * len(TILES), TILE, lambda x, y: TILES[x // TILE](x % TILE, y))
