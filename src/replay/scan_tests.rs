use super::*;
use crate::protocol::Writer;

fn components(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn side_comes_from_battle_shield_models_and_equippable_assets() {
    assert_eq!(side_of_components(&components(&[("minecraft:item_model", json!("solaris_battle:red_heater_shield"))])), Some(Side::Red));
    assert_eq!(side_of_components(&components(&[("minecraft:item_model", json!("minecraft:shield")),
        ("minecraft:equippable", json!({"slot":"chest","asset_id":"solaris_battle:blue_surcoat"}))])), Some(Side::Blue));
    assert_eq!(side_of_components(&components(&[("minecraft:equippable", json!({"slot":"body","asset_id":"solaris_battle:red_caparison"}))])), Some(Side::Red));
    // Side-neutral battle visuals and vanilla items say nothing about the side.
    assert_eq!(side_of_components(&components(&[("minecraft:equippable", json!({"slot":"head","asset_id":"solaris_battle:iron_plate"}))])), None);
    assert_eq!(side_of_components(&components(&[("minecraft:item_model", json!("solaris_battle:iron_longsword"))])), None);
    assert_eq!(side_of_components(&components(&[("minecraft:item_model", json!("minecraft:red_banner"))])), None);
}

#[test]
fn spawn_split_separates_armies_across_the_gap_not_along_the_frontage() {
    // 200-wide frontage along x with regiment gaps of 3 blocks; armies 40 blocks apart on z.
    let mut points = Vec::new();
    for regiment in 0..4 {
        for x in 0..47 {
            for rank in 0..5 {
                let x = -100. + f64::from(regiment) * 50. + f64::from(x);
                points.push([x, -30. - f64::from(rank)]);
                points.push([x, 10. + f64::from(rank)]);
            }
        }
    }
    let split = spawn_split(&points).unwrap();
    assert_eq!(split.axis, 1);
    assert!((split.threshold - -10.).abs() < 1e-9);
    assert_eq!(split.side([0., -34.]), Side::Red);
    assert_eq!(split.side([0., 14.]), Side::Blue);
    // One dense blob has no gap: no split rather than an arbitrary one.
    let blob: Vec<[f64; 2]> = (0..100).map(|i| [f64::from(i % 10), f64::from(i / 10)]).collect();
    assert_eq!(spawn_split(&blob), None);
}

#[test]
fn contact_clusters_need_opposite_sides_within_reach() {
    let mut fighters = Vec::new();
    let mut id = 0;
    let mut add = |x: f64, z: f64, side| { id += 1; fighters.push(Fighter { id, position: [x, 66., z], side }); };
    // A 10-block front: red on z=0, blue on z=2 (in reach), blue reserves at z=8 (out of reach).
    for x in 0..10 {
        let x = f64::from(x);
        add(x, 0., Side::Red);
        add(x, 2., Side::Blue);
        add(x, 8., Side::Blue);
    }
    // A separate duel 100 blocks away, and a same-side pair that never counts.
    add(100., 0., Side::Red);
    add(101., 0.5, Side::Blue);
    add(-50., 0., Side::Red);
    add(-50., 1., Side::Red);
    // Opposite sides within horizontal reach but a floor apart are not fighting.
    add(-80., 0., Side::Red);
    add(-80., 1., Side::Blue);
    fighters.last_mut().unwrap().position[1] = 70.;
    let clusters = contact_clusters(&fighters, CONTACT_REACH, 4.);
    assert_eq!(clusters.len(), 2);
    let front = &clusters[0];
    assert_eq!((front.members.len(), front.red, front.blue), (20, 10, 10));
    assert!(front.members.iter().all(|&m| fighters[m].position[2] < 3.));
    assert!((front.centroid[0] - 4.5).abs() < 1e-9 && (front.centroid[2] - 1.).abs() < 1e-9);
    assert!((front.normal[0]).abs() < 1e-9 && (front.normal[1] - 1.).abs() < 1e-9, "red->blue is +z: {:?}", front.normal);
    assert!((front.line[0].abs() - 1.).abs() < 1e-9 && front.line[1].abs() < 1e-9, "line runs along x: {:?}", front.line);
    assert!((front.length - 9.).abs() < 1e-9 && (front.depth - 2.).abs() < 1e-9);
    assert_eq!((front.min, front.max), ([0., 0.], [9., 2.]));
    let duel = &clusters[1];
    assert_eq!((duel.red, duel.blue), (1, 1));
}

#[test]
fn mixed_melee_uses_the_long_axis_as_contact_line() {
    // Alternating sides on one row along z: side centroids coincide.
    let fighters: Vec<Fighter> = (0..8).map(|i| Fighter { id: i, position: [5., 66., f64::from(i) * 1.5],
        side: if i % 2 == 0 { Side::Red } else { Side::Blue } }).collect();
    let clusters = contact_clusters(&fighters, CONTACT_REACH, 4.);
    assert_eq!(clusters.len(), 1);
    let mixed = &clusters[0];
    assert_eq!(mixed.members.len(), 8);
    // Red mean z 4.5, blue 6.0: separation 1.5 still defines the normal (+z).
    assert!((mixed.normal[1] - 1.).abs() < 1e-9);
    let equal: Vec<Fighter> = [(0., Side::Red), (1., Side::Blue), (2., Side::Blue), (3., Side::Red)].iter().enumerate()
        .map(|(i, &(x, side))| Fighter { id: i as i32, position: [x, 66., 0.], side }).collect();
    let line = contact_clusters(&equal, CONTACT_REACH, 4.).remove(0);
    assert!((line.line[0].abs() - 1.).abs() < 1e-6 && line.line[1].abs() < 1e-6, "{:?}", line.line);
}

#[test]
fn groups_link_chains_and_keep_far_points_apart() {
    let points = [[0., 0., 0.], [5., 0., 0.], [10., 0., 0.], [30., 0., 0.], [31., 0., 0.]];
    let groups = groups(&points, 6.);
    assert_eq!(groups, vec![vec![0, 1, 2], vec![3, 4]]);
}

#[test]
fn column_tops_put_the_floor_under_leaves_and_grass() {
    use crate::protocol::world::{Palette, Section};
    use std::sync::Arc;
    let section = |state: u32, (x, y, z): (usize, usize, usize), other: u32| {
        // One block `other` at (x, y, z) in a section of `state` (4-bit palette).
        let mut words = vec![0u64; 256];
        let index = y * 256 + z * 16 + x;
        words[index / 16] |= 1 << ((index % 16) * 4);
        Arc::new(Section { blocks: Palette { bits: 4, values: vec![state, other], words, size: 4096 },
            biomes: Palette { bits: 0, values: vec![0], words: Vec::new(), size: 64 }, non_empty_block_count: 1 })
    };
    // 0 air, 1 stone, 2 leaves/grass: stone below -32, a leaf at (3, -20, 5), a stone pillar top at (7, -28, 0).
    let sections = vec![section(1, (0, 0, 0), 1), section(1, (0, 0, 0), 1), section(0, (3, 12, 5), 2), section(0, (0, 0, 0), 0)];
    let mut sections = sections;
    sections[2] = {
        let mut words = vec![0u64; 256];
        for (index, value) in [(12 * 256 + 5 * 16 + 3, 2u64), (4 * 256 + 7, 1)] { words[index / 16] |= value << ((index % 16) * 4); }
        Arc::new(Section { blocks: Palette { bits: 4, values: vec![0, 1, 2], words, size: 4096 },
            biomes: Palette { bits: 0, values: vec![0], words: Vec::new(), size: 64 }, non_empty_block_count: 2 })
    };
    let kind = |state| Ok(match state { 0 => Terrain::Air, 1 => Terrain::Ground, _ => Terrain::Cover });
    let [surface, ground] = column_tops(&sections, -64, kind).unwrap();
    let column = |x: usize, z: usize| x + 16 * z;
    assert_eq!((surface[column(0, 0)], ground[column(0, 0)]), (-32, -32));
    assert_eq!((surface[column(3, 5)], ground[column(3, 5)]), (-19, -32), "the leaf is cover, the floor stays below it");
    assert_eq!((surface[column(7, 0)], ground[column(7, 0)]), (-27, -27));
}

#[test]
fn arrows_fly_ballistically_until_the_surface() {
    // Level shot at 3 blocks/tick from 3 blocks above flat ground (first free y 67).
    let (landing, landed) = fly([0., 70., 0.], [0., 0., 3.], 100, |_, _| Some(67.));
    assert!(landed);
    assert_eq!(landing[1], 67.);
    assert!((25.0..40.0).contains(&landing[2]), "{landing:?}");
    // A wall of trees 10 blocks out stops it early; unrecorded terrain never lands it.
    let (hit, landed) = fly([0., 70., 0.], [0., 0., 3.], 100, |_, z| Some(if z >= 10. { 80. } else { 67. }));
    assert!(landed && (10.0..13.0).contains(&hit[2]), "{hit:?}");
    let (far, landed) = fly([0., 70., 0.], [0., 0., 3.], 10, |_, _| None);
    assert!(!landed && far[2] > 25.);
}

#[test]
fn arrow_velocity_is_rederived_from_sparse_server_positions() {
    // The server syncs an arrow every 20 ticks and never sends its velocity: after a
    // sync the stale launch velocity would loft it again from mid-flight.
    let (launch, velocity) = ([0., 70., 0.], [0., 1.5, 2.5]);
    let (at_sync, _) = fly(launch, velocity, 20, |_, _| None);
    let mut arrow = Tracked { role: Role::Arrow, position: launch, spawn: launch, sampled: launch, side: None, weapon: None,
        vehicle: None, dead: false, stopped: false, velocity, base_tick: 100, spawn_tick: 100, first_contact: None, death_tick: None, combat: Combat::default() };
    arrow.moved(at_sync, 120);
    let mut truth = velocity;
    for _ in 0..20 { truth = [truth[0] * 0.99, truth[1] * 0.99 - 0.05, truth[2] * 0.99]; }
    for a in 0..3 { assert!((arrow.velocity[a] - truth[a]).abs() < 0.08, "{:?} vs {truth:?}", arrow.velocity); }
    // Predicted landing from the sync matches the uninterrupted flight within a block or two.
    let ground = |_: f64, _: f64| Some(67.);
    let (direct, _) = fly(launch, velocity, 200, ground);
    let (resumed, landed) = fly(arrow.position, arrow.velocity, 180, ground);
    assert!(landed && (direct[2] - resumed[2]).abs() < 3., "{direct:?} vs {resumed:?}");
}

fn fighter(position: [f64; 3], side: Side, weapon: &str) -> Tracked {
    Tracked { role: Role::Fighter, position, spawn: position, sampled: position, side: Some(side), weapon: Some(weapon.into()),
        vehicle: None, dead: false, stopped: false, velocity: [0.; 3], base_tick: 100, spawn_tick: 100, first_contact: None, death_tick: None,
        combat: Combat { dims: [0.6, 1.95], ..Combat::default() } }
}

#[test]
fn melee_attackers_count_distinct_main_hand_swingers_in_a_sliding_two_second_window() {
    let content = Content::test_cube();
    let mut scan = Scan::new(-64, 384);
    for (id, e) in [
        (1, fighter([0., 64., 0.], Side::Red, "minecraft:iron_sword")),
        (2, fighter([1., 64., 0.], Side::Blue, "minecraft:iron_sword")),
        (3, fighter([0., 64., 1.], Side::Red, "minecraft:bow")),
        (4, fighter([0., 64., 2.], Side::Red, "minecraft:iron_sword")),
        (5, fighter([0., 64., 3.], Side::Red, "minecraft:iron_sword")),
        (6, fighter([50., 64., 0.], Side::Red, "minecraft:iron_sword")),
    ] { scan.entities.insert(id, e); }
    let mut apply = |scan: &mut Scan, id: i32, body: &[u8]| scan.apply(id, body, &content).unwrap();
    let events = |row: &Value| ["melee_attack_swings", "melee_attackers_2s"].map(|k| row["events"][k].as_u64().unwrap());
    // SWING_ANIMATION body: entity, hand (0 main, 1 off), type (0 none, 1 whack), duration.
    scan.tick = Some(100);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]); // the same soldier twice
    apply(&mut scan, cb::SWING_ANIMATION, &[3, 0, 1, 21]); // bow holder
    apply(&mut scan, cb::ANIMATE, &[4, 3]); // ANIMATE action 3: off hand on older protocols
    apply(&mut scan, cb::SWING_ANIMATION, &[4, 1, 1, 10]); // off-hand parry
    apply(&mut scan, cb::SWING_ANIMATION, &[5, 0, 0, 10]); // main hand, type none: a stagger
    apply(&mut scan, cb::ANIMATE, &[2, 0]); // ANIMATE action 0 is WakeUp on 26.3 (777), not a swing
    apply(&mut scan, cb::SWING_ANIMATION, &[6, 0, 2, 21]); // stab
    apply(&mut scan, cb::SWING_ANIMATION, &[99, 0, 1, 21]); // not tracked
    assert_eq!(events(&scan.sample(100, 0.)), [3, 2]);
    scan.tick = Some(120);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]);
    assert_eq!(events(&scan.sample(120, 1.)), [1, 2], "1 counted once; 6 still inside 40 ticks");
    assert_eq!(events(&scan.sample(139, 1.95))[1], 2, "tick 100 is the 40th tick back from 139");
    assert_eq!(events(&scan.sample(140, 2.)), [0, 1], "only the swing of tick 120 is left");
}

