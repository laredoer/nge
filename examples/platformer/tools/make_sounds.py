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
save("stomp.wav", sweep(500, 150, 0.12, 20, 0.6))
save("fall.wav", sweep(600, 120, 0.5, 4, 0.5))
# 动画事件放的（见 docs/anim-graph.md）：走跑时的脚步、落地
save("step.wav", sweep(160, 80, 0.06, 50, 0.4))
save("land.wav", sweep(240, 60, 0.12, 25, 0.55))
save("win.wav", notes([523, 659, 784, 1047], 0.12, 8, 0.4) + notes([1047], 0.5, 5, 0.4))


def loop_hum(seconds, base_hz, volume):
    """无缝循环的嗡嗡声（整数周期，适合听 3D 绕圈的左右声像和多普勒变调）。"""
    n = int(RATE * seconds)
    cycles = round(base_hz * seconds)
    f = cycles / seconds
    return [(0.65 * math.sin(2 * math.pi * f * i / RATE) + 0.25 * math.sin(4 * math.pi * f * i / RATE) + 0.1 * math.sin(6 * math.pi * f * i / RATE)) * volume for i in range(n)]


def loop_beacon(seconds, volume):
    """带高频泛音的循环脉冲提示音（在墙后面时低通遮挡非常明显）。"""
    n = int(RATE * seconds)
    out = []
    for i in range(n):
        t = (i / RATE) % 0.5
        env = math.exp(-t * 9.0) * min(1.0, t * 400.0)
        s = 0.5 * math.sin(2 * math.pi * 880.0 * t) + 0.35 * math.sin(2 * math.pi * 1760.0 * t) + 0.15 * math.sin(2 * math.pi * 2640.0 * t)
        out.append(s * env * volume)
    return out


def loop_fire(seconds, volume):
    """篝火环境声（低频轰鸣加轻微噼啪声，首尾交叉淡变无缝循环）。"""
    n = int(RATE * seconds)
    state = 1234567
    raw = []
    lp = 0.0
    for _ in range(n):
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        noise = (state / 0x3FFFFFFF) - 1.0
        lp = lp * 0.92 + noise * 0.08
        crackle = noise * 0.55 if (state & 0x3FF) == 0 else 0.0
        raw.append((lp * 2.2 + crackle) * volume)
    # 首尾 20 ms 交叉混合，确保循环不咔哒
    xf = min(int(RATE * 0.02), n // 4)
    for i in range(xf):
        k = i / xf
        raw[i] = raw[i] * k + raw[n - xf + i] * (1.0 - k)
    return raw[: n - xf]


# 3D 试验场的循环音效（见 docs/audio-3d.md）：绕圈多普勒、墙后遮挡、篝火环境声
save("lab-orbit.wav", loop_hum(1.0, 220.0, 0.35))
save("lab-wall.wav", loop_beacon(1.0, 0.4))
save("lab-fire.wav", loop_fire(2.0, 0.45))

