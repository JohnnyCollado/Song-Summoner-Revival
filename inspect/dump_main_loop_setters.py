"""Dump the disasm of MainLoop_Set_* functions to identify all state struct offsets."""

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

    md = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)

    # Iterate every ~12-byte block from 0x3200 to 0x3400 to find the setter/getter pairs.
    print("=== Functions in 0x3200-0x3400 region (MainLoop accessor table) ===")
    for addr in range(0x3200, 0x3400, 4):
        off = vm_to_off(addr)
        if off is None:
            continue
        blob = data[off:off + 32]
        # Detect Thumb function start: ldr r3, [pc, #4]; ...; bx lr
        # First instruction's bytes:
        if len(blob) >= 4 and blob[1] == 0x4B:
            # ldr r3, [pc, #imm]
            # Decode 8 instructions max.
            print(f"\n@ {addr:#x}:")
            for ins in md.disasm(blob, addr):
                print(f"  {ins.address:#x}: {ins.bytes.hex():<8} {ins.mnemonic:<6} {ins.op_str}")
                if ins.mnemonic == "bx" and "lr" in ins.op_str:
                    break


if __name__ == "__main__":
    main()