fn bare_models() -> crate::entities::Models {
    crate::entities::Models {
        rigs: BTreeMap::new(), schemas: BTreeMap::new(), families: BTreeMap::new(), builtin_variants: BTreeMap::new(),
        simple_particles: vec![], default_skins: vec![], poses: BTreeMap::new(), attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(), digest: String::new(), armor_models: BTreeMap::new(), motion: BTreeMap::new(),
    }
}

#[test]
fn strike_range_comes_from_the_attack_range_component() {
    // iron spear data: min 2, max 4.5, mob_factor 0.5 -> [1.0, 2.25]; no hitbox margin for a mob.
    let spear = json!({"min_reach":2.0,"max_reach":4.5,"min_creative_reach":2.0,"max_creative_reach":6.5,"hitbox_margin":0.125,"mob_factor":0.5});
    assert_eq!(attack_range(&components(&[("minecraft:attack_range", spear)])), MeleeReach { min: 1., max: 2.25 });
    // Codec defaults for omitted fields; no component: the default mob reach.
    assert_eq!(attack_range(&components(&[("minecraft:attack_range", json!({}))])), MeleeReach { min: 0., max: 3. });
    assert_eq!(attack_range(&components(&[("minecraft:weapon", json!({}))])), MeleeReach::mob(None));
}

