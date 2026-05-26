"""Decode the literal pool of IPDSongsTableVC.didSelectRow at 0x5c761 and
print the selectors used in path A (0x5c7a8-0x5c884) vs path B (0x5c886-0x5c8fc).

For each `ldr Rd, [pc, #imm]` we resolve (PC&~3) + imm to get a literal pool
entry, then read the 4-byte word there. If that word points into __cstring or
__objc_methname, we treat it as a selector pointer.
"""

import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"


def main():
    m = MachO(BIN)
    arm_header = None
    for h in m.headers:
        if h.header.cputype == 12:
            arm_header = h
            break
    with open(BIN, "rb") as f:
        data = f.read()

    sections = []
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sections.append((sect.sectname.rstrip(b"\x00"), sect))

    def vmaddr_to_offset(vmaddr):
        for _, s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    def read_cstring(addr):
        off = vmaddr_to_offset(addr)
        if off is None:
            return None
        end = data.find(b"\x00", off, off + 200)
        return data[off:end].decode("ascii", errors="replace") if end > 0 else None

    # Selector refs typically live in __objc_selrefs and point to __cstring.
    # We try to read 4 bytes at the resolved literal address and check if it's
    # a valid c-string.
    def resolve_selector_at(literal_vm):
        off = vmaddr_to_offset(literal_vm)
        if off is None or off + 4 > len(data):
            return None
        ptr = struct.unpack_from("<I", data, off)[0]
        s = read_cstring(ptr)
        if s and len(s) > 1 and all(32 <= ord(c) < 127 for c in s):
            return s
        return None

    # Walk Thumb instructions from start through path A and B.
    # We mark each 2-byte ldr [pc, #imm] (encoding: 01001ddd iiiiiiii where high
    # byte is in second nibble). Format: ldr Rd, [pc, #imm*4] -> bytes are:
    #   low byte: 0x48..0x4F (T1 LDR)
    #   actually low/high reversed due to little endian: bytes (hi, lo) =>
    #   instr is hi<<8 | lo. For LDR T1: pattern 01001 ddd iiiiiiii so high
    #   byte = 0x48..0x4F.

    def walk(start, end, label):
        print(f"\n=== {label} {start:#x} – {end:#x} ===")
        # Single Thumb pass.
        i = start
        while i < end:
            off = vmaddr_to_offset(i)
            if off is None:
                i += 2
                continue
            b0 = data[off]
            b1 = data[off + 1]
            instr = (b1 << 8) | b0
            # LDR T1: 01001 Rd imm8  (high 5 bits of high byte = 01001)
            if 0x48 <= b1 <= 0x4F:
                rd = b1 & 0x07
                imm8 = b0
                # PC = i + 4, align to 4
                pc = (i + 4) & ~3
                literal_vm = pc + imm8 * 4
                sel = resolve_selector_at(literal_vm)
                if sel:
                    print(f"  {i:#x}: ldr r{rd}, [pc, #{imm8*4:#x}] -> {literal_vm:#x} -> sel='{sel}'")
            i += 2

    walk(0x5c7a8, 0x5c886, "Path A (full build)")
    walk(0x5c886, 0x5c8fe, "Path B (early exit)")


if __name__ == "__main__":
    main()
