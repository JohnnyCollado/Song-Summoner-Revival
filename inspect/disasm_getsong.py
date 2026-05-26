"""Find and disassemble -[IPDSongsTableViewController getSong:] in S.S.Encore.
Output the function body so we can read which ivar offset / data structure
it accesses.

Approach (extension of find_imp.py):
  1. Locate the selector string `getSong:` in the __cstring/__objc_methname.
  2. Find method_t entries whose name pointer equals that vmaddr; the IMP
     is at offset +8.
  3. Filter to IMPs that sit inside __text (the actual code segment).
  4. Resolve file offset of the IMP and disassemble ~200 bytes with
     capstone (thumb mode -- armv6 + iOS 3.x uses Thumb-2 in __text for
     most app code; we'll try Thumb first, fall back to ARM).
"""

import struct
import sys

from macholib.MachO import MachO
import capstone

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"
TARGETS = [
    "getSong:",
    "tableView:didSelectRowAtIndexPath:",
    "tableView:numberOfRowsInSection:",
    "tableView:cellForRowAtIndexPath:",
    "loadResource",
]


def main() -> None:
    m = MachO(BIN)
    arm_header = None
    for h in m.headers:
        if h.header.cputype == 12:  # CPU_TYPE_ARM
            arm_header = h
            break
    assert arm_header is not None

    with open(BIN, "rb") as f:
        data = f.read()

    # Map vmaddr -> file offset for each section + collect __text range.
    sections = []
    text_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sn = sect.sectname.rstrip(b"\x00")
            sg = sect.segname.rstrip(b"\x00")
            sections.append((sn, sg, sect))
            if sn == b"__text":
                text_section = sect

    def vmaddr_to_offset(vmaddr):
        for _, _, s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    # Find selector strings.
    string_sections = [s for n, _, s in sections if n in (b"__objc_methname", b"__cstring")]
    sel_addrs = {}
    for sect in string_sections:
        start = arm_header.offset + sect.offset
        chunk = data[start : start + sect.size]
        for target in TARGETS:
            if target in sel_addrs:
                continue
            tb = target.encode() + b"\x00"
            pos = 0
            while True:
                p = chunk.find(tb, pos)
                if p < 0:
                    break
                if p == 0 or chunk[p - 1] == 0:
                    sel_addrs[target] = sect.addr + p
                    break
                pos = p + 1

    print("Selector vmaddrs:")
    for k, v in sel_addrs.items():
        print(f"  {k}: {v:#x}")

    # Search for method_t entries matching each selector.
    text_start = text_section.addr
    text_end = text_section.addr + text_section.size

    md_thumb = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)
    md_arm = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_ARM)

    for target, sel_vm in sel_addrs.items():
        print(f"\n=== {target} (sel vmaddr {sel_vm:#x}) ===")
        sel_bytes = struct.pack("<I", sel_vm)
        pos = 0
        imps = set()
        while True:
            p = data.find(sel_bytes, pos)
            if p < 0:
                break
            if p % 4 == 0 and p + 12 <= len(data):
                imp = struct.unpack_from("<I", data, p + 8)[0]
                # iOS 3.x Thumb code: bit-0 set means Thumb.
                imp_clean = imp & ~1
                if text_start <= imp_clean < text_end:
                    imps.add(imp)
            pos = p + 4

        print(f"  {len(imps)} IMP(s) found")
        for imp in sorted(imps):
            imp_clean = imp & ~1
            thumb = bool(imp & 1)
            off = vmaddr_to_offset(imp_clean)
            print(f"  IMP {imp:#x} ({'thumb' if thumb else 'arm'}) file_off={off:#x}")
            if off is None:
                continue
            blob = data[off : off + 240]
            md = md_thumb if thumb else md_arm
            for ins in md.disasm(blob, imp_clean):
                line = f"    {ins.address:08x}: {ins.bytes.hex():<8} {ins.mnemonic} {ins.op_str}"
                print(line)
                # Stop at the first 'bx lr' / function epilogue.
                if ins.mnemonic in ("bx",) and "lr" in ins.op_str:
                    break
                if ins.mnemonic == "pop" and "pc" in ins.op_str:
                    break


if __name__ == "__main__":
    main()
