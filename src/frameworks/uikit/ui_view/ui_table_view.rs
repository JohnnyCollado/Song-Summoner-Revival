/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `UITableView`, `UITableViewCell` and `UITableViewController`.
//!
//! Just enough to make apps that drive a song-list–style table (notably
//! Song Summoner's `IPDMediaPickerController`) produce a visible list of
//! rows the user can tap. No scrolling, no cell reuse, no headers, no
//! editing. Single-section by default.

use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil,
    objc_classes, release, retain, ClassExports, NSZonePtr, SEL,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Song Summoner HUD palette. The picker is touchHLE-rendered (the game's
/// custom cell drawing is Core Graphics-text we don't emulate), so we paint
/// it in the game's own dark-teal / cyan style instead of stock white iPod
/// chrome. Values picked to match the in-game confirmation panel.
/// Per-tick scene observer (read-only). Fired from `ns_run_loop.rs` once
/// per main-loop iteration. Tracks scene-table entry 5 (the iPod scene),
/// MainLoop state vars, and logs every time anything changes — plus
/// unconditionally on every transition into one of the iPod scene's
/// interesting state values {2, 5, 6, 9, 15, 30, 31}.
///
/// Purpose: catch the EXACT tick at which `scene+0xc` reaches 9 on pick 1
/// (when the panel build fires) and compare what's different on pick 2.
///
/// Reads (no writes):
/// - Current scene index (0x1136b0)
/// - Scene table entry 5's scene_ptr (at 0x1134f0 + 5*0x1c + 0x18)
/// - scene_ptr+0xc (scene state)
/// - MainLoop_state global ptr (at *0xc48d0):
///   - +0xc iPodState, +0x10 iPodCancel, +0x14/+0x18 iPodMusicID
pub fn observe_ipod_scene_tick(env: &mut crate::Environment) {
    use std::sync::Mutex;

    #[derive(Default, Copy, Clone, PartialEq, Eq)]
    struct Snapshot {
        current_idx: u32,
        entry5_scene_ptr: u32,
        scene_state: u32,
        ipod_state: u32,
        ipod_cancel: u32,
        ipod_music_id_lo: u32,
        ipod_music_id_hi: u32,
        tick: u64,
    }
    static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);

    const SCENE_TABLE_BASE: u32 = 0x1134f0;
    const SCENE_INDEX_ADDR: u32 = 0x1136b0;
    const ENTRY5_VM: u32 = SCENE_TABLE_BASE + 5 * 0x1c;
    const ENTRY5_SCENE_PTR_VM: u32 = ENTRY5_VM + 0x18;
    const MAINLOOP_GLOBAL_PTR: u32 = 0xc48d0;

    // CORRECTED: 0xc48d0 IS the MainLoop struct base — NOT a pointer to it.
    // The setter MainLoop_Set_iPodState at 0x322c does:
    //   ldr r3, [pc, #4]   -> r3 = 0xc48d0 (literal value)
    //   str r0, [r3, #0xc] -> store directly at 0xc48dc
    // No indirection. Read iPodState directly from 0xc48dc.
    //
    // We don't need an early-MainLoop guard — the struct exists from boot
    // (it's in __bss). We just need to wait for entry5 to have a non-null
    // scene pointer.
    let idx_ptr: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(SCENE_INDEX_ADDR);
    let current_idx: u32 = env.mem.read(idx_ptr);
    let sp_ptr: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(ENTRY5_SCENE_PTR_VM);
    let entry5_scene_ptr: u32 = env.mem.read(sp_ptr);
    if entry5_scene_ptr == 0 {
        return;
    }
    let scene_state: u32 = if entry5_scene_ptr != 0 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(entry5_scene_ptr + 0xc);
        env.mem.read(p)
    } else {
        u32::MAX
    };
    // MainLoop struct is at MAINLOOP_GLOBAL_PTR directly (no indirection).
    let p_state: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0xc);
    let p_cancel: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x10);
    let p_id_lo: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x14);
    let p_id_hi: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x18);
    let ipod_state: u32 = env.mem.read(p_state);
    let ipod_cancel: u32 = env.mem.read(p_cancel);
    let ipod_music_id_lo: u32 = env.mem.read(p_id_lo);
    let ipod_music_id_hi: u32 = env.mem.read(p_id_hi);

    // Also read the iPodView2 / animation-state singleton at 0xc7890 (the
    // global that 0xa800 manipulates). Field +0x00 is an "in-progress"
    // flag, +0xc is suspected "panel built" or similar, +0x14 is an
    // animation handle passed to 0xc57c. We track these to see whether
    // the natural No flow clears them — if so, calling 0xa800(0) on No
    // tap is the minimal cleanup we need.
    const IPODVIEW_GLOBAL: u32 = 0xc7890;
    let ipv_f00: u32 = {
        let p: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(IPODVIEW_GLOBAL);
        env.mem.read(p)
    };
    let ipv_f0c: u32 = {
        let p: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(IPODVIEW_GLOBAL + 0xc);
        env.mem.read(p)
    };
    let ipv_f14: u32 = {
        let p: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(IPODVIEW_GLOBAL + 0x14);
        env.mem.read(p)
    };

    // Read additional scene header fields for transition-edge analysis:
    //   +0x00 — scene type/vtable selector (constant 0x0e for iPod scene)
    //   +0x04 — flag/refcount (typically 0)
    //   +0x08 — PREV_STATE cookie (this is the suspected gate field for
    //           the panel-build sub-predicate: state==6 && prev_state==3
    //           fires the rebuild on the transition edge, not in idle)
    //   +0x0c — current scene state (already captured above)
    let scene_field_00: u32 = if entry5_scene_ptr != 0 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(entry5_scene_ptr + 0x00);
        env.mem.read(p)
    } else { 0 };
    let scene_field_04: u32 = if entry5_scene_ptr != 0 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(entry5_scene_ptr + 0x04);
        env.mem.read(p)
    } else { 0 };
    let scene_field_08: u32 = if entry5_scene_ptr != 0 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(entry5_scene_ptr + 0x08);
        env.mem.read(p)
    } else { 0 };

    let mut g = LAST.lock().unwrap();
    let tick = g.map(|p| p.tick + 1).unwrap_or(0);
    let snap = Snapshot {
        current_idx,
        entry5_scene_ptr,
        scene_state,
        ipod_state,
        ipod_cancel,
        ipod_music_id_lo,
        ipod_music_id_hi,
        tick,
    };
    let interesting = matches!(scene_state, 2 | 3 | 5 | 6 | 9 | 15 | 30 | 31);
    let changed = match *g {
        None => true,
        Some(prev) => {
            prev.entry5_scene_ptr != snap.entry5_scene_ptr
                || prev.scene_state != snap.scene_state
                || prev.ipod_state != snap.ipod_state
                || prev.ipod_cancel != snap.ipod_cancel
                || prev.ipod_music_id_lo != snap.ipod_music_id_lo
                || prev.ipod_music_id_hi != snap.ipod_music_id_hi
                || prev.current_idx != snap.current_idx
        }
    };
    let star = match scene_state {
        6 => " **STATE_6 (panel-build)**",
        9 => " *STATE_9 (init)*",
        s if matches!(s, 2 | 3 | 5 | 15 | 30 | 31) => " *iPod*",
        _ => "",
    };
    if changed {
        log!(
            "[ipod_observer tick={}] idx={} entry5.scene={:#x} +0x00={:#x} +0x04={:#x} +0x08={} +0x0c={}{} | iPodState={} iPodCancel={} iPodMusicID={:08x}{:08x} | [0xc7890]+0x00={:#x} +0x0c={:#x} +0x14={:#x}",
            tick, current_idx, entry5_scene_ptr,
            scene_field_00, scene_field_04, scene_field_08,
            scene_state, star,
            ipod_state, ipod_cancel, ipod_music_id_hi, ipod_music_id_lo,
            ipv_f00, ipv_f0c, ipv_f14
        );
        // Whenever scene state lands on 6 (panel-build coarse state) OR
        // on 9 (constructor init value), dump the full +0x00..+0x3c
        // window so we can compare the substate cookies across picks.
        if (scene_state == 6 || scene_state == 9) && entry5_scene_ptr != 0 {
            let mut fields = [0u32; 16];
            for k in 0..16u32 {
                let p: crate::mem::ConstPtr<u32> =
                    crate::mem::Ptr::from_bits(entry5_scene_ptr + k * 4);
                fields[k as usize] = env.mem.read(p);
            }
            log!(
                "[ipod_observer tick={} STATE_{}_DUMP] scene+0x00..+0x3c=[{:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}]",
                tick, scene_state,
                fields[0], fields[1], fields[2], fields[3],
                fields[4], fields[5], fields[6], fields[7],
                fields[8], fields[9], fields[10], fields[11],
                fields[12], fields[13], fields[14], fields[15],
            );
        }
    }
    *g = Some(snap);
}

/// Force the iPod scene's tuple of (type_id, prev_state, state) to the
/// known-good "fresh panel build" combination that Pick 1 was observed to
/// have when its +songsQuery fired through the full cleanup-then-build
/// branch of the scene render at 0x17800.
///
/// Pick 1 observed values at the moment +songsQuery fired:
///   scene+0x00 = 0x0e  (scene type ID)
///   scene+0x08 = 3     (prev_state cookie)
///   scene+0x0c = 6     (current state — the STABLE IDLE state, NOT the
///                       transient-init state 9)
///
/// Pick 2 (without this patch) naturally lands at:
///   scene+0x00 = 0x11, scene+0x08 = 1, scene+0x0c = 31
/// State 31 IS a +songsQuery-firing branch, but it's the "refresh / reuse"
/// path that doesn't include the sprite-list cleanup. Result: new metadata
/// is queried and rendered, but old OpenGL sprites/textures remain stacked
/// underneath. Visible duplication.
///
/// Rationale for writing +0x0c=6 in addition to the cookies:
///   - State 6 is observed as a STABLE idle state (the FSM sits at 6 between
///     events — see log line 8072 `STATE_6_DUMP` at idle). It is NOT a
///     transient-only state like 9 (which locked up when we tried forcing).
///   - State 6's branch in 0x17800 includes `bl 0xa800(0)` at 0x18142 — a
///     reset call that clears flags on the iPodView2 singleton (global at
///     0xc7890). The state-31 branch likely skips this prologue.
///   - Routing the FSM into state 6 lets the natural build sequence run,
///     which includes both cleanup and rebuild — exactly what Pick 1 did.
/// Call the game's own animation/panel reset function (0xa800) with arg=0.
/// This mimics what the natural No→idle lifecycle does:
///   - r4 = global at 0xc7890 (iPodView2 / animation singleton)
///   - calls 0xc57c (stop current animation) with r4[+0x14] as handle
///   - if arg <= 0 (our case, 0):
///       r4[+0x0c] = 0
///       r4[+0x00] = 0
///       calls 0xa788 (state-readback)
///       calls 0xc57c(r4[+0x14], 0) again
///
/// The observer shows [0xc7890]+0x0c flips 0xff → 0 during natural idle
/// transitions. We bypass that natural flip by jumping straight from
/// confirmation → next pick, which leaves stale GL panel sprites visible.
/// Calling 0xa800(0) on No tap restores the natural cleanup.
///
/// Logs +0x00..+0x40 of the singleton before/after so we can see what got
/// cleared (and identify any additional fields if cleanup is incomplete).
/// GL texture lifecycle tracker. Called from gles_guest.rs hooks for
/// glGenTextures / glDeleteTextures. Goal: see whether confirmation-panel
/// rebuilds on pick 2+ generate fresh textures WITHOUT deleting the old
/// ones — that's the leak driving the visible duplication.
///
/// We tag each gen call with the current "generation" (incremented by
/// `bump_picker_generation()` on each picker-swap dispatch). On delete,
/// we log which generation the freed texture came from. After a few
/// picks: textures from gen 0 that are still alive when gen 2 is gen'ing
/// new ones = the leaked panel sprites.
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

struct GlTextureTracker {
    /// Texture ID → generation it was created during.
    live: HashMap<u32, u32>,
    current_generation: u32,
    /// Counts per-generation: how many gen'd, how many deleted.
    gen_count: HashMap<u32, (u32, u32)>,
    /// Per-generation set of texture IDs still alive from that gen.
    /// Used by `take_prev_gen_ids_for_cleanup()` to harvest the leaked
    /// confirmation-panel textures from prior picker cycles.
    gen_live: HashMap<u32, Vec<u32>>,
}

static GL_TRACKER: StdMutex<Option<GlTextureTracker>> = StdMutex::new(None);

fn ensure_tracker(g: &mut std::sync::MutexGuard<Option<GlTextureTracker>>) {
    if g.is_none() {
        **g = Some(GlTextureTracker {
            live: HashMap::new(),
            current_generation: 0,
            gen_count: HashMap::new(),
            gen_live: HashMap::new(),
        });
    }
}

