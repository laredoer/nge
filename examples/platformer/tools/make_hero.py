"""生成 assets/hero.png：主角的精灵图集（8 × 2 格，每格 24 × 32，脸朝右），给动画状态机用（见 docs/anim-graph.md）。

格子：0–1 待机（呼吸）、2–5 走、6–9 跑、10 跳、11 落、12 落地（蹲一下），13–15 空着。
走、跑的第 0、2 格是一只脚落地的样子（hero-anim.json 在这两格放 step 事件）。

    python3 examples/platformer/tools/make_hero.py
"""

import struct
import zlib
from pathlib import Path

CELL_W, CELL_H = 24, 32
COLS, ROWS = 8, 2
OUT = Path(__file__).resolve().parent.parent / "assets" / "hero.png"

CLEAR = (0, 0, 0, 0)
BODY = (255, 122, 89, 255)
BODY_DARK = (214, 92, 66, 255)
SKIN = (255, 214, 176, 255)
HAIR = (92, 58, 40, 255)
LEGS = (74, 59, 92, 255)
SHOE = (40, 32, 48, 255)
EYE = (30, 24, 30, 255)


def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


class Cell:
    """一格，坐标原点在左上，y 朝下。"""

    def __init__(self) -> None:
        self.px = [[CLEAR] * CELL_W for _ in range(CELL_H)]

    def rect(self, x: int, y: int, w: int, h: int, color) -> None:
        for yy in range(max(0, y), min(CELL_H, y + h)):
            for xx in range(max(0, x), min(CELL_W, x + w)):
                self.px[yy][xx] = color


def leg(c: Cell, hip_x: int, hip_y: int, dx: int, lift: int) -> None:
    """一条腿：从髋往下，脚往前（dx > 0）或往后挪，`lift` 是脚离地多少。"""
    foot_y = CELL_H - 2 - lift
    length = foot_y - hip_y
    for i in range(length):
        x = hip_x + round(dx * i / max(1, length))
        c.rect(x, hip_y + i, 3, 1, LEGS)
    c.rect(hip_x + dx - (0 if dx >= 0 else 1), foot_y, 4, 2, SHOE)


def hero(bob: int = 0, lean: int = 0, legs=((0, 0), (0, 0)), arms=(0, 0), crouch: int = 0, arms_up: bool = False) -> Cell:
    """`bob` 身子往下沉几格，`lean` 往前倾，`legs` 两条腿的 (dx, lift)，`arms` 两只手往前摆多少。"""
    c = Cell()
    top = 4 + bob + crouch
    hip = 20 + bob + crouch // 2
    # 后面那条腿先画，颜色深一点由鞋盖住
    leg(c, 9 + lean, hip, legs[1][0], legs[1][1])
    # 身子
    c.rect(7 + lean, top + 8, 10, hip - top - 7, BODY)
    c.rect(7 + lean, hip - 2, 10, 2, BODY_DARK)
    # 头和头发、眼睛（朝右）
    c.rect(7 + lean, top, 10, 9, SKIN)
    c.rect(6 + lean, top - 1, 12, 3, HAIR)
    c.rect(6 + lean, top + 2, 3, 3, HAIR)
    c.rect(14 + lean, top + 4, 2, 2, EYE)
    # 前面那条腿
    leg(c, 11 + lean, hip, legs[0][0], legs[0][1])
    # 手
    for i, swing in enumerate(arms):
        x = 11 + lean + swing
        if arms_up:
            # 举在头两边
            side = 4 + lean if i else 17 + lean
            c.rect(side, top + 1, 3, 9, BODY_DARK if i else BODY)
            c.rect(side, top - 1, 3, 2, SKIN)
        else:
            c.rect(x, top + 10, 3, 8, BODY_DARK if i else BODY)
            c.rect(x, top + 17, 3, 2, SKIN)
    return c


def frames() -> list[Cell]:
    walk = [((3, 0), (-3, 0)), ((1, 2), (-1, 0)), ((-3, 0), (3, 0)), ((-1, 0), (1, 2))]
    run = [((5, 0), (-5, 1)), ((2, 4), (-2, 0)), ((-5, 1), (5, 0)), ((-2, 0), (2, 4))]
    cells = [hero(), hero(bob=1)]
    cells += [hero(bob=i % 2, legs=l, arms=(2 if i < 2 else -2, -2 if i < 2 else 2)) for i, l in enumerate(walk)]
    cells += [hero(bob=i % 2, lean=2, legs=l, arms=(4 if i < 2 else -4, -4 if i < 2 else 4)) for i, l in enumerate(run)]
    cells.append(hero(legs=((3, 6), (-2, 4)), arms_up=True))
    cells.append(hero(legs=((2, 0), (-2, 1)), arms_up=True))
    cells.append(hero(crouch=4, legs=((4, 0), (-4, 0)), arms=(3, -3)))
    while len(cells) < COLS * ROWS:
        cells.append(Cell())
    return cells


def main() -> None:
    cells = frames()
    rows = []
    for row in range(ROWS):
        for y in range(CELL_H):
            line = bytearray()
            for col in range(COLS):
                for pixel in cells[row * COLS + col].px[y]:
                    line += bytes(pixel)
            rows.append(bytes(line))
    OUT.write_bytes(png(CELL_W * COLS, CELL_H * ROWS, rows))
    print(f"写好 {OUT}（{CELL_W * COLS}×{CELL_H * ROWS}）")


if __name__ == "__main__":
    main()
