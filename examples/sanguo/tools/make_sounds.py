#!/usr/bin/env python3
"""合成长坂坡的音效（只用标准库）：python3 examples/sanguo/tools/make_sounds.py，输出到 examples/sanguo/assets/。

挥枪、击中、射箭、马蹄（冲锋）、胜利、失败。噪声用固定种子，重跑字节不变。
"""

import math
import random
import struct
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets"
FADE = int(RATE * 0.005)


def fade(samples):
    n = len(samples)
    return [s * min(1.0, (n - 1 - i) / FADE, i / FADE if FADE else 1.0) for i, s in enumerate(samples)]


def save(name, samples):
    with wave.open(str(OUT / name), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32000)) for s in samples))
    print(f"wrote assets/{name}")


def noise(seconds, seed):
    rng = random.Random(seed)
    return [rng.uniform(-1, 1) for _ in range(int(RATE * seconds))]


def lowpass(samples, cutoff):
    """一阶低通；cutoff 可以是随样本变的函数（赫兹）。"""
    out, y = [], 0.0
    for i, s in enumerate(samples):
        f = cutoff(i / RATE) if callable(cutoff) else cutoff
        a = 1 - math.exp(-2 * math.pi * f / RATE)
        y += a * (s - y)
        out.append(y)
    return out


def mix(*tracks):
    n = max(len(t) for t in tracks)
    return [sum(t[i] for t in tracks if i < len(t)) for i in range(n)]


def env(samples, attack, decay, volume):
    return [s * volume * min(1.0, (i / RATE) / attack) * math.exp(-(i / RATE) * decay) for i, s in enumerate(samples)]


def tone(freq, seconds, decay, volume, bend=0.0):
    out, phase = [], 0.0
    for i in range(int(RATE * seconds)):
        t = i / RATE
        phase += 2 * math.pi * freq * (1 + bend * t) / RATE
        out.append((math.sin(phase) + 0.25 * math.sin(2 * phase)) * volume * math.exp(-t * decay) * min(1.0, t * 400))
    return out


def swing():
    """挥枪：一阵由低到高再落下的风声。"""
    n = noise(0.22, 1)
    body = lowpass(n, lambda t: 600 + 3200 * math.sin(math.pi * min(1.0, t / 0.22)))
    return fade(env(body, 0.03, 9, 1.6))


def hit():
    """击中：短促的闷响加一点脆的噪声。"""
    thump = tone(110, 0.18, 22, 0.8, bend=-2.5)
    crack = env(lowpass(noise(0.06, 2), 2500), 0.002, 50, 0.9)
    return fade(mix(thump, crack))


def arrow():
    """射箭：弦响（低音一弹）+ 箭飞出去的哨声。"""
    twang = tone(180, 0.15, 18, 0.6, bend=0.6)
    whistle = env(lowpass(noise(0.3, 3), lambda t: 1800 + 2500 * t), 0.05, 7, 0.35)
    return fade(mix(twang, [0.0] * int(RATE * 0.02) + whistle))


def hooves():
    """冲锋的马蹄：一串由慢到快的嗒嗒声，三拍一组。"""
    out = [0.0] * int(RATE * 1.3)
    t, gap = 0.0, 0.16
    for k in range(12):
        clop = mix(tone(170 + 30 * (k % 3), 0.07, 45, 0.7), env(lowpass(noise(0.04, 10 + k), 1600), 0.001, 70, 0.6))
        start = int(t * RATE)
        for i, s in enumerate(clop):
            if start + i < len(out):
                out[start + i] += s
        t += gap * (0.55 if k % 3 == 2 else 0.35)
        gap = max(0.09, gap * 0.95)
    return fade(out)


def notes(seq, volume):
    """一串音：(频率, 秒)，带一点泛音，每个音末尾淡出。"""
    out = []
    for f, sec in seq:
        out += fade(tone(f, sec, 3.5, volume))
    return out


def win():
    # 五声音阶往上走，最后一个长音
    return notes([(392, 0.14), (440, 0.14), (523, 0.14), (659, 0.2), (784, 0.7)], 0.45)


def lose():
    # 往下走，最后低沉的长音
    return notes([(440, 0.25), (392, 0.25), (330, 0.3), (220, 0.9)], 0.45)


def main():
    OUT.mkdir(exist_ok=True)
    save("swing.wav", swing())
    save("hit.wav", hit())
    save("arrow.wav", arrow())
    save("hooves.wav", hooves())
    save("win.wav", win())
    save("lose.wav", lose())


if __name__ == "__main__":
    main()