pub fn track_gl_textures_gen(ids: &[u32]) {
    let mut g = GL_TRACKER.lock().unwrap();
    ensure_tracker(&mut g);
    let t = g.as_mut().unwrap();
    let gen = t.current_generation;
    for &id in ids {
        t.live.insert(id, gen);
        t.gen_live.entry(gen).or_insert_with(Vec::new).push(id);
    }
    let e = t.gen_count.entry(gen).or_insert((0, 0));
    e.0 += ids.len() as u32;
    let live_count = t.live.len();
    log!(
        "[gl_tex] gen +{} (gen={}) | new IDs={:?} | live total={}",
        ids.len(), gen, ids, live_count
    );
}

pub fn track_gl_textures_delete(ids: &[u32]) {
    let mut g = GL_TRACKER.lock().unwrap();
    ensure_tracker(&mut g);
    let t = g.as_mut().unwrap();
    let mut deleted_from_gen: HashMap<u32, u32> = HashMap::new();
    for &id in ids {
        if let Some(src_gen) = t.live.remove(&id) {
            *deleted_from_gen.entry(src_gen).or_insert(0) += 1;
            let e = t.gen_count.entry(src_gen).or_insert((0, 0));
            e.1 += 1;
            // Remove from per-gen live list.
            if let Some(v) = t.gen_live.get_mut(&src_gen) {
                if let Some(pos) = v.iter().position(|&x| x == id) {
                    v.swap_remove(pos);
                }
            }
        } else {
            *deleted_from_gen.entry(u32::MAX).or_insert(0) += 1;
        }
    }
    let live_count = t.live.len();
    log!(
        "[gl_tex] del -{} | deleted from gens={:?} | live total={}",
        ids.len(), deleted_from_gen, live_count
    );
}

/// Harvest all GL texture IDs still alive from generations 1..current_gen.
/// Gen 0 is excluded — those are setup textures for the picker / library
/// that we must not free. Returns the IDs and removes them from our
/// tracker (caller will pass them to glDeleteTextures).
pub fn take_prev_gen_panel_textures() -> Vec<u32> {
    let mut g = GL_TRACKER.lock().unwrap();
    ensure_tracker(&mut g);
    let t = g.as_mut().unwrap();
    let current = t.current_generation;
    let mut ids = Vec::new();
    let gens_to_clean: Vec<u32> = t.gen_live.keys()
        .copied()
        .filter(|&gen| gen > 0 && gen < current)
        .collect();
    for gen in gens_to_clean {
        if let Some(v) = t.gen_live.remove(&gen) {
            for id in &v {
                t.live.remove(id);
            }
            ids.extend(v);
        }
    }
    ids
}

/// Mark a new picker-swap generation. Call from the dispatch in the
/// touch handler so we can correlate texture lifecycle with picks.
pub fn bump_picker_generation() {
    let mut g = GL_TRACKER.lock().unwrap();
    ensure_tracker(&mut g);
    let t = g.as_mut().unwrap();
    t.current_generation += 1;
    let live_count = t.live.len();
    // Dump per-generation counts so far.
    let mut counts: Vec<(u32, (u32, u32))> = t.gen_count.iter().map(|(k, v)| (*k, *v)).collect();
    counts.sort_by_key(|(k, _)| *k);
    log!(
        "[gl_tex] === PICKER GENERATION {} starts | live total={} | per-gen (gen, gen'd, del'd)={:?}",
        t.current_generation, live_count, counts
    );
}

/// Surgical iPod-scene sprite cleanup.
///
/// The iPod scene struct at entry 5 stores indices into the global 1024-
/// entry render-object array (the same one 0xd2c8 iterates). Indices live
/// at scene+0x10 through scene+0x3c. Non-(-1) non-0 values are live entry
/// IDs whose backing texture refs need releasing.
///
/// We call destroy_at_index (game function 0xc708) on each, which:
///   - clears the entry's status flag
///   - decrements its texture refcount via release_texture_ref (0xd290)
///   - calls glDeleteTextures(1, &id) when refcount hits 0
///
/// Then we write -1 to each cleared slot so the scene's own cleanup pass
/// (which runs every frame after counter > 5) sees them as already gone.
///
/// This is the same cleanup the render function at 0x17800 does one slot
/// per frame after counter > 5 — we just do all slots in one go before
/// the next pick rebuilds the panel.
/// Call the game's "destroy all render-pool entries" sweep at 0xd2c8.
///
/// This is the same function that runs during natural scene teardown
/// (verified in earlier logs: a Back→re-enter cycle triggered a burst of
/// glDeleteTextures that came through this path). It iterates all 1024
/// entries in the global render-object array and calls release_texture_ref
/// on each that has a non-zero status field.
///
/// Risk: this is GLOBAL — it'll also free non-iPod-scene sprites if any
/// are alive (picker buttons, background, etc.). On a real device the
/// natural teardown is OK because it happens during a scene transition
/// when the picker is being torn down anyway. We're calling it mid-picker
/// session, so anything else live in the pool will get its texture
/// released too. If the picker re-creates its own sprites on the next
/// frame, this is fine. If not, picker visuals will break.
pub fn invoke_destroy_all_render_entries(env: &mut crate::Environment) {
    use crate::abi::{CallFromHost, GuestFunction};
    let destroy_all = GuestFunction::from_addr_and_thumb_flag(0xd2c8, true);
    log!("invoke_destroy_all_render_entries: calling 0xd2c8 (global sweep)");
    let _: () = destroy_all.call_from_host(env, ());
    log!("invoke_destroy_all_render_entries: 0xd2c8 returned");
}

pub fn invoke_ipodview_reset(env: &mut crate::Environment) {
    const IPODVIEW_GLOBAL: u32 = 0xc7890;
    let mut before = [0u32; 16];
    for k in 0..16u32 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(IPODVIEW_GLOBAL + k * 4);
        before[k as usize] = env.mem.read(p);
    }
    use crate::abi::{CallFromHost, GuestFunction};
    let reset_fn = GuestFunction::from_addr_and_thumb_flag(0xa800, true);
    let _: () = reset_fn.call_from_host(env, (0i32,));
    let mut after = [0u32; 16];
    for k in 0..16u32 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(IPODVIEW_GLOBAL + k * 4);
        after[k as usize] = env.mem.read(p);
    }
    log!(
        "invoke_ipodview_reset: 0xa800(0) called. [0xc7890]+0x00..+0x3c\n  BEFORE: [{:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}]\n  AFTER:  [{:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}]",
        before[0], before[1], before[2], before[3], before[4], before[5], before[6], before[7],
        before[8], before[9], before[10], before[11], before[12], before[13], before[14], before[15],
        after[0], after[1], after[2], after[3], after[4], after[5], after[6], after[7],
        after[8], after[9], after[10], after[11], after[12], after[13], after[14], after[15],
    );
}

pub fn force_panel_rebuild_predicate(env: &mut crate::Environment) {
    const SCENE_TABLE_BASE: u32 = 0x1134f0;
    const ENTRY5_SCENE_PTR_VM: u32 = SCENE_TABLE_BASE + 5 * 0x1c + 0x18;
    let sp: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(ENTRY5_SCENE_PTR_VM);
    let scene_ptr: u32 = env.mem.read(sp);
    if scene_ptr == 0 {
        log!("force_panel_rebuild_predicate: entry5 scene_ptr is null; skipping");
        return;
    }
    let p00: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x00);
    let p04: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x04);
    let p08: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x08);
    let p0c: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x0c);
    let before_00 = env.mem.read(p00);
    let before_04 = env.mem.read(p04);
    let before_08 = env.mem.read(p08);
    let before_0c = env.mem.read(p0c);
    let w00: crate::mem::MutPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x00);
    let w08: crate::mem::MutPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x08);
    let w0c: crate::mem::MutPtr<u32> = crate::mem::Ptr::from_bits(scene_ptr + 0x0c);
    env.mem.write(w00, 0x0eu32);
    env.mem.write(w08, 3u32);
    // EXPERIMENT: write +0x0c = 0 (counter reset, fresh-scene-like start)
    // instead of 6 (rewind-from-idle to build keyframe). The fresh scene
    // 0x30022bd0 was observed at counter=1 just after construction and
    // fired +songsQuery at counter=3. Fresh-scene init path likely
    // includes sprite-list reset that the rewind-to-6 path skips. By
    // writing 0, the counter starts over and naturally hits frames
    // 1, 2, 3 — invoking the same keyframe handlers that brand-new
    // scenes execute.
    env.mem.write(w0c, 0u32);
    let after_00 = env.mem.read(p00);
    let after_08 = env.mem.read(p08);
    let after_0c = env.mem.read(p0c);
    log!(
        "force_panel_rebuild_predicate: scene={:#x} BEFORE +0x00={:#x} +0x04={:#x} +0x08={} +0x0c={} AFTER +0x00={:#x} +0x08={} +0x0c={}",
        scene_ptr, before_00, before_04, before_08, before_0c, after_00, after_08, after_0c
    );
}

/// Snapshot every UILabel and UIImageView under the keyWindow. For each,
/// log its class, frame, superview's class, and text/image presence.
/// Used to detect duplicate-UI accumulation across pick cycles.
pub fn snapshot_panel_views(env: &mut crate::Environment, phase: &str) {
    use crate::frameworks::core_graphics::CGRect;
    use crate::frameworks::foundation::NSUInteger;
    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window == nil {
        log!("[panel_snap {}] no keyWindow", phase);
        return;
    }
    let mut labels: Vec<(id, String, CGRect, String)> = Vec::new();
    let mut images: Vec<(id, bool, CGRect, String)> = Vec::new();
    let mut stack: Vec<(id, id)> = vec![(window, nil)];
    while let Some((v, parent)) = stack.pop() {
        if v == nil || env.objc.get_host_object(v).is_none() {
            continue;
        }
        let cls: crate::objc::Class = msg![env; v class];
        let cname = env.objc.get_class_name(cls).to_string();
        let parent_cname = if parent != nil && env.objc.get_host_object(parent).is_some() {
            let pc: crate::objc::Class = msg![env; parent class];
            env.objc.get_class_name(pc).to_string()
        } else {
            "(none)".to_string()
        };
        if cname == "UILabel" || cname.ends_with("Label") {
            let text: id = msg![env; v text];
            let rust_text = if text != nil {
                crate::frameworks::foundation::ns_string::to_rust_string(env, text).to_string()
            } else {
                String::new()
            };
            let frame: CGRect = msg![env; v frame];
            labels.push((v, rust_text, frame, parent_cname.clone()));
        } else if cname == "UIImageView" || cname.ends_with("ImageView") {
            let image: id = msg![env; v image];
            let frame: CGRect = msg![env; v frame];
            images.push((v, image != nil, frame, parent_cname.clone()));
        }
        let subviews: id = msg![env; v subviews];
        if subviews != nil {
            let n: NSUInteger = msg![env; subviews count];
            for i in 0..n {
                let child: id = msg![env; subviews objectAtIndex:i];
                if child != nil {
                    stack.push((child, v));
                }
            }
        }
    }
    log!(
        "[panel_snap {}] {} labels, {} imageviews",
        phase, labels.len(), images.len()
    );
    for (v, text, frame, parent) in &labels {
        let (x, y, w, h) = (frame.origin.x, frame.origin.y, frame.size.width, frame.size.height);
        log!(
            "  LABEL {:?} parent={} frame=({:.0},{:.0},{:.0},{:.0}) text={:?}",
            v, parent, x, y, w, h, text
        );
    }
    for (v, has_image, frame, parent) in &images {
        let (x, y, w, h) = (frame.origin.x, frame.origin.y, frame.size.width, frame.size.height);
        log!(
            "  IMAGEVIEW {:?} parent={} frame=({:.0},{:.0},{:.0},{:.0}) has_image={}",
            v, parent, x, y, w, h, has_image
        );
    }
}

/// Recursively walk the keyWindow's view tree and return the first view
/// whose class name matches `class_name`. Returns nil if not found.
/// Used to locate the iPodView2 instance for picker-swap dispatch when the
/// IPDSongsTab early-exits its didSelectRow on pick 2+.
fn find_view_by_class(env: &mut crate::Environment, class_name: &str) -> id {
    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window == nil {
        return nil;
    }
    let mut stack: Vec<id> = vec![window];
    while let Some(v) = stack.pop() {
        if v == nil || env.objc.get_host_object(v).is_none() {
            continue;
        }
        let cls: crate::objc::Class = msg![env; v class];
        if env.objc.get_class_name(cls) == class_name {
            return v;
        }
        let subviews: id = msg![env; v subviews];
        if subviews != nil {
            let n: crate::frameworks::foundation::NSUInteger =
                msg![env; subviews count];
            for i in 0..n {
                let child: id = msg![env; subviews objectAtIndex:i];
                if child != nil {
                    stack.push(child);
                }
            }
        }
    }
    nil
}

