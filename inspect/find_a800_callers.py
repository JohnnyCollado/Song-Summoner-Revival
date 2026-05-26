"""Find all BL/BLX callers of 0xa800 (the suspected cleanup function)."""
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
            if o < 0: break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5: return text_section.addr + o
        return None

    def decode_bl(off, cur_addr):
        if off + 4 > len(text_chunk): return None
        hw1 = text_chunk[off] | (text_chunk[off + 1] << 8)
        hw2 = text_chunk[off + 2] | (text_chunk[off + 3] << 8)
        if ((hw1 >> 11) & 0x1F) != 0x1E: return None
        if ((hw2 >> 14) & 0x3) != 0x3: return None
        s = (hw1 >> 10) & 1
        imm10 = hw1 & 0x3FF
        j1 = (hw2 >> 13) & 1
        bit12 = (hw2 >> 12) & 1
        j2 = (hw2 >> 11) & 1
        imm11 = hw2 & 0x7FF
        i1 = 1 - (j1 ^ s); i2 = 1 - (j2 ^ s)
        imm32 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
        if imm32 & (1 << 24): imm32 -= 1 << 25
        target = cur_addr + 4 + imm32
        if bit12 == 0: target = target & ~3
        return target

    targets = {0xa800, 0xa801, 0xa788, 0xa789, 0xc57c, 0xc57d}
    names = {0xa800: "a800", 0xa801: "a800", 0xa788: "a788",
             0xa789: "a788", 0xc57c: "c57c", 0xc57d: "c57c"}
    for off in range(0, len(text_chunk) - 3, 2):
        t = decode_bl(off, text_section.addr + off)
        if t and t in targets:
            fn = find_func_start(off)
            caller_vm = text_section.addr + off
            # Look back for movs r0, #imm setting the arg
            arg = None
            for back in range(2, 14, 2):
                if off - back < 0: break
                prev = text_chunk[off - back] | (text_chunk[off - back + 1] << 8)
                if 0x2000 <= prev <= 0x20FF:
                    arg = prev & 0xFF; break
            arg_s = f"arg={arg}" if arg is not None else "arg=?"
            fn_s = f"{fn:#x}" if fn else "?"
            print(f"  {caller_vm:#x} (fn={fn_s}) bl {names[t]} ; {arg_s}")


if __name__ == "__main__":
    main()
