"""List all Obj-C class names + ivar counts."""
import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"


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
            name = s.sectname.rstrip(b"\x00").decode()
            sections[(seg, name)] = s

    def v2o(vma):
        for s in sections.values():
            if s.addr <= vma < s.addr + s.size:
                return h.offset + s.offset + (vma - s.addr)
        return None

    def rstr(vma):
        off = v2o(vma)
        if off is None: return None
        end = data.find(b"\x00", off)
        return data[off:end].decode("ascii", errors="replace")

    cl = sections.get(("__DATA", "__objc_classlist"))
    cl_off = h.offset + cl.offset
    ptrs = [
        struct.unpack_from("<I", data, cl_off + i)[0]
        for i in range(0, cl.size, 4)
    ]
    rows = []
    for cvma in ptrs:
        co = v2o(cvma)
        isa, sup, cache, vt, dp = struct.unpack_from("<IIIII", data, co)
        ro_off = v2o(dp & ~3)
        if ro_off is None: continue
        fl, ist, isz, il, np, bm, bp, iv, w, bps = struct.unpack_from(
            "<IIIIIIIIII", data, ro_off)
        name = rstr(np)
        ivc = 0
        if iv:
            ivo = v2o(iv)
            _es, ivc = struct.unpack_from("<II", data, ivo)
        rows.append((name, isz, ivc, iv))
    rows.sort(key=lambda r: r[1], reverse=True)
    print(f"{'class':45} {'inst_size':>9} {'ivars':>6} {'ivar_list_vma':>14}")
    for n, sz, c, iv in rows:
        print(f"{n:45} {sz:9} {c:6} {iv:#014x}")


if __name__ == "__main__":
    main()
