"""Find the function that prints 'select!!!!' via CFString."""

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
    cfstring_section = None
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        for sect in sects:
            sn = sect.sectname.rstrip(b"\x00")
            sections.append((sn, sect))
            if sn == b"__text":
                text_section = sect
            if sn == b"__cfstring":
                cfstring_section = sect

    def vmaddr_to_offset(vmaddr):
        for _, s in sections:
            if s.addr <= vmaddr < s.addr + s.size:
                return arm_header.offset + s.offset + (vmaddr - s.addr)
        return None

    select_str_addr = 0x6c128

    # Walk __cfstring for an entry whose string_data ptr == select_str_addr.
    cfstring_start = arm_header.offset + cfstring_section.offset
    cfstring_end = cfstring_start + cfstring_section.size
    cf_chunk = data[cfstring_start:cfstring_end]
    # __cfstring entries are 16 bytes: isa, flags, str_ptr, len
    cf_entries = []
    for i in range(0, len(cf_chunk), 16):
        isa, flags, sp, ln = struct.unpack_from("<IIII", cf_chunk, i)
        cf_entries.append((cfstring_section.addr + i, sp, ln))

    targets = [b"select!!!!", b"SONG : "]
    target_addrs = {}
    for _, s in sections:
        if s.sectname.rstrip(b"\x00") != b"__cstring":
            continue
        start = arm_header.offset + s.offset
        chunk = data[start : start + s.size]
        for t in targets:
            p = chunk.find(t)
            if p >= 0:
                target_addrs[t] = s.addr + p
    print("String addrs:")
    for k, v in target_addrs.items():
        print(f"  {k!r}: {v:#x}")

    cfaddr_for = {}
    for cf_vm, sp, ln in cf_entries:
        for t, sa in target_addrs.items():
            if sp == sa:
                cfaddr_for[t] = cf_vm
    print("\nCFString entry addrs:")
    for k, v in cfaddr_for.items():
        print(f"  {k!r}: {v:#x}")

    # Find references in __text to these CFString addrs.
    text_start = arm_header.offset + text_section.offset
    text_end = text_start + text_section.size
    text_chunk = data[text_start:text_end]
    md = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)

    for t, cf_vm in cfaddr_for.items():
        word = struct.pack("<I", cf_vm)
        pos = 0
        print(f"\nReferences to CFString {t!r} ({cf_vm:#x}):")
        hits = []
        while True:
            p = text_chunk.find(word, pos)
            if p < 0:
                break
            vm = text_section.addr + p
            hits.append(vm)
            pos = p + 4
        for h in hits[:10]:
            print(f"  literal at {h:#x}")
            # Walk back to find function start: push {...,lr}
            off_in_text = h - text_section.addr
            for back in range(0, 800, 2):
                off = off_in_text - back
                if off < 0:
                    break
                ins_bytes = text_chunk[off : off + 2]
                if len(ins_bytes) == 2 and ins_bytes[1] == 0xB5:
                    fn = text_section.addr + off
                    print(f"    function start: {fn:#x} (thumb)")
                    break


if __name__ == "__main__":
    main()
