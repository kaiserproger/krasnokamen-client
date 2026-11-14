//! Authored geometry fixtures; no game assets or captured renderer constants.
use super::*;
use crate::entities::animation_tests::fixture;

fn subject(kind: &str) -> (Models, Entity) {
    let (mut models, mut entity) = fixture();
    let schema = models.schemas.remove("minecraft:pig").unwrap();
    models.schemas.insert(kind.into(), schema);
    entity.type_name = kind.into();
    let random = entity.source.as_ref().unwrap().random.clone();
    entity.source.as_mut().unwrap().special = super::super::State::new(&entity, random);
    (models, entity)
}

fn draw(kind: &str, fuse: i32) -> DynamicGeometry {
    let (models, mut entity) = subject(kind);
    entity.spawn_data = 1;
    entity.source.as_mut().unwrap().special.fuse = fuse;
    let mut store = Store::new();
    store.slots.push(entity);
    // The authored cube substitutes for a received pack model, not for geometry emission.
    store.special_render_cache.tnt_state = Some(1);
    let mut out = DynamicGeometry::default();
    let result = super::super::append(
        &mut store,
        0,
        &models,
        &Content::test_cube(),
        [0.; 3],
        0,
        0.,
        None,
        &|_| Ok(Some([7., 11.])),
        &mut out,
    )
    .unwrap();
    assert_eq!(result, super::super::Handled::Rendered);
    out
}

#[test]
fn tnt_and_received_falling_block_emit_the_pack_model() {
    for kind in ["minecraft:tnt", "minecraft:falling_block"] {
        let out = draw(kind, 60);
        assert_eq!(out.vertices.len(), 24, "{kind}");
        assert_eq!(out.indices[EntityMaterial::ItemCutout as usize].len(), 36);
        assert!(out.vertices.iter().all(|v| v.light == [7., 11.]));
        assert_eq!(out.vertices[0].uv, [0., 0.]);
        let max_x = out
            .vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((max_x - 0.49).abs() < 1e-5);
    }
}

#[test]
fn falling_block_uses_spawn_state_and_reports_missing_models() {
    let (models, mut entity) = subject("minecraft:falling_block");
    entity.spawn_data = 0; // Authored air state has no model; do not silently draw sand.
    let mut store = Store::new();
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    assert!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            None,
            &|_| Ok(Some([7., 11.])),
            &mut out
        )
        .is_err()
    );
    assert!(out.vertices.is_empty());
}

#[test]
fn tnt_flashes_white_and_swells_only_at_fuse_end() {
    let early = draw("minecraft:tnt", 60);
    let flashing = draw("minecraft:tnt", 2);
    let width = |g: &DynamicGeometry| {
        g.vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::NEG_INFINITY, f32::max)
            * 2.
    };
    assert!(width(&flashing) > width(&early));
    assert!(width(&flashing) < width(&early) * 1.2);
    assert!(early.vertices.iter().all(|v| v.overlay_uv == [0., 10.]));
    assert!(flashing.vertices.iter().all(|v| v.overlay_uv == [15., 10.]));
    assert_eq!(
        flashing.indices[EntityMaterial::EntityCutout as usize].len(),
        36
    );
    assert!(flashing.indices[EntityMaterial::ItemCutout as usize].is_empty());
    assert_ne!(tnt_appearance(2., 0.).1, tnt_appearance(10., 0.).1);
    assert_eq!(tnt_appearance(60., 0.).0, 1.);
    assert!(tnt_appearance(3., 0.75).0 > tnt_appearance(3., 0.).0);
}

#[test]
fn lightning_dispatch_emits_seeded_geometry_without_light_or_pack_textures() {
    let (models, entity) = subject("minecraft:lightning_bolt");
    let mut store = Store::new();
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            None,
            &|_| panic!("lightning must be fullbright"),
            &mut out
        )
        .unwrap(),
        super::super::Handled::Rendered
    );
    assert!(!out.indices[EntityMaterial::Lightning as usize].is_empty());
    let bolt = |seed| {
        let mut out = DynamicGeometry::default();
        append_lightning(seed, Vec3::ZERO, &mut out).unwrap();
        out.vertices
            .into_iter()
            .map(|v| v.position)
            .collect::<Vec<_>>()
    };
    assert_eq!(bolt(123), bolt(123));
    assert_ne!(bolt(123), bolt(124));
    assert!(
        out.vertices
            .iter()
            .all(|v| v.position.iter().all(|p| p.is_finite()))
    );
    assert!(out.vertices.iter().any(|v| v.position[1] >= 127.));
}

#[test]
fn lightning_is_absent_between_sky_flashes() {
    let (models, mut entity) = subject("minecraft:lightning_bolt");
    // Advance the actual timeline, rather than substituting a separate visibility flag.
    let mut source = entity.source.take().unwrap();
    let mut saw_gap = false;
    for age in 0..80 {
        let effects = source.special.tick(&models, &entity, age, false);
        assert_eq!(
            source.special.lightning_visible(),
            effects.sky_flash && !source.special.discarded
        );
        if !source.special.lightning_visible() {
            saw_gap = true;
            break;
        }
    }
    assert!(saw_gap);
    entity.source = Some(source);
    let mut store = Store::new();
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            None,
            &|_| panic!("inactive bolt samples no light"),
            &mut out
        )
        .unwrap(),
        super::super::Handled::NoGeometry
    );
    assert!(out.vertices.is_empty());
}

