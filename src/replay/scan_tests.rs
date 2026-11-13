use super::*;

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
        vehicle: None, dead: false, stopped: false, velocity, base_tick: 100, spawn_tick: 100, first_contact: None, death_tick: None };
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
        vehicle: None, dead: false, stopped: false, velocity: [0.; 3], base_tick: 100, spawn_tick: 100, first_contact: None, death_tick: None }
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
        (7, fighter([51., 64., 0.], Side::Blue, "minecraft:iron_sword")),
    ] { scan.entities.insert(id, e); }
    let mut apply = |scan: &mut Scan, id: i32, body: &[u8]| scan.apply(id, body, &content).unwrap();
    // SWING_ANIMATION body: entity, hand (0 main, 1 off), type (0 none, 1 whack), duration.
    scan.tick = Some(100);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]); // the same soldier twice
    apply(&mut scan, cb::SWING_ANIMATION, &[3, 0, 1, 21]); // bow holder
    apply(&mut scan, cb::ANIMATE, &[4, 3]); // off-hand swing (ANIMATE action 3)
    apply(&mut scan, cb::SWING_ANIMATION, &[4, 1, 1, 10]); // off-hand parry
    apply(&mut scan, cb::SWING_ANIMATION, &[5, 0, 0, 10]); // main hand, type none: a stagger
    apply(&mut scan, cb::ANIMATE, &[2, 0]); // ANIMATE action 0: main hand
    apply(&mut scan, cb::SWING_ANIMATION, &[6, 0, 1, 21]);
    apply(&mut scan, cb::SWING_ANIMATION, &[99, 0, 1, 21]); // not tracked
    apply(&mut scan, cb::DAMAGE_EVENT, &[2]); // blue 2 hurt one block from red 1 only
    let events = |row: &Value| ["melee_attack_swings", "melee_attackers_2s", "melee_hitters_2s"].map(|k| row["events"][k].as_u64().unwrap());
    assert_eq!(events(&scan.sample(100, 0.)), [4, 3, 1]);
    // 6 swung at tick 100, so its hurt victim 7 at tick 115 is 15 ticks too late for a hit.
    scan.tick = Some(115);
    apply(&mut scan, cb::DAMAGE_EVENT, &[7]);
    scan.tick = Some(120);
    apply(&mut scan, cb::SWING_ANIMATION, &[1, 0, 1, 21]);
    assert_eq!(events(&scan.sample(120, 1.)), [1, 3, 1], "1 counted once; 2 and 6 still inside 40 ticks");
    assert_eq!(events(&scan.sample(139, 1.95))[1], 3, "tick 100 is the 40th tick back from 139");
    assert_eq!(events(&scan.sample(140, 2.)), [0, 1, 0], "only the swing of tick 120 is left, the hit of tick 100 dropped");
}