fn ss_color_bg_dark(env: &mut crate::Environment) -> id {
    msg_class![env; UIColor colorWithRed:0.039f32 green:0.118f32 blue:0.149f32 alpha:1.0f32]
}
fn ss_color_cell_bg(env: &mut crate::Environment) -> id {
    msg_class![env; UIColor colorWithRed:0.063f32 green:0.157f32 blue:0.196f32 alpha:1.0f32]
}
fn ss_color_panel_bg(env: &mut crate::Environment) -> id {
    msg_class![env; UIColor colorWithRed:0.094f32 green:0.196f32 blue:0.243f32 alpha:1.0f32]
}
fn ss_color_text_bright(env: &mut crate::Environment) -> id {
    msg_class![env; UIColor colorWithRed:0.553f32 green:0.898f32 blue:0.937f32 alpha:1.0f32]
}
fn ss_color_text_dim(env: &mut crate::Environment) -> id {
    msg_class![env; UIColor colorWithRed:0.349f32 green:0.612f32 blue:0.671f32 alpha:1.0f32]
}
fn ss_color_btn_primary(env: &mut crate::Environment) -> id {
    // Highlighted-confirm tone — used for the "Y / Create" affirm button.
    msg_class![env; UIColor colorWithRed:0.157f32 green:0.314f32 blue:0.376f32 alpha:1.0f32]
}
fn ss_color_btn_secondary(env: &mut crate::Environment) -> id {
    // Dim-back tone — used for the "N / Cancel" button.
    msg_class![env; UIColor colorWithRed:0.078f32 green:0.176f32 blue:0.220f32 alpha:1.0f32]
}

/// Pointer to the currently-promoted large UITableView (the Songs picker).
/// Used to determine whether the synthetic tab bar should be visible: the
/// bar is shown only while this table is still a direct subview of the key
/// window, and hidden once the game dismisses the picker.
pub static PROMOTED_TABLE: Mutex<Option<u32>> = Mutex::new(None);

/// Deferred picker-swap dispatch. On a row tap we immediately flip the
/// game's iPodState to 1 (so the next render tick observes a state≠4 edge),
/// stash the dispatch parameters here, and return. On the next NSRunLoop
/// tick (drained from `media_player::handle_players` proximity) we execute
/// the dispatch — which causes `didPickMediaNumber:` to write state=4,
/// giving the render observer a real 1→4 transition each pick instead of
/// the same-frame 4→4 no-op that broke pick 2+.
pub struct PendingPickerSwap {
    pub row: crate::frameworks::foundation::NSUInteger,
    pub pid: u64,
    pub songs_table_bits: u32,
    pub songs_delegate_bits: u32,
    pub promoted_table_bits: u32,
    pub target_idx_bits: u32,
    /// Number of additional drain ticks to skip before firing. We use 1 so
    /// there's guaranteed to be at least one render tick BETWEEN our state=1
    /// write (in the touch handler) and the dispatch that calls
    /// `didPickMediaNumber:` (writing state=4). Without the gap, both writes
    /// happen within a single iteration and the render observer only sees
    /// the final value.
    pub frames_to_wait: u32,
}
pub static PENDING_PICKER_SWAP: Mutex<Option<PendingPickerSwap>> = Mutex::new(None);

/// Tear down the currently-promoted Songs picker: remove its UITableView and
/// synthetic tab bar from the key window, clear the per-host
/// `promoted_to_window` flag so the next picker session re-promotes a fresh
/// table, and clear [PROMOTED_TABLE]. Used by ui_touch when the user commits
/// the song via "Create Trooper" -- the game advances past the picker, so the
/// drill UI should go away cleanly rather than linger behind the next scene.
/// Aggressive picker re-mount used when the user taps "No" on the
/// confirmation panel. Re-runs the core of the promotion logic on the
/// currently-stashed [PROMOTED_TABLE]: yanks it off whatever superview the
/// game's layout chain put it back on, re-parents it onto the key window,
/// resets frame/bounds, flushes the cell cache, and re-builds the
/// synthetic tab bar. This is the equivalent of "promote from scratch"
/// without going through `reloadData` again, and it's deliberately
/// idempotent — calling it twice in a row is safe.
///
/// Returns true if a picker was actually re-mounted, false if there was
/// no [PROMOTED_TABLE] to act on.
pub fn remount_promoted_picker(env: &mut crate::Environment) -> bool {
    let promoted_bits = { *PROMOTED_TABLE.lock().unwrap() };
    let Some(bits) = promoted_bits else { return false };
    let table = crate::objc::id::from_bits(bits);
    if env.objc.get_host_object(table).is_none() {
        return false;
    }
    let app: crate::objc::id =
        msg_class![env; UIApplication sharedApplication];
    let window: crate::objc::id = msg![env; app keyWindow];
    if window == crate::objc::nil {
        return false;
    }

    // Pull the table off its current parent (whatever it is) and put it
    // back as a direct child of the window. Defensive against the game
    // having re-parented it during the confirmation flow.
    () = msg![env; table removeFromSuperview];
    () = msg![env; window addSubview:table];

    // Reset frame + landscape bounds — same values the promotion block uses.
    let screen: crate::objc::id = msg_class![env; UIScreen mainScreen];
    let sb: CGRect = msg![env; screen bounds];
    let full = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: sb.size,
    };
    () = msg![env; table setFrame:full];
    let landscape_bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: sb.size.height,
            height: sb.size.width,
        },
    };
    () = msg![env; table setBounds:landscape_bounds];

    // Re-apply state and styling.
    {
        let host = env.objc.borrow_mut::<UITableViewHostObject>(table);
        host.promoted_to_window = true;
        host.permanently_dismissed = false;
    }
    let bg: id = ss_color_bg_dark(env);
    () = msg![env; table setBackgroundColor:bg];
    () = msg![env; table setHidden:false];

    // Flush any cells that were laid out under stale bounds or are now
    // detached, then re-layout against current bounds.
    let old_cells = std::mem::take(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(table).cells_by_row,
    );
    for (_, cell) in old_cells {
        () = msg![env; cell removeFromSuperview];
        release(env, cell);
    }
    layout_visible_cells(env, table);

    // Rebuild the tab bar — remove any existing one first so a stale bar
    // from a previous promotion doesn't linger underneath.
    const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;
    let existing_bar: crate::objc::id =
        msg![env; window viewWithTag:TAB_BAR_TAG];
    if existing_bar != crate::objc::nil {
        () = msg![env; existing_bar removeFromSuperview];
    }
    build_synthetic_tab_bar(env, window);

    () = msg![env; window bringSubviewToFront:table];
    let bar: crate::objc::id = msg![env; window viewWithTag:TAB_BAR_TAG];
    if bar != crate::objc::nil {
        () = msg![env; window bringSubviewToFront:bar];
    }
    true
}

/// Pop the topmost view controller off the nav stack reachable through the
/// captured IPDSongsTab. Used by ui_touch on a "No" tap to remove the
/// confirmation VC that path A of IPDSongsTableVC.didSelectRow pushed on the
/// previous pick — so the next pick can re-enter path A (which alloc+init+
/// pushes a fresh VC, the only path that builds a refreshed panel).
pub fn pop_confirmation_nav_vc(env: &mut crate::Environment) {
    let ipd_songs_bits = *IPD_SONGS_TABLE.lock().unwrap();
    let Some(bits) = ipd_songs_bits else {
        log!("pop_confirmation_nav_vc: no IPD_SONGS_TABLE captured");
        return;
    };
    let songs_table = crate::objc::id::from_bits(bits);
    if env.objc.get_host_object(songs_table).is_none() {
        log!("pop_confirmation_nav_vc: songs_table host gone");
        return;
    }
    let songs_delegate: id = env
        .objc
        .borrow::<UITableViewHostObject>(songs_table)
        .delegate;
    if songs_delegate == nil || env.objc.get_host_object(songs_delegate).is_none() {
        log!("pop_confirmation_nav_vc: songs_delegate nil/gone");
        return;
    }
    let cls: crate::objc::Class = msg![env; songs_delegate class];
    let cname = env.objc.get_class_name(cls).to_string();
    let nav_sel: SEL = env
        .objc
        .register_host_selector("navigationController".to_string(), &mut env.mem);
    let responds_nav: bool = msg![env; songs_delegate respondsToSelector:nav_sel];
    let parent_sel: SEL = env
        .objc
        .register_host_selector("parentViewController".to_string(), &mut env.mem);
    let responds_parent: bool = msg![env; songs_delegate respondsToSelector:parent_sel];
    let nav: id = if responds_nav {
        msg![env; songs_delegate navigationController]
    } else {
        nil
    };
    let parent: id = if responds_parent {
        msg![env; songs_delegate parentViewController]
    } else {
        nil
    };
    log!(
        "pop_confirmation_nav_vc: delegate={:?} class={} responds_nav={} nav={:?} responds_parent={} parent={:?}",
        songs_delegate, cname, responds_nav, nav, responds_parent, parent
    );
    if nav != nil && env.objc.get_host_object(nav).is_some() {
        log!("Picker-swap: popping nav VC (nav={:?})", nav);
        let _: id = msg![env; nav popViewControllerAnimated:false];
    } else if parent != nil && env.objc.get_host_object(parent).is_some() {
        // Try the parent's nav controller as a fallback.
        let parent_nav: id = if msg![env; parent respondsToSelector:nav_sel] {
            msg![env; parent navigationController]
        } else {
            nil
        };
        if parent_nav != nil && env.objc.get_host_object(parent_nav).is_some() {
            log!(
                "Picker-swap: popping nav VC via parent (parent_nav={:?})",
                parent_nav
            );
            let _: id = msg![env; parent_nav popViewControllerAnimated:false];
        }
    }
}

/// Drain the deferred picker-swap dispatch queued by the touch handler.
/// Called once per NSRunLoop main-loop iteration from ns_run_loop.rs.
///
/// Edge-triggered transition fix: the touch handler set iPodState=1 and
/// stashed the dispatch info here. After `frames_to_wait` drain ticks, we
/// run the actual `tableView:didSelectRowAtIndexPath:` dispatch — which
/// causes the game's `iPodView2.mediaPicker:didPickMediaNumber:` to write
/// iPodState=4. The render observer then sees a real 1→4 transition (since
/// at least one render tick fired between the writes) and rebuilds the
/// confirmation panel with the new song's properties.
pub fn drain_pending_picker_swap(env: &mut crate::Environment) {
    // Take the pending entry if its wait counter has elapsed.
    let pending = {
        let mut guard = PENDING_PICKER_SWAP.lock().unwrap();
        let Some(p) = guard.as_mut() else { return };
        if p.frames_to_wait > 0 {
            p.frames_to_wait -= 1;
            return;
        }
        guard.take().unwrap()
    };

    let songs_table = crate::objc::id::from_bits(pending.songs_table_bits);
    let songs_delegate = crate::objc::id::from_bits(pending.songs_delegate_bits);
    let promoted_table = crate::objc::id::from_bits(pending.promoted_table_bits);
    let target_idx = crate::objc::id::from_bits(pending.target_idx_bits);

    if env.objc.get_host_object(songs_table).is_none()
        || env.objc.get_host_object(songs_delegate).is_none()
    {
        log!(
            "Picker-swap drain: stale objects (songs_table={:?} delegate={:?}); skipping",
            songs_table, songs_delegate
        );
        // Drop the retained NSIndexPath if it's still alive.
        if env.objc.get_host_object(target_idx).is_some() {
            release(env, target_idx);
        }
        return;
    }

    log!(
        "Picker-swap drain: dispatching row {} (PID {:016X}) on deferred tick",
        pending.row, pending.pid
    );

    // Purge our cell cache so cellForRowAtIndexPath: re-fires.
    {
        let old_cells = std::mem::take(
            &mut env.objc.borrow_mut::<UITableViewHostObject>(songs_table).cells_by_row,
        );
        for (_, cell) in old_cells {
            () = msg![env; cell removeFromSuperview];
            release(env, cell);
        }
    }

    let env_ptr = env as *mut crate::Environment;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || unsafe {
        let env = &mut *env_ptr;
        () = msg![env; songs_table reloadData];
    }));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || unsafe {
        let env = &mut *env_ptr;
        () = msg![env; songs_delegate tableView:songs_table didSelectRowAtIndexPath:target_idx];
    }));

    crate::frameworks::media_player::music_library::play_song(pending.row as usize);

    // Release the retain we placed when stashing the index path.
    if env.objc.get_host_object(target_idx).is_some() {
        release(env, target_idx);
    }

    // Hide the promoted picker so the confirmation panel underneath is visible.
    if env.objc.get_host_object(promoted_table).is_some() {
        () = msg![env; promoted_table setHidden:true];
        let app: crate::objc::id = msg_class![env; UIApplication sharedApplication];
        let window: crate::objc::id = msg![env; app keyWindow];
        if window != crate::objc::nil {
            const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;
            let bar: crate::objc::id = msg![env; window viewWithTag:TAB_BAR_TAG];
            if bar != crate::objc::nil {
                () = msg![env; bar setHidden:true];
            }
        }
    }
}

