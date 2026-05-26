"""Static RE: find callers of 0x16602 (scene constructor) and writers to
the scene table entry 5 scene_ptr slot at vmaddr 0x113594.

Goals:
1. Find BL callers of 0x16602 — these construct iPod scenes.
2. Find code that writes the constructor's return into entry 5's slot at
   0x113594 — that's the "registration" routine.
3. Find code that nulls or replaces the slot — destructor / scene swap.
4. Find what destroys the old scene before replacement — texture
   cleanup burst we observed.
"""
import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"
SCENE_CTOR = 0x16602
ENTRY5_SCENE_PTR_VM = 0x1134f0 + 5 * 0x1c + 0x18  # = 0x113594


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
        for back in range(0, 6000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    def decode_bl(off, cur_addr):
        if off + 4 > len(text_chunk):
            return None
        hw1 = text_chunk[off] | (text_chunk[off + 1] << 8)
        hw2 = text_chunk[off + 2] | (text_chunk[off + 3] << 8)
        if ((hw1 >> 11) & 0x1F) != 0x1E:
            return None
        if ((hw2 >> 14) & 0x3) != 0x3:
            return None
        s = (hw1 >> 10) & 1
        imm10 = hw1 & 0x3FF
        j1 = (hw2 >> 13) & 1
        bit12 = (hw2 >> 12) & 1
        j2 = (hw2 >> 11) & 1
        imm11 = hw2 & 0x7FF
        i1 = 1 - (j1 ^ s)
        i2 = 1 - (j2 ^ s)
        imm32 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
        if imm32 & (1 << 24):
            imm32 -= 1 << 25
        target = cur_addr + 4 + imm32
        if bit12 == 0:
            target = target & ~3
        return target

    # 1. Find BL callers of 0x16602.
    print(f"=== Direct BL callers of scene constructor 0x{SCENE_CTOR:x} ===")
    targets = {SCENE_CTOR, SCENE_CTOR | 1}
    for off in range(0, len(text_chunk) - 3, 2):
        t = decode_bl(off, text_section.addr + off)
        if t in targets:
            fn = find_func_start(off)
            fn_s = f"{fn:#x}" if fn else "?"
            caller_vm = text_section.addr + off
            print(f"  {caller_vm:#x} (fn={fn_s}) bl {SCENE_CTOR:#x}")

    # 2. Find words in __text or __data containing 0x16603 (Thumb-bit-set
    #    ptr to constructor) — this is what a function-pointer table for
    #    scene constructors would hold.
    print(f"\n=== Words containing constructor address (0x16602 or 0x16603) ===")
    for tgt in (SCENE_CTOR, SCENE_CTOR | 1):
        word = struct.pack("<I", tgt)
        for s in sections:
            sn = s.sectname.rstrip(b"\x00")
            if sn not in (b"__data", b"__const", b"__text"):
                continue
            sect_start = arm_header.offset + s.offset
            sect_chunk = data[sect_start:sect_start + s.size]
            pos = 0
            while True:
                p = sect_chunk.find(word, pos)
                if p < 0:
                    break
                vm = s.addr + p
                # Try to read 16 bytes around for context (potential vtable entry)
                ctx = sect_chunk[max(0, p - 8):p + 16].hex()
                print(f"  {vm:#x} ({sn.decode()}) match for {tgt:#x}, context: {ctx}")
                pos = p + 4

    # 3. Find code that LOADS 0x113594 (the entry5 scene_ptr slot address)
    #    via literal pool, then STR to it. This is the registration code.
    print(f"\n=== Code that loads literal == entry5 slot address ({ENTRY5_SCENE_PTR_VM:#x}) ===")
    word = struct.pack("<I", ENTRY5_SCENE_PTR_VM)
    for off in range(0, len(text_chunk) - 3, 4):
        if text_chunk[off:off + 4] == word:
            vm = text_section.addr + off
            # Find what function this literal pool entry belongs to (the
            # function's BL/LDR instruction pointing at this offset).
            print(f"  literal pool entry at {vm:#x} = {ENTRY5_SCENE_PTR_VM:#x}")

    # 4. Find code that uses the scene table base 0x1134f0 directly.
    print(f"\n=== Code that loads literal == scene table base (0x1134f0) ===")
    base = 0x1134f0
    word = struct.pack("<I", base)
    for off in range(0, len(text_chunk) - 3, 4):
        if text_chunk[off:off + 4] == word:
            vm = text_section.addr + off
            print(f"  literal pool entry at {vm:#x} = {base:#x}")


if __name__ == "__main__":
    main()
