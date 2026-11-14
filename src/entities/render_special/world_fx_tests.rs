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
    let transform = store.slots[0].presented_transform(0, 0.);
    let result = super::super::append(
        &mut store,
        0,
        &models,
        &Content::test_cube(),
        [0.; 3],
        0,
        0.,
        transform,
        None,
        crate::ui::MapRenderFixture::empty().view(),
        &|_| Ok(Some([7., 11.])),
        &|_, _| Ok([1.; 3]),
        &mut out,
    )
    .unwrap();
    assert_eq!(result, super::super::Handled::Rendered);
    out
}

#[test]
fn tnt_and_received_falling_block_emit_the_pack_model() {
    for kind in ["minecraft:tnt", "minecraft:falling_block"] {
        let out = draw(kind, 64);
        assert_eq!(out.vertices.len(), 24, "{kind}");
        assert_eq!(out.indices[EntityMaterial::ItemCutout as usize].len(), 36);
        assert!(out.vertices.iter().all(|v| v.light == [7., 11.]));
        assert_eq!(out.vertices[0].uv, [0., 0.]);
        let max_x = out
            .vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((max_x - 0.5).abs() < 1e-5);
    }
}

#[test]
fn falling_air_state_emits_no_geometry() {
    let (models, mut entity) = subject("minecraft:falling_block");
    entity.spawn_data = 0; // Authored air state has no model; do not silently draw sand.
    let mut store = Store::new();
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    let transform = store.slots[0].presented_transform(0, 0.);
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            transform,
            None,
            crate::ui::MapRenderFixture::empty().view(),
            &|_| Ok(Some([7., 11.])),
            &|_, _| Ok([1.; 3]),
            &mut out
        )
        .unwrap(),
        super::super::Handled::NoGeometry
    );
    assert!(out.vertices.is_empty());
}

#[test]
fn tnt_flashes_white_and_swells_only_at_fuse_end() {
    let early = draw("minecraft:tnt", 64);
    let flashing = draw("minecraft:tnt", 2);
    let width = |g: &DynamicGeometry| {
        g.vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::NEG_INFINITY, f32::max)
            * 2.
    };
    assert!(width(&flashing) > width(&early));
    assert!(width(&flashing) < width(&early) * 1.3);
    assert!(early.vertices.iter().all(|v| v.overlay_uv == [0., 10.]));
    assert!(flashing.vertices.iter().all(|v| v.overlay_uv == [15., 10.]));
    assert_eq!(
        flashing.indices[EntityMaterial::EntityCutout as usize].len(),
        36
    );
    assert!(flashing.indices[EntityMaterial::ItemCutout as usize].is_empty());
    assert_ne!(tnt_appearance(2., 0.).1, tnt_appearance(7., 0.).1);
    assert_eq!(tnt_appearance(60., 0.).0, 1.);
    assert!(tnt_appearance(3., 0.75).0 > tnt_appearance(3., 0.).0);
}

#[test]
fn tnt_uses_received_block_state_and_fuse_boundaries() {
    let (mut models, mut entity) = subject("minecraft:tnt");
    models.schemas.get_mut("minecraft:tnt").unwrap().accessors
        .insert("DATA_BLOCK_STATE_ID".into(), [8, 0]);
    entity.metadata.insert(8, super::super::Metadata::BlockState(0));
    let mut store = Store::new();
    store.special_render_cache.tnt_state = Some(1);
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    let render = |store: &mut Store, out: &mut DynamicGeometry| {
        let transform = store.slots[0].presented_transform(0, 0.);
        super::super::append(store, 0, &models, &Content::test_cube(), [0.; 3],
            0, 0., transform, None, crate::ui::MapRenderFixture::empty().view(), &|_| Ok(Some([7., 11.])), &|_, _| Ok([1.; 3]), out).unwrap()
    };
    assert_eq!(render(&mut store, &mut out), Handled::NoGeometry);
    assert!(out.vertices.is_empty());
    store.slots[0].metadata.insert(8, super::super::Metadata::BlockState(1));
    assert_eq!(render(&mut store, &mut out), Handled::Rendered);
    assert_eq!(out.vertices.len(), 24);
    assert_eq!(tnt_appearance(9., 0.).0, 1.);
    assert!(tnt_appearance(8., 0.).0 > 1.);
    assert_ne!(tnt_appearance(4., 0.).1, tnt_appearance(4., 0.01).1);
    let flashing = draw("minecraft:tnt", 2);
    let low = flashing.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
    let high = flashing.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
    assert!(((low + high) * 0.5 - 0.5).abs() < 1e-5);
}