#[test]
fn rope_anchors_sag_light_and_unleash_follow_received_entities() {
    let (mut models, mut mob) = subject("minecraft:pig");
    let (_, mut holder) = subject("minecraft:leash_knot");
    let (mut holder_models, _) = subject("minecraft:leash_knot");
    let mut schema = holder_models
        .schemas
        .remove("minecraft:leash_knot")
        .unwrap();
    schema.dimensions.height = 0.5;
    schema.dimensions.width = 0.375;
    models.schemas.insert("minecraft:leash_knot".into(), schema);
    holder.handle.network_id = 42;
    holder.transform.position = [4., 1., 0.];
    holder.position = holder.transform.position;
    mob.leash = Some(holder.handle);
    let mut store = Store::new();
    store.ids.insert(41, 0);
    store.ids.insert(42, 1);
    store.slots.extend([mob, holder]);
    let mut out = DynamicGeometry::default();
    let light = |p: [f64; 3]| Ok(Some([p[0] as f32, 9.]));
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            None,
            &light,
            &mut out
        )
        .unwrap(),
        super::super::Handled::Unhandled
    );
    assert!(!out.indices[EntityMaterial::Leash as usize].is_empty());
    let (start, end) = leash_anchors(&store.slots[0], &store.slots[1], &models, 0, 0.).unwrap();
    for (actual, expected) in start.into_iter().zip([0., 0.7, 0.5]) {
        assert!((actual - expected).abs() < 1e-6);
    }
    assert_eq!(end, [4., 1.75, 0.]);
    let a = Vec3::from_array(start.map(|v| v as f32));
    let b = Vec3::from_array(end.map(|v| v as f32));
    assert_eq!(rope_point(a, b, 0.), a);
    assert_eq!(rope_point(a, b, 1.), b);
    assert!(rope_point(a, b, 0.5).y < (a.y + b.y) * 0.5);
    let emitted_ring_center = |offset: usize, end: bool| {
        out.vertices[offset..offset + 16]
            .chunks_exact(4)
            .map(|quad| Vec3::from_array(quad[if end { 2 } else { 0 }].position))
            .sum::<Vec3>()
            / 4.
    };
    assert!(emitted_ring_center(0, false).distance(a) < 1e-5);
    assert!(emitted_ring_center(23 * 16, true).distance(b) < 1e-5);
    assert!(emitted_ring_center(12 * 16, false).y < (a.y + b.y) * 0.5);
    assert!(out.vertices.iter().any(|v| v.light == [0., 9.]));
    assert!(out.vertices.iter().any(|v| v.light == [4., 9.]));
    store.slots[0].predicted_leash = Some(None);
    let mut removed = DynamicGeometry::default();
    append_leash(
        &store.slots[0],
        &store,
        &models,
        [0.; 3],
        0,
        0.,
        &light,
        &mut removed,
    )
    .unwrap();
    assert!(removed.vertices.is_empty());
    store.slots[0].predicted_leash = None;
    store.slots[0].leash = None;
    append_leash(
        &store.slots[0],
        &store,
        &models,
        [0.; 3],
        0,
        0.,
        &light,
        &mut removed,
    )
    .unwrap();
    assert!(removed.vertices.is_empty());
}

#[test]
fn local_player_holder_resolves_even_without_a_received_entity_row() {
    let (mut models, mut mob) = subject("minecraft:pig");
    let (_, mut holder) = subject("minecraft:player");
    let (mut holder_models, _) = subject("minecraft:player");
    let schema = holder_models.schemas.remove("minecraft:player").unwrap();
    models.schemas.insert("minecraft:player".into(), schema);
    holder.handle.network_id = 42;
    mob.leash_holder_id = Some(42);
    let mut store = Store::new();
    store.local_avatar = Some(holder);
    store.slots.push(mob);
    let mut out = DynamicGeometry::default();
    append_leash(
        &store.slots[0],
        &store,
        &models,
        [0.; 3],
        0,
        0.,
        &|_| Ok(Some([15., 3.])),
        &mut out,
    )
    .unwrap();
    assert!(!out.indices[EntityMaterial::Leash as usize].is_empty());
    let local = store.local_avatar.take().unwrap();
    store.slots[0].predicted_leash = Some(Some(local.handle));
    store.slots.push(local);
    store.local_render_slot = Some(1);
    let mut third_person = DynamicGeometry::default();
    append_leash(
        &store.slots[0],
        &store,
        &models,
        [0.; 3],
        0,
        0.,
        &|_| Ok(Some([15., 3.])),
        &mut third_person,
    )
    .unwrap();
    assert!(!third_person.indices[EntityMaterial::Leash as usize].is_empty());
}