pub fn dismiss_promoted_picker(env: &mut crate::Environment) {
    let promoted_bits = { *PROMOTED_TABLE.lock().unwrap() };
    let Some(bits) = promoted_bits else { return };
    let table = crate::objc::id::from_bits(bits);
    // Clear the host flag while we still have a valid id.
    if env.objc.get_host_object(table).is_some() {
        let host = env.objc.borrow_mut::<UITableViewHostObject>(table);
        host.promoted_to_window = false;
        host.permanently_dismissed = true;
    }
    let app: crate::objc::id =
        msg_class![env; UIApplication sharedApplication];
    let window: crate::objc::id = msg![env; app keyWindow];
    if window != crate::objc::nil {
        const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;
        let bar: crate::objc::id = msg![env; window viewWithTag:TAB_BAR_TAG];
        if bar != crate::objc::nil {
            () = msg![env; bar removeFromSuperview];
        }
    }
    () = msg![env; table removeFromSuperview];
    *PROMOTED_TABLE.lock().unwrap() = None;
}

/// Pointer to the most-recent visible 5-row IPDSongsTab table. Used by the
/// picker-swap trick: when the user taps in the promoted 1309-row table,
/// we stage their song's PID + trigger reloadData on this table + dispatch
/// row-0 select on its delegate. The game's working pick handler then
/// picks the staged song.
pub(super) static IPD_SONGS_TABLE: Mutex<Option<u32>> = Mutex::new(None);

/// Cached UIImage for the per-cell album-art placeholder. Loaded lazily on
/// first use from `res/album_placeholder.png` (cwd-relative). Stored as a
/// guest `id` integer because `id` isn't `Send`; we treat it as a fire-and-
/// forget retain held for the life of the process.
static PLACEHOLDER_IMAGE: Mutex<Option<Result<u32, ()>>> = Mutex::new(None);

type UITableViewStyle = NSInteger; // 0 = plain, 1 = grouped
type UITableViewCellStyle = NSInteger;
type UITableViewCellSeparatorStyle = NSInteger;
type UITableViewCellAccessoryType = NSInteger;
type UITableViewCellSelectionStyle = NSInteger;
type UITableViewRowAnimation = NSInteger;
type UITableViewScrollPosition = NSInteger;

const DEFAULT_ROW_HEIGHT: f32 = 44.0;

// Tags identifying our synthetic Y/N pick-confirmation overlay views, used
// so UIControl's touchesEnded hook can recognise them and route to the
// pick-or-cancel handlers.
const PICK_PROMPT_TAG: crate::frameworks::foundation::NSInteger = 0x71_C_71_C;
const PICK_PROMPT_YES_TAG: crate::frameworks::foundation::NSInteger = 0x71_C_77;
const PICK_PROMPT_NO_TAG: crate::frameworks::foundation::NSInteger = 0x71_C_44;

/// Persistent ID of the song the user most recently tapped (and that the
/// Y/N prompt is currently asking about). Read by the Y-button handler so
/// it knows which song to pick.
pub(super) static STAGED_PICK_PID: Mutex<u64> = Mutex::new(0);

#[derive(Default)]
struct UITableViewHostObject {
    superclass: super::ui_scroll_view::UIScrollViewHostObject,
    /// Non-retaining (weak) reference per UIKit semantics.
    data_source: id,
    /// Non-retaining (weak) reference per UIKit semantics.
    delegate: id,
    row_height: f32,
    /// `UITableViewStyle`.
    style: UITableViewStyle,
    /// Total number of rows the data source last reported (cached on
    /// `reloadData`). `-1` means "not loaded yet" so the next `reloadData`
    /// can't no-op away the first load.
    total_rows: i64,
    /// Currently-instantiated cells, keyed by row index. We only allocate
    /// cells for rows in (or near) the viewport — without this, a 1000-song
    /// picker would alloc 1000 cells per reload and crater performance. As
    /// scrolling moves the visible range, cells outside it get released and
    /// new ones for newly-visible rows get vended by the data source. A
    /// `BTreeMap` keeps iteration in row order which is nice for diff math.
    cells_by_row: BTreeMap<NSUInteger, id>,
    /// `NSIndexPath*`, autoreleased — current selection. Updated when a cell
    /// is tapped so apps that read `indexPathForSelectedRow` from inside
    /// `tableView:didSelectRowAtIndexPath:` see the right row.
    selected_index_path: id,
    /// Touch tracking so a drag-to-scroll doesn't get reported as a row tap.
    /// `touch_began_loc` is the window-coord position when the current touch
    /// started; `is_dragging` flips true once total movement exceeds the
    /// tap-slop threshold. Cells inspect this on `touchesEnded` to decide
    /// whether to fire `didSelectRowAtIndexPath:` (tap) or stay silent (drag).
    touch_began_loc: CGPoint,
    is_dragging: bool,
    /// Set to `true` the first time we reparent a large table onto the key
    /// window (Song-Summoner-picker hack). Without this, reparent triggers
    /// the game's layout chain to call `reloadData` again, which re-enters
    /// the reparent — stack-overflows in under a frame.
    promoted_to_window: bool,
    /// Sticky flag set by [dismiss_promoted_picker] to prevent re-promotion
    /// after the user commits via Create Trooper. Without it, any later
    /// `reloadData` on this same table instance would see
    /// `promoted_to_window = false` and `rows > 50` and re-add the table
    /// to the window — making the picker reappear behind the
    /// trooper-creation scene. A fresh table created by the next picker
    /// session has this flag default-false and promotes normally.
    permanently_dismissed: bool,
    /// Row index of the last tap, for double-tap detection.
    last_tap_row: Option<NSUInteger>,
    /// Time of the last tap (as nanos since some Instant), for double-tap
    /// detection. We store epoch-nanos instead of `Instant` so the struct
    /// can keep its derived `Default` impl.
    last_tap_time_nanos: u128,
}
impl_HostObject_with_superclass!(UITableViewHostObject);

impl UITableViewHostObject {
    fn new_default() -> Self {
        let mut h = UITableViewHostObject::default();
        h.total_rows = -1;
        h
    }
}

#[derive(Default)]
struct UITableViewCellHostObject {
    superclass: super::UIViewHostObject,
    /// `UILabel*`, retained.
    text_label: id,
    /// `UILabel*`, retained.
    detail_text_label: id,
    /// `UIImageView*`, retained.
    image_view: id,
    /// Owning `UITableView*`, weak. Used to find the delegate on tap.
    table_view: id,
    /// Our position in the table — section is currently always 0.
    section: NSUInteger,
    row: NSUInteger,
    accessory_type: UITableViewCellAccessoryType,
    selection_style: UITableViewCellSelectionStyle,
    /// `NSString*`, retained.
    reuse_identifier: id,
}
impl_HostObject_with_superclass!(UITableViewCellHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITableView: UIScrollView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let mut host = Box::new(UITableViewHostObject::new_default());
    host.row_height = DEFAULT_ROW_HEIGHT;
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];
    // Game-style dark teal background instead of stock white, so the
    // synthetic Songs picker we promote to the window matches the rest of
    // Song Summoner's HUD aesthetic rather than reading as a generic iPod
    // picker layered on top.
    let bg: id = ss_color_bg_dark(env);
    () = msg![env; this setBackgroundColor:bg];

    // touchHLE renders the iOS framebuffer rotated -90° to landscape via the
    // window's rotation_matrix. iOS UIKit views are in portrait coords by
    // default, so on a landscape-only app like Song Summoner the picker
    // table would otherwise appear rotated 90° on screen (cells stacked
    // sideways, text reading bottom-to-top). Real iOS handles this through
    // the view-controller auto-rotation chain, but the game's picker doesn't
    // hit our presentModalViewController path. Apply the inverse rotation
    // here so the table renders upright in the landscape SDL window.
    if let Some(angle) = env.window.as_ref().and_then(|w| match w.current_rotation() {
        crate::window::DeviceOrientation::LandscapeLeft => {
            Some(std::f32::consts::FRAC_PI_2)
        }
        crate::window::DeviceOrientation::LandscapeRight => {
            Some(-std::f32::consts::FRAC_PI_2)
        }
        crate::window::DeviceOrientation::Portrait => None,
    }) {
        use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
        let transform = CGAffineTransform::make_rotation(angle);
        () = msg![env; this setTransform:transform];
        // After setTransform on a non-square frame the visual bounding rect
        // shifts and the cells slide off the visible area (cells earlier in
        // iOS Y rotated to be off-screen on the right). Snap the rotated
        // table to fill the full iOS portrait window so its cells land
        // visibly. Cells inside use the table's bounds (which UIView updates
        // to match the new frame considering the transform) so the layout
        // stays consistent.
        let screen: id = msg_class![env; UIScreen mainScreen];
        let screen_bounds: CGRect = msg![env; screen bounds];
        let full_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: screen_bounds.size,
        };
        () = msg![env; this setFrame:full_frame];
    }

    this
}

- (id)initWithFrame:(CGRect)frame style:(UITableViewStyle)style {
    let this: id = msg![env; this initWithFrame:frame];
    env.objc.borrow_mut::<UITableViewHostObject>(this).style = style;
    this
}

- (())dealloc {
    let cells = std::mem::take(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(this).cells_by_row,
    );
    for (_, cell) in cells {
        release(env, cell);
    }
    msg_super![env; this dealloc]
}

// Touches in the game's bottom-right back-button area pass through us
// so the game's own back button receives them. Only applies to the
// PROMOTED instance — other UITableViews behave normally.
- (id)hitTest:(CGPoint)point withEvent:(id)event {
    let is_promoted =
        *PROMOTED_TABLE.lock().unwrap() == Some(this.to_bits());
    if is_promoted {
        // The promoted picker's bounds are landscape (480x320). The back
        // button visually lives in the bottom-right ~50x50 area of that
        // canvas. Return nil for taps inside that corner so the touch
        // system tests the next sibling — the game's view with the back
        // button.
        const BACK_X_MIN: f32 = 430.0;
        const BACK_Y_MIN: f32 = 270.0;
        if point.x >= BACK_X_MIN && point.y >= BACK_Y_MIN {
            return nil;
        }
    }
    msg_super![env; this hitTest:point withEvent:event]
}

- (id)dataSource {
    env.objc.borrow::<UITableViewHostObject>(this).data_source
}
- (())setDataSource:(id)data_source {
    env.objc.borrow_mut::<UITableViewHostObject>(this).data_source = data_source;
    // iOS lays the table out (and queries the data source) automatically on
    // the next run-loop pass. Our stub doesn't have an autolayout pass, so
    // populate now — otherwise the table stays blank because nothing else
    // calls reloadData.
    if data_source != nil {
        () = msg![env; this reloadData];
    }
}

- (id)delegate {
    env.objc.borrow::<UITableViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UITableViewHostObject>(this).delegate = delegate;
}

- (f32)rowHeight {
    env.objc.borrow::<UITableViewHostObject>(this).row_height
}
- (())setRowHeight:(f32)h {
    env.objc.borrow_mut::<UITableViewHostObject>(this).row_height = h;
}

- (())setSeparatorStyle:(UITableViewCellSeparatorStyle)_s {}
- (())setSeparatorColor:(id)_c {}
- (())setBackgroundView:(id)_v {}
- (())setAllowsSelection:(bool)_v {}
- (())setSectionHeaderHeight:(f32)_h {}
- (())setSectionFooterHeight:(f32)_h {}

// Cell reuse: we don't actually pool cells, so always force the caller to
// alloc a fresh one — exactly the "no cell found" path apps already handle.
- (id)dequeueReusableCellWithIdentifier:(id)_identifier { nil }

- (id)cellForRowAtIndexPath:(id)index_path {
    let row: NSUInteger = msg![env; index_path row];
    env.objc.borrow::<UITableViewHostObject>(this).cells_by_row.get(&row).copied().unwrap_or(nil)
}

- (id)indexPathForCell:(id)cell {
    let map = env.objc.borrow::<UITableViewHostObject>(this).cells_by_row.clone();
    for (&row, &c) in map.iter() {
        if c == cell {
            return msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32];
        }
    }
    nil
}

- (NSInteger)numberOfSections { 1 }
- (NSInteger)numberOfRowsInSection:(NSInteger)_section {
    env.objc.borrow::<UITableViewHostObject>(this).total_rows.max(0) as NSInteger
}

- (())deselectRowAtIndexPath:(id)_index_path animated:(bool)_animated {
    env.objc.borrow_mut::<UITableViewHostObject>(this).selected_index_path = nil;
}

- (id)indexPathForSelectedRow {
    env.objc.borrow::<UITableViewHostObject>(this).selected_index_path
}
- (id)indexPathsForVisibleRows {
    let empty = crate::frameworks::foundation::ns_array::from_vec(env, Vec::new());
    autorelease(env, empty)
}
- (id)visibleCells {
    let empty = crate::frameworks::foundation::ns_array::from_vec(env, Vec::new());
    autorelease(env, empty)
}
- (id)indexPathForRowAtPoint:(crate::frameworks::core_graphics::CGPoint)_p { nil }

- (())scrollToRowAtIndexPath:(id)_path
              atScrollPosition:(UITableViewScrollPosition)_pos
                      animated:(bool)_animated {}
- (())selectRowAtIndexPath:(id)_path
                    animated:(bool)_animated
              scrollPosition:(UITableViewScrollPosition)_pos {}
