"""Find callers of playlistSongs: and albumSongs: selectors."""

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
    string_sections = []  # __objc_methname AND __cstring
    selrefs_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for s in sects:
            sections.append(s)
            sn = s.sectname.rstrip(b"\x00")
            if sn == b"__text":
                text_section = s
            if sn in (b"__objc_methname", b"__cstring"):
                string_sections.append(s)
            if sn == b"__objc_selrefs":
                selrefs_section = s

    def vm_to_off(vmaddr):
        for s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    text_start = arm_header.offset + text_section.offset
    text_chunk = data[text_start:text_start + text_section.size]

    # Find selref entries for our selectors.
    sel_strings = [b"playlistSongs:", b"albumSongs:",
                    b"artistSongs:", b"isLoadedCollections:",
                    b"getSong:", b"items"]
    sel_cstring_addrs = {}
    for ss in string_sections:
        mn_start = arm_header.offset + ss.offset
        mn_chunk = data[mn_start:mn_start + ss.size]
        for sel_name in sel_strings:
            if sel_name in sel_cstring_addrs:
                continue
            pos = 0
            while True:
                p = mn_chunk.find(sel_name + b"\x00", pos)
                if p < 0:
                    break
                if p == 0 or mn_chunk[p - 1] == 0:
                    sel_cstring_addrs[sel_name] = ss.addr + p
                    break
                pos = p + 1
    for k, v in sel_cstring_addrs.items():
        print(f"{k}: c-string at {v:#x}")

    sr_start = arm_header.offset + selrefs_section.offset
    sr_chunk = data[sr_start:sr_start + selrefs_section.size]
    selref_addrs = {}  # sel_name -> selref vmaddr
    for i in range(0, len(sr_chunk), 4):
        word = struct.unpack_from("<I", sr_chunk, i)[0]
        for name, cs_addr in sel_cstring_addrs.items():
            if word == cs_addr:
                selref_addrs[name] = selrefs_section.addr + i
                break
    for k, v in selref_addrs.items():
        print(f"{k}: selref at {v:#x}")

    def find_func_start(text_off):
        for back in range(0, 4000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    print("\n=== Callers ===")
    for sel_name, sa in selref_addrs.items():
        print(f"\n-- {sel_name} (selref @ {sa:#x}) --")
        word = struct.pack("<I", sa)
        pos = 0
        hits = set()
        while True:
            p = text_chunk.find(word, pos)
            if p < 0:
                break
            fn = find_func_start(p)
            if fn:
                hits.add(fn)
            pos = p + 4
        for fn in sorted(hits):
            print(f"  caller fn: {fn:#x}")


if __name__ == "__main__":
    main()
