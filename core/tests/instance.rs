//! Headless probe for instance stepping (no wgpu). Mirrors the instance logic
//! in `render_node` to check that independent children actually advance.

use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssruntime::{Context, InstanceSlot, SsabData, SsabResource};
use std::sync::Arc;

fn load(path: &str) -> Arc<SsabResource> {
    let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("read {path}"));
    SsabResource::new(SsabData::load_copy(&bytes).unwrap())
}

/// (part_index, ref_pack, ref_anime) of the first instance part.
fn instance_ref(ctx: &Context<'static>) -> Option<(usize, String, String)> {
    let bytes = ctx.get_ssab_data()?;
    let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes) };
    for (i, part) in ab.parts().iter().enumerate() {
        if part.part_type_type() == PartType::PartTypeInstance
            && let Some(inst) = part.part_type_as_part_type_instance()
        {
            return Some((i, inst.ref_anime_pack().to_string(), inst.ref_anime_name().to_string()));
        }
    }
    None
}

fn probe(anim: &str, frames: usize) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/overall/overall");
    let parent_res = load(&format!("{dir}/Instance.ssab"));

    eprintln!("=== Instance.ssab animations: {:?}", parent_res.resource().map(|r| r.get_animation_names()));

    let mut parent = Context::new();
    parent.bind_resource(unsafe { &*parent_res.resource_static_ptr() });
    // Skip (not fail) when the fixture pack lacks this animation, so the probe
    // harness is a no-op on the minimal fixtures but runs on richer ssab.
    if parent.setup_animation(anim).is_err() {
        eprintln!("=== {anim}: not in Instance.ssab, skipping ===");
        return;
    }
    parent.play();

    let (part_idx, ref_pack, ref_anime) = instance_ref(&parent).expect("instance part");
    eprintln!("=== {anim} (part_idx={part_idx}, ref={ref_pack}/{ref_anime}) ===");

    let child_res = load(&format!("{dir}/{ref_pack}.ssab"));
    let mut child = Context::new();
    child.bind_resource(unsafe { &*child_res.resource_static_ptr() });
    child.setup_animation(&ref_anime).unwrap();

    let mut slot = InstanceSlot::new();
    let mut prev = 0.0f32;
    let mut last_parent = 0.0f32;
    let dt = 1.0 / 60.0;

    for f in 0..frames {
        parent.update(dt * 1000.0);
        let pf = parent.get_frame_no();
        let parent_looped = pf + 0.5 < last_parent;
        last_parent = pf;
        let _ = parent.get_frame_data(pf);
        let ev = parent.get_active_event_instance(part_idx as u32);
        let r = slot.step(ev, &mut child, prev, pf, dt, parent_looped);
        if r.transitioned {
            child.play();
        }
        let cf = child.get_frame_no();
        prev = cf;
        eprintln!("f{f:03} parent={pf:6.1} plooped={} child={cf:7.2} vis={} indep={} speed={:.1} trans={} clooped={}", parent_looped as u8, r.visible, r.independent, ev.speed, r.transitioned, r.child_looped);
    }
}

#[test]
fn probe_anime_1() {
    probe("anime_1", 200);
}

// These run only if the fixture Instance.ssab contains richer animations
// (full ssae conversion); otherwise they skip. Use to diagnose INDEP glitches.
#[test]
fn probe_indep_label_pingpong() {
    probe("INDEP_LABEL_PINGPONG", 400);
}

#[test]
fn probe_indep_speed_2x() {
    probe("INDEP_SPEED_2x", 120);
}

#[test]
fn scan_all_ssab() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/overall/overall");
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.extension().and_then(|s| s.to_str()) == Some("ssab") {
            let res = load(path.to_str().unwrap());
            let names = res.resource().map(|r| r.get_animation_names()).unwrap_or_default();
            eprintln!("{:<28} {:?}", path.file_name().unwrap().to_str().unwrap(), names);
        }
    }
}
