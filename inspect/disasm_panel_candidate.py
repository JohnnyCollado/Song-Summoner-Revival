"""Disassemble panel-builder candidate functions and dump selectors."""

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

    targets = [0x5fef8, 0x5fd50, 0x64f84, 0x62a84]
    # Approx end addresses (use until next push or 600 bytes).
    for fn_start in targets:
        print(f"\n=== Function @ {fn_start:#x} ===")
        # Scan up to 800 bytes or first pop {...,pc}.
        i = fn_start
        end_limit = fn_start + 1500
        while i < end_limit:
            off = vm_to_off(i)
            if off is None or off + 2 > len(data):
                i += 2
                continue
            instr = data[off] | (data[off + 1] << 8)

            # Detect 32-bit Thumb-2 prefix
            if 0xE800 <= instr <= 0xFFFF and 0xF000 <= instr <= 0xF7FF:
                # 32-bit Thumb instruction
                i += 4
                continue

            # LDR (literal): 01001 ddd iiiiiiii
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
                        print(f"  {i:#x}: r{rd} = sel/str {sel!r}")
                    else:
                        print(f"  {i:#x}: r{rd} = literal -> {ptr:#x}")

            # pop {...,pc}: 1011110 P RegList - low 8 bits is reg list, bit 8 is P.
            # T1 POP: 1011110P r0..r7. 0xBD?? where bit 8=1 means pop pc.
            if 0xBD00 <= instr <= 0xBDFF:
                # POP with PC (returns from function)
                print(f"  {i:#x}: pop {{..., pc}} (return)")
                break
            # 32-bit POP with PC encoding (T2):
            # bytes: E8BD <reglist> where bit 15 of reglist = PC
            if instr == 0xE8BD:
                # Read second halfword
                next_instr = data[off + 2] | (data[off + 3] << 8)
                if next_instr & 0x8000:
                    print(f"  {i:#x}: pop.w (T2) {{..., pc}} (return)")
                    break
                i += 4
                continue
            i += 2


if __name__ == "__main__":
    main()
