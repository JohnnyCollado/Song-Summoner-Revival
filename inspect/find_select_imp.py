"""Find the function that prints 'select!!!!' and 'SONG :' strings."""

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
    text_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sn = sect.sectname.rstrip(b"\x00")
            sections.append((sn, sect))
            if sn == b"__text":
                text_section = sect

    def vmaddr_to_offset(vmaddr):
        for _, s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    # Locate string addresses.
    targets = [b"select!!!!", b"SONG : %s , PID : %llX", b"SONG : %s , PID : %qX"]
    string_addrs = {}
    for _, s in sections:
        if s.sectname.rstrip(b"\x00") not in (b"__cstring", b"__cfstring"):
            continue
        start = arm_header.offset + s.offset
        chunk = data[start : start + s.size]
        for t in targets:
            p = chunk.find(t)
            if p >= 0:
                string_addrs[t] = s.addr + p
    print("String addrs found:")
    for k, v in string_addrs.items():
        print(f"  {k!r}: {v:#x}")

    # Search __text for instructions that load these string addresses (e.g.
    # via PC-relative literal pools).
    if text_section is None:
        return
    text_start = arm_header.offset + text_section.offset
    text_end = text_start + text_section.size
    chunk = data[text_start:text_end]

    md_thumb = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)
    md_thumb.detail = True

    # Find every 4-byte word in text that matches a target string address.
    for label, addr in string_addrs.items():
        word = struct.pack("<I", addr)
        pos = 0
        hits = []
        while True:
            p = chunk.find(word, pos)
            if p < 0:
                break
            vm = text_section.addr + p
            hits.append(vm)
            pos = p + 4
        print(f"\nReferences to {label!r} ({addr:#x}):")
        for h in hits[:5]:
            print(f"  literal at vmaddr {h:#x}")
            # Walk back ~200 bytes to find a function start (push {...}).
            start_search = max(0, p - 400)
            # Find prev push instruction
            for back in range(0, 400, 2):
                off = (h - text_section.addr) - back
                if off < 0:
                    break
                ins_bytes = chunk[off : off + 2]
                # Thumb push: 0xB5 high byte
                if len(ins_bytes) == 2 and ins_bytes[1] in (0xB5, 0xB4):
                    func_start = text_section.addr + off
                    print(f"    likely function start: {func_start:#x} (thumb)")
                    break


if __name__ == "__main__":
    main()
