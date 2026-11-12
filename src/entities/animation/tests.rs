//! Clip format, interpolation, baking, selection and the shipped clips' contracts.
use super::{
    bake, catalog,
    format::{self, Curve, Key, Lerp, Looping, Part},
    receive_swing, select, Swing, SwingKind, ACTION, FLINCH,
};
use crate::{
    entities::{
        animation_tests::fixture, clip_pose, metadata::Metadata, presentation, resources::Rig,
        krasnokamen_models, Entity, Models,
    },
    inventory::Stack,
    protocol::content::Content,
};
use glam::{Mat4, Vec3};
use std::sync::Arc;

fn key(time: f32, value: [f32; 3], lerp: Lerp) -> Key {
    let value = Vec3::from_array(value);
    Key { time, pre: value, post: value, lerp, split: false }
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn clip_format_parses_bedrock_channels_loops_and_contact() {
    let clips = format::parse(r#"{"format_version":"1.8.0","animations":{
        "animation.test.a":{"animation_length":1.0,"loop":"hold_on_last_frame",
            "override_previous_animation":true,"timeline":{"0.4":"contact"},
            "bones":{"right_arm":{"rotation":{"0.0":[0,0,0],"0.5":{"pre":[10,0,0],"post":[20,0,0],"lerp_mode":"catmullrom"},"1.0":[30,0,0]},
                                  "position":[1,2,3]},
                     "root":{"scale":{"0.0":1,"0.2":{"post":1.0,"lerp_mode":"step"}}}}},
        "animation.test.b":{"loop":true,"bones":{"head":{"rotation":{"0.25":[0,5,0],"0.75":[0,-5,0]}}}}}}"#)
    .unwrap();
    assert_eq!(clips.len(), 2);
    let a = &clips[0];
    assert_eq!((a.name.as_str(), a.length, a.looping, a.replace, a.contact), ("animation.test.a", 1., Looping::Hold, true, Some(0.4)));
    let arm = a.tracks.iter().find(|track| track.part == Part::RightArm).unwrap();
    let keys = &arm.rotation.as_ref().unwrap().keys;
    assert_eq!(keys.len(), 3);
    assert_eq!((keys[1].pre, keys[1].post, keys[1].lerp, keys[1].split),
        (Vec3::new(10., 0., 0.), Vec3::new(20., 0., 0.), Lerp::CatmullRom, true));
    assert_eq!(arm.position.as_ref().unwrap().keys, vec![key(0., [1., 2., 3.], Lerp::Linear)]);
    let root = a.tracks.iter().find(|track| track.part == Part::Root).unwrap();
    assert_eq!(root.scale.as_ref().unwrap().keys[1].lerp, Lerp::Step);
    let b = &clips[1];
    assert_eq!((b.length, b.looping, b.replace, b.contact), (0.75, Looping::Loop, false, None), "length defaults to the last key");
}

