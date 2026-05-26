"""Dump getSong: selectors at IMP 0x5bc78 (Thumb)."""

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
        for s in sects:
            sections.append(s)

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

    i = 0x5bc78
    end = 0x5bcf8
    while i < end:
        off = vm_to_off(i)
        if off is None or off + 2 > len(data):
            i += 2
            continue
        instr = data[off] | (data[off + 1] << 8)
        if 0x4800 <= instr <= 0x4FFF:
            rd = (instr >> 8) & 0x07
            imm = instr & 0xFF
            pc = (i + 4) & ~3
            lit_vm = pc + imm * 4
            lit_off = vm_to_off(lit_vm)
            if lit_off:
                ptr = struct.unpack_from("<I", data, lit_off)[0]
                selref_off = vm_to_off(ptr)
                sel = None
                if selref_off:
                    cstr_addr = struct.unpack_from("<I", data, selref_off)[0]
                    sel = read_cstr(cstr_addr)
                if not sel:
                    sel = read_cstr(ptr)
                tag = f"sel={sel!r}" if sel else f"obj/class @ {ptr:#x}"
                print(f"  {i:#x}: ldr r{rd}, [pc, #{imm*4:#x}] -> {tag}")
        i += 2


if __name__ == "__main__":
    main()
