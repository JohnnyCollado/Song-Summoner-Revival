"""Find callers of glDeleteTextures and related cleanup functions.

Strategy:
1. Find the dyld stub for glDeleteTextures (in __symbol_stub).
2. Find all BL/BLX callers of the stub.
3. For each caller, identify the containing function.
4. Cross-reference with functions that also touch the scene table at
   0x1134f0 — these are top candidates for the iPod scene destructor.
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
    stub_section = None
    sym_ptr_section = None
    cstring_sections = []
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for s in sects:
            sections.append(s)
            sn = s.sectname.rstrip(b"\x00")
            if sn == b"__text":
                text_section = s
            if sn in (b"__symbol_stub", b"__symbolstub1", b"__picsymbol_stub", b"__symbol_stub1"):
                stub_section = s
            if sn in (b"__la_symbol_ptr", b"__nl_symbol_ptr"):
                sym_ptr_section = s
            if sn in (b"__cstring", b"__objc_methname"):
                cstring_sections.append(s)

    print(f"Sections found:")
    print(f"  text: {text_section.sectname if text_section else None}")
    print(f"  stub: {stub_section.sectname if stub_section else None} @ {stub_section.addr:#x} size {stub_section.size:#x}" if stub_section else "  no stub section")
    print(f"  la_symbol_ptr: {sym_ptr_section.sectname if sym_ptr_section else None}")

    # Find the cstring address of "_glDeleteTextures" (or "glDeleteTextures")
    targets = [b"_glDeleteTextures", b"glDeleteTextures", b"_alcDestroyContext", b"alcDestroyContext"]
    target_addrs = {}
    for s in cstring_sections:
        start = arm_header.offset + s.offset
        chunk = data[start:start + s.size]
        for t in targets:
            pos = 0
            while True:
                p = chunk.find(t + b"\x00", pos)
                if p < 0:
                    break
                if p == 0 or chunk[p - 1] == 0:
                    target_addrs[t.decode()] = s.addr + p
                    break
                pos = p + 1
    print(f"\nSymbol c-string addrs:")
    for k, v in target_addrs.items():
        print(f"  {k}: {v:#x}")

    # Print summary
    text_start = arm_header.offset + text_section.offset
    text_chunk = data[text_start:text_start + text_section.size]

    def find_func_start(text_off):
        for back in range(0, 6000, 2):
            o = text_off - back
            if o < 0:
                break
            b1 = text_chunk[o + 1] if o + 1 < len(text_chunk) else 0
            if b1 == 0xB5:
                return text_section.addr + o
        return None

    def decode_bl(off, cur_addr):
        if off + 4 > len(text_chunk):
            return None
        hw1 = text_chunk[off] | (text_chunk[off + 1] << 8)
        hw2 = text_chunk[off + 2] | (text_chunk[off + 3] << 8)
        if ((hw1 >> 11) & 0x1F) != 0x1E:
            return None
        if ((hw2 >> 14) & 0x3) != 0x3:
            return None
        s = (hw1 >> 10) & 1
        imm10 = hw1 & 0x3FF
        j1 = (hw2 >> 13) & 1
        bit12 = (hw2 >> 12) & 1
        j2 = (hw2 >> 11) & 1
        imm11 = hw2 & 0x7FF
        i1 = 1 - (j1 ^ s)
        i2 = 1 - (j2 ^ s)
        imm32 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
        if imm32 & (1 << 24):
            imm32 -= 1 << 25
        target = cur_addr + 4 + imm32
        if bit12 == 0:
            target = target & ~3
        return target

    # Find stub for glDeleteTextures: search the stub section's contents
    # for stubs that reference the la_symbol_ptr to glDeleteTextures.
    # Simpler approach: each stub is typically 8-12 bytes of code that
    # loads from a fixed la_symbol_ptr address and jumps. We can look
    # for the stub address by finding references to the cstring symbol
    # name in __nl_symbol_ptr or by searching the symbol table.
    #
    # Easier alternative: armv6 dyld stubs have a fixed pattern. Find
    # all 12-byte sequences in __symbol_stub and look for the one that
    # references our target la_symbol_ptr. Since we know touchHLE
    # exports glDeleteTextures (we hook it), the stub exists.

    if stub_section is None:
        print("\nNo stub section — armv6 binary may use a different scheme.")
        print("Falling back to scanning for any BL/BLX whose target falls in __nl_symbol_ptr range.")
        # Instead, we'll look for `bl` to anywhere in the la_symbol_ptr range
        # and try to match the target to a symbol.
    else:
        # For each 12-byte stub, decode it to find which la_symbol_ptr it references.
        stub_off = arm_header.offset + stub_section.offset
        stub_chunk = data[stub_off:stub_off + stub_section.size]
        print(f"\n=== Stubs (size {stub_section.size} bytes, ~{stub_section.size // 12} stubs) ===")
        # Show the first 5 stubs as raw bytes for diagnostic
        for i in range(0, min(stub_section.size, 60), 12):
            vm = stub_section.addr + i
            hexstr = stub_chunk[i:i + 12].hex()
            print(f"  stub @ {vm:#x}: {hexstr}")


if __name__ == "__main__":
    main()
