"""Find all callers of MainLoop_Set_iPodState (0x322c) and decode the arg value."""

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
        for back in range(0, 6000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    def decode_bl_target(off, cur_addr):
        if off + 4 > len(text_chunk):
            return None
        hw1 = text_chunk[off] | (text_chunk[off + 1] << 8)
        hw2 = text_chunk[off + 2] | (text_chunk[off + 3] << 8)
        # BL/BLX T1: hw1 bits 15:11 = 11110, hw2 bits 15:14 = 11.
        if ((hw1 >> 11) & 0x1F) != 0x1E:
            return None
        if ((hw2 >> 14) & 0x3) != 0x3:
            return None
        s = (hw1 >> 10) & 1
        imm10 = hw1 & 0x3FF
        j1 = (hw2 >> 13) & 1
        bit12 = (hw2 >> 12) & 1   # 1 = BL, 0 = BLX
        j2 = (hw2 >> 11) & 1
        imm11 = hw2 & 0x7FF
        i1 = 1 - (j1 ^ s)
        i2 = 1 - (j2 ^ s)
        imm32 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
        if imm32 & (1 << 24):
            imm32 -= 1 << 25
        is_blx = bit12 == 0
        target = cur_addr + 4 + imm32
        if is_blx:
            target = target & ~3
        return target

    targets = {0x16602, 0x16603, 0xc9f4, 0xc9f5, 0x17800, 0x17801}
    target_names = {0x16602: "scene_ctor_state9", 0x16603: "scene_ctor_state9",
                    0xc9f4: "get_scene_obj", 0xc9f5: "get_scene_obj",
                    0x17800: "scene_renderer", 0x17801: "scene_renderer"}
    print("=== Callers of Set_iPodState / Set_iPodCancel ===")
    for off in range(0, len(text_chunk) - 3, 2):
        tgt = decode_bl_target(off, text_section.addr + off)
        if tgt is None:
            continue
        if tgt in targets:
            name = target_names[tgt]
            caller_vm = text_section.addr + off
            fn = find_func_start(off)
            fn_str = f"{fn:#x}" if fn else "?"
            # Look back for MOVS r0, #imm setting the arg
            arg = None
            for back in range(2, 14, 2):
                if off - back < 0:
                    break
                prev = text_chunk[off - back] | (text_chunk[off - back + 1] << 8)
                # MOVS r0, #imm: 00100 000 imm8 → 0x2000..0x20FF
                if 0x2000 <= prev <= 0x20FF:
                    arg = prev & 0xFF
                    break
            arg_str = f"arg={arg}" if arg is not None else "arg=?"
            print(f"  {caller_vm:#x} (fn={fn_str}): bl {name} ; {arg_str}")


if __name__ == "__main__":
    main()
