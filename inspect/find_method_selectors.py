"""For given IMP addresses, find the selectors that point to them via method_t entries."""

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

    # IMP addresses to find (with thumb bit set)
    targets = {0x5fd51, 0x5fef9, 0x60171, 0x62a85, 0x64f85,
               # Also check without thumb bit
               0x5fd50, 0x5fef8, 0x60170, 0x62a84, 0x64f84}

    # method_t entries are 12 bytes: SEL_ptr, types_ptr, IMP. Iterate all of data
    # looking for IMP word matches.
    print("=== method_t entries pointing to target IMPs ===")
    # Find all 4-byte words matching any target.
    for tgt in sorted(targets):
        word = struct.pack("<I", tgt)
        pos = 0
        while True:
            p = data.find(word, pos)
            if p < 0:
                break
            # Check if this is at offset+8 of a method_t (so name ptr at p-8).
            if p >= 8 and p % 4 == 0:
                name_ptr = struct.unpack_from("<I", data, p - 8)[0]
                types_ptr = struct.unpack_from("<I", data, p - 4)[0]
                sel = read_cstr(name_ptr)
                types = read_cstr(types_ptr)
                if sel and len(sel) > 1 and all(0x20 <= ord(c) < 0x7F for c in sel):
                    print(f"  IMP {tgt:#x}: SEL='{sel}' types='{types}'")
            pos = p + 4


if __name__ == "__main__":
    main()
