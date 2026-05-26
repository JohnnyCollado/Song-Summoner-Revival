"""Find and disassemble specific Mach-O symbols in S.S.Encore.

We focus on:
  * `_Z22MainLoop_Set_iPodStatei` (C++) — sets the iPod state.
  * `_Z22MainLoop_Get_iPodStatev` (C++) — reads it (tells us the global var address).
  * `start_iPodView` (Obj-C selector on ViewManager) — should call Set_iPodState.

The goal: identify the state value that activates the iPod browser render
path, and the address of the state global. With those, we can poke the state
from touchHLE's host code to force-activate the picker render.
"""

import struct
import sys
from macholib.MachO import MachO
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB, CS_MODE_ARM

BIN = "Payload/S.S.Encore.app/S.S.Encore"


def load_macho():
    m = MachO(BIN)
    # The IPA contains a fat binary (armv6 + armv7). We pick the armv6 slice
    # because the Song Summoner main binary loads its armv6 slice in touchHLE.
    armv6 = None
    for h in m.headers:
        cpu_subtype = h.header.cpusubtype & 0xFF
        if h.header.cputype == 12 and cpu_subtype == 6:  # CPU_TYPE_ARM, ARM_V6
            armv6 = h
            break
    if armv6 is None:
        for h in m.headers:
            if h.header.cputype == 12:
                armv6 = h
                break
    assert armv6 is not None, "no ARM slice"
    return m, armv6


def read_text_section(m, header):
    """Return (text_bytes, text_vmaddr, text_file_off)."""
    # Locate __TEXT,__text. macholib gives us load commands; we iterate them.
    for lc, cmd, data in header.commands:
        # LC_SEGMENT (1) is the 32-bit segment.
        if hasattr(cmd, "segname") and cmd.segname.rstrip(b"\x00") == b"__TEXT":
            seg = cmd
            for sect in data:
                if sect.sectname.rstrip(b"\x00") == b"__text":
                    file_off = header.offset + sect.offset
                    return file_off, sect.size, sect.addr
    raise SystemExit("__TEXT,__text not found")


def find_symbol_addrs(header, names):
    """Return {sym_name: vmaddr} from the symbol table."""
    found = {}
    # We need LC_SYMTAB.
    symtab = None
    for lc, cmd, data in header.commands:
        if lc.cmd == 0x2:  # LC_SYMTAB
            symtab = cmd
            break
    if symtab is None:
        return found

    with open(BIN, "rb") as f:
        f.seek(header.offset + symtab.symoff)
        # Each nlist is 12 bytes on 32-bit: n_strx, n_type, n_sect, n_desc, n_value.
        sym_bytes = f.read(symtab.nsyms * 12)
        f.seek(header.offset + symtab.stroff)
        str_bytes = f.read(symtab.strsize)

    for i in range(symtab.nsyms):
        off = i * 12
        n_strx, n_type, n_sect, n_desc, n_value = struct.unpack_from(
            "<IBBHI", sym_bytes, off
        )
        if n_strx == 0 or n_strx >= len(str_bytes):
            continue
        end = str_bytes.find(b"\x00", n_strx)
        sym = str_bytes[n_strx:end].decode("utf-8", "replace")
        for want in names:
            if sym == want or sym == "_" + want:
                # n_desc bit 0x8 = N_ARM_THUMB_DEF.
                thumb_bit = 1 if (n_desc & 0x0008) else 0
                found[want] = n_value | thumb_bit
    return found


def disassemble_at(file_off, text_file_off, text_size, text_vmaddr, vmaddr, count=40):
    """Disassemble `count` instructions starting at the given virtual address.

    The bottom bit of vmaddr tells us thumb (1) vs arm (0).
    """
    thumb = vmaddr & 1
    addr = vmaddr & ~1
    rel = addr - text_vmaddr
    if rel < 0 or rel >= text_size:
        print(f"  (vmaddr {vmaddr:#x} outside __text)")
        return
    fpos = text_file_off + rel
    with open(BIN, "rb") as f:
        f.seek(fpos)
        blob = f.read(count * 4)
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB if thumb else CS_MODE_ARM)
    md.detail = False
    n = 0
    for ins in md.disasm(blob, addr):
        print(f"  {ins.address:#08x}: {ins.mnemonic:7s} {ins.op_str}")
        n += 1
        if n >= count:
            break


def main():
    m, header = load_macho()
    text_off, text_size, text_vmaddr = read_text_section(m, header)
    print(f"__TEXT,__text: file {text_off:#x}, size {text_size:#x}, vmaddr {text_vmaddr:#x}")

    syms = find_symbol_addrs(
        header,
        [
            "__Z22MainLoop_Set_iPodStatei",
            "__Z22MainLoop_Get_iPodStatev",
            "__Z14MainLoop_Drawv",
            "__Z15MainLoop_FrameInitv",
            "__Z15MainLoop_Update_FrameInitv",
            "__Z16MainLoop_iPodViewv",
            "__Z18MainLoop_StateMainv",
            "__Z19MainLoop_StateInitiv",
            "__Z18MainLoop_Init_iPodv",
            "__Z18MainLoop_Exit_iPodv",
            "__Z14iPodView_Mainv",
            "__Z14iPodView_Initv",
            "__Z14iPodView_Exitv",
            "__Z14iPodView_Drawv",
        ],
    )
    print("\nFound symbols:")
    for k, v in syms.items():
        print(f"  {k} @ {v:#x}")

    for k, vmaddr in syms.items():
        if vmaddr == 0:
            continue
        print(f"\n=== {k} @ {vmaddr:#x} ===")
        disassemble_at(0, text_off, text_size, text_vmaddr, vmaddr, count=30)


if __name__ == "__main__":
    main()
