"""Find direct callers (bl / blx imm) of given functions."""

import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"


def main():
    m = MachO(BIN)
    arm_header = next(h for h in m.headers if h.header.cputype == 12)
    with open(BIN, "rb") as f:
        data = f.read()
    sections = []
    text_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for s in sects:
            sections.append(s)
            if s.sectname.rstrip(b"\x00") == b"__text":
                text_section = s

    text_start = arm_header.offset + text_section.offset
    text_chunk = data[text_start:text_start + text_section.size]

    def find_func_start(text_off):
        for back in range(0, 4000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    # Thumb BL/BLX is 32-bit encoded with imm10 + imm11 + S/J1/J2.
    # Bytes pattern: hw1 = F0..F7 then 0?, hw2 = D0..DF (BL) or C0..CF (BLX).
    # Decode each pair.
    def decode_bl_target(off, cur_addr):
        hw1 = text_chunk[off] | (text_chunk[off + 1] << 8)
        hw2 = text_chunk[off + 2] | (text_chunk[off + 3] << 8)
        if not (0xF000 <= hw1 <= 0xF7FF):
            return None
        if not (0xD000 <= hw2 <= 0xDFFF or 0xC000 <= hw2 <= 0xCFFF):
            return None
        s = (hw1 >> 10) & 1
        imm10 = hw1 & 0x3FF
        j1 = (hw2 >> 13) & 1
        j2 = (hw2 >> 11) & 1
        imm11 = hw2 & 0x7FF
        i1 = 1 - (j1 ^ s)
        i2 = 1 - (j2 ^ s)
        imm32 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
        # Sign-extend from 25 bits
        if imm32 & (1 << 24):
            imm32 -= 1 << 25
        is_blx = 0xC000 <= hw2 <= 0xCFFF
        target = (cur_addr + 4 + imm32)
        if is_blx:
            # BLX target is 4-byte aligned
            target = target & ~3
        return target

    targets = {0x5fd50, 0x5fef8, 0x60170}  # didPickMediaNumber at 0x60170 (0x60171 with thumb bit)
    print(f"Looking for callers of {[hex(t) for t in targets]}")
    for off in range(0, len(text_chunk) - 3, 2):
        tgt = decode_bl_target(off, text_section.addr + off)
        if tgt is None:
            continue
        if tgt in targets or (tgt & ~1) in targets:
            caller_vm = text_section.addr + off
            fn = find_func_start(off)
            fn_str = f"{fn:#x}" if fn else "?"
            print(f"  {caller_vm:#x} (in fn {fn_str}): bl {tgt:#x}")


if __name__ == "__main__":
    main()
