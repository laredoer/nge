"""合成示例用的声音：python3 tools/make_sounds.py（在 examples/physics 下跑），输出到 assets/。"""

import math
import struct
import wave

RATE = 22050


def save(name, samples):
    with wave.open(f"assets/{name}", "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32000)) for s in samples))


def ding():
    """撞击：一个带泛音、很快衰减的音。"""
    n = int(RATE * 0.3)
    return [
        (math.sin(2 * math.pi * 1046 * t) + 0.4 * math.sin(2 * math.pi * 2093 * t)) * 0.45 * math.exp(-t * 14) * min(1.0, t * 400)
        for t in (i / RATE for i in range(n))
    ]


def pop():
    """点中：从高到低滑下来的短音。"""
    n = int(RATE * 0.14)
    phase, out = 0.0, []
    for i in range(n):
        t = i / RATE
        phase += 2 * math.pi * (700 - 3500 * t) / RATE
        out.append(math.sin(phase) * 0.6 * math.exp(-t * 25) * min(1.0, t * 600))
    return out


def music():
    """8 秒一循环的琶音，每个音都在下一个音开始前衰减完，首尾能接上。"""
    chords = [[220, 277, 330, 440], [196, 247, 294, 392], [175, 220, 262, 349], [196, 247, 294, 392]]
    step = 0.25
    out = []
    for chord in chords:
        for k in range(8):
            f = chord[k % 4] * (2 if k >= 4 else 1)
            n = int(RATE * step)
            for i in range(n):
                t = i / RATE
                env = math.sin(math.pi * i / n) ** 2 * math.exp(-t * 6)
                out.append((math.sin(2 * math.pi * f * t) + 0.3 * math.sin(2 * math.pi * f * 2 * t)) * 0.25 * env)
    return out


save("ding.wav", ding())
save("pop.wav", pop())
save("music.wav", music())
