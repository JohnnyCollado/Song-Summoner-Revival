"""Find the function that uses MPMediaQuery +songsQuery to build the
confirmation panel. This is what fires on pick 1 but not pick 2+.

Steps:
1. Find selref to 'songsQuery'.
2. Find LDR instructions that load that selref address.
3. For each LDR, identify the containing function.
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
    selrefs_section = None
    methname_section = None
    cstring_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for s in sects:
            sections.append(s)
            sn = s.sectname.rstrip(b"\x00")
            if sn == b"__text":
                text_section = s
            if sn == b"__objc_selrefs":
                selrefs_section = s
            if sn == b"__objc_methname":
                methname_section = s
            if sn == b"__cstring":
                cstring_section = s

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

    # 1. Find 'songsQuery' c-string in __objc_methname or __cstring.
    target = b"songsQuery"
    found_strs = []
    for s in [methname_section, cstring_section]:
        if s is None:
            continue
        start = arm_header.offset + s.offset
        chunk = data[start:start + s.size]
        pos = 0
        while True:
            p = chunk.find(target + b"\x00", pos)
            if p < 0:
                break
            if p == 0 or chunk[p - 1] == 0:
                found_strs.append(s.addr + p)
            pos = p + 1
    print(f"songsQuery c-string addrs: {[hex(a) for a in found_strs]}")

    # 2. Find selrefs (4-byte words in __objc_selrefs that point to one of these).
    selref_addrs = []
    if selrefs_section:
        sr_start = arm_header.offset + selrefs_section.offset
        sr_chunk = data[sr_start:sr_start + selrefs_section.size]
        for i in range(0, len(sr_chunk), 4):
            word = struct.unpack_from("<I", sr_chunk, i)[0]
            if word in found_strs:
                selref_addrs.append(selrefs_section.addr + i)
    print(f"songsQuery selref addrs: {[hex(a) for a in selref_addrs]}")

    # 3. Find all 4-byte words in __text that match any selref address.
    text_start = arm_header.offset + text_section.offset
    text_chunk = data[text_start:text_start + text_section.size]

    def find_func_start(text_off):
        for back in range(0, 4000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    print("\n=== Text references to songsQuery selrefs ===")
    for sa in selref_addrs:
        word = struct.pack("<I", sa)
        pos = 0
        while True:
            p = text_chunk.find(word, pos)
            if p < 0:
                break
            vm = text_section.addr + p
            fn = find_func_start(p)
            fn_str = f"{fn:#x}" if fn else "?"
            print(f"  literal at {vm:#x} (in fn {fn_str}) -> selref {sa:#x}")
            pos = p + 4


if __name__ == "__main__":
    main()
