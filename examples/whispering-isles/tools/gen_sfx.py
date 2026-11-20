#!/usr/bin/env python3
"""Generate warm atmospheric WAV sound effects for Whispering Isles."""
import math
import os
import random
import struct
import wave

SAMPLE_RATE = 44100
OUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets")


def write_wav(name: str, samples: list[float]) -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    with wave.open(os.path.join(OUT_DIR, name), "w") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(SAMPLE_RATE)
        frames = bytearray()
        for s in samples:
            v = max(-1.0, min(1.0, s))
            frames += struct.pack("<h", int(v * 32767))
        wf.writeframes(frames)


def synth_jump() -> list[float]:
    n = int(SAMPLE_RATE * 0.16)
    out, phase = [], 0.0
    for i in range(n):
        k = i / n
        freq = 240.0 + 320.0 * (k ** 0.7)
        phase += 2.0 * math.pi * freq / SAMPLE_RATE
        env = math.sin(math.pi * k) * (1.0 - 0.5 * k)
        out.append(math.sin(phase) * env * 0.35)
    return out


def synth_glide() -> list[float]:
    n = int(SAMPLE_RATE * 0.28)
    rng = random.Random(11)
    out, lp = [], 0.0
    for i in range(n):
        k = i / n
        lp = lp * 0.88 + rng.uniform(-1.0, 1.0) * 0.12
        env = math.sin(math.pi * k)
        out.append(lp * 2.2 * env * 0.42)
    return out


def synth_shrine() -> list[float]:
    # Warm major-9th temple chord
    freqs = [261.63, 329.63, 392.0, 493.88, 587.33]
    dur = 1.4
    n = int(SAMPLE_RATE * dur)
    out = []
    for i in range(n):
        t = i / SAMPLE_RATE
        k = i / n
        env = min(1.0, t / 0.06) * ((1.0 - k) ** 1.6)
        s = sum(math.sin(2.0 * math.pi * f * t) / len(freqs) for f in freqs)
        shimmer = 0.15 * math.sin(2.0 * math.pi * 1046.5 * t) * math.exp(-t * 3.0)
        out.append((s + shimmer) * env * 0.55)
    return out


def synth_relic() -> list[float]:
    notes = [659.25, 880.0, 1318.5]
    n = int(SAMPLE_RATE * 0.36)
    out, phase = [], 0.0
    for i in range(n):
        k = i / n
        idx = min(2, int(k * 3))
        phase += 2.0 * math.pi * notes[idx] / SAMPLE_RATE
        env = (1.0 - k) ** 1.3
        out.append((0.75 * math.sin(phase) + 0.25 * math.sin(phase * 2.0)) * env * 0.38)
    return out


def synth_time() -> list[float]:
    n = int(SAMPLE_RATE * 0.35)
    out, phase = [], 0.0
    for i in range(n):
        k = i / n
        freq = 320.0 + 180.0 * math.sin(math.pi * k)
        phase += 2.0 * math.pi * freq / SAMPLE_RATE
        env = math.sin(math.pi * k)
        out.append(math.sin(phase) * env * 0.32)
    return out


if __name__ == "__main__":
    write_wav("jump.wav", synth_jump())
    write_wav("glide.wav", synth_glide())
    write_wav("shrine.wav", synth_shrine())
    write_wav("relic.wav", synth_relic())
    write_wav("time.wav", synth_time())
    print("Generated 5 atmospheric WAV files.")
