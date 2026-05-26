"""Disasm function at 0x62914 (Thumb) that also references SONG string."""

import struct
from macholib.MachO import MachO
import capstone

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

    # Both functions to dump.
    targets = [
        ("alt_SONG_printer", 0x62914, True),
        ("path_after_5c886", 0x5c886, True),  # path B in didSelect
    ]
    md = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)
    for name, addr, thumb in targets:
        off = vmaddr_to_offset(addr & ~1)
        print(f"\n=== {name} @ {addr:#x} (off={off:#x}) ===")
        blob = data[off : off + 500]
        for ins in md.disasm(blob, addr & ~1):
            print(f"  {ins.address:08x}: {ins.bytes.hex():<8} {ins.mnemonic:<8} {ins.op_str}")
            if ins.mnemonic == "bx" and "lr" in ins.op_str:
                break
            if ins.mnemonic == "pop" and "pc" in ins.op_str:
                break


if __name__ == "__main__":
    main()
