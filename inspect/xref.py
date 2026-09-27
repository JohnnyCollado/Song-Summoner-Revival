"""Find Thumb BL/BLX callers of given targets across __text; print the
enclosing function (nearest preceding symbol) and the constant loaded into
r0 just before the call (for setter calls)."""
import sys, bisect
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB
from mo import *
targets = {int(a,16)&~1 for a in sys.argv[1:]}
a, sz, o = sect("__TEXT","__text")
md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
names = sorted(set(list(SYMS)+list(IMP2NAME)))
def owner(x):
    i = bisect.bisect_right(names, x)-1
    n = names[i]
    return "%x %s"%(n, IMP2NAME.get(n) or SYMS.get(n))
code = DATA[o:o+sz]
i = 0
last_r0 = {}
while i < sz-2:
    hw = int.from_bytes(code[i:i+2],'little')
    if (hw & 0xF800) == 0xF000 and i+4 <= sz:
        hw2 = int.from_bytes(code[i+2:i+4],'little')
        if (hw2 & 0xE800) in (0xE800, 0xF800):
            off_ = ((hw & 0x7FF) << 12) | ((hw2 & 0x7FF) << 1)
            if off_ & 0x400000: off_ -= 0x800000
            t = a + i + 4 + off_
            if (hw2 & 0x1000) == 0: t &= ~3
            if t in targets:
                # find movs r0,#imm in previous 8 halfwords
                imm = None
                for k in range(2, 20, 2):
                    p = int.from_bytes(code[i-k:i-k+2],'little')
                    if (p & 0xFF00) == 0x2000: imm = p & 0xFF; break
                print("%06x -> %06x  r0=%s  in %s"%(a+i, t, imm, owner(a+i)))
            i += 4; continue
    i += 2
