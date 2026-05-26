"""Disassemble a specific function at a known vmaddr."""

import struct
import sys

from macholib.MachO import MachO
import capstone

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"
TARGETS = [
    ("IPDSongsTableVC.didSelectRow", 0x5c761, True),
    ("iPodView2.mediaPicker:didPickMediaNumber:", 0x60171, True),
    ("MainLoop_Set_iPodState", 0x322d, True),
    ("MainLoop_Get_iPodState", 0x3239, True),
    ("MainLoop_Set_iPodMusicID", 0x325d, True),
]
SIZE = 600  # bytes


def main() -> None:
    m = MachO(BIN)
    arm_header = None
    for h in m.headers:
        if h.header.cputype == 12:
            arm_header = h
            break
    assert arm_header is not None

    with open(BIN, "rb") as f:
        data = f.read()

    sections = []
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sn = sect.sectname.rstrip(b"\x00")
            sg = sect.segname.rstrip(b"\x00")
            sections.append((sn, sg, sect))

    def vmaddr_to_offset(vmaddr):
        for _, _, s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    md_thumb = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)
    md_arm = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_ARM)

    for name, addr, thumb in TARGETS:
        addr_clean = addr & ~1
        off = vmaddr_to_offset(addr_clean)
        print(f"\n=== {name} @ {addr:#x} (off={off:#x}) ===")
        if off is None:
            continue
        blob = data[off : off + SIZE]
        md = md_thumb if thumb else md_arm
        for ins in md.disasm(blob, addr_clean):
            line = f"  {ins.address:08x}: {ins.bytes.hex():<8} {ins.mnemonic:<8} {ins.op_str}"
            print(line)
            if ins.mnemonic == "bx" and "lr" in ins.op_str:
                break
            if ins.mnemonic == "pop" and "pc" in ins.op_str:
                break


if __name__ == "__main__":
    main()
