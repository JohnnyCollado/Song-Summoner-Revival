"""Find all reads of the MainLoop state struct.

Strategy:
1. Read the global pointer (from 0x3234) to find the state struct address.
2. Find all instructions that load/store from MainLoop+0xc (iPodState),
   MainLoop+0x14/0x18 (iPodMusicID), MainLoop+0x20 (tabIdx).
3. For each access, identify the enclosing function (find prev push).
4. Report.
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

    def vm_to_off(vmaddr):
        for s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    # Read the state struct base ptr.
    # MainLoop_Set_iPodState at 0x322c: ldr r3, [pc, #4] -> PC=0x3230&~3=0x3230, +4 = 0x3234.
    # Wait, PC for Thumb LDR T1 is (PC+4)&~3 where PC is instr addr.
    # instr at 0x322c, PC = 0x322c+4 = 0x3230. & ~3 = 0x3230. + (imm=4 → imm*4=... no wait,
    # ldr r3, [pc, #4] means immediate 4, NOT shifted. Wait actually imm is *4 in the
    # encoding but the displayed offset already accounts for that. Let me just trust it.
    # Per the disasm we have: 0x322c: ldr r3, [pc, #4]. So the operand 4 IS the byte offset.
    # PC = (instr_addr + 4) & ~3 = (0x322c+4)&~3 = 0x3230. + 4 = 0x3234.
    state_global_off = vm_to_off(0x3234)
    state_global_word = struct.unpack_from("<I", data, state_global_off)[0]
    print(f"State global ptr at {state_global_word:#x}")
    # That's the ADDRESS of a global variable that holds the state struct.
    # The struct itself is elsewhere — at *(state_global_word).
    state_global2_off = vm_to_off(state_global_word)
    if state_global2_off:
        state_struct_addr = struct.unpack_from("<I", data, state_global2_off)[0]
        print(f"State struct address: {state_struct_addr:#x}")

    # Now search __text for Thumb LDR/STR instructions that access [Rx, #imm]
    # where imm is one of our state offsets (0xc, 0x14, 0x18, 0x20).
    # Thumb T1 LDR: 01101 imm5 Rn Rt -> 0x6800..0x6FFF. imm5 is bits 6-10, scaled by 4.
    # Thumb T1 STR: 01100 imm5 Rn Rt -> 0x6000..0x67FF.
    # imm offset = imm5 * 4.
    # We want imm5 in {3, 5, 6, 8} → byte offsets 0xc, 0x14, 0x18, 0x20.
    text_start = arm_header.offset + text_section.offset
    text_end = text_start + text_section.size

    offsets_of_interest = {3: "+0xc (iPodState?)", 5: "+0x14 (iPodMusicID-lo?)",
                            6: "+0x18 (iPodMusicID-hi?)", 8: "+0x20 (tabIdx?)"}
    # Also include +0x10, +0x1c, +0x24, +0x28, +0x2c just in case.
    extras = {4: "+0x10", 7: "+0x1c", 9: "+0x24", 10: "+0x28", 11: "+0x2c"}
    offsets_of_interest.update(extras)

    def find_func_start(text_off):
        # Walk backwards looking for push {.,..,lr} (Thumb): 0xB5XX where the
        # second byte (high) is B5/B4.
        for back in range(0, 4000, 2):
            o = text_off - back
            if o < text_start:
                break
            b0 = data[o]
            b1 = data[o + 1]
            if b1 == 0xB5:
                return text_section.addr + (o - text_start)
        return None

    print("\n=== LDR Rt, [Rn, #imm] accesses (load from state struct) ===")
    for off in range(text_start, text_end - 1, 2):
        instr = data[off] | (data[off + 1] << 8)
        # T1 LDR(immediate): 01101 imm5 Rn Rt → 0x6800..0x6FFF
        if 0x6800 <= instr <= 0x6FFF:
            imm5 = (instr >> 6) & 0x1F
            rn = (instr >> 3) & 0x07
            rt = instr & 0x07
            if imm5 in offsets_of_interest:
                vm = text_section.addr + (off - text_start)
                fn = find_func_start(off)
                fn_str = f"{fn:#x}" if fn else "?"
                print(f"  {vm:#x} (fn={fn_str}): ldr r{rt}, [r{rn}, {offsets_of_interest[imm5]}]")
    print("\n=== STR Rt, [Rn, #imm] (store to state struct) ===")
    for off in range(text_start, text_end - 1, 2):
        instr = data[off] | (data[off + 1] << 8)
        # T1 STR(immediate): 01100 imm5 Rn Rt → 0x6000..0x67FF
        if 0x6000 <= instr <= 0x67FF:
            imm5 = (instr >> 6) & 0x1F
            rn = (instr >> 3) & 0x07
            rt = instr & 0x07
            if imm5 in offsets_of_interest:
                vm = text_section.addr + (off - text_start)
                fn = find_func_start(off)
                fn_str = f"{fn:#x}" if fn else "?"
                print(f"  {vm:#x} (fn={fn_str}): str r{rt}, [r{rn}, {offsets_of_interest[imm5]}]")


if __name__ == "__main__":
    main()