#[test]
fn fighting_needs_an_enemy_in_own_weapon_reach_and_a_recent_swing_or_raised_shield() {
    let mut content = Content::test_cube();
    content.sound_names.insert(5, "minecraft:item.shield.block".into());
    content.sound_names.insert(6, "minecraft:entity.generic.hurt".into());
    let mut scan = Scan::new(-64, 384);
    scan.models = Some(bare_models());
    scan.living_flags = Some(8);
    let mut shielded = fighter([1.4, 64., 0.], Side::Blue, "minecraft:iron_sword");
    shielded.combat.offhand = Some("minecraft:shield".into());
    let mut spear = fighter([100., 64., 0.], Side::Red, "minecraft:iron_spear");
    spear.combat.range = MeleeReach { min: 1., max: 2.25 };
    let mut close_spear = fighter([300., 64., 0.], Side::Red, "minecraft:iron_spear");
    close_spear.combat.range = MeleeReach { min: 1., max: 2.25 };
    for (id, e) in [
        (1, fighter([0., 64., 0.], Side::Red, "minecraft:iron_sword")), // enemy 1.4 away (box gap 0.8): in reach both ways
        (2, shielded),
        (3, spear), // blue sword 2.8 away (gap 2.2): the spear reaches it (2.25), the sword does not reach the spear
        (4, fighter([102.8, 64., 0.], Side::Blue, "minecraft:iron_sword")),
        (5, fighter([200., 64., 0.], Side::Red, "minecraft:iron_sword")), // nobody near
        (6, close_spear), // blue gap 0.3: inside the spear's minimum, the sword still reaches
        (7, fighter([300.9, 64., 0.], Side::Blue, "minecraft:iron_sword")),
    ] { scan.entities.insert(id, e); }
    let mut apply = |scan: &mut Scan, id: i32, body: &[u8]| scan.apply(id, body, &content).unwrap();
    let row = |scan: &mut Scan, tick: u64| { let v = scan.sample(tick, 0.); (
        ["fighting_25t", "fighting_2s", "shields_raised", "shield_blocks"].map(|k| v["events"][k].as_u64().unwrap()),
        [v["contact_fighters"].as_u64().unwrap(), v["contact_fighters_legacy"].as_u64().unwrap()]) };
    scan.tick = Some(100);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]); // 1 attacks
    apply(&mut scan, cb::SWING_ANIMATION, &[5, 0, 1, 21]); // 5 swings at nobody
    apply(&mut scan, cb::SET_ENTITY_DATA, &[2, 8, 0, 3, 255]); // 2: using item (1) with the off hand (2)
    apply(&mut scan, cb::SET_ENTITY_DATA, &[4, 8, 0, 1, 255]); // 4: using the main hand, which holds a sword: not a shield
    let mut sound = Writer::default();
    sound.var(0); sound.string("minecraft:item.shield.block"); sound.0.push(0); sound.var(7); sound.var(2); sound.f32(1.); sound.f32(1.); sound.i64(0);
    apply(&mut scan, cb::SOUND_ENTITY, &sound.0); // inline holder
    let mut registry = Writer::default();
    registry.var(6); registry.var(7); registry.var(2); registry.f32(1.); registry.f32(1.); registry.i64(0);
    apply(&mut scan, cb::SOUND_ENTITY, &registry.0); // registry id 5 + 1
    let mut other = Writer::default();
    other.var(7); other.var(7); other.var(2); other.f32(1.); other.f32(1.); other.i64(0);
    apply(&mut scan, cb::SOUND_ENTITY, &other.0); // a different sound
    let mut stranger = Writer::default();
    stranger.var(6); stranger.var(7); stranger.var(99); stranger.f32(1.); stranger.f32(1.); stranger.i64(0);
    apply(&mut scan, cb::SOUND_ENTITY, &stranger.0); // untracked entity
    assert_eq!(row(&mut scan, 100), ([2, 2, 1, 2], [4, 4]), "1 (swing) and 2 (shield) fight; 1, 2, 3 (spear reaches 4) and 7 (sword reaches the spear) are in reach; 4 and 6 are not; 5 touches nobody");
    // The shield goes down at tick 125: it still counts for 25 ticks, the swing of tick 100 for 25 (40 in `fighting_2s`).
    scan.tick = Some(125);
    apply(&mut scan, cb::SET_ENTITY_DATA, &[2, 8, 0, 0, 255]);
    assert_eq!(row(&mut scan, 130), ([1, 2, 0, 0], [4, 4]));
    assert_eq!(row(&mut scan, 140), ([1, 1, 0, 0], [4, 4]), "the swing is 40 ticks old, only the lowered shield is left");
    assert_eq!(row(&mut scan, 150), ([0, 1, 0, 0], [4, 4]));
    assert_eq!(row(&mut scan, 165), ([0, 0, 0, 0], [4, 4]));
}

