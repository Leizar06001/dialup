#!/usr/bin/env python3
"""Génère les icônes de l'appli (PNG RGBA, ICO, ICNS) sans dépendance.

Usage : python3 app/tools/gen_icons.py
Dessine un petit modem externe façon années 90 : boîtier sombre,
rangée de LEDs rouges, une verte, et une onde phosphore.
"""
import math
import os
import struct
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "icons")


def png_bytes(w, h, pixels):
    """pixels : liste de (r, g, b, a) en lignes."""
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            raw.extend(pixels[y * w + x])

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)  # 8 bits, RGBA
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")


def draw(size):
    s = size / 128.0
    px = [(0, 0, 0, 0)] * (size * size)

    def blend(x, y, col, a):
        if 0 <= x < size and 0 <= y < size and a > 0:
            r0, g0, b0, a0 = px[y * size + x]
            a = min(1.0, a)
            na = a + (a0 / 255.0) * (1 - a)
            if na <= 0:
                return
            mix = lambda c1, c0: int((c1 * a + c0 * (a0 / 255.0) * (1 - a)) / na)
            px[y * size + x] = (mix(col[0], r0), mix(col[1], g0), mix(col[2], b0), int(na * 255))

    def rounded_rect(x0, y0, x1, y1, r, col_fn):
        for y in range(size):
            for x in range(size):
                cx = min(max(x + 0.5, x0 + r), x1 - r)
                cy = min(max(y + 0.5, y0 + r), y1 - r)
                d = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
                a = max(0.0, min(1.0, r - d + 0.5))
                if a > 0:
                    blend(x, y, col_fn(x, y), a)

    def circle(cx, cy, r, col, glow=0.0):
        for y in range(int(cy - r - glow - 2), int(cy + r + glow + 3)):
            for x in range(int(cx - r - glow - 2), int(cx + r + glow + 3)):
                d = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
                if glow > 0 and d > r:
                    blend(x, y, col, 0.45 * max(0.0, 1 - (d - r) / glow) ** 2)
                blend(x, y, col, max(0.0, min(1.0, r - d + 0.5)))

    # Boîtier
    rounded_rect(6 * s, 22 * s, 122 * s, 106 * s, 14 * s,
                 lambda x, y: (int(58 - 22 * y / size), int(52 - 20 * y / size), int(46 - 18 * y / size)))
    # Façade (bande plus claire)
    rounded_rect(14 * s, 64 * s, 114 * s, 96 * s, 6 * s, lambda x, y: (24, 21, 18))
    # Écran phosphore
    rounded_rect(16 * s, 30 * s, 112 * s, 58 * s, 5 * s, lambda x, y: (10, 26, 14))
    # Onde (porteuse modulée)
    for i in range(int(92 * s * 4)):
        t = i / (4.0 * s)  # abscisse en unités « 128 px »
        x = (18 + t) * s
        y = (44 + 9 * math.sin(t / 4.0) * (0.55 + 0.45 * math.sin(t / 15.0))) * s
        circle(x, y, 1.3 * s, (110, 255, 140), glow=2.0 * s)
    # LEDs
    cols = [(255, 60, 40)] * 5 + [(80, 255, 90)]
    for i, c in enumerate(cols):
        circle((24 + i * 16) * s, 80 * s, 4.2 * s, c, glow=5 * s)
    return px


def main():
    os.makedirs(OUT, exist_ok=True)
    pngs = {}
    for sz in (32, 64, 128, 256):
        pngs[sz] = png_bytes(sz, sz, draw(sz))
    open(os.path.join(OUT, "32x32.png"), "wb").write(pngs[32])
    open(os.path.join(OUT, "128x128.png"), "wb").write(pngs[128])
    open(os.path.join(OUT, "icon.png"), "wb").write(pngs[256])

    # ICO : images PNG embarquées (Windows Vista+)
    entries = [(32, pngs[32]), (64, pngs[64]), (256, pngs[256])]
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dirs, blobs = b"", b""
    for sz, data in entries:
        dim = 0 if sz >= 256 else sz
        dirs += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    open(os.path.join(OUT, "icon.ico"), "wb").write(header + dirs + blobs)

    # ICNS : ic07 = 128x128, ic08 = 256x256, ic11 = 32x32 (16@2x)
    chunks = b""
    for tag, sz in ((b"ic11", 32), (b"ic07", 128), (b"ic08", 256)):
        chunks += tag + struct.pack(">I", len(pngs[sz]) + 8) + pngs[sz]
    open(os.path.join(OUT, "icon.icns"), "wb").write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
    print("icônes écrites dans", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