- (())beginUpdates {}
- (())endUpdates {}
- (())insertRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {}
- (())deleteRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {}
- (())reloadRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {}

// Override scroll-related setters so we re-evaluate which cells should be
// instantiated whenever the visible window shifts. Without these, panning the
// scroll view would expose blank rows below the originally-built set.
- (())setContentOffset:(CGPoint)offset {
    let oy = offset.y;
    log!("UITableView {:?} setContentOffset y={}", this, oy);
    let () = msg_super![env; this setContentOffset:offset];
    layout_visible_cells(env, this);
}
- (())setBounds:(CGRect)bounds {
    let () = msg_super![env; this setBounds:bounds];
    layout_visible_cells(env, this);
}
// Record the start of a touch so the cell can later tell "tap" from "drag".
- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch_arr: id = msg![env; touches allObjects];
    let count: NSUInteger = msg![env; touch_arr count];
    if count == 0 { return; }
    let touch: id = msg![env; touch_arr objectAtIndex:0u32];
    let loc: CGPoint = msg![env; touch locationInView:nil];
    log!("UITableView {:?} touchesBegan", this);
    let host = env.objc.borrow_mut::<UITableViewHostObject>(this);
    host.touch_began_loc = loc;
    host.is_dragging = false;
}

// Custom scroll handling. We don't trust UIScrollView's touchesMoved here
// because in landscape mode the iOS portrait coord system means a visual
// vertical drag arrives as an X-axis delta on the touch; computing the scroll
// in window-coords gives a delta that always matches user intent.
- (())touchesMoved:(id)touches withEvent:(id)_event {
    let touch_arr: id = msg![env; touches allObjects];
    let count: NSUInteger = msg![env; touch_arr count];
    if count == 0 { return; }
    let touch: id = msg![env; touch_arr objectAtIndex:0u32];
    let prev_loc: CGPoint = msg![env; touch previousLocationInView:nil];
    let cur_loc: CGPoint = msg![env; touch locationInView:nil];
    let raw_dx = cur_loc.x - prev_loc.x;
    let raw_dy = cur_loc.y - prev_loc.y;
    let primary = if raw_dy.abs() >= raw_dx.abs() { raw_dy } else { -raw_dx };

    // Cumulative drag distance from touch start (in either iOS axis, whichever
    // is larger). If we cross the tap-slop threshold, this is a drag, not a
    // tap — record that so the cell's touchesEnded doesn't fire a row select.
    let began = env.objc.borrow::<UITableViewHostObject>(this).touch_began_loc;
    let cur_x = cur_loc.x; let cur_y = cur_loc.y;
    let began_x = began.x; let began_y = began.y;
    let total_dx = (cur_x - began_x).abs();
    let total_dy = (cur_y - began_y).abs();
    if total_dx.max(total_dy) > 15.0 {
        env.objc.borrow_mut::<UITableViewHostObject>(this).is_dragging = true;
    }

    let offset: CGPoint = msg![env; this contentOffset];
    let content_size: CGSize = msg![env; this contentSize];
    let bounds: CGRect = msg![env; this bounds];
    let off_y = offset.y;
    let off_x = offset.x;
    let csize_h = content_size.height;
    let bsize_h = bounds.size.height;
    let max_y = (csize_h - bsize_h).max(0.0);
    let new_y = (off_y - primary).clamp(0.0, max_y);
    if (new_y - off_y).abs() < 0.5 { return; }
    let new_offset = CGPoint { x: off_x, y: new_y };
    () = msg![env; this setContentOffset:new_offset];
}

- (())reloadData {
    let data_source = env.objc.borrow::<UITableViewHostObject>(this).data_source;
    if data_source == nil {
        return;
    }

    // We only handle a single section for now. Ask the data source how many
    // sections it has so apps that vend zero sections still work.
    let num_sections_sel: SEL = env
        .objc
        .register_host_selector("numberOfSectionsInTableView:".to_string(), &mut env.mem);
    let num_sections: NSInteger = if msg![env; data_source respondsToSelector:num_sections_sel] {
        msg![env; data_source numberOfSectionsInTableView:this]
    } else {
        1
    };
    if num_sections <= 0 {
        return;
    }

    let row_height = env.objc.borrow::<UITableViewHostObject>(this).row_height;
    let bounds: CGRect = msg![env; this bounds];

    let rows: NSInteger = msg![env; data_source tableView:this numberOfRowsInSection:0i32];
    let prev_rows = env.objc.borrow::<UITableViewHostObject>(this).total_rows;
    let row_count_changed = (rows as i64) != prev_rows;
    env.objc.borrow_mut::<UITableViewHostObject>(this).total_rows = rows as i64;
    log!(
        "UITableView {:?} reloadData: {} rows, row_height {} (was {})",
        this, rows, row_height, prev_rows
    );

    // Always refresh content size in case width changed.
    let content_size = CGSize {
        width: bounds.size.width,
        height: (rows.max(0) as f32) * row_height,
    };
    () = msg![env; this setContentSize:content_size];

    // If the row count changed, throw out everything and start fresh; the
    // data behind the rows is presumed different. Otherwise leave the cells
    // alone — repeated reloadData calls with the same data shouldn't churn
    // 1000+ cell allocations (was crashing the picker for users with large
    // music libraries).
    if row_count_changed {
        let old_cells = std::mem::take(
            &mut env.objc.borrow_mut::<UITableViewHostObject>(this).cells_by_row,
        );
        for (_, cell) in old_cells {
            () = msg![env; cell removeFromSuperview];
            release(env, cell);
        }
    }

    layout_visible_cells(env, this);

    // Song Summoner's iPod picker builds 4 tables (Playlists/Artists/Albums/
    // Songs) in separate view subtrees with no rendered tab bar UI, so the
    // user is stuck on whichever sub-view the game made visible (typically
    // 5-row Playlists). When we see a table reload with hundreds of rows
    // it's almost certainly the "All Songs" list — forcibly reparent it
    // onto the topmost UIWindow so it covers the visible picker no matter
    // which subtree the game placed it in. This is a hack specific to apps
    // that build their own picker chrome; well-behaved tables aren't going
    // to hit a 50-row threshold for an off-screen subtree.
    // Song Summoner's picker has 4 separate sibling table views (Playlists/
    // Artists/Albums/Songs) in different view subtrees, and it doesn't render
    // any tab-bar UI. The Songs table (only one with > 50 rows) is otherwise
    // unreachable. As a workaround, the first time a table reloads with a
    // large row count, reparent it directly onto the key window so it
    // becomes the topmost visible view. Guarded by `promoted_to_window` so
    // subsequent reloads (triggered by the game's layout chain after the
    // reparent) don't recurse and stack-overflow.
    // Capture a reference to the visible 5-row IPDSongsTab table so the
    // picker-swap trick can later trigger reloadData + dispatch a row-0
    // pick on it.
    if rows <= 5 {
        let ds: id = env.objc.borrow::<UITableViewHostObject>(this).delegate;
        if ds != nil && env.objc.get_host_object(ds).is_some() {
            let cls: crate::objc::Class = msg![env; ds class];
            let cname = env.objc.get_class_name(cls).to_string();
            if cname == "IPDSongsTab" {
                *IPD_SONGS_TABLE.lock().unwrap() = Some(this.to_bits());
                // NB: do NOT hide IPDSongsTab here. The game appears to
                // render its confirmation panel as a re-styled view of
                // this table (the "picked song + Create Trooper / No"
                // screen is the IPDSongsTab in its own confirmation
                // layout). Hiding it produces a black screen instead of
                // confirmation. Leave it visible; the "5-row leak" we
                // saw earlier was a separate symptom of the picker-swap
                // dispatch failing for some rows.
            }
        }
    }

    // Re-enable 1309-row table promotion so the user gets a scrollable
    // full-library browser. Taps on cells in this promoted table will
    // bypass its native (broken) IPDPlaylistsTab pick path and instead
    // stage a swap + replay-row-0 on the captured IPDSongsTab (see the
    // cell touchesEnded hook).
    let (already_promoted, permanently_dismissed) = {
        let h = env.objc.borrow::<UITableViewHostObject>(this);
        (h.promoted_to_window, h.permanently_dismissed)
    };
    if rows > 50 && !already_promoted && !permanently_dismissed {
        env.objc
            .borrow_mut::<UITableViewHostObject>(this)
            .promoted_to_window = true;
        let app: id = msg_class![env; UIApplication sharedApplication];
        let window: id = msg![env; app keyWindow];
        if window != nil {
            log!(
                "UITableView {:?} large reload ({} rows): reparenting to keyWindow {:?}",
                this, rows, window
            );
            () = msg![env; this removeFromSuperview];
            () = msg![env; window addSubview:this];
            *PROMOTED_TABLE.lock().unwrap() = Some(this.to_bits());
            // Fill the entire window (iOS portrait coords). This gives the
            // table bounds.size matching the full screen, so cells inherit
            // the full width and render edge-to-edge in landscape after the
            // table's own +π/2 transform + window's -π/2 rotation.
            //
            // The old (50, 0, sb.width-50, sb.height) frame reserved a 50px
            // iOS-x strip for the synthetic tab bar, but that's not needed
            // — the tab bar is added as a separate sibling view on the
            // window and brought-to-front by the resurface block in
            // reloadData, so it draws on top of the table's bottom 50px in
            // landscape. The inset was the source of the persistent
            // white-on-right strip in the picker (cells were 270 wide on a
            // 480-wide screen).
            let screen: id = msg_class![env; UIScreen mainScreen];
            let sb: CGRect = msg![env; screen bounds];
            let full = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: sb.size,
            };
            () = msg![env; this setFrame:full];
            let landscape_bounds = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: sb.size.height,
                    height: sb.size.width,
                },
            };
            () = msg![env; this setBounds:landscape_bounds];

            // Flush the cell cache: layout_visible_cells already ran once
            // during reloadData with the pre-promotion (portrait, 320-wide)
            // bounds. Those cells are now stale — their frames were
            // computed against the old width. layout_visible_cells's
            // contains_key guard skips re-laying-out cells already in
            // cells_by_row, so without this flush the picker keeps
            // rendering at the narrow portrait width and the black/white
            // strip on the right persists.
            let old_cells = std::mem::take(
                &mut env.objc.borrow_mut::<UITableViewHostObject>(this).cells_by_row,
            );
            for (_, cell) in old_cells {
                () = msg![env; cell removeFromSuperview];
                release(env, cell);
            }

            // Re-apply the dark-teal background after promotion. The game's
            // UITableView subclass (IPDPlaylistsTab) sets its own white
            // backgroundColor during init *after* our base initWithFrame
            // runs, so without this override the picker reads as a white
            // panel against the game HUD.
            let promoted_bg: id = ss_color_bg_dark(env);
            () = msg![env; this setBackgroundColor:promoted_bg];
            layout_visible_cells(env, this);
            build_synthetic_tab_bar(env, window);
        }
    }

    // Keep the picker chrome on top of whatever the game has restacked since.
    // The 5-row picker subtree often gets re-added on top of our reparented
    // 1309-row Songs table, intercepting drag events; force the promoted
    // table back to the front every reload so it stays receptive to touches.
    // Then layer the tab bar above it. Reparent the table into the window
    // if the game removed it (e.g. picker dismissal followed by re-open).
    {
        const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;
        let app: id = msg_class![env; UIApplication sharedApplication];
        let window: id = msg![env; app keyWindow];
        if window != nil {
            let bar: id = msg![env; window viewWithTag:TAB_BAR_TAG];
            let promoted_bits = *PROMOTED_TABLE.lock().unwrap();
            let promoted: id = match promoted_bits {
                Some(bits) => crate::objc::id::from_bits(bits),
                None => nil,
            };
            if promoted != nil {
                let sv: id = msg![env; promoted superview];
                if sv == window {
                    () = msg![env; window bringSubviewToFront:promoted];
                } else if sv != nil {
                    // Game put it back somewhere else. Yank it onto the
                    // window again so it stays the topmost touch receiver.
                    () = msg![env; promoted removeFromSuperview];
                    () = msg![env; window addSubview:promoted];
                }
                // (Removed: un-hide-on-other-table-reload safety net. It
                // fired whenever any sibling table reloadData ran after
                // picker-swap, including incidental reloads from
                // IPDSongsTab's own handler — un-hiding the promoted
                // table prematurely and letting it eat touches that
                // should reach the confirmation buttons. The remount path
                // on No is the canonical way to bring the picker back.)
            }
            if bar != nil {
                () = msg![env; bar setHidden:false];
                () = msg![env; window bringSubviewToFront:bar];
            }
        }
    }
}

@end

@implementation UITableViewCell: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UITableViewCellHostObject>::default(), &mut env.mem)
}

