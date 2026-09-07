"""Generate a 1024x1024 app icon PNG (rounded square + mark) with stdlib only."""
import struct, zlib, math

SIZE = 1024
CORNER = 224
# indigo gradient top -> bottom
TOP = (99, 102, 241)
BOT = (67, 56, 202)


def in_rounded_rect(x, y):
    if CORNER <= x <= SIZE - CORNER or CORNER <= y <= SIZE - CORNER:
        return 0 <= x < SIZE and 0 <= y < SIZE
    cx = min(max(x, CORNER), SIZE - CORNER)
    cy = min(max(y, CORNER), SIZE - CORNER)
    return (x - cx) ** 2 + (y - cy) ** 2 <= CORNER ** 2


def in_triangle(px, py):
    a = (512, 250)
    b = (272, 760)
    c = (752, 760)

    def sign(p1, p2, p3):
        return (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])

    d1 = sign((px, py), a, b)
    d2 = sign((px, py), b, c)
    d3 = sign((px, py), c, a)
    has_neg = (d1 < 0) or (d2 < 0) or (d3 < 0)
    has_pos = (d1 > 0) or (d2 > 0) or (d3 > 0)
    return not (has_neg and has_pos)


def in_crossbar(px, py):
    return 272 + (py - 560) * 0.22 <= px <= 752 - (py - 560) * 0.22 and 560 <= py <= 636


rows = []
for y in range(SIZE):
    row = bytearray([0])  # filter byte
    for x in range(SIZE):
        if not in_rounded_rect(x, y):
            row += b"\x00\x00\x00\x00"
            continue
        t = y / (SIZE - 1)
        r = round(TOP[0] + (BOT[0] - TOP[0]) * t)
        g = round(TOP[1] + (BOT[1] - TOP[1]) * t)
        b = round(TOP[2] + (BOT[2] - TOP[2]) * t)
        if in_triangle(x, y) and not in_crossbar(x, y):
            r = g = b = 255
        row += bytes((r, g, b, 255))
    rows.append(bytes(row))

raw = b"".join(rows)


def chunk(tag, data):
    c = struct.pack(">I", len(data)) + tag + data
    return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw, 9))
    + chunk(b"IEND", b"")
)
with open("src-tauri/app-icon.png", "wb") as f:
    f.write(png)
print("icon written:", len(png), "bytes")
