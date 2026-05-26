"""Full disasm of the two iPodState==4 candidate functions."""

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
        return data[off:end].decode("ascii", errors="replace") if end > 0 else None

    for fn_start in [0xa258]:
        print(f"\n========== Function @ {fn_start:#x} (longer dump) ==========")
        i = fn_start
        end_limit = fn_start + 8000
        # Track distinct selectors only.
        seen = []
        printed_state_check = False
        while i < end_limit:
            off = vm_to_off(i)
            if off is None or off + 2 > len(data):
                break
            instr = data[off] | (data[off + 1] << 8)
            instr32 = None
            if 0xF000 <= instr <= 0xF7FF and off + 4 <= len(data):
                instr32 = instr | (data[off + 2] << 16) | (data[off + 3] << 24)

            # LDR (literal): try to resolve selector or string
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
                    if sel and len(sel) > 1 and len(sel) < 80 and all(0x20 <= ord(c) < 0x7F for c in sel):
                        # Skip the assert framework strings to reduce noise.
                        if not sel.startswith("/Users/") and sel not in ("currentHandler", "handleFailureInMethod:object:file:lineNumber:description:", "stringWithUTF8String:"):
                            seen.append((i, sel))

            # CMP imm
            if 0x2800 <= instr <= 0x2FFF:
                rn = (instr >> 8) & 0x07
                imm8 = instr & 0xFF
                if rn != 0 or imm8 in (1, 2, 3, 4, 5, 9, 15, 30):
                    seen.append((i, f"cmp r{rn}, #{imm8}"))

            # T1 LDR struct field
            if 0x6800 <= instr <= 0x6FFF:
                imm5 = (instr >> 6) & 0x1F
                rn = (instr >> 3) & 0x07
                rt = instr & 0x07
                if imm5 == 3:  # +0xc = iPodState
                    seen.append((i, f"ldr r{rt}, [r{rn}, iPodState]"))

            # POP {pc}
            if 0xBD00 <= instr <= 0xBDFF:
                seen.append((i, "pop pc (RETURN)"))
                break
            i += 2 if instr32 is None else 4

        for ipt, info in seen:
            print(f"  {ipt:#x}: {info}")


if __name__ == "__main__":
    main()