- (id)initWithStyle:(UITableViewCellStyle)_style
    reuseIdentifier:(id)reuse_identifier {
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 320.0, height: DEFAULT_ROW_HEIGHT },
    };
    let this: id = msg_super![env; this initWithFrame:frame];

    retain(env, reuse_identifier);
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).reuse_identifier =
        reuse_identifier;

    // Default cell content: a single full-width UILabel that occupies the
    // cell minus a small left inset. Apps that use the default
    // `textLabel.text` API will Just Work.
    let label_frame = CGRect {
        origin: CGPoint { x: 12.0, y: 0.0 },
        size: CGSize { width: frame.size.width - 12.0, height: frame.size.height },
    };
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:label_frame];
    // Explicit clearColor instead of nil. Passing nil here turned out to
    // leave the UILabel in its default opaque-white state in touchHLE,
    // which painted a white rectangle behind every cell's row content in
    // the picker.
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).text_label = label;
    () = msg![env; this addSubview:label];

    // Transparent cell background so the table's dark-teal panel shows
    // through uniformly. The game's stock UITableViewCell otherwise paints
    // its own white background here, which would draw a bright stripe on
    // every row against the HUD palette.
    let cell_bg: id = msg_class![env; UIColor clearColor];
    () = msg![env; this setBackgroundColor:cell_bg];

    this
}

- (id)initWithFrame:(CGRect)frame
    reuseIdentifier:(id)reuse_identifier {
    let this: id = msg![env; this initWithStyle:0i32 reuseIdentifier:reuse_identifier];
    () = msg![env; this setFrame:frame];
    this
}

- (())dealloc {
    let &UITableViewCellHostObject {
        superclass: _,
        text_label,
        detail_text_label,
        image_view,
        reuse_identifier,
        ..
    } = env.objc.borrow(this);
    release(env, text_label);
    release(env, detail_text_label);
    release(env, image_view);
    release(env, reuse_identifier);
    msg_super![env; this dealloc]
}

- (id)textLabel {
    env.objc.borrow::<UITableViewCellHostObject>(this).text_label
}

- (id)detailTextLabel {
    // Created lazily on first access since most callers won't need it.
    let existing = env.objc.borrow::<UITableViewCellHostObject>(this).detail_text_label;
    if existing != nil { return existing; }
    let bounds: CGRect = msg![env; this bounds];
    let frame = CGRect {
        origin: CGPoint { x: 12.0, y: bounds.size.height * 0.5 },
        size: CGSize {
            width: bounds.size.width - 12.0,
            height: bounds.size.height * 0.5,
        },
    };
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    // UILabel defaults to an opaque white background. The picker overlays
    // our own dark-teal layout over the cells, so leaving these lazy
    // labels at their default would paint a white rectangle behind every
    // row. Knock them out to clear.
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).detail_text_label = label;
    () = msg![env; this addSubview:label];
    label
}

- (id)imageView {
    let existing = env.objc.borrow::<UITableViewCellHostObject>(this).image_view;
    if existing != nil { return existing; }
    let bounds: CGRect = msg![env; this bounds];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: bounds.size.height, height: bounds.size.height },
    };
    let iv: id = msg_class![env; UIImageView alloc];
    let iv: id = msg![env; iv initWithFrame:frame];
    // Match the detailTextLabel comment: UIImageView's default background
    // is opaque white, which would paint a square behind whatever image
    // the caller eventually sets. Force clear so the cell looks correct
    // until and after the caller assigns an image.
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; iv setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).image_view = iv;
    () = msg![env; this addSubview:iv];
    iv
}

- (id)contentView { this }

- (id)reuseIdentifier {
    env.objc.borrow::<UITableViewCellHostObject>(this).reuse_identifier
}

- (UITableViewCellAccessoryType)accessoryType {
    env.objc.borrow::<UITableViewCellHostObject>(this).accessory_type
}
- (())setAccessoryType:(UITableViewCellAccessoryType)t {
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).accessory_type = t;
}

- (UITableViewCellSelectionStyle)selectionStyle {
    env.objc.borrow::<UITableViewCellHostObject>(this).selection_style
}
- (())setSelectionStyle:(UITableViewCellSelectionStyle)s {
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).selection_style = s;
}

- (())setSelected:(bool)_selected animated:(bool)_animated {}
- (())prepareForReuse {}

- (())touchesEnded:(id)_touches withEvent:(id)_event {
    let &UITableViewCellHostObject { table_view, row, section, .. } =
        env.objc.borrow(this);
    log!("UITableViewCell {:?} touchesEnded row={} section={}", this, row, section);
    if table_view == nil { return; }
    // If the user was dragging (i.e. scrolling), don't treat this release as
    // a row tap. The table view tracks this for us based on cumulative touch
    // distance vs. the tap-slop threshold; reset it for the next touch.
    let was_dragging = {
        let host = env.objc.borrow_mut::<UITableViewHostObject>(table_view);
        let d = host.is_dragging;
        host.is_dragging = false;
        d
    };
    log!(
        "UITableViewCell {:?} touchesEnded row={} was_dragging={}",
        this, row, was_dragging
    );
    if was_dragging {
        return;
    }

    // Picker-swap hijack: if this tap is on the promoted 1309-row table,
    // don't dispatch to its native (broken) IPDPlaylistsTab delegate.
    // Instead: dispatch `tableView:didSelectRowAtIndexPath:row` on the
    // captured visible 5-row IPDSongsTab delegate, passing the user's
    // actual row index from the drill. The IPDSongsTab data source's
    // didSelect reads its own items array (cached from [query collections]
    // at picker-open) — since we expose all 1309 collections, that array
    // already has the user's song at index `row`. Then hide the drill UI
    // so the game's confirmation screen is visible.
    let promoted = *PROMOTED_TABLE.lock().unwrap();
    if promoted == Some(table_view.to_bits()) {
        if let Some(song) =
            crate::frameworks::media_player::music_library::song(row as usize)
        {
            let pid = song.persistent_id;
            log!(
                "Picker-swap: tap on promoted-table row {} -> dispatching row {} to IPDSongsTab (PID {:016X})",
                row, row, pid
            );
            let songs_table_bits = *IPD_SONGS_TABLE.lock().unwrap();
            if let Some(bits) = songs_table_bits {
                let songs_table = crate::objc::id::from_bits(bits);
                if env.objc.get_host_object(songs_table).is_some() {
                    let songs_delegate: id = env
                        .objc
                        .borrow::<UITableViewHostObject>(songs_table)
                        .delegate;
                    if songs_delegate != nil
                        && env.objc.get_host_object(songs_delegate).is_some()
                    {
                        // Stage picked PID for media_query collections
                        // swap. We still go through the original row=row
                        // dispatch (NOT row=0) because items_ is the
                        // game's PID-keyed dictionary — overwriting it
                        // with an NSArray crashes the game's
                        // `[items_ objectForKey:pid]` lookups. Dispatch
                        // at the original row index reads items_[pid_at_row]
                        // which has the correct song. The confirmation
                        // panel refresh on pick 2+ is the open issue.
                        crate::frameworks::media_player::music_library::set_swapped_first_pid(Some(pid));
                        let target_idx: id = msg_class![env;
                            NSIndexPath indexPathForRow:row inSection:0u32];
                        let sel: SEL = env.objc.register_host_selector(
                            "tableView:didSelectRowAtIndexPath:".to_string(),
                            &mut env.mem,
                        );
                        let responds: bool =
                            msg![env; songs_delegate respondsToSelector:sel];
                        if responds {
                            log!(
                                "Picker-swap: dispatching row-{} select to IPDSongsTab delegate {:?} (staged PID {:016X})",
                                row, songs_delegate, pid
                            );
                            crate::frameworks::media_player::music_library::set_swapped_first_pid(Some(pid));
                            let env_ptr = env as *mut crate::Environment;
                            // Purge cell cache to force cellForRow re-fire.
                            {
                                let old_cells = std::mem::take(
                                    &mut env.objc.borrow_mut::<UITableViewHostObject>(songs_table).cells_by_row,
                                );
                                for (_, cell) in old_cells {
                                    () = msg![env; cell removeFromSuperview];
                                    release(env, cell);
                                }
                            }
                            let _ = std::panic::catch_unwind(
                                std::panic::AssertUnwindSafe(move || unsafe {
                                    let env = &mut *env_ptr;
                                    () = msg![env; songs_table reloadData];
                                }),
                            );
                            let _ = std::panic::catch_unwind(
                                std::panic::AssertUnwindSafe(move || unsafe {
                                    let env = &mut *env_ptr;
                                    () = msg![env; songs_delegate tableView:songs_table didSelectRowAtIndexPath:target_idx];
                                }),
                            );
                            crate::frameworks::media_player::music_library::play_song(
                                row as usize,
                            );
                            // Hide the promoted picker so the confirmation
                            // panel underneath is visible.
                            let app: id = msg_class![env; UIApplication sharedApplication];
                            let window: id = msg![env; app keyWindow];
                            () = msg![env; table_view setHidden:true];
                            if window != nil {
                                const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;
                                let bar: id = msg![env; window viewWithTag:TAB_BAR_TAG];
                                if bar != nil {
                                    () = msg![env; bar setHidden:true];
                                }
                            }
                            return;
                        }
                    }
                }
            } else {
                log!("Picker-swap: no IPDSongsTab captured yet, can't dispatch");
            }
        }
        return;
    }

    let delegate: id = env
        .objc
        .borrow::<UITableViewHostObject>(table_view)
        .delegate;
    let idx: id = msg_class![env; NSIndexPath indexPathForRow:row inSection:section];
    env.objc.borrow_mut::<UITableViewHostObject>(table_view).selected_index_path = idx;
    if delegate == nil {
        log!("UITableView {:?} tap on row {}: delegate is nil — no didSelect dispatch", table_view, row);
        return;
    }
    let sel: SEL = env.objc.register_host_selector(
        "tableView:didSelectRowAtIndexPath:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if !responds {
        log!(
            "UITableView {:?} tap on row {}: delegate {:?} does not respond to didSelectRowAtIndexPath: — no dispatch",
            table_view, row, delegate
        );
        return;
    }
    let dcls: crate::objc::Class = msg![env; delegate class];
    let dcls_name = env.objc.get_class_name(dcls);
    log!(
        "UITableView {:?} tap on row {}: dispatching didSelectRowAtIndexPath to delegate {:?} of class {:?}",
        table_view, row, delegate, dcls_name
    );
    () = msg![env; delegate tableView:table_view didSelectRowAtIndexPath:idx];

    // Single tap = pick this song. The game's didSelectRowAtIndexPath
    // dispatch above already triggers the trooper-creation flow on the
    // 5-row picker tables; for the 1309-row promoted table the game's
    // IPDPlaylistsTab tries to drill rather than pick, so picks from there
    // don't currently route to a trooper. Either way: no preview, no Y/N
    // overlay, no double-tap escalation — just dispatch and let the game
    // do whatever it does.
}

@end

@implementation UITableViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::<crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject>::default();
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)initWithStyle:(UITableViewStyle)style {
    let this: id = msg_super![env; this initWithNibName:nil bundle:nil];
    let table: id = msg_class![env; UITableView alloc];
    let frame: CGRect = msg![env; (msg_class![env; UIScreen mainScreen]) applicationFrame];
    let table: id = msg![env; table initWithFrame:frame style:style];
    () = msg![env; table setDataSource:this];
    () = msg![env; table setDelegate:this];
    () = msg![env; this setView:table];
    release(env, table);
    this
}

- (id)initWithNibName:(id)nib_name bundle:(id)bundle {
    msg_super![env; this initWithNibName:nib_name bundle:bundle]
}

// UITableViewController's view is, by default, a UITableView. Real iOS
// creates this lazily in -loadView. Our base UIViewController.loadView falls
// back to a plain UIView, which makes subclasses that call setSeparatorColor:
// or other UITableView APIs on self.view crash. Override here so subclasses
// like IPDSongsTab inherit the right view type even when only -[init] is used.
- (())loadView {
    let table: id = msg_class![env; UITableView alloc];
    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];
    // touchHLE's framebuffer pipeline maps iOS coords 1:1 to SDL pixels (no
    // rotation in the blit). When the app runs landscape the SDL window is
    // landscape-shaped (480x320) but UIScreen.bounds is still 320x480 portrait
    // -- so a portrait-sized table fills only the top-left corner. Swap the
    // dimensions when the device is in landscape so the picker fills the
    // window like the iPod Music app does.
    let frame = match env.window.as_ref().map(|w| w.current_rotation()) {
        Some(crate::window::DeviceOrientation::LandscapeLeft)
        | Some(crate::window::DeviceOrientation::LandscapeRight) => CGRect {
            origin: bounds.origin,
            size: CGSize {
                width: bounds.size.height,
                height: bounds.size.width,
            },
        },
        _ => bounds,
    };
    let table: id = msg![env; table initWithFrame:frame style:0i32];
    () = msg![env; table setDataSource:this];
    () = msg![env; table setDelegate:this];
    () = msg![env; this setView:table];
    release(env, table);
}

- (id)tableView {
    msg![env; this view]
}

- (())viewWillAppear:(bool)_animated {
    let v: id = msg![env; this view];
    () = msg![env; v reloadData];
}

@end

};

