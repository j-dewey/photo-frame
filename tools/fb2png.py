#!/usr/bin/env python3
"""Turn a raw frame written to fb_file (32-bit BGRA) into a PNG.

    python3 tools/fb2png.py fb.raw screen.png [WIDTHxHEIGHT]

The size defaults to 1920x1080, the same as the program's fb_size default.
Needs nothing beyond the Python standard library.
"""
import struct
import sys
import zlib


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    src, dest = sys.argv[1], sys.argv[2]
    size = sys.argv[3] if len(sys.argv) == 4 else "1920x1080"
    width, height = (int(n) for n in size.split("x"))

    with open(src, "rb") as f:
        raw = f.read(width * height * 4)
    if len(raw) != width * height * 4:
        sys.exit(f"{src} is smaller than {size} at 4 bytes per pixel")

    # BGRA rows -> RGB rows, each starting with PNG filter byte 0.
    rows = bytearray()
    for y in range(height):
        row = raw[y * width * 4 : (y + 1) * width * 4]
        rgb = bytearray(width * 3)
        rgb[0::3] = row[2::4]
        rgb[1::3] = row[1::4]
        rgb[2::3] = row[0::4]
        rows += b"\0" + rgb

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    with open(dest, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", header))
        f.write(chunk(b"IDAT", zlib.compress(bytes(rows), 6)))
        f.write(chunk(b"IEND", b""))


if __name__ == "__main__":
    main()