#[test]
fn a_rider_sits_at_the_attachment_height_and_dismounting_frees_him() {
    // Horse 1.4 x 1.6 with PASSENGER attachment 1.44375; zombie VEHICLE attachment 0.7: the rider sits 0.74375 up.
    let horse_combat = Combat { dims: [1.4, 1.6], attach: [1.44375, 0.], ..Combat::default() };
    let rider_combat = Combat { dims: [0.6, 1.95], attach: [2.0125, 0.7], ..Combat::default() };
    let (own, strike) = mounted_boxes([0., 0., 0.], &horse_combat, &rider_combat);
    assert!((own.min[1] - 0.74375).abs() < 1e-12 && (strike.min[1] - 0.74375).abs() < 1e-12, "{own:?} {strike:?}");
    assert_eq!((strike.min[0], strike.max[0]), (-0.7, 0.7));
    // A target on ground 1.3 lower (top 0.65) is beside a rider on foot but under a mounted one.
    let low = Aabb::at([1.4, -1.3, 0.], 0.6, 1.95);
    assert!(in_melee_range(Aabb::at([0., 0., 0.], 0.6, 1.95), low, MeleeReach::mob(None)), "on foot");
    assert!(!in_melee_range(strike, low, MeleeReach::mob(None)), "mounted: no vertical overlap");

    let content = Content::test_cube();
    let mut scan = Scan::new(-64, 384);
    let mut horse = fighter([0., 64., 0.], Side::Red, "minecraft:iron_sword");
    horse.role = Role::Horse;
    scan.entities.insert(1, horse);
    scan.entities.insert(2, fighter([0., 64., 0.], Side::Red, "minecraft:iron_sword"));
    scan.apply(cb::SET_PASSENGERS, &[1, 1, 2], &content).unwrap();
    assert_eq!(scan.entities[&2].vehicle, Some(1));
    scan.apply(cb::SET_PASSENGERS, &[1, 0], &content).unwrap();
    assert_eq!(scan.entities[&2].vehicle, None, "an empty passenger list dismounts him");
}
