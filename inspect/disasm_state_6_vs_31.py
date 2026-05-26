"""Disassemble the state-6 and state-31 branches of the iPod scene render
function (0x17800). Goal: identify what state-6 does that state-31 doesn't —
specifically, the cleanup function call that clears prior panel sprites
before re-building.

Strategy:
1. Scan 0x17800 for `cmp r3, #imm` where imm ∈ {6, 0x1f=31} and the
   register comparison is on scene+0xc.
2. For each, dump ~150 bytes of instructions starting at the matching
   branch target, decoding bl/blx targets and ldr-resolved selectors.
3. Diff the two for "cleanup-like" calls (release, free, removeAll,
   setHidden, etc).
"""

import struct
from macholib.MachO import MachO
import capstone

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

    # Disassemble 0x17800 and find all (offset, instruction) pairs.
    fn_start = 0x17800
    fn_size = 0x1300  # ~4.7KB — should cover the whole function
    fn_off = vm_to_off(fn_start)
    fn_blob = data[fn_off:fn_off + fn_size]
    md = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB)
    md.detail = True
    insns = list(md.disasm(fn_blob, fn_start))

    # Pass 1: find candidate "scene_state cmp" instructions.
    print("=== State comparison sites in 0x17800 ===")
    state_check_addrs = []
    for i, ins in enumerate(insns):
        # cmp Rd, #imm (T1, 16-bit) → 0x28-0x2F + imm8
        if ins.mnemonic == "cmp" and ins.op_str.startswith("r"):
            # Look at preceding ldr to see if it's the scene state field.
            if i >= 1 and insns[i - 1].mnemonic == "ldr":
                preceding = insns[i - 1]
                if "#0xc" in preceding.op_str:
                    val_str = ins.op_str.split(",")[-1].strip().lstrip("#")
                    try:
                        val = int(val_str, 0)
                        if val in (2, 3, 5, 6, 9, 15, 30, 31):
                            state_check_addrs.append((ins.address, val, preceding.op_str))
                            print(f"  {ins.address:#x}: ldr {preceding.op_str} ; cmp r3, #{val}")
                    except ValueError:
                        pass

    # Pass 2: for each branch destination (state==6 and state==31), follow
    # the taken branch and dump ~80 instructions, including ldr-resolved
    # selectors.
    def dump_branch_starting_at(insn_idx, label):
        print(f"\n=== Branch starting at instruction index {insn_idx} ({label}) ===")
        end_idx = min(insn_idx + 120, len(insns))
        for j in range(insn_idx, end_idx):
            ins = insns[j]
            extra = ""
            # Resolve ldr literal to selector / string
            if ins.mnemonic in ("ldr", "ldr.w") and "[pc," in ins.op_str:
                parts = ins.op_str.split("#")
                if len(parts) >= 2:
                    try:
                        offset_str = parts[-1].rstrip("]")
                        imm = int(offset_str, 0)
                        pc = (ins.address + 4) & ~3
                        lit_vm = pc + imm
                        lit_off = vm_to_off(lit_vm)
                        if lit_off:
                            ptr = struct.unpack_from("<I", data, lit_off)[0]
                            selref_off = vm_to_off(ptr)
                            sel = None
                            if selref_off:
                                cstr_addr = struct.unpack_from("<I", data, selref_off)[0]
                                sel = read_cstr(cstr_addr)
                            if not sel:
                                sel = read_cstr(ptr)
                            if sel and len(sel) > 1 and len(sel) < 80 and all(0x20 <= ord(c) < 0x7F for c in sel) and not sel.startswith("/"):
                                extra = f" ; sel/str='{sel}'"
                            else:
                                extra = f" ; literal={ptr:#x}"
                    except (ValueError, IndexError):
                        pass
            # Resolve bl/blx to known functions
            if ins.mnemonic in ("bl", "blx") and ins.op_str.startswith("#"):
                tgt_str = ins.op_str.lstrip("#")
                try:
                    tgt = int(tgt_str, 0)
                    known = {
                        0x322c: "Set_iPodState",
                        0x3244: "Set_iPodCancel",
                        0x325c: "Set_iPodMusicID",
                        0xc9f4: "get_scene_obj",
                        0xca54: "malloc?",
                        0x667cc: "objc_msgSend",
                        0x66124: "NSLog?",
                    }
                    if tgt in known:
                        extra = f" ; → {known[tgt]}"
                except ValueError:
                    pass
            print(f"  {ins.address:#x}: {ins.mnemonic:<8} {ins.op_str}{extra}")
            # Stop at unconditional branches / pop pc
            if ins.mnemonic == "b" and not ins.op_str.startswith("r"):
                break
            if ins.mnemonic == "pop" and "pc" in ins.op_str:
                break

    # For each interesting state, find the matching insn_idx and follow.
    for target_state in (6, 31):
        for i, ins in enumerate(insns):
            if ins.address in [a for a, v, _ in state_check_addrs if v == target_state]:
                dump_branch_starting_at(i, f"state=={target_state}")
                break


if __name__ == "__main__":
    main()
