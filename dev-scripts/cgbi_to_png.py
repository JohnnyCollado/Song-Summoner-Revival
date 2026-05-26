#!/usr/bin/env python3
"""
Convert an iOS CgBI-encoded PNG to a standard PNG.

CgBI quirks the converter undoes:
  * A CgBI chunk is prepended ahead of IHDR -- strip it.
  * IDAT data uses raw DEFLATE (no zlib wrapper).
  * Per-row filter bytes are present like normal, but pixel data is BGRA
    with premultiplied alpha. Convert to non-premultiplied RGBA.
  * (Some Apple PNGs split IDAT across multiple chunks -- concat them.)

Usage: cgbi_to_png.py <input.png> <output.png>
"""

import struct, zlib, sys

def read_chunks(data):
    sig, data = data[:8], data[8:]
    assert sig == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    out = []
    while data:
        (size,) = struct.unpack('>I', data[:4])
        ctype = data[4:8]
        cdata = data[8 : 8 + size]
        data = data[8 + size + 4 :]  # skip CRC
        out.append((ctype, cdata))
    return out

def write_chunks(chunks):
    out = bytearray(b'\x89PNG\r\n\x1a\n')
    for ctype, cdata in chunks:
        out += struct.pack('>I', len(cdata))
        out += ctype
        out += cdata
        out += struct.pack('>I', zlib.crc32(ctype + cdata) & 0xFFFFFFFF)
    return bytes(out)

def unfilter_and_swap(raw, width, height):
    # raw: decompressed IDAT. Each row = filter_byte + width*4 BGRA bytes.
    stride = width * 4
    out_rgba = bytearray(width * height * 4)
    prev_row = bytearray(stride)
    idx = 0
    for y in range(height):
        ftype = raw[idx]
        idx += 1
        row = bytearray(raw[idx : idx + stride])
        idx += stride
        # PNG filter reverse (None=0, Sub=1, Up=2, Average=3, Paeth=4)
        if ftype == 0:
            pass
        elif ftype == 1:
            for x in range(stride):
                left = row[x - 4] if x >= 4 else 0
                row[x] = (row[x] + left) & 0xFF
        elif ftype == 2:
            for x in range(stride):
                row[x] = (row[x] + prev_row[x]) & 0xFF
        elif ftype == 3:
            for x in range(stride):
                left = row[x - 4] if x >= 4 else 0
                up = prev_row[x]
                row[x] = (row[x] + ((left + up) >> 1)) & 0xFF
        elif ftype == 4:
            for x in range(stride):
                a = row[x - 4] if x >= 4 else 0
                b = prev_row[x]
                c = prev_row[x - 4] if x >= 4 else 0
                p = a + b - c
                pa = abs(p - a)
                pb = abs(p - b)
                pc = abs(p - c)
                if pa <= pb and pa <= pc:
                    pr = a
                elif pb <= pc:
                    pr = b
                else:
                    pr = c
                row[x] = (row[x] + pr) & 0xFF
        else:
            raise ValueError(f'unknown filter byte {ftype}')
        prev_row = row
        # BGRA premultiplied -> RGBA non-premultiplied.
        for x in range(0, stride, 4):
            b, g, r, a = row[x], row[x + 1], row[x + 2], row[x + 3]
            if a != 0 and a != 255:
                r = min(255, (r * 255 + a // 2) // a)
                g = min(255, (g * 255 + a // 2) // a)
                b = min(255, (b * 255 + a // 2) // a)
            o = (y * width + x // 4) * 4
            out_rgba[o + 0] = r
            out_rgba[o + 1] = g
            out_rgba[o + 2] = b
            out_rgba[o + 3] = a
    return bytes(out_rgba)

def refilter_none(rgba, width, height):
    # Re-add per-row filter byte 0 (None).
    stride = width * 4
    out = bytearray()
    for y in range(height):
        out.append(0)
        out += rgba[y * stride : (y + 1) * stride]
    return bytes(out)

def main():
    in_path, out_path = sys.argv[1], sys.argv[2]
    chunks = read_chunks(open(in_path, 'rb').read())
    ihdr = next(c[1] for c in chunks if c[0] == b'IHDR')
    w, h, bit_depth, color_type, comp, filt, inter = struct.unpack('>IIBBBBB', ihdr)
    assert color_type == 6 and bit_depth == 8, 'only RGBA 8bpc supported'
    assert inter == 0, 'interlaced not supported'
    idat = b''.join(c[1] for c in chunks if c[0] == b'IDAT')
    # CgBI uses raw deflate (no zlib header); standard uses zlib wrap.
    try:
        raw = zlib.decompress(idat, -15)
    except zlib.error:
        raw = zlib.decompress(idat)
    rgba = unfilter_and_swap(raw, w, h)
    refilt = refilter_none(rgba, w, h)
    new_idat = zlib.compress(refilt, 9)
    # Reassemble: keep IHDR; drop CgBI/iDOT/iTXt-vendor chunks; new IDAT; IEND.
    out_chunks = []
    for ctype, cdata in chunks:
        if ctype in (b'CgBI', b'iDOT'):
            continue
        if ctype == b'IDAT':
            continue
        if ctype == b'IEND':
            continue
        out_chunks.append((ctype, cdata))
    out_chunks.append((b'IDAT', new_idat))
    out_chunks.append((b'IEND', b''))
    open(out_path, 'wb').write(write_chunks(out_chunks))
    print(f'wrote {out_path} ({w}x{h})')

if __name__ == '__main__':
    main()
