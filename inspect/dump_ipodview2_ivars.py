"""Extract iPodView2's ivar layout from the Mach-O.

The Obj-C runtime metadata in Mach-O has this chain:
  __objc_classlist -> class_t -> class_ro_t -> ivar_list_t -> entries

Each ivar entry contains:
  offset_ptr (-> uint32 offset)
  name (-> cstring)
  type (-> cstring, Obj-C type encoding)
  alignment (uint32)
  size (uint32)

We dump iPodView2's ivars so we can pick which ones to inspect at runtime.
"""
import struct
from macholib.MachO import MachO

BIN = r"F:\ios_emu\inspect\Payload\S.S.Encore.app\S.S.Encore"
TARGET_CLASSES = {
    "iPodView2",
    "IPDMediaPickerLoadingView",
    "GLLoadingView",
    "ViewManager",
    "MainView",
    "IPDMediaPickerController",
}


def main():
    m = MachO(BIN)
    arm_header = next(h for h in m.headers if h.header.cputype == 12)
    with open(BIN, "rb") as f:
        data = f.read()

    sections = {}
    for lc, cmd, sects in arm_header.commands:
        if not hasattr(cmd, "segname"):
            continue
        seg = cmd.segname.rstrip(b"\x00").decode()
        for s in sects:
            name = s.sectname.rstrip(b"\x00").decode()
            sections[(seg, name)] = s

    def vmaddr_to_off(vma):
        for s in sections.values():
            if s.addr <= vma < s.addr + s.size:
                return arm_header.offset + s.offset + (vma - s.addr)
        return None

    def read_u32(vma):
        off = vmaddr_to_off(vma)
        if off is None: return None
        return struct.unpack_from("<I", data, off)[0]

    def read_cstr(vma):
        off = vmaddr_to_off(vma)
        if off is None: return None
        end = data.find(b"\x00", off)
        return data[off:end].decode("ascii", errors="replace")

    classlist = sections.get(("__DATA", "__objc_classlist"))
    if classlist is None:
        print("no __objc_classlist section"); return

    cl_off = arm_header.offset + classlist.offset
    cl_size = classlist.size
    class_ptrs = [
        struct.unpack_from("<I", data, cl_off + i)[0]
        for i in range(0, cl_size, 4)
    ]
    print(f"found {len(class_ptrs)} classes in __objc_classlist")

    # class_t layout (32-bit):
    #   isa (4) | superclass (4) | cache (4) | vtable (4) | data (4) = 20 bytes
    # data ptr & ~3 -> class_ro_t
    # class_ro_t layout:
    #   flags (4) | instanceStart (4) | instanceSize (4) | ivarLayout (4) |
    #   name (4) | baseMethods (4) | baseProtocols (4) | ivars (4) |
    #   weakIvarLayout (4) | baseProperties (4)
    # ivar_list_t layout:
    #   entsize (4) | count (4) | entries[count]
    # ivar_t layout:
    #   offset (4, pointer to a uint32)
    #   name (4) | type (4) | alignment (4) | size (4) = 20 bytes per entry

    for class_vma in class_ptrs:
        class_off = vmaddr_to_off(class_vma)
        if class_off is None: continue
        isa, super_, cache, vt, data_ptr = struct.unpack_from(
            "<IIIII", data, class_off)
        ro_ptr = data_ptr & ~3
        ro_off = vmaddr_to_off(ro_ptr)
        if ro_off is None: continue
        (flags, inst_start, inst_size, ivar_layout, name_ptr,
         base_methods, base_protos, ivars_ptr, weak_iv,
         base_props) = struct.unpack_from("<IIIIIIIIII", data, ro_off)
        name = read_cstr(name_ptr) or ""
        if name not in TARGET_CLASSES: continue

        print(f"\n=== class {name} ===")
        print(f"  class_vma={class_vma:#x} class_ro_vma={ro_ptr:#x}")
        print(f"  instanceStart={inst_start} instanceSize={inst_size}")
        print(f"  ivar_list_vma={ivars_ptr:#x}")
        if ivars_ptr == 0:
            print("  no ivars"); continue
        ivl_off = vmaddr_to_off(ivars_ptr)
        ent_size, count = struct.unpack_from("<II", data, ivl_off)
        print(f"  ivar entries: count={count} entsize={ent_size}")
        for i in range(count):
            eo = ivl_off + 8 + i * ent_size
            off_ptr, iname_p, itype_p, align, sz = struct.unpack_from(
                "<IIIII", data, eo)
            iname = read_cstr(iname_p) or "?"
            itype = read_cstr(itype_p) or "?"
            offset_value = read_u32(off_ptr)
            print(f"    +0x{offset_value:04x}  size={sz:3}  "
                  f"type={itype!r}  name={iname!r}")


if __name__ == "__main__":
    main()
