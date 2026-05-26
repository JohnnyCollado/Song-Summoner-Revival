"""Find code patterns: load iPodState then cmp with 4 (or specific values).

Thumb T1 LDR (struct field): 01101 imm5 Rn Rt → opcode 0x68..0x6F + imm.
For +0xc (iPodState), imm5=3 so high byte = 0x68 | ((3 >> 3) | (Rn << 3))...
Actually let me just decode 16-bit and check.

For CMP immediate T1: 00101 Rn imm8 → 0x28..0x2F.
For cmp r0, #4: 0x2804.
For cmp r1, #4: 0x2904.
For cmp r2, #4: 0x2A04.
For cmp r3, #4: 0x2B04.

Pattern:
- LDR Rt, [Rn, #0xc]    where imm5=3 → 0x6800..0x68FF style
- followed shortly by CMP Rt, #4

Search for these pairs.
"""

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

    # Find all LDR Rt, [Rn, #0xc] (imm5=3).
    # Encoding: 01101 imm5(5) Rn(3) Rt(3) → 0110 1iii iinn nttt
    # For imm5=3 (binary 00011), the 5-bit field is bits 6-10.
    # 0x6800 | (3 << 6) | (Rn << 3) | Rt = 0x68C0 | (Rn << 3) | Rt
    # All Rn (0-7) and Rt (0-7) combinations.
    print("=== LDR Rt, [Rn, +0xc] followed by CMP Rt, #N (any value) ===")
    for off in range(0, len(text_chunk) - 6, 2):
        instr = text_chunk[off] | (text_chunk[off + 1] << 8)
        # LDR Rt, [Rn, +0xc]: imm5=3, encoding 0x68C0..0x68FF
        if 0x68C0 <= instr <= 0x68FF:
            rn = (instr >> 3) & 0x07
            rt = instr & 0x07
            # Look ahead up to 10 bytes for CMP Rt, #N
            cmp_pattern_base = 0x2800 | (rt << 8)  # CMP Rt, #imm8
            for la in range(2, 12, 2):
                if off + la + 1 >= len(text_chunk):
                    break
                ni = text_chunk[off + la] | (text_chunk[off + la + 1] << 8)
                if (ni & 0xFF00) == cmp_pattern_base:
                    imm8 = ni & 0xFF
                    vm = text_section.addr + off
                    fn = find_func_start(off)
                    fn_str = f"{fn:#x}" if fn else "?"
                    print(f"  {vm:#x} (fn={fn_str}): ldr r{rt}, [r{rn}, +0xc] ; cmp r{rt}, #{imm8}")
                    break


if __name__ == "__main__":
    main()
