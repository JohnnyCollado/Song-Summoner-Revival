"""Find the function that logs '+++++ iPodView Create!! +++++'.

The string is a C string literal in __cstring. Find its address, then find
the function that references it (likely via NSLog or printf-like wrapper).
"""
import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"
NEEDLE = b"+++++ iPodView Create!! +++++"


def main():
    m = MachO(BIN)
    h = next(x for x in m.headers if x.header.cputype == 12)
    with open(BIN, "rb") as f:
        data = f.read()

    sections = {}
    for lc, cmd, sects in h.commands:
        if not hasattr(cmd, "segname"): continue
        seg = cmd.segname.rstrip(b"\x00").decode()
        for s in sects:
            n = s.sectname.rstrip(b"\x00").decode()
            sections[(seg, n)] = s

    def v2o(vma):
        for s in sections.values():
            if s.addr <= vma < s.addr + s.size:
                return h.offset + s.offset + (vma - s.addr)
        return None

    cstr = sections.get(("__TEXT", "__cstring"))
    cstr_off = h.offset + cstr.offset
    blob = data[cstr_off:cstr_off + cstr.size]
    idx = blob.find(NEEDLE)
    if idx < 0:
        print("needle not found"); return
    string_vma = cstr.addr + idx
    print(f"string @ vma {string_vma:#x}")

    # Find references in __text. The ARM instruction sequence for loading
    # this string is typically a 2-instruction MOV.W/MOVT pair, or a literal
    # pool reference. The simplest scan: look for the 32-bit VMA inside the
    # __text section as a literal pool entry.
    text = sections.get(("__TEXT", "__text"))
    tbase = h.offset + text.offset
    tchunk = data[tbase:tbase + text.size]
    needle_le = struct.pack("<I", string_vma)
    refs = []
    pos = 0
    while True:
        i = tchunk.find(needle_le, pos)
        if i < 0: break
        refs.append(text.addr + i)
        pos = i + 1
    print(f"literal-pool refs in __text: {refs}")

    # Walk back from each ref to find the enclosing function start (heuristic
    # = nearest preceding push {..., lr} which is opcode pattern 0xB5xx or
    # 0xE92D 0x...).
    for ref in refs:
        ref_off = ref - text.addr
        func_start = None
        for back in range(0, 4000, 2):
            o = ref_off - back
            if o < 0: break
            b0 = tchunk[o]
            b1 = tchunk[o + 1] if o + 1 < len(tchunk) else 0
            # Thumb push {..., lr}: 0xB5 in high byte
            if b1 == 0xB5:
                func_start = text.addr + o; break
        print(f"ref @ {ref:#x} -> func start ~= {func_start and hex(func_start)}")


if __name__ == "__main__":
    main()