/// Walk up the view hierarchy from `view` looking for a UIViewController
/// whose class name contains "Picker" (Song Summoner's IPDMediaPickerController
/// and Apple's MPMediaPickerController both fit). Returns nil if no such
/// ancestor exists.
/// Mount a "Create unit?" Y/N overlay panel on the key window with the
/// staged song's title above two coloured UIControl buttons (tagged so the
/// existing UIControl.touchesEnded hook can route them). Replaces any
/// existing prompt so retapping a different song updates the title.
pub(super) fn show_pick_prompt(env: &mut crate::Environment, window: id, song_title: String) {
    use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
    // If a prompt already exists, just update its title label and unhide it
    // — don't tear down and rebuild. Rebuilding releases the old UIControl
    // buttons; if a touch was mid-flight on them (common when tapping fast)
    // the next dispatch hits freed memory and crashes with
    // `borrow::<UIControlHostObject>(...): no such object`.
    let existing: id = msg![env; window viewWithTag:PICK_PROMPT_TAG];
    if existing != nil {
        () = msg![env; existing setHidden:false];
        () = msg![env; window bringSubviewToFront:existing];
        // Update title text. We tagged the title with a child-position
        // approach: it's the FIRST UILabel subview of the panel. Iterate
        // subviews to find it.
        let subs = env
            .objc
            .borrow::<super::UIViewHostObject>(existing)
            .subviews
            .clone();
        let label_cls = env.objc.get_known_class("UILabel", &mut env.mem);
        for sub in subs {
            if env.objc.get_host_object(sub).is_none() {
                continue;
            }
            let is_label: bool = msg![env; sub isKindOfClass:label_cls];
            if is_label {
                let prompt_text = format!("Create unit from: {}?", song_title);
                let title_ns =
                    crate::frameworks::foundation::ns_string::from_rust_string(env, prompt_text);
                () = msg![env; sub setText:title_ns];
                release(env, title_ns);
                break;
            }
        }
        return;
    }

    // Place the panel along the iOS-right edge (= SDL top in landscape-left
    // rotation) so it doesn't overlap the visible song list along the SDL
    // bottom. Then rotate it the same +π/2 the picker uses so its labels
    // read horizontally on screen.
    let panel_frame = CGRect {
        origin: CGPoint { x: 260.0, y: 60.0 },
        size: CGSize { width: 60.0, height: 360.0 },
    };
    let panel: id = msg_class![env; UIView alloc];
    let panel: id = msg![env; panel initWithFrame:panel_frame];
    () = msg![env; panel setTag:PICK_PROMPT_TAG];
    let bg: id = ss_color_panel_bg(env);
    () = msg![env; panel setBackgroundColor:bg];
    let transform = CGAffineTransform::make_rotation(std::f32::consts::FRAC_PI_2);
    () = msg![env; panel setTransform:transform];
    () = msg![env; panel setFrame:panel_frame];

    // Internal layout uses the panel's *post-rotation* local coords —
    // labels stack horizontally on screen because the transform converts
    // iOS-portrait Y into SDL-landscape X.
    let panel_w = 360.0_f32;
    let panel_h = 60.0_f32;

    // Title label across the top half.
    let title_frame = CGRect {
        origin: CGPoint { x: 8.0, y: 4.0 },
        size: CGSize { width: panel_w - 16.0, height: 22.0 },
    };
    let title_lbl: id = msg_class![env; UILabel alloc];
    let title_lbl: id = msg![env; title_lbl initWithFrame:title_frame];
    let prompt_text = format!("Create unit from: {}?", song_title);
    let title_ns =
        crate::frameworks::foundation::ns_string::from_rust_string(env, prompt_text);
    () = msg![env; title_lbl setText:title_ns];
    release(env, title_ns);
    let title_color: id = ss_color_text_bright(env);
    () = msg![env; title_lbl setTextColor:title_color];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; title_lbl setBackgroundColor:clear];
    () = msg![env; panel addSubview:title_lbl];
    release(env, title_lbl);

    // "Y" button (left half of bottom row). Slightly lighter teal so the
    // confirm action reads as the highlighted option, in line with how the
    // game styles its "Create Trooper" button vs the dimmer "No" button.
    let btn_y = 30.0_f32;
    let btn_h = panel_h - btn_y - 4.0;
    let y_frame = CGRect {
        origin: CGPoint { x: 8.0, y: btn_y },
        size: CGSize { width: panel_w / 2.0 - 16.0, height: btn_h },
    };
    let y_btn: id = msg_class![env; UIControl alloc];
    let y_btn: id = msg![env; y_btn initWithFrame:y_frame];
    () = msg![env; y_btn setTag:PICK_PROMPT_YES_TAG];
    let y_bg: id = ss_color_btn_primary(env);
    () = msg![env; y_btn setBackgroundColor:y_bg];
    () = msg![env; panel addSubview:y_btn];

    let y_lbl_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: y_frame.size,
    };
    let y_lbl: id = msg_class![env; UILabel alloc];
    let y_lbl: id = msg![env; y_lbl initWithFrame:y_lbl_frame];
    let y_text = crate::frameworks::foundation::ns_string::from_rust_string(
        env,
        "Y — Create".to_string(),
    );
    () = msg![env; y_lbl setText:y_text];
    release(env, y_text);
    let btn_text_color: id = ss_color_text_bright(env);
    () = msg![env; y_lbl setTextColor:btn_text_color];
    () = msg![env; y_lbl setBackgroundColor:clear];
    () = msg![env; y_lbl setTextAlignment:1i32];
    () = msg![env; y_btn addSubview:y_lbl];
    release(env, y_lbl);
    release(env, y_btn);

    // "N" button (right half).
    let n_frame = CGRect {
        origin: CGPoint { x: panel_w / 2.0 + 8.0, y: btn_y },
        size: CGSize { width: panel_w / 2.0 - 16.0, height: btn_h },
    };
    let n_btn: id = msg_class![env; UIControl alloc];
    let n_btn: id = msg![env; n_btn initWithFrame:n_frame];
    () = msg![env; n_btn setTag:PICK_PROMPT_NO_TAG];
    let n_bg: id = ss_color_btn_secondary(env);
    () = msg![env; n_btn setBackgroundColor:n_bg];
    () = msg![env; panel addSubview:n_btn];

    let n_lbl: id = msg_class![env; UILabel alloc];
    let n_lbl: id = msg![env; n_lbl initWithFrame:y_lbl_frame];
    let n_text = crate::frameworks::foundation::ns_string::from_rust_string(
        env,
        "N — Cancel".to_string(),
    );
    () = msg![env; n_lbl setText:n_text];
    release(env, n_text);
    () = msg![env; n_lbl setTextColor:btn_text_color];
    () = msg![env; n_lbl setBackgroundColor:clear];
    () = msg![env; n_lbl setTextAlignment:1i32];
    () = msg![env; n_btn addSubview:n_lbl];
    release(env, n_lbl);
    release(env, n_btn);

    () = msg![env; window addSubview:panel];
    () = msg![env; window bringSubviewToFront:panel];
    release(env, panel);
}

pub(super) fn hide_pick_prompt(env: &mut crate::Environment, window: id) {
    // Hide rather than remove — keeping the panel and its UIControl buttons
    // in the view tree means a touch dispatched into them right before the
    // hide doesn't end up calling touchesEnded on freed memory.
    let existing: id = msg![env; window viewWithTag:PICK_PROMPT_TAG];
    if existing != nil {
        () = msg![env; existing setHidden:true];
    }
}

/// Dispatch the song-pick to whichever live UIViewController in the window
/// subtree responds to `mediaPicker:didPickMediaNumber:`. Best-effort: the
/// game's handler often crashes because its picker arg is fragile, so we
/// wrap each attempt in `catch_unwind`.
pub(super) fn attempt_pick(env: &mut crate::Environment, pid: u64) {
    let pick_sel: SEL = env.objc.register_host_selector(
        "mediaPicker:didPickMediaNumber:".to_string(),
        &mut env.mem,
    );
    let vcs = collect_window_view_controllers(env);
    for vc in vcs {
        let candidates: Vec<id> = {
            let respond_sel: SEL = env
                .objc
                .register_host_selector("delegate".to_string(), &mut env.mem);
            let has_delegate: bool = msg![env; vc respondsToSelector:respond_sel];
            let mut list = vec![vc];
            if has_delegate {
                let d: id = msg![env; vc delegate];
                if d != nil
                    && env.objc.get_host_object(d).is_some()
                    && !list.contains(&d)
                {
                    list.push(d);
                }
            }
            list
        };
        for cand in candidates {
            if env.objc.get_host_object(cand).is_none() {
                continue;
            }
            let responds: bool = msg![env; cand respondsToSelector:pick_sel];
            if responds {
                let env_ptr = env as *mut crate::Environment;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    move || unsafe {
                        let env = &mut *env_ptr;
                        () = msg![env; cand mediaPicker:cand didPickMediaNumber:pid];
                    },
                ));
            }
        }
    }
}

/// Collect every UIViewController in the key-window subtree. Used by the
/// double-tap-pick path to try the song-pick selector on any candidate that
/// might handle it (in Song Summoner: IPDSongsTab, ViewManager, the
/// UITabBarController's delegate, etc. — there's no `IPDMediaPickerController`
/// instance in the view tree).
fn collect_window_view_controllers(env: &mut crate::Environment) -> Vec<id> {
    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window == nil {
        return Vec::new();
    }
    let mut stack: Vec<id> = vec![window];
    let mut out: Vec<id> = Vec::new();
    while let Some(v) = stack.pop() {
        let vc = env.objc.borrow::<super::UIViewHostObject>(v).view_controller;
        // view_controller is a weak ref — if the VC was freed, the field
        // still holds the stale pointer. Skip vc pointers whose host
        // object is gone, otherwise msg_send crashes with `orig_class !=
        // nil`.
        if vc != nil && env.objc.get_host_object(vc).is_some() && !out.contains(&vc) {
            out.push(vc);
        }
        let subviews = env
            .objc
            .borrow::<super::UIViewHostObject>(v)
            .subviews
            .clone();
        for sub in subviews {
            stack.push(sub);
        }
    }
    out
}

/// First walk up the superview chain from `view`; if no picker view-controller
/// is found there, fall back to a depth-first scan of the whole window tree.
/// Returns the picker view controller (any UIViewController whose class name
/// contains "Picker") or nil.
fn find_ancestor_picker(env: &mut crate::Environment, view: id) -> id {
    // 1) Walk superviews — the cheap case (only works if the table is still
    //    in the game's original view hierarchy, not after our reparent hack).
    let mut current = view;
    while current != nil {
        let vc = env
            .objc
            .borrow::<super::UIViewHostObject>(current)
            .view_controller;
        if vc != nil {
            let cls: crate::objc::Class = msg![env; vc class];
            let name = env.objc.get_class_name(cls).to_string();
            if name.contains("Picker") {
                return vc;
            }
        }
        let next: id = msg![env; current superview];
        if next == current {
            break;
        }
        current = next;
    }
    // 2) Fall back: DFS through the whole window tree. After the promoted-
    //    table reparent the picker controller is no longer an ancestor of
    //    the visible table, but it still exists somewhere in the window
    //    subtree of the original picker views.
    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window == nil {
        return nil;
    }
    let mut stack: Vec<id> = vec![window];
    let mut seen_vcs: Vec<String> = Vec::new();
    while let Some(v) = stack.pop() {
        let vc = env.objc.borrow::<super::UIViewHostObject>(v).view_controller;
        if vc != nil {
            let cls: crate::objc::Class = msg![env; vc class];
            let name = env.objc.get_class_name(cls).to_string();
            seen_vcs.push(name.clone());
            if name.contains("Picker")
                || name.contains("iPod")
                || name.contains("Media")
                || name == "IPDController"
            {
                return vc;
            }
        }
        let subviews = env
            .objc
            .borrow::<super::UIViewHostObject>(v)
            .subviews
            .clone();
        for sub in subviews {
            stack.push(sub);
        }
    }
    log!(
        "find_ancestor_picker DFS: no picker. view-controller classes seen: {:?}",
        seen_vcs
    );
    nil
}