#[test]
fn clip_format_rejects_what_the_runtime_cannot_honour() {
    let wrap = |clip: &str| format!(r#"{{"format_version":"1.8.0","animations":{{"animation.test":{clip}}}}}"#);
    for (clip, why) in [
        (r#"{"bones":{"tail":{"rotation":[1,2,3]}}}"#, "unknown bone"),
        (r#"{"bones":{"head":{"rotation":["math.sin(query.anim_time)",0,0]}}}"#, "MoLang"),
        (r#"{"bones":{"head":{"rotation":{"0":{"post":[1,2,3],"lerp_mode":"bezier"}}}}}"#, "bezier"),
        (r#"{"animation_length":1,"timeline":{"0.5":"contact","0.6":"contact"},"bones":{"head":{"rotation":[0,0,0]}}}"#, "two contacts"),
        (r#"{"animation_length":1,"timeline":{"0.5":"/say hi"},"bones":{"head":{"rotation":[0,0,0]}}}"#, "command"),
        (r#"{"animation_length":1,"timeline":{"1.5":"contact"},"bones":{"head":{"rotation":[0,0,0]}}}"#, "contact past the end"),
        (r#"{"loop":"forever","bones":{"head":{"rotation":{"1":[0,0,0]}}}}"#, "loop mode"),
        (r#"{"bones":{"head":{}}}"#, "empty bone"),
        (r#"{"bones":{"head":{"rotation":{"-1":[0,0,0]}}}}"#, "negative time"),
        (r#"{"bones":{"head":{"rotation":[0,0,0]}}}"#, "zero length"),
    ] {
        assert!(format::parse(&wrap(clip)).is_err(), "{why} must be rejected");
    }
    assert!(format::parse(r#"{"format_version":"1.10.0","animations":{}}"#).is_err());
}

#[test]
fn clip_interpolation_linear_step_and_edges() {
    let curve = Curve { keys: vec![key(0., [0., 0., 0.], Lerp::Linear), key(1., [10., -20., 4.], Lerp::Step), key(2., [0., 0., 0.], Lerp::Linear)] };
    assert!(close(curve.sample(0.25, None), Vec3::new(2.5, -5., 1.)));
    assert!(close(curve.sample(1.6, None), Vec3::new(10., -20., 4.)), "step holds until the next key");
    assert!(close(curve.sample(-1., None), Vec3::ZERO));
    assert!(close(curve.sample(5., None), Vec3::ZERO));
    let split = Curve { keys: vec![
        key(0., [0.; 3], Lerp::Linear),
        Key { time: 1., pre: Vec3::splat(1.), post: Vec3::splat(5.), lerp: Lerp::Linear, split: true },
        key(2., [5.; 3], Lerp::Linear),
    ] };
    assert!(close(split.sample(0.5, None), Vec3::splat(0.5)), "approach uses pre");
    assert!(close(split.sample(1., None), Vec3::splat(5.)), "from the key on, post applies");
}

#[test]
fn clip_interpolation_catmull_rom_passes_keys_with_neighbour_tangents() {
    let keys = [0., 1., 4., 4.];
    let curve = Curve { keys: keys.iter().enumerate()
        .map(|(i, v)| key(i as f32, [*v, 0., 0.], Lerp::CatmullRom)).collect() };
    for (i, v) in keys.iter().enumerate() {
        assert!((curve.sample(i as f32, None).x - v).abs() < 1e-5, "passes key {i}");
    }
    // Segment 1..2 with neighbours 0 and 4: three.js uniform Catmull-Rom at t = 0.5.
    let expected = 0.5 * (2. * 1. + (-0. + 4.) * 0.5 + (2. * 0. - 5. * 1. + 4. * 4. - 4.) * 0.25
        + (-0. + 3. * 1. - 3. * 4. + 4.) * 0.125);
    assert!((curve.sample(1.5, None).x - expected).abs() < 1e-5);
    assert!((curve.sample(1.5, None).x - 2.5).abs() > 0.05, "differs from linear");
    // The first segment repeats its start for the missing neighbour.
    let edge = 0.5 * (2. * 0. + (1. - 0.) * 0.5 + (2. * 0. - 5. * 0. + 4. * 1. - 4.) * 0.25
        + (-0. + 3. * 0. - 3. * 1. + 4.) * 0.125);
    assert!((curve.sample(0.5, None).x - edge).abs() < 1e-5);
    // A split end stops the neighbour from shaping the segment.
    let mut split = curve.clone();
    split.keys[1].split = true;
    let flat_start = 0.5 * (2. * 1. + (4. - 1.) * 0.5 + (2. * 1. - 5. * 1. + 4. * 4. - 4.) * 0.25
        + (-1. + 3. * 1. - 3. * 4. + 4.) * 0.125);
    assert!((split.sample(1.5, None).x - flat_start).abs() < 1e-5);
}

fn shipped() -> Vec<format::Clip> {
    super::SOURCES.iter().flat_map(|(file, source)| {
        format::parse(source).unwrap_or_else(|error| panic!("{file}: {error}"))
    }).collect()
}

#[test]
fn shipped_clips_parse_bake_and_install_every_selectable_clip() {
    let clips = shipped();
    for name in select::ALL {
        let clip = clips.iter().find(|clip| clip.name == name).unwrap_or_else(|| panic!("{name} missing"));
        let table = bake::bake(clip).unwrap();
        assert_eq!(table.frames.len(), bake::frame_count(clip.length), "{name}");
        assert!(catalog().get(name).is_some(), "{name} installed");
    }
    assert_eq!(clips.len(), select::ALL.len(), "no unused clips ship");
}

#[test]
fn bake_envelope_keeps_contact_at_full_weight_and_marks_replaced_channels() {
    for clip in shipped() {
        let table = bake::bake(&clip).unwrap();
        let at = |time: f32| &table.frames[(time * clip_pose::BAKE_HZ).round() as usize];
        if let Some(contact) = clip.contact {
            assert_eq!(bake::envelope(&clip, contact), 1., "{}", clip.name);
        }
        let first = at(0.);
        let last = table.frames.last().unwrap();
        match clip.looping {
            Looping::Once | Looping::Hold => assert!(
                first.iter().all(|s| s.weight == 0. && s.rotation == [0.; 3] && s.offset == [0.; 3]),
                "{} fades in from the procedural pose", clip.name),
            Looping::Loop => {}
        }
        match clip.looping {
            Looping::Once => assert!(last.iter().all(|s| s.weight == 0. && s.rotation == [0.; 3]),
                "{} fades out", clip.name),
            Looping::Hold => assert!(clip.replace && last[Part::Root.channel() as usize].weight == 1.,
                "{} holds its last frame at full weight", clip.name),
            Looping::Loop => {}
        }
        let middle = at(clip.contact.unwrap_or(clip.length * 0.5));
        for track in &clip.tracks {
            let sample = middle[track.part.channel() as usize];
            let replaced = clip.replace && matches!(track.part, Part::RightArm | Part::LeftArm | Part::Root);
            assert_eq!(sample.weight, if replaced { 1. } else { 0. }, "{} {:?}", clip.name, track.part);
        }
    }
}

#[test]
fn bake_rejects_scale_and_a_contact_inside_a_fade() {
    let parse = |clip: &str| format::parse(&format!(r#"{{"format_version":"1.8.0","animations":{{"animation.test":{clip}}}}}"#)).unwrap().remove(0);
    assert!(bake::bake(&parse(r#"{"animation_length":1,"bones":{"head":{"scale":2}}}"#)).is_err());
    assert!(bake::bake(&parse(r#"{"animation_length":1,"bones":{"head":{"scale":1}}}"#)).is_ok());
    assert!(bake::bake(&parse(r#"{"animation_length":1,"timeline":{"0.05":"contact"},"bones":{"head":{"rotation":[0,0,0]}}}"#)).is_err());
}

/// Contact tick when a clip plays over `duration` ticks as `receive_swing` stretches it.
fn contact_tick(name: &str, duration: u32) -> f32 {
    let clip = shipped().into_iter().find(|clip| clip.name == name).unwrap();
    let rate = clip.length / (duration as f32 / 20.);
    clip.contact.unwrap() / rate * 20.
}

#[test]
fn duration_scaling_puts_contact_at_the_contract_fraction() {
    // PLAN.md line 16 (melee) and the BvCombatActions emission for ranged and shields.
    for (name, duration, windup) in [
        (select::SWORD_COMBO[0], 21, 10.), (select::SWORD_COMBO[1], 21, 10.), (select::SWORD_COMBO[2], 21, 10.),
        (select::MOUNTED_SWORD, 21, 10.), (select::SPEAR, 23, 11.), (select::LANCE, 23, 11.),
        (select::MACE, 25, 12.), (select::BOW, 40, 30.), (select::CROSSBOW, 75, 30.), (select::SHIELD, 10, 2.),
    ] {
        let tick = contact_tick(name, duration);
        assert!((tick - windup).abs() < 1e-3, "{name}: contact at {tick} of {duration}, contract {windup}");
        // Any other duration keeps the same fraction (vanilla 6-tick swings included).
        assert!((contact_tick(name, 6) / 6. - windup / duration as f32).abs() < 1e-5, "{name}");
    }
}

fn armed_fixture(item: &str) -> (Models, Entity, Content) {
    let (mut models, mut e) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.accessors.insert("DATA_PLAYER_MAIN_HAND".into(), [15, 42]);
    let motion = models.motion.remove("minecraft:pig").unwrap();
    e.type_name = "minecraft:player".into();
    models.schemas.insert(e.type_name.clone(), schema);
    models.motion.insert(e.type_name.clone(), motion);
    e.metadata.insert(15, Metadata::Arm(1));
    let mut content = Content::test_cube();
    content.item_names.insert(1, item.into());
    content.item_names.insert(2, "minecraft:shield".into());
    content.item_names.insert(3, "minecraft:stick".into());
    e.equipment[0] = Some(stack(1));
    (models, e, content)
}

fn stack(id: u32) -> Stack {
    let mut stack = Stack::default();
    stack.id = id;
    stack.count = 1;
    stack.components = Arc::new(Default::default());
    stack
}

fn id(name: &str) -> u16 {
    catalog().get(name).unwrap().id
}

/// Regression: the client used to count `swing_animation` as an ignored presentation
/// packet (`session/packets.rs`), so server attack presentations never animated anyone.
#[test]
fn received_swing_animation_packet_starts_the_clip_and_reports_it() {
    use crate::protocol::{packet_ids::play::clientbound as cb, Reader};
    let (models, entity, content) = armed_fixture("minecraft:iron_sword");
    let mut store = crate::entities::Store::new();
    store.models = Some(Arc::new(models));
    store.slots = vec![entity];
    store.ids.insert(41, 0);
    // 26.3 wire body: VarInt entity 41, hand 0 (main), type 1 (whack), duration 21.
    assert!(store.apply(cb::SWING_ANIMATION, &mut Reader::new(&[41, 0, 1, 21]), &content, 100).unwrap());
    let layer = store.get(41).unwrap().clip.layers[ACTION];
    assert_eq!((layer.clip, layer.start_tick), (id(select::SWORD_COMBO[0]), 100));
    assert!((layer.rate - 1.05 / (21. / 20.)).abs() < 1e-6, "stretched to 21 ticks: {}", layer.rate);
    let reported = store.events.iter().any(|event| {
        event["type"] == "entity_swing" && event["clip"] == select::SWORD_COMBO[0] && event["duration"] == 21
    });
    assert!(reported, "{:?}", store.events);
    for malformed in [[41, 2, 1, 21], [41, 0, 3, 21]] {
        assert!(store.apply(cb::SWING_ANIMATION, &mut Reader::new(&malformed), &content, 101).is_err(), "{malformed:?}");
    }
    assert!(store.apply(cb::SWING_ANIMATION, &mut Reader::new(&[41, 0, 1, 21, 0]), &content, 101).is_err(), "trailing bytes");
}

#[test]
fn sword_swings_cycle_the_three_hit_combo() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    let mut played = Vec::new();
    for tick in 0..4 {
        let swing = receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, tick * 21);
        assert!(matches!(swing, Swing::Clip(..)), "{swing:?}");
        let layer = e.clip.layers[ACTION];
        assert_eq!((layer.start_tick, layer.weight, layer.mirror), (tick * 21, 1., false));
        played.push(layer.clip);
    }
    let combo = select::SWORD_COMBO.map(id);
    assert_eq!(played, [combo[0], combo[1], combo[2], combo[0]]);
    assert!(e.animation.is_none(), "a clip replaces the vanilla swing");
}

#[test]
fn selection_follows_item_class_type_and_mount() {
    let (models, mut e, mut content) = armed_fixture("minecraft:iron_spear");
    let mut play = |e: &mut Entity, content: &Content, hand, kind| {
        receive_swing(e, Some(&models), content, hand, kind, 23, 0);
        e.clip.layers[ACTION].clip
    };
    assert_eq!(play(&mut e, &content, 0, SwingKind::Stab), id(select::SPEAR));
    e.vehicle = Some(crate::entities::Handle { network_id: 7, generation: 1 });
    assert_eq!(play(&mut e, &content, 0, SwingKind::Stab), id(select::LANCE));
    content.item_names.insert(1, "minecraft:diamond_sword".into());
    assert_eq!(play(&mut e, &content, 0, SwingKind::Whack), id(select::MOUNTED_SWORD));
    e.vehicle = None;
    assert_eq!(play(&mut e, &content, 0, SwingKind::None), id(select::STAGGER));
    for (name, clip) in [("minecraft:mace", select::MACE), ("minecraft:iron_axe", select::MACE),
        ("minecraft:bow", select::BOW), ("minecraft:crossbow", select::CROSSBOW)]
    {
        content.item_names.insert(1, name.into());
        assert_eq!(play(&mut e, &content, 0, SwingKind::Whack), id(clip), "{name}");
    }
    e.equipment[1] = Some(stack(2));
    assert_eq!(play(&mut e, &content, 1, SwingKind::Whack), id(select::SHIELD));
    assert!(!e.clip.layers[ACTION].mirror, "a right-handed entity's off-hand shield is the authored left arm");
}

#[test]
fn left_arm_swings_mirror_right_handed_clips() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 0);
    assert!(!e.clip.layers[ACTION].mirror);
    e.metadata.insert(15, Metadata::Arm(0));
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 0);
    assert!(e.clip.layers[ACTION].mirror, "left-handed main hand");
    e.equipment[1] = Some(stack(2));
    receive_swing(&mut e, Some(&models), &content, 1, SwingKind::Whack, 10, 0);
    assert!(e.clip.layers[ACTION].mirror, "a left-handed entity holds its off-hand shield in the right arm");
    e.metadata.insert(15, Metadata::Arm(1));
    e.equipment[1] = Some(stack(1));
    receive_swing(&mut e, Some(&models), &content, 1, SwingKind::Whack, 21, 0);
    assert!(e.clip.layers[ACTION].mirror, "off-hand sword of a right-handed entity is the left arm");
}

#[test]
fn unmatched_swings_fall_back_to_the_vanilla_swing() {
    let (models, mut e, content) = armed_fixture("minecraft:stick");
    let swing = receive_swing(&mut e, Some(&models), &content, 1, SwingKind::Whack, 9, 40);
    assert_eq!(swing, Swing::Vanilla, "empty off hand");
    assert_eq!((e.animation, e.physical.combat.vanilla_ticks()), (Some((3, 40)), 9));
    assert_eq!(e.clip, Default::default());
    e.equipment[0] = Some(stack(3));
    assert_eq!(receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Stab, 6, 50), Swing::Vanilla);
    assert_eq!(e.animation, Some((0, 50)));
    assert_eq!(receive_swing(&mut e, Some(&models), &content, 0, SwingKind::None, 6, 60), Swing::Ignored);
    assert_eq!(e.animation, Some((0, 50)), "vanilla none plays no swing");
    assert_eq!(receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 0, 70), Swing::Ignored);
}

#[test]
fn hurt_and_death_clips_only_play_on_armed_entities() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    super::on_hurt(&mut e, 5);
    super::on_death(&mut e, 6);
    assert_eq!(e.clip, Default::default(), "an entity that never swung keeps the vanilla hurt and death");
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 10);
    super::on_hurt(&mut e, 12);
    assert_eq!(e.clip.layers[FLINCH].clip, id(select::HURT));
    assert_eq!(e.clip.layers[ACTION].clip, id(select::SWORD_COMBO[0]), "the flinch layers over the swing");
    e.death_tick = Some(20);
    super::on_death(&mut e, 20);
    assert_eq!((e.clip.layers[ACTION].clip, e.clip.layers[ACTION].start_tick), (id(select::DEATH), 20));
    assert_eq!(e.clip.layers[FLINCH].clip, 0);
    assert_eq!(receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 21), Swing::Ignored);
    assert_eq!(e.clip.layers[ACTION].clip, id(select::DEATH), "the dead do not swing");
}

fn zombie() -> &'static Rig {
    krasnokamen_models::authored("minecraft:zombie#main").expect("authored zombie rig")
}

fn pose(e: &Entity, models: &Models, content: &Content, tick: u64, partial: f32) -> Vec<Mat4> {
    let mut out = Vec::new();
    presentation::evaluate_rig(zombie(), e, models, content, tick, partial, Mat4::IDENTITY, &mut out);
    out
}

fn bone(name: &str) -> usize {
    zombie().part_by_name[name]
}

#[test]
fn swing_clip_drives_the_rig_and_leaves_vanilla_untouched_without_one() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    let idle = pose(&e, &models, &content, 10, 0.);
    super::on_hurt(&mut e, 10);
    super::on_death(&mut e, 10);
    assert_eq!(e.clip, Default::default());
    assert_eq!(pose(&e, &models, &content, 10, 0.), idle, "hurt and death of an unarmed entity change no bone");
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 10);
    assert_eq!(pose(&e, &models, &content, 10, 0.), idle, "clip time 0 blends from the vanilla pose");
    let contact = pose(&e, &models, &content, 20, 0.);
    let arm = bone("right_arm");
    let reach = contact[arm].transform_vector3(Vec3::Y);
    assert!(reach.z < -0.8, "the slash reaches forward at contact: {reach:?}");
    assert_eq!(pose(&e, &models, &content, 32, 0.), idle, "the clip has ended after 21 ticks");
}

#[test]
fn mirrored_swing_reflects_the_arms() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 0);
    let right = pose(&e, &models, &content, 8, 0.);
    e.clip.layers[ACTION].mirror = true;
    let left = pose(&e, &models, &content, 8, 0.);
    let reflect = Mat4::from_scale(Vec3::new(-1., 1., 1.));
    for (a, b) in [("right_arm", "left_arm"), ("left_arm", "right_arm"), ("head", "head")] {
        let expected = reflect * right[bone(a)] * reflect;
        let actual = left[bone(b)];
        assert!(expected.abs_diff_eq(actual, 1e-4), "{a} mirrors to {b}:\n{expected:?}\n{actual:?}");
    }
}

#[test]
fn legs_keep_walking_during_an_attack() {
    let (models, mut e, content) = armed_fixture("minecraft:iron_sword");
    receive_swing(&mut e, Some(&models), &content, 0, SwingKind::Whack, 21, 0);
    let standing = pose(&e, &models, &content, 10, 0.);
    let leg = bone("left_leg");
    assert!(standing[leg].transform_vector3(Vec3::Y).z < -0.2, "standing slash steps the left foot forward");
    let walk = |e: &mut Entity, x: f64| {
        let mut source = e.source.take().unwrap();
        source.living.as_mut().unwrap().tick(&models, e, &crate::entities::render_living::TickInput {
            age: 1, position: [x, 0., 0.], yaw: 0., alive: true, passenger: false, baby: false,
            visually_swimming: false, in_water: false, uses_y_for_walk: false,
            rotation_changed_before_base_tick: false, is_mob: true, first_passenger_is_mob: false,
            max_head_yaw: 75., max_body_head_yaw: 75., attack_active: false, camel_sitting: false,
            camel_visually_sitting: false, camel_sitting_down: false, camel_pose_transition: false,
            camel_pose_time: 0,
        });
        e.source = Some(source);
    };
    let mut x = 0.;
    for _ in 0..6 {
        x += 0.3;
        walk(&mut e, x);
    }
    let first = pose(&e, &models, &content, 10, 0.);
    x += 0.3;
    walk(&mut e, x);
    let second = pose(&e, &models, &content, 10, 0.);
    assert_ne!(first[leg], second[leg], "the gait still moves the legs mid-attack");
    assert!((first[bone("right_arm")].transform_vector3(Vec3::Y)
        - second[bone("right_arm")].transform_vector3(Vec3::Y)).length() < 1e-4,
        "the arms follow the clip, not the gait");
}

/// Mounted strips wait for core mob riders (B1+B3); until then the rig contract is
/// checked here: the couched lance and the mounted cut reach forward at contact while
/// the legs keep the riding pose.
#[test]
fn mounted_clips_strike_forward_and_keep_the_seat() {
    for (item, kind, clip, duration, contact) in [
        ("minecraft:iron_spear", SwingKind::Stab, select::LANCE, 23, 11),
        ("minecraft:iron_sword", SwingKind::Whack, select::MOUNTED_SWORD, 21, 10),
    ] {
        let (models, mut e, content) = armed_fixture(item);
        e.vehicle = Some(crate::entities::Handle { network_id: 7, generation: 1 });
        let seated = pose(&e, &models, &content, 0, 0.);
        assert_eq!(receive_swing(&mut e, Some(&models), &content, 0, kind, duration, 0), Swing::Clip(clip, e.vehicle));
        let strike = pose(&e, &models, &content, contact, 0.);
        let reach = strike[bone("right_arm")].transform_vector3(Vec3::Y);
        assert!(reach.z < -0.3, "{clip}: the strike reaches forward: {reach:?}");
        for leg in ["right_leg", "left_leg"] {
            let (a, b) = (seated[bone(leg)].transform_vector3(Vec3::Y), strike[bone(leg)].transform_vector3(Vec3::Y));
            assert!((a - b).length() < 0.05, "{clip}: {leg} stays seated: {a:?} vs {b:?}");
        }
    }
}