#[test]
fn lightning_dispatch_emits_seeded_geometry_without_light_or_pack_textures() {
    let (models, entity) = subject("minecraft:lightning_bolt");
    let mut store = Store::new();
    store.slots.push(entity);
    let mut out = DynamicGeometry::default();
    let transform = store.slots[0].presented_transform(0, 0.);
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            transform,
            None,
            crate::ui::MapRenderFixture::empty().view(),
            &|_| panic!("lightning must be fullbright"),
            &|_, _| panic!("lightning samples no tint"),
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
    let transform = store.slots[0].presented_transform(0, 0.);
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            transform,
            None,
            crate::ui::MapRenderFixture::empty().view(),
            &|_| panic!("inactive bolt samples no light"),
            &|_, _| panic!("lightning samples no tint"),
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
    let transform = store.slots[0].presented_transform(0, 0.);
    assert_eq!(
        super::super::append(
            &mut store,
            0,
            &models,
            &Content::test_cube(),
            [0.; 3],
            0,
            0.,
            transform,
            None,
            crate::ui::MapRenderFixture::empty().view(),
            &light,
            &|_, _| Ok([1.; 3]),
            &mut out
        )
        .unwrap(),
        super::super::Handled::Unhandled
    );
    assert!(!out.indices[EntityMaterial::Leash as usize].is_empty());
    let (start, end) = leash_anchors(&store.slots[0], &store.slots[1], &models, &store, 0, 0.).unwrap();
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

#[test]
fn missing_holder_light_does_not_hide_the_leashed_entity() {
    let (mut models, mut mob) = subject("minecraft:pig");
    let (mut holder_models, mut holder) = subject("minecraft:leash_knot");
    models.schemas.insert("minecraft:leash_knot".into(),
        holder_models.schemas.remove("minecraft:leash_knot").unwrap());
    holder.handle.network_id = 42;
    holder.transform.position = [4., 0., 0.];
    holder.position = holder.transform.position;
    mob.leash = Some(holder.handle);
    let mut store = Store::new();
    store.ids.insert(42, 1);
    store.slots.extend([mob, holder]);
    let mut out = DynamicGeometry::default();
    let transform = store.slots[0].presented_transform(0, 0.);
    let result = super::super::append(
        &mut store, 0, &models, &Content::test_cube(), [0.; 3], 0, 0., transform, None,
        crate::ui::MapRenderFixture::empty().view(),
        &|pos| Ok(if pos[0] > 2. { None } else { Some([7., 11.]) }),
        &|_, _| Ok([1.; 3]),
        &mut out,
    ).unwrap();
    assert_eq!(result, Handled::Unhandled);
    assert!(out.indices[EntityMaterial::Leash as usize].is_empty());
    let result = super::super::append(
        &mut store, 0, &models, &Content::test_cube(), [0.; 3], 0, 0., transform, None,
        crate::ui::MapRenderFixture::empty().view(),
        &|_| Ok(Some([7., 11.])), &|_, _| Ok([1.; 3]), &mut out,
    ).unwrap();
    assert_eq!(result, Handled::Unhandled);
    assert!(!out.indices[EntityMaterial::Leash as usize].is_empty());
}

#[test]
fn burning_flag_transitions_reach_each_entity_draw_route() {
    use crate::entities::presentation::{Binding, RootStyle};
    use crate::protocol::content::TextureRegion;
    use std::sync::Arc;

    let region = |layer| TextureRegion { uv: [0., 0., 1., 1.], layer, width: 16, height: 16 };
    let mut content = Content::test_cube();
    for (name, layer) in [
        ("minecraft:block/fire_0", 42),
        ("minecraft:block/fire_1", 43),
        ("minecraft:entity/experience/experience_orb", 44),
        ("minecraft:test/frame", 45),
    ] {
        content.insert_test_texture(name, region(layer));
    }
    let faces: serde_json::Map<String, serde_json::Value> =
        ["north", "south", "east", "west", "up", "down"].into_iter()
            .map(|face| (face.into(), serde_json::json!({"texture":"#frame"}))).collect();
    let frame_model = serde_json::to_vec(&serde_json::json!({
        "textures":{"frame":"test/frame"},
        "elements":[{"from":[2,2,14],"to":[14,14,16],"faces":faces}]
    })).unwrap();
    content.insert_test_resource("assets/minecraft/models/block/item_frame.json", &frame_model);

    for kind in ["minecraft:pig", "minecraft:tnt", "minecraft:falling_block", "minecraft:item_frame", "minecraft:experience_orb"] {
        let (mut models, mut entity) = subject(kind);
        models.schemas.get_mut(kind).unwrap().accessors.insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
        entity.spawn_data = if kind == "minecraft:falling_block" { 1 } else { 2 };
        if kind == "minecraft:pig" {
            entity.binding = Some(Ok(Binding {
                rig: "minecraft:pig#renderer_main".into(), texture: region(1),
                style: RootStyle::Living, layers: None, anger: None,
            }));
        }
        let id = entity.handle.network_id;
        let mut store = Store::new();
        store.special_render_cache.tnt_state = Some(1);
        store.models = Some(Arc::new(models));
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        store.slots.push(entity);
        let mut baseline = 0;
        for flags in [0, 1, 0, 33] {
            store.slots[0].metadata.insert(0, super::super::Metadata::Byte(flags));
            let mut out = DynamicGeometry::default();
            let stats = p::append(&mut store, crate::entities::EntityRenderFrame {
                content: &content, origin: [0.; 3], camera: [0.; 3],
                camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0.,
                game_time: 0, clock_environment: None,
                maps: crate::ui::MapRenderFixture::empty().view(),
                tint_source: &|_, _| Ok([1.; 3]),
                light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None,
            }, &mut out).unwrap();
            assert!(stats.unrendered.is_empty(), "{kind}: {:?}", stats.unrendered);
            let fire = out.indices[EntityMaterial::EntityFire as usize].len();
            if flags == 1 && kind != "minecraft:falling_block" {
                assert_eq!(fire, 12, "{kind}: expected one two-quad overlay");
                baseline = fire;
            } else {
                assert_eq!(fire, 0, "{kind} flags {flags}");
            }
        }
        assert_eq!(baseline > 0, kind != "minecraft:falling_block", "{kind}");
    }
}

#[test]
fn biome_tints_color_emitted_block_faces_and_cache_per_kind() {
    use std::cell::Cell;
    let content = Content::test_cube();
    let mut model = content.model_for(1).unwrap().unwrap().clone();
    for quad in &mut model.quads {
        quad.tint = TintKind::Grass;
    }
    model.quads[0].tint = TintKind::Foliage;
    let count = Cell::new(0);
    let cache = Cell::new([None; 4]);
    let sample = |kind, position| {
        assert_eq!(position, [-2, 65, 4]);
        count.set(count.get() + 1);
        Ok(if kind == TintKind::Grass { [0.1, 0.6, 0.2] } else { [0.3, 0.4, 0.5] })
    };
    let mut out = DynamicGeometry::default();
    p::append_block_quads(&model, 0, glam::Mat4::IDENTITY, [15., 15.],
        |quad| block_tint(quad.tint, [-2, 65, 4], &cache, &sample), &mut out).unwrap();
    assert_eq!(count.get(), 2);
    assert!(out.vertices.iter().any(|v| v.color == [0.1, 0.6, 0.2]));
    assert!(out.vertices.iter().any(|v| v.color == [0.3, 0.4, 0.5]));
}
