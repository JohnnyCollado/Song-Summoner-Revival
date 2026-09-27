"""Tiny Mach-O / ObjC2 reader for the thin armv6 S.S.Encore binary.

Extract Payload/S.S.Encore.app/S.S.Encore from your own IPA into the
current folder first. Never commit the binary.
"""
import struct
from macholib.MachO import MachO

BIN = "Payload/S.S.Encore.app/S.S.Encore"
DATA = open(BIN, "rb").read()
_m = MachO(BIN)
H = _m.headers[0]
SECTS = {}
SEGS = []
for lc, cmd, data in H.commands:
    if hasattr(cmd, "segname"):
        SEGS.append((cmd.vmaddr, cmd.vmsize, cmd.fileoff, cmd.filesize))
        for s in data:
            SECTS[(cmd.segname.rstrip(b"\0").decode(),
                   s.sectname.rstrip(b"\0").decode())] = (s.addr, s.size, s.offset)


def off(va):
    for vm, vs, fo, fs in SEGS:
        if vm <= va < vm + fs:
            return va - vm + fo
    return None


def u32(va):
    o = off(va)
    if o is None:
        return 0
    return struct.unpack_from("<I", DATA, o)[0]


def cstr(va):
    o = off(va)
    if o is None:
        return None
    e = DATA.index(b"\0", o)
    return DATA[o:e].decode("latin1")


def sect(seg, name):
    return SECTS[(seg, name)]


# ---- symbols (local + imported) ----
SYMS = {}      # addr -> name
IMPORTS = []   # ordered undefined symbols
for lc, cmd, data in H.commands:
    if lc.cmd == 0x2:
        so, n, stro = cmd.symoff, cmd.nsyms, cmd.stroff
        for i in range(n):
            strx, ntype, nsect, ndesc, nvalue = struct.unpack_from(
                "<IBBhI", DATA, so + i * 12)
            e = DATA.index(b"\0", stro + strx)
            name = DATA[stro + strx:e].decode("latin1")
            if ntype & 0x0E == 0x0E and nvalue:
                a = nvalue
                if ndesc & 0x8:  # N_ARM_THUMB_DEF
                    a |= 1
                SYMS[a & ~1] = name
            IMPORTS.append(name)
    if lc.cmd == 0xB:
        DYSYM = cmd

# indirect symbols -> stubs / lazy pointers
STUBS = {}
indsym = []
so = DYSYM.indirectsymoff
for i in range(DYSYM.nindirectsyms):
    indsym.append(struct.unpack_from("<I", DATA, so + i * 4)[0])
for lc, cmd, data in H.commands:
    if hasattr(cmd, "segname"):
        for s in data:
            t = s.flags & 0xFF
            if t in (6, 7, 8):  # non-lazy, lazy, stubs
                size = s.reserved2 if t == 8 else 4
                for j in range(s.size // size):
                    idx = indsym[s.reserved1 + j]
                    if idx & 0xC0000000:
                        continue
                    STUBS[s.addr + j * size] = IMPORTS[idx]

# ---- ObjC ----
SELREFS = {}
a, sz, _ = sect("__DATA", "__objc_selrefs")
for i in range(0, sz, 4):
    SELREFS[a + i] = cstr(u32(a + i))


def class_name(cls):
    ro = u32(cls + 16) & ~3
    return cstr(u32(ro + 16))


CLASSREFS = {}
a, sz, _ = sect("__DATA", "__objc_classrefs")
for i in range(0, sz, 4):
    c = u32(a + i)
    CLASSREFS[a + i] = class_name(c) if c else "<ext>"


def methods(ml):
    out = []
    if not ml:
        return out
    es, cnt = u32(ml) & 0xFFFF, u32(ml + 4)
    for i in range(cnt):
        p = ml + 8 + i * es
        out.append((cstr(u32(p)), cstr(u32(p + 4)), u32(p + 8)))
    return out


def ivars(il):
    out = []
    if not il:
        return out
    es, cnt = u32(il), u32(il + 4)
    for i in range(cnt):
        p = il + 8 + i * es
        out.append((u32(u32(p)), cstr(u32(p + 4)), cstr(u32(p + 8))))
    return out


def protos(pl):
    if not pl:
        return []
    n = u32(pl)
    return [cstr(u32(u32(pl + 4 + i * 4) + 4)) for i in range(n)]


CLASSES = {}
a, sz, _ = sect("__DATA", "__objc_classlist")
for i in range(0, sz, 4):
    cls = u32(a + i)
    ro = u32(cls + 16) & ~3
    meta = u32(cls)
    mro = u32(meta + 16) & ~3
    sup = u32(cls + 4)
    CLASSES[cstr(u32(ro + 16))] = dict(
        addr=cls,
        superclass=class_name(sup) if sup else None,
        inst=methods(u32(ro + 20)), cls=methods(u32(mro + 20)),
        protos=protos(u32(ro + 24)), ivars=ivars(u32(ro + 28)),
        size=u32(ro + 8))

IMP2NAME = {}
for cn, c in CLASSES.items():
    for n, t, imp in c["inst"]:
        IMP2NAME[imp & ~1] = "-[%s %s]" % (cn, n)
    for n, t, imp in c["cls"]:
        IMP2NAME[imp & ~1] = "+[%s %s]" % (cn, n)

# external relocations: address -> imported symbol (e.g. superclass binds)
EXTREL = {}
_syms_all = []
for lc, cmd, data in H.commands:
    if lc.cmd == 0x2:
        for i in range(cmd.nsyms):
            strx = struct.unpack_from("<I", DATA, cmd.symoff + i * 12)[0]
            e = DATA.index(b"\0", cmd.stroff + strx)
            _syms_all.append(DATA[cmd.stroff + strx:e].decode("latin1"))
for i in range(DYSYM.nextrel):
    ra, info = struct.unpack_from("<Ii", DATA, DYSYM.extreloff + i * 8)
    symnum = info & 0xFFFFFF
    EXTREL[ra] = _syms_all[symnum]
for cn, c in CLASSES.items():
    if c["superclass"] is None:
        s = EXTREL.get(c["addr"] + 4)
        if s:
            c["superclass"] = s.replace("_OBJC_CLASS_$_", "")
for k, v in list(CLASSREFS.items()):
    if v == "<ext>" and k in EXTREL:
        CLASSREFS[k] = EXTREL[k].replace("_OBJC_CLASS_$_", "")
