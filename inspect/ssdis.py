"""Annotated disassembler: resolves literals, selrefs, classrefs, stubs,
objc_msgSend selectors and cfstrings with light register tracking.

Run from a folder holding the extracted Payload/ (see mo.py).
usage: python ssdis.py <hexaddr|"-[Class sel]"> ...
"""
import sys
import struct
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB, CS_MODE_ARM
from capstone.arm import ARM_OP_REG, ARM_OP_IMM, ARM_OP_MEM
from capstone.arm import ARM_REG_R0, ARM_REG_R1, ARM_REG_R2, ARM_REG_R3
from mo import *

CFSTR = {}
a, sz, _ = sect("__DATA", "__cfstring")
for i in range(0, sz, 16):
    CFSTR[a + i] = cstr(u32(a + i + 8))


def describe(v):
    if v is None:
        return None
    if v in SELREFS:
        return "selref:" + SELREFS[v]
    if v in CLASSREFS:
        return "class:" + CLASSREFS[v]
    if v in CFSTR:
        return "@%r" % CFSTR[v]
    if v in STUBS:
        return "stub:" + STUBS[v]
    if (v & ~1) in IMP2NAME:
        return IMP2NAME[v & ~1]
    if (v & ~1) in SYMS:
        return SYMS[v & ~1]
    s = SECTS.get(("__TEXT", "__cstring"))
    if s and s[0] <= v < s[0] + s[1]:
        return "cstr:%r" % cstr(v)
    return None


def func_end(start):
    # next known symbol / imp after start
    cands = [x for x in list(SYMS) + list(IMP2NAME) if x > start]
    return min(cands) if cands else start + 0x400


def dis(start, count=None, thumb=None, out=print):
    if thumb is None:
        thumb = bool(start & 1) or (start & ~1) in IMP2NAME
    start &= ~1
    end = func_end(start) if count is None else start + count * 4
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB if thumb else CS_MODE_ARM)
    md.detail = True
    code = DATA[off(start):off(end)]
    regs = {}
    name = IMP2NAME.get(start) or SYMS.get(start) or ""
    out("; ---- 0x%x %s" % (start, name))
    for ins in md.disasm(code, start):
        note = []
        ops = ins.operands
        mn = ins.mnemonic
        pcv = (ins.address + (4 if thumb else 8))
        try:
            if mn.startswith("ldr") and len(ops) == 2 and ops[1].type == ARM_OP_MEM:
                m = ops[1].mem
                dst = ops[0].reg
                if ins.reg_name(m.base) == "pc":
                    la = (pcv & ~3) + m.disp
                    v = u32(la)
                    regs[dst] = v
                    d = describe(v)
                    note.append("=0x%x%s" % (v, " " + d if d else ""))
                elif m.base in regs and m.index == 0:
                    ea = regs[m.base] + m.disp
                    d = describe(ea)
                    if d:
                        note.append("[%s]" % d)
                    v = u32(ea)
                    if m.disp == 0 and d is None and describe(v):
                        note.append(describe(v))
                    # A resolved name (selref, classref...) is kept as a
                    # string so objc_msgSend can print receiver/selector.
                    regs[dst] = d if d else (v if off(ea) else None)
                else:
                    regs.pop(dst, None)
            elif mn == "add" and len(ops) == 2 and ops[1].type == ARM_OP_REG \
                    and ins.reg_name(ops[1].reg) == "pc":
                r = ops[0].reg
                if isinstance(regs.get(r), int):
                    regs[r] = (regs[r] + pcv) & 0xFFFFFFFF
                    d = describe(regs[r])
                    note.append("-> 0x%x%s" % (regs[r], " " + d if d else ""))
            elif mn in ("mov", "movs") and len(ops) == 2:
                if ops[1].type == ARM_OP_REG and ops[1].reg in regs:
                    regs[ops[0].reg] = regs[ops[1].reg]
                elif ops[1].type == ARM_OP_IMM:
                    regs[ops[0].reg] = ops[1].imm
                else:
                    regs.pop(ops[0].reg, None)
            elif mn in ("bl", "blx") and ops and ops[0].type == ARM_OP_IMM:
                t = ops[0].imm
                d = describe(t) or describe(t | 1) or ""
                if "objc_msgSend" in d:
                    s = regs.get(ARM_REG_R1)
                    rcv = regs.get(ARM_REG_R0)
                    note.append("%s  rcv=%s sel=%s arg=%s" % (
                        d, rcv if isinstance(rcv, str) else "?",
                        s.replace("selref:", "") if isinstance(s, str) else "?",
                        regs.get(ARM_REG_R2) if isinstance(regs.get(ARM_REG_R2), str) else ""))
                else:
                    note.append(d or "sub_%x" % t)
                for r in (ARM_REG_R0, ARM_REG_R1, ARM_REG_R2, ARM_REG_R3):
                    regs.pop(r, None)
            else:
                if ops and ops[0].type == ARM_OP_REG and mn not in (
                        "cmp", "tst", "str", "strb", "strh", "push", "pop",
                        "b", "bx", "cbz", "cbnz") and not mn.startswith("b"):
                    regs.pop(ops[0].reg, None)
        except Exception as e:
            note.append("!" + str(e))
        out("  %06x: %-8s %-28s %s" % (ins.address, mn, ins.op_str,
                                        ("; " + " ".join(note)) if note else ""))


def resolve(arg):
    if arg.startswith(("-[", "+[")):
        for k, v in IMP2NAME.items():
            if v == arg:
                return k | 1
        raise SystemExit("no such method " + arg)
    return int(arg, 16)


if __name__ == "__main__":
    for a in sys.argv[1:]:
        dis(resolve(a))
