"""Disassemble candidate functions that read iPodState and might be the
'main loop tick' that renders the confirmation panel."""

import struct
from macholib.MachO import MachO
import capstone

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
        return data[off:end].decode("ascii", errors="replace") if end > 0 else None

    # Functions that read iPodState (from earlier dump). The candidates we
    # care about most are the ones that BOTH read state AND call songsQuery
    # downstream (panel rendering).
    candidates = [0x126b8, 0x1339c]

    for fn_start in candidates:
        print(f"\n========== Function @ {fn_start:#x} ==========")
        i = fn_start
        end_limit = fn_start + 4000  # large functions
        instr_count = 0
        while i < end_limit and instr_count < 600:
            off = vm_to_off(i)
            if off is None or off + 2 > len(data):
                break
            instr = data[off] | (data[off + 1] << 8)

            # LDR (literal)
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
                    if sel and len(sel) > 1 and all(0x20 <= ord(c) < 0x7F for c in sel):
                        print(f"  {i:#x}: r{rd} = '{sel}'")

            # T1 LDR immediate from struct field
            if 0x6800 <= instr <= 0x6FFF:
                imm5 = (instr >> 6) & 0x1F
                rn = (instr >> 3) & 0x07
                rt = instr & 0x07
                # Only print if it's likely a state struct field access
                if imm5 in (3, 4, 5, 6, 7, 8):
                    print(f"  {i:#x}: ldr r{rt}, [r{rn}, #{imm5*4:#x}]")

            # T1 STR immediate
            if 0x6000 <= instr <= 0x67FF:
                imm5 = (instr >> 6) & 0x1F
                rn = (instr >> 3) & 0x07
                rt = instr & 0x07
                if imm5 in (3, 4, 5, 6, 7, 8):
                    print(f"  {i:#x}: str r{rt}, [r{rn}, #{imm5*4:#x}]")

            # pop {...,pc}
            if 0xBD00 <= instr <= 0xBDFF:
                print(f"  {i:#x}: pop pc (return)")
                break
            i += 2
            instr_count += 1


if __name__ == "__main__":
    main()
