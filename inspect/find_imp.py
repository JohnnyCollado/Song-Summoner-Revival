"""Find the IMP (function address) of a given Objective-C selector in
S.S.Encore by locating the selector string and scanning for the method
definition struct that points at it.

iPhone OS 3.x apps use objc 2.0 ABI. Method-list entries in __objc_const
are 12 bytes: {SEL name, char* types, IMP imp}. So if the name field equals
the address of our selector string, the next 4 bytes after the types are
the IMP.

Output: imp addresses for the given selectors.
"""

import struct
import sys
from macholib.MachO import MachO

BIN = "Payload/S.S.Encore.app/S.S.Encore"
TARGETS = ["start_iPodView", "open_iPod", "end_iPodView", "start_MainView"]


def main():
    m = MachO(BIN)
    header = None
    for h in m.headers:
        if h.header.cputype == 12:  # CPU_TYPE_ARM
            header = h
            break
    assert header is not None

    with open(BIN, "rb") as f:
        data = f.read()

    # Locate each target selector string's vmaddr by scanning sections.
    sel_strings = {}
    # iOS 3.x objc 2.0 keeps selectors as C strings in __cstring (or sometimes
    # a dedicated __objc_methname). Try both.
    string_sections = []
    for lc, cmd, sects in header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sn = sect.sectname.rstrip(b"\x00")
            if sn in (b"__objc_methname", b"__cstring"):
                string_sections.append(sect)
    if not string_sections:
        print("no string section found")
        return

    for sect in string_sections:
        start = header.offset + sect.offset
        chunk = data[start : start + sect.size]
        for target in TARGETS:
            if target in sel_strings:
                continue
            tb = target.encode() + b"\x00"
            idx = 0
            while True:
                pos = chunk.find(tb, idx)
                if pos < 0:
                    break
                if pos == 0 or chunk[pos - 1] == 0:
                    sel_strings[target] = sect.addr + pos
                    break
                idx = pos + 1

    print("Selector string vmaddrs:")
    for k, v in sel_strings.items():
        print(f"  {k}: {v:#x}")

    # Now find method-list entries. method_t = {SEL, types, IMP}, 12 bytes.
    # We scan all 4-byte-aligned positions in the binary for a u32 matching
    # the selector vmaddr; the IMP is 8 bytes after that.
    for target, sel_vm in sel_strings.items():
        sel_bytes = struct.pack("<I", sel_vm)
        hits = []
        pos = 0
        while True:
            pos = data.find(sel_bytes, pos)
            if pos < 0:
                break
            # Must be 4-aligned to be plausibly a method_t field.
            if pos % 4 == 0:
                # The IMP is at pos + 8.
                if pos + 12 <= len(data):
                    types_ptr, imp = struct.unpack_from("<II", data, pos + 4)
                    # IMP should be a plausible code address (look in __text range
                    # at file offset 0x1fe4..). VMaddr roughly 0x2fe4..0x62f54+.
                    if 0x2000 < imp < 0x100000:
                        hits.append((pos, types_ptr, imp))
            pos += 4
        print(f"\n{target}: {len(hits)} candidate method_t entries")
        for fp, tp, imp in hits[:10]:
            print(f"  file_off {fp:#x} types_ptr {tp:#x} IMP {imp:#x}")


if __name__ == "__main__":
    main()