/// Lazily load `res/album_placeholder.png` (relative to the current working
/// directory) into a UIImage and cache the result. Returns `nil` if the file
/// is missing or undecodable; callers fall back to a coloured swatch.
fn get_or_load_placeholder_image(env: &mut crate::Environment) -> id {
    {
        let guard = PLACEHOLDER_IMAGE.lock().unwrap();
        if let Some(cached) = guard.as_ref() {
            return match cached {
                Ok(bits) => crate::objc::id::from_bits(*bits),
                Err(()) => nil,
            };
        }
    }
    // Try CWD-relative first (works on Windows / macOS / Linux desktop
    // builds where touchHLE runs from the unpacked distro folder).
    // On Android CWD is typically `/`, so we fall back to the path
    // user_data_base_path resolves to (/sdcard/touchHLE/ or the app's
    // ext-files dir) — MainActivity extracts the asset there on launch.
    let bytes = match std::fs::read("res/album_placeholder.png") {
        Ok(b) => b,
        Err(_e_cwd) => {
            let fallback = crate::paths::user_data_base_path()
                .join("res")
                .join("album_placeholder.png");
            match std::fs::read(&fallback) {
                Ok(b) => b,
                Err(e) => {
                    log!(
                        "Couldn't read res/album_placeholder.png (cwd or {}): {e}; \
                         using colored swatches instead.",
                        fallback.display()
                    );
                    *PLACEHOLDER_IMAGE.lock().unwrap() = Some(Err(()));
                    return nil;
                }
            }
        }
    };
    let image = match crate::image::Image::from_bytes(&bytes) {
        Ok(i) => i,
        Err(e) => {
            log!("Couldn't decode album_placeholder.png: {e:?}");
            *PLACEHOLDER_IMAGE.lock().unwrap() = Some(Err(()));
            return nil;
        }
    };
    let cg_image = crate::frameworks::core_graphics::cg_image::from_image(env, image);
    let ui_image_cls = env.objc.get_known_class("UIImage", &mut env.mem);
    let img: id = msg![env; ui_image_cls alloc];
    let img: id = msg![env; img initWithCGImage:cg_image];
    // Permanent retain so the cached UIImage outlives the autorelease pool.
    crate::objc::retain(env, img);
    *PLACEHOLDER_IMAGE.lock().unwrap() = Some(Ok(img.to_bits()));
    log!("Loaded res/album_placeholder.png as UIImage {:?}", img);
    img
}

/// Render a synthetic iPod-style tab bar on top of the given window. Used by
/// the picker-promotion hack to give the user a visual "you are on the Songs
/// tab" indicator even though Song Summoner's picker doesn't draw any tab
/// chrome of its own. Idempotent — checks for an existing tag-marked tab bar
/// and skips re-creation.
fn build_synthetic_tab_bar(_env: &mut crate::Environment, _window: id) {
    // Disabled — the synthetic Playlists/Artists/Albums/Songs strip was
    // purely decorative (we don't handle taps on its tabs) and it was
    // covering the game's own bottom-right back button. Kept the function
    // around as a no-op so other call sites don't break.
    #[allow(unreachable_code)]
    { return; }
    use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
    let env = _env;
    let window = _window;
    const TAB_BAR_TAG: crate::frameworks::foundation::NSInteger = 0x7AB_BA;

    // If a tab bar already exists, just bring it to the front so it stays
    // above any newly-reparented tables.
    let existing: id = msg![env; window viewWithTag:TAB_BAR_TAG];
    if existing != nil {
        () = msg![env; window bringSubviewToFront:existing];
        return;
    }

    // Place the tab bar along the bottom edge of the SDL landscape display.
    // Empirically (verified in the LandscapeLeft screenshot): iOS portrait
    // x=high lands at SDL *top*, x=low lands at SDL *bottom*. So a 50pt-wide
    // strip on the iOS left edge (x=0..50) becomes a 50pt-tall bar at SDL
    // bottom under the -π/2 rotation.
    let bar_view: id = msg_class![env; UIView alloc];
    // Height is in iOS-portrait coords = landscape width post-rotation.
    // Leave a 50px slice on the right of the landscape screen so the
    // game's bottom-right back button stays uncovered.
    const BAR_LANDSCAPE_WIDTH: f32 = 480.0 - 50.0;
    let bar_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 50.0, height: BAR_LANDSCAPE_WIDTH },
    };
    let bar_view: id = msg![env; bar_view initWithFrame:bar_frame];
    () = msg![env; bar_view setTag:TAB_BAR_TAG];
    // Slightly raised dark teal so the bar is visually distinct from the
    // table above while matching the HUD palette.
    let bar_bg: id = ss_color_panel_bg(env);
    () = msg![env; bar_view setBackgroundColor:bar_bg];

    // Counter-rotate by +π/2 so the labels inside the bar read left-to-right
    // on the SDL display (matching the table). Same convention used by
    // UITableView's initWithFrame.
    let transform = CGAffineTransform::make_rotation(std::f32::consts::FRAC_PI_2);
    () = msg![env; bar_view setTransform:transform];
    () = msg![env; bar_view setFrame:bar_frame];

    // Inside the bar's *local* coords (post-rotation it's a horizontal strip
    // BAR_LANDSCAPE_WIDTH wide, 50 tall), drop in four labels evenly
    // distributed across the shortened width.
    let labels = ["Playlists", "Artists", "Albums", "Songs"];
    let label_width = BAR_LANDSCAPE_WIDTH / labels.len() as f32;
    for (i, name) in labels.iter().enumerate() {
        let label_frame = CGRect {
            origin: CGPoint {
                x: (i as f32) * label_width,
                y: 0.0,
            },
            size: CGSize {
                width: label_width,
                height: 50.0,
            },
        };
        let lbl: id = msg_class![env; UILabel alloc];
        let lbl: id = msg![env; lbl initWithFrame:label_frame];
        let text_ns =
            crate::frameworks::foundation::ns_string::from_rust_string(env, name.to_string());
        () = msg![env; lbl setText:text_ns];
        release(env, text_ns);
        // Highlight "Songs" (the active tab) in bright cyan; dim others.
        let color: id = if *name == "Songs" {
            ss_color_text_bright(env)
        } else {
            ss_color_text_dim(env)
        };
        () = msg![env; lbl setTextColor:color];
        let clear: id = msg_class![env; UIColor clearColor];
        () = msg![env; lbl setBackgroundColor:clear];
        // Center-align labels.
        () = msg![env; lbl setTextAlignment:1i32]; // NSTextAlignmentCenter
        () = msg![env; bar_view addSubview:lbl];
        release(env, lbl);
    }

    () = msg![env; window addSubview:bar_view];
    release(env, bar_view);
    log!("Synthetic iPod tab bar mounted on window {:?}", window);
}

/// Ensure exactly the cells in the current viewport (plus a small overscan)
/// are instantiated. Releases cells that scrolled out of view and asks the
/// data source for cells for any newly-visible rows. This is what makes the
/// picker viable for libraries with thousands of songs — without it, every
/// `reloadData` allocates one cell per row.
fn layout_visible_cells(env: &mut crate::Environment, table: id) {
    let (data_source, row_height, total_rows) = {
        let h = env.objc.borrow::<UITableViewHostObject>(table);
        (h.data_source, h.row_height, h.total_rows)
    };
    if data_source == nil || total_rows <= 0 || row_height <= 0.0 {
        return;
    }
    let total_rows = total_rows as usize;
    let bounds: CGRect = msg![env; table bounds];

    // One row of overscan above and below so the row at the edge of the
    // viewport doesn't flicker in/out as the user drags.
    let top = bounds.origin.y;
    let btm = bounds.origin.y + bounds.size.height;
    let first = ((top / row_height).floor() as isize - 1).max(0) as usize;
    let last_excl = (((btm / row_height).ceil() as isize) + 1).max(0) as usize;
    let last_excl = last_excl.min(total_rows);
    let first = first.min(last_excl);

    // Drop cells that fell out of the visible window.
    let current: Vec<(NSUInteger, id)> = env
        .objc
        .borrow::<UITableViewHostObject>(table)
        .cells_by_row
        .iter()
        .map(|(&r, &c)| (r, c))
        .collect();
    for (row, cell) in current.iter().copied() {
        let r = row as usize;
        if r < first || r >= last_excl {
            () = msg![env; cell removeFromSuperview];
            release(env, cell);
            env.objc
                .borrow_mut::<UITableViewHostObject>(table)
                .cells_by_row
                .remove(&row);
        }
    }

    // Add cells for newly-visible rows.
    for row in first..last_excl {
        let row_u = row as NSUInteger;
        if env
            .objc
            .borrow::<UITableViewHostObject>(table)
            .cells_by_row
            .contains_key(&row_u)
        {
            continue;
        }
        let idx: id = msg_class![env; NSIndexPath indexPathForRow:row_u inSection:0u32];
        let cell: id = msg![env; data_source tableView:table cellForRowAtIndexPath:idx];
        if cell == nil {
            continue;
        }
        retain(env, cell);
        // The game's UITableViewCell subclass forces a white background in
        // its own init, which paints over our dark-teal default. Knock it
        // out (and the contentView) to clear so the table's own panel
        // colour shows through uniformly — no per-row stripe.
        let cell_bg: id = msg_class![env; UIColor clearColor];
        () = msg![env; cell setBackgroundColor:cell_bg];
        let content_view: id = msg![env; cell contentView];
        if content_view != nil {
            () = msg![env; content_view setBackgroundColor:cell_bg];
        }
        let frame = CGRect {
            origin: CGPoint { x: 0.0, y: (row as f32) * row_height },
            size: CGSize { width: bounds.size.width, height: row_height },
        };
        () = msg![env; cell setFrame:frame];
        env.objc.borrow_mut::<UITableViewCellHostObject>(cell).table_view = table;
        env.objc.borrow_mut::<UITableViewCellHostObject>(cell).row = row_u;
        () = msg![env; table addSubview:cell];

        // Inject readable content since the game's custom cell uses Core
        // Graphics text APIs we don't render. Layout per row:
        //   [art swatch] | Song Title
        //                 Artist Name
        // The art swatch is a coloured placeholder (we don't extract actual
        // cover art yet — that requires lofty Picture decoding + UIImage
        // construction from raw bytes); coloured deterministically per song
        // so the picker isn't a wall of identical squares.
        if let Some(song) =
            crate::frameworks::media_player::music_library::song(row)
        {
            let art_size = row_height - 4.0;
            let art_x = 2.0;
            let title_x = art_size + 8.0;
            let title_width = bounds.size.width - title_x - 8.0;
            let title_height = row_height * 0.55;
            let artist_height = row_height - title_height;

            // Art view: use the bundled music-note placeholder PNG if
            // available, otherwise a per-song colored swatch as a fallback.
            let art_frame = CGRect {
                origin: CGPoint { x: art_x, y: 2.0 },
                size: CGSize { width: art_size, height: art_size },
            };
            let placeholder_img = get_or_load_placeholder_image(env);
            if placeholder_img != nil {
                let iv_cls = env.objc.get_known_class("UIImageView", &mut env.mem);
                let iv: id = msg![env; iv_cls alloc];
                let iv: id = msg![env; iv initWithFrame:art_frame];
                () = msg![env; iv setImage:placeholder_img];
                () = msg![env; cell addSubview:iv];
                release(env, iv);
            } else {
                let art: id = msg_class![env; UIView alloc];
                let art: id = msg![env; art initWithFrame:art_frame];
                let pid = song.persistent_id;
                let r = ((pid >> 5) & 0xFF) as f32 / 255.0;
                let g = ((pid >> 13) & 0xFF) as f32 / 255.0;
                let b = ((pid >> 21) & 0xFF) as f32 / 255.0;
                let swatch_color: id =
                    msg_class![env; UIColor colorWithRed:r green:g blue:b alpha:1.0f32];
                () = msg![env; art setBackgroundColor:swatch_color];
                () = msg![env; cell addSubview:art];
                release(env, art);
            }

            // Title label
            let label_cls = env.objc.get_known_class("UILabel", &mut env.mem);
            let title_frame = CGRect {
                origin: CGPoint { x: title_x, y: 0.0 },
                size: CGSize { width: title_width, height: title_height },
            };
            let title_lbl: id = msg![env; label_cls alloc];
            let title_lbl: id = msg![env; title_lbl initWithFrame:title_frame];
            let title_ns =
                crate::frameworks::foundation::ns_string::from_rust_string(env, song.title.clone());
            () = msg![env; title_lbl setText:title_ns];
            release(env, title_ns);
            let title_color: id = ss_color_text_bright(env);
            () = msg![env; title_lbl setTextColor:title_color];
            let clear: id = msg_class![env; UIColor clearColor];
            () = msg![env; title_lbl setBackgroundColor:clear];
            () = msg![env; cell addSubview:title_lbl];
            release(env, title_lbl);

            // Artist label (smaller, dimmer)
            let artist_frame = CGRect {
                origin: CGPoint { x: title_x, y: title_height },
                size: CGSize { width: title_width, height: artist_height },
            };
            let artist_lbl: id = msg![env; label_cls alloc];
            let artist_lbl: id = msg![env; artist_lbl initWithFrame:artist_frame];
            let artist_ns =
                crate::frameworks::foundation::ns_string::from_rust_string(env, song.artist.clone());
            () = msg![env; artist_lbl setText:artist_ns];
            release(env, artist_ns);
            let artist_color: id = ss_color_text_dim(env);
            () = msg![env; artist_lbl setTextColor:artist_color];
            () = msg![env; artist_lbl setBackgroundColor:clear];
            // Smaller font for the artist subscript.
            let small_font: id = msg_class![env; UIFont systemFontOfSize:12.0f32];
            () = msg![env; artist_lbl setFont:small_font];
            () = msg![env; cell addSubview:artist_lbl];
            release(env, artist_lbl);
        }

        env.objc
            .borrow_mut::<UITableViewHostObject>(table)
            .cells_by_row
            .insert(row_u, cell);
    }
}
