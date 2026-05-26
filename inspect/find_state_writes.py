"""Find all STR Rt, [Rn, +0xc] (writes to iPodState) and decode the
constant being stored (if immediate move before)."""

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

    # STR Rt, [Rn, +0xc]: imm5=3 → encoding 0x60C0..0x60FF (T1 STR)
    print("=== STR Rt, [Rn, +0xc] (writes to iPodState) ===")
    for off in range(0, len(text_chunk) - 4, 2):
        instr = text_chunk[off] | (text_chunk[off + 1] << 8)
        if 0x60C0 <= instr <= 0x60FF:
            rn = (instr >> 3) & 0x07
            rt = instr & 0x07
            vm = text_section.addr + off
            fn = find_func_start(off)
            fn_str = f"{fn:#x}" if fn else "?"
            # Look back up to 12 bytes for MOVS Rt, #imm (0x20..0x27 followed by imm8)
            # Or MOV register-only.
            stored_const = None
            for back in range(2, 14, 2):
                if off - back < 0:
                    break
                prev = text_chunk[off - back] | (text_chunk[off - back + 1] << 8)
                # MOVS Rt, #imm: 00100 Rd imm8 → 0x2000..0x27FF, Rd = (prev>>8)&7
                if (prev & 0xF800) == 0x2000:
                    rd = (prev >> 8) & 0x07
                    imm = prev & 0xFF
                    if rd == rt:
                        stored_const = imm
                        break
            const_str = f"const={stored_const}" if stored_const is not None else "const=?"
            print(f"  {vm:#x} (fn={fn_str}): str r{rt}, [r{rn}, +0xc] ; {const_str}")


if __name__ == "__main__":
    main()
