"""合成平台跳跃的音效：python3 examples/platformer/tools/make_sounds.py，输出到 examples/platformer/assets/。"""

import math
import struct
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets"
# 每个音末尾淡出的样本数（约 5 ms），免得截断处「咔哒」一声
FADE = int(RATE * 0.005)


def fade(samples):
    n = len(samples)
    return [s * min(1.0, (n - 1 - i) / FADE) for i, s in enumerate(samples)]


def save(name, samples):
    with wave.open(str(OUT / name), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32000)) for s in samples))


def sweep(start, end, seconds, decay, volume):
    """频率从 start 滑到 end 的短音，指数衰减。"""
    n = int(RATE * seconds)
    phase, out = 0.0, []
    for i in range(n):
        t = i / RATE
        phase += 2 * math.pi * (start + (end - start) * t / seconds) / RATE
        out.append(math.sin(phase) * volume * math.exp(-t * decay) * min(1.0, t * 600))
    return fade(out)


def notes(freqs, step, decay, volume):
    """一串音，每个音 step 秒，带一个八度的泛音。"""
    out = []
    for f in freqs:
        n = int(RATE * step)
        out += fade([(math.sin(2 * math.pi * f * i / RATE) + 0.3 * math.sin(4 * math.pi * f * i / RATE)) * volume * math.exp(-i / RATE * decay) * min(1.0, i / RATE * 600) for i in range(n)])
    return out


save("jump.wav", sweep(300, 700, 0.15, 12, 0.5))
save("coin.wav", notes([988, 1319], 0.08, 10, 0.4) + notes([1319], 0.2, 14, 0.4))
save("fall.wav", sweep(600, 120, 0.5, 4, 0.5))
save("win.wav", notes([523, 659, 784, 1047], 0.12, 8, 0.4) + notes([1047], 0.5, 5, 0.4))
