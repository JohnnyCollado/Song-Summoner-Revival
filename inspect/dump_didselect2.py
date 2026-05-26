"""Resolve selector strings from the literal pool of didSelectRow.
Walks LDR Rd, [pc, #imm] instructions, follows to the pool word,
then to the selector string in __cstring."""

import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"


def main():
    m = MachO(BIN)
    arm_header = next(h for h in m.headers if h.header.cputype == 12)
    with open(BIN, "rb") as f:
        data = f.read()

    sections = []
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sections.append(sect)
    text = next(s for s in sections if s.sectname.rstrip(b"\x00") == b"__text")
    text_off = arm_header.offset + text.offset

    def vm_to_off(vmaddr):
        for s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    def read_cstr(addr):
        off = vm_to_off(addr)
        if off is None:
            return None
        end = data.find(b"\x00", off, off + 200)
        if end < 0:
            return None
        try:
            return data[off:end].decode("ascii")
        except UnicodeDecodeError:
            return None

    def walk(start, end, name):
        print(f"\n=== {name} {start:#x} - {end:#x} ===")
        i = start
        while i < end:
            off = vm_to_off(i)
            if off is None or off + 2 > len(data):
                i += 2
                continue
            instr = data[off] | (data[off + 1] << 8)
            # Thumb LDR (literal): 01001 ddd iiiiiiii  => 0x4800..0x4FFF
            if 0xF000 <= instr <= 0xF7FF:
                # Possibly 32-bit Thumb instruction; skip 4 bytes
                i += 4
                continue
            if 0x4800 <= instr <= 0x4FFF:
                rd = (instr >> 8) & 0x07
                imm = instr & 0xFF
                pc = (i + 4) & ~3
                literal_vm = pc + imm * 4
                literal_off = vm_to_off(literal_vm)
                if literal_off is not None and literal_off + 4 <= len(data):
                    selref_addr = struct.unpack_from("<I", data, literal_off)[0]
                    # Selref -> deref once more to get the c-string pointer.
                    selref_off = vm_to_off(selref_addr)
                    sel = None
                    if selref_off is not None and selref_off + 4 <= len(data):
                        cstr_addr = struct.unpack_from("<I", data, selref_off)[0]
                        sel = read_cstr(cstr_addr)
                    # Also try treating the first ptr as a direct c-string.
                    if not sel:
                        sel = read_cstr(selref_addr)
                    if sel and len(sel) > 1 and all(0x20 <= ord(c) < 0x7F for c in sel):
                        print(f"  {i:#x}: '{sel}'")
                    else:
                        print(f"  {i:#x}: ldr r{rd}, [pc, #{imm*4:#x}] -> {selref_addr:#x} (no string)")
            i += 2

    walk(0x5c760, 0x5c7a8, "Prologue (up to first branch)")
    walk(0x5c7a8, 0x5c886, "Path A (full build)")
    walk(0x5c886, 0x5c8fe, "Path B (short path)")


if __name__ == "__main__":
    main()
