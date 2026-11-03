//! Village block-entity regression tests: chest, bed and bell admit the real
//! static geometry pass with authored fixture rigs and textures.
//!
//! These tests route through `Store::append_static_block_geometry`, the same
//! entry the native scene uses for geometry-only factories, so a missing rig,
//! missing texture or scene-integration skip fails here instead of rendering
//! an invisible village. Quad counts are honest authored-fixture counts, not
//! vanilla asset bytes: they prove the rig path emits nonempty geometry with
//! the correct entity texture region, without depending on ignored Mojang
//! resources.
use super::super::resources::{Bone, BoneRole};
use super::*;
use std::{collections::BTreeMap, sync::Arc};

fn quad(x: f32, y: f32, z: f32, w: f32, h: f32) -> [[f32; 5]; 4] {
    [
        [x, y, z, 0., 0.],
        [x + w, y, z, 1., 0.],
        [x + w, y + h, z, 1., 1.],
        [x, y + h, z, 0., 1.],
    ]
}

fn bone(name: &str, pivot: [f32; 3], quads: Vec<[[f32; 5]; 4]>) -> Bone {
    Bone {
        parent: None,
        pivot: Vec3::from_array(pivot),
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::Fixed,
        name: name.into(),
        normals: vec![[0., 0., 1.]; quads.len()],
        quads,
    }
}

fn models_with(rigs: Vec<(String, Rig)>) -> Models {
    Models {
        rigs: rigs.into_iter().collect(),
        schemas: BTreeMap::new(),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: Vec::new(),
        default_skins: Vec::new(),
        poses: BTreeMap::new(),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        armor_models: BTreeMap::new(),
        motion: BTreeMap::new(),
    }
}

fn test_region(layer: u32) -> TextureRegion {
    TextureRegion {
        layer,
        uv: [0., 0., 1., 1.],
        width: 64,
        height: 64,
    }
}

/// Single-state fixture: `Content::test_cube` state 1 carries the village
/// block, its received properties and its constructor projection.
fn village_content(
    block: &str,
    entity: &str,
    props: BTreeMap<String, String>,
    textures: Vec<(&str, u32)>,
) -> Content {
    let mut content = Content::test_cube();
    content.names[1] = block.into();
    content.block_entity_names.insert(7, entity.into());
    content.insert_test_state_properties(1, props);
    content.insert_test_placement_metadata(1, serde_json::json!({"block_entity_type": entity}));
    for (name, layer) in textures {
        content.insert_test_texture(name, test_region(layer));
    }
    content
}

fn place(store: &mut super::super::Store, content: &Content, position: [i32; 3]) {
    store.ensure_block_entity(position, 1, content, 1).unwrap();
}

fn render_static(
    store: &mut super::super::Store,
    content: &Content,
    world: &World,
    origin: [f64; 3],
    camera: [f64; 3],
    partial: f32,
    out: &mut DynamicGeometry,
) -> super::super::PresentationStats {
    store.append_static_block_geometry(crate::entities::StaticBlockSceneFrame { content, world, origin, camera, camera_rotation: [0., 0.], radius: 32., scoping: false, text_filtering: false, game_time: 0, partial, sample_light: &|_| Ok(Some([7., 3.])) }, out)
        .unwrap()
}

fn camera_for(position: [i32; 3]) -> ([f64; 3], [f64; 3]) {
    let origin = position.map(f64::from);
    let camera = [
        f64::from(position[0]) + 0.5,
        f64::from(position[1]) + 1.62,
        f64::from(position[2]) + 0.5,
    ];
    (origin, camera)
}

#[test]
fn authored_village_models_have_upright_geometry() {
    use super::super::solaris_models::ModelFile;
    let world = World::new(0, 256);
    let position = [0, 64, 0];
    let (origin, camera) = camera_for(position);
    for (part, model, key, texture) in [
        ("single", include_str!("../../../assets/solaris/models/entity/chest.json"), "minecraft:chest#main", "minecraft:entity/chest/normal"),
        ("left", include_str!("../../../assets/solaris/models/entity/double_chest_left.json"), "minecraft:double_chest_left#main", "minecraft:entity/chest/normal_left"),
        ("right", include_str!("../../../assets/solaris/models/entity/double_chest_right.json"), "minecraft:double_chest_right#main", "minecraft:entity/chest/normal_right"),
    ] {
        for facing in ["north", "south", "east", "west"] {
            let content = village_content("minecraft:chest", "minecraft:chest",
                BTreeMap::from([("type".into(), part.into()), ("facing".into(), facing.into())]),
                vec![(texture, 3)]);
            let mut store = super::super::Store::new();
            store.activate_models(Arc::new(models_with(vec![(key.into(), ModelFile::parse(model).unwrap().bake().unwrap())])));
            place(&mut store, &content, position);
            let mut geometry = DynamicGeometry::default();
            render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
            assert_eq!(geometry.vertices.len(), 72);
            let body_y = geometry.vertices[..24].iter().map(|v| v.position[1]).sum::<f32>();
            let lid_y = geometry.vertices[24..48].iter().map(|v| v.position[1]).sum::<f32>();
            assert!(lid_y > body_y, "{part} {facing}: lid must be above base");
            let latch = &geometry.vertices[48..72];
            let center = |axis: usize| latch.iter().map(|v| v.position[axis]).sum::<f32>() / latch.len() as f32;
            let (axis, sign) = match facing {
                "north" => (2, -1.),
                "south" => (2, 1.),
                "east" => (0, 1.),
                _ => (0, -1.),
            };
            assert!((center(axis) - 0.5) * sign > 0.4, "{part} {facing}: latch must face outward");
            assert!(geometry.vertices[..48].iter().all(|v| (-0.00001..=1.00001).contains(&v.position[0]) && (-0.00001..=1.00001).contains(&v.position[2])), "{part} {facing}: chest body must stay in its block");
        }
    }
    for attachment in ["floor", "ceiling", "single_wall", "double_wall"] {
        let content = village_content("minecraft:bell", "minecraft:bell",
            BTreeMap::from([("attachment".into(), attachment.into()), ("facing".into(), "north".into())]),
            vec![("minecraft:entity/bell/bell_body", 3)]);
        let rig = ModelFile::parse(include_str!("../../../assets/solaris/models/entity/bell.json")).unwrap().bake().unwrap();
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models_with(vec![("minecraft:bell#main".into(), rig)])));
        place(&mut store, &content, position);
        let mut geometry = DynamicGeometry::default();
        render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
        assert!(!geometry.vertices.is_empty());
        assert!(geometry.vertices.iter().all(|v| (0.0..=1.0).contains(&v.position[0]) && (0.0..=1.0).contains(&v.position[1]) && (0.0..=1.0).contains(&v.position[2])), "{attachment}: bell must hang inside its block");
        let top = geometry.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
        let bottom = geometry.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
        assert_eq!(top, 13. / 16., "{attachment}: bell top must meet its support");
        assert_eq!(bottom, 4. / 16., "{attachment}: bell rim must hang below support");
    }
}

fn chest_rig() -> Rig {
    Rig {
        bones: vec![
            bone("body", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
            bone("lid", [0., 1., 0.], vec![quad(0., 1., 0., 1., 1.)]),
            bone("lock", [0., 1., 0.], vec![quad(0.4, 1., 0., 0.2, 0.5)]),
        ],
        part_by_name: BTreeMap::new(),
    }
}

#[test]
fn village_chest_single_renders_body_lid_and_lock_with_chest_texture() {
    let content = village_content(
        "minecraft:chest",
        "minecraft:chest",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("type".into(), "single".into()),
        ]),
        vec![("minecraft:entity/chest/normal", 3)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:chest#main".into(),
        chest_rig(),
    )])));
    let position = [258, 65, 135];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut geometry = DynamicGeometry::default();
    let stats = render_static(
        &mut store,
        &content,
        &world,
        origin,
        camera,
        0.,
        &mut geometry,
    );
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    // Three authored bones, one quad each: the real emit path must carry all
    // of them, not just claim a rendered chest.
    assert_eq!(geometry.vertices.len(), 12);
    assert_eq!(
        geometry.indices[EntityMaterial::EntityCutoutCull as usize].len(),
        18
    );
    assert_eq!(stats.vertices, 12);
    assert!(geometry.vertices.iter().all(|v| v.texture_layer == 3));
    assert!(geometry.vertices.iter().all(|v| v.light == [7., 3.]));
}

#[test]
fn village_chest_lid_opens_around_hinge_while_body_stays_put() {
    let content = village_content(
        "minecraft:chest",
        "minecraft:chest",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("type".into(), "single".into()),
        ]),
        vec![("minecraft:entity/chest/normal", 3)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:chest#main".into(),
        chest_rig(),
    )])));
    let position = [258, 65, 135];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut closed = DynamicGeometry::default();
    render_static(
        &mut store,
        &content,
        &world,
        origin,
        camera,
        0.,
        &mut closed,
    );
    assert_eq!(closed.vertices.len(), 12);
    let entity = store.block_entities.get_mut(&position).unwrap();
    entity.animation.progress = 1.;
    entity.animation.previous = 1.;
    let mut open = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut open);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(open.vertices.len(), 12);
    // Bone emission order is body, lid, lock: the lid rotation must move the
    // lid/lock quads while the body quad stays exactly put.
    for (shut, swung) in closed.vertices.iter().zip(&open.vertices) {
        assert_eq!(shut.texture_layer, swung.texture_layer);
    }
    for (shut, swung) in closed.vertices[..4].iter().zip(&open.vertices[..4]) {
        assert_eq!(shut.position, swung.position);
    }
    let moved = open.vertices[4..]
        .iter()
        .zip(&closed.vertices[4..])
        .any(|(shut, swung)| {
            let d: [f32; 3] = std::array::from_fn(|i| shut.position[i] - swung.position[i]);
            d.iter().map(|v| v * v).sum::<f32>() > 1e-10
        });
    assert!(moved, "open lid must move lid/lock vertices");
}

#[test]
fn village_bed_foot_and_head_use_distinct_rigs_and_bed_texture() {
    for (part, key, quads) in [
        ("foot", "minecraft:bed_foot#main", 2usize),
        ("head", "minecraft:bed_head#main", 3usize),
    ] {
        let content = village_content(
            "minecraft:red_bed",
            "minecraft:bed",
            BTreeMap::from([
                ("facing".into(), "north".into()),
                ("part".into(), part.into()),
            ]),
            vec![("minecraft:entity/bed/red", 5)],
        );
        let rig = Rig {
            bones: vec![bone(
                "bed",
                [0., 0., 0.],
                (0..quads).map(|i| quad(i as f32, 0., 0., 1., 1.)).collect(),
            )],
            part_by_name: BTreeMap::new(),
        };
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models_with(vec![(key.into(), rig)])));
        let position = [11, 64, -20];
        place(&mut store, &content, position);
        let (origin, camera) = camera_for(position);
        let world = World::new(0, 256);
        let mut geometry = DynamicGeometry::default();
        let stats = render_static(
            &mut store,
            &content,
            &world,
            origin,
            camera,
            0.,
            &mut geometry,
        );
        assert_eq!(stats.block_entities, 1, "bed {part} must render");
        assert!(
            stats.unrendered.is_empty(),
            "bed {part}: {:?}",
            stats.unrendered
        );
        // Distinct authored quad counts per half prove the foot/head rig key
        // selection actually ran instead of rendering one shared quad.
        assert_eq!(geometry.vertices.len(), quads * 4, "bed {part} quads");
        assert_eq!(
            geometry.indices[EntityMaterial::EntitySolid as usize].len(),
            quads * 6,
            "bed {part} indices"
        );
        assert_eq!(stats.vertices, quads * 4);
        assert!(geometry.vertices.iter().all(|v| v.texture_layer == 5));
        assert!(geometry.vertices.iter().all(|v| v.light == [7., 3.]));
    }
}

#[test]
fn village_bed_facing_rotates_rendered_geometry() {
    let mut content = village_content(
        "minecraft:red_bed",
        "minecraft:bed",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("part".into(), "foot".into()),
        ]),
        vec![("minecraft:entity/bed/red", 5)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:bed_foot#main".into(),
        Rig {
            bones: vec![bone("bed", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)])],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [11, 64, -20];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut north = DynamicGeometry::default();
    render_static(&mut store, &content, &world, origin, camera, 0., &mut north);
    assert_eq!(north.vertices.len(), 4);
    content.insert_test_state_properties(
        1,
        BTreeMap::from([
            ("facing".into(), "east".into()),
            ("part".into(), "foot".into()),
        ]),
    );
    let mut east = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut east);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(east.vertices.len(), 4);
    let moved = east.vertices.iter().zip(&north.vertices).any(|(a, b)| {
        let d: [f32; 3] = std::array::from_fn(|i| a.position[i] - b.position[i]);
        d.iter().map(|v| v * v).sum::<f32>() > 1e-10
    });
    assert!(moved, "east-facing bed must sit rotated from north");
}

#[test]
fn village_bell_body_renders_and_swing_moves_it() {
    let content = village_content(
        "minecraft:bell",
        "minecraft:bell",
        BTreeMap::new(),
        vec![("minecraft:entity/bell/bell_body", 7)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:bell#main".into(),
        Rig {
            bones: vec![bone(
                "bell_body",
                [0., 2., 0.],
                vec![quad(0., 1., 0., 1., 1.), quad(0., 0., 0., 1., 1.)],
            )],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [13, 64, -20];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut rest = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut rest);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(rest.vertices.len(), 8);
    assert_eq!(rest.indices[EntityMaterial::EntitySolid as usize].len(), 12);
    assert!(rest.vertices.iter().all(|v| v.texture_layer == 7));
    assert!(rest.vertices.iter().all(|v| v.light == [7., 3.]));
    assert_eq!([rest.vertices[0].position[0], rest.vertices[0].position[2]], [0.5, 0.5]);
    let swing = super::math::bell_angles(10.5, 2);
    assert!(
        swing[0] != 0. || swing[1] != 0.,
        "fixture swing time must actually swing"
    );
    let entity = store.block_entities.get_mut(&position).unwrap();
    entity.animation.source.bell_direction = Some(2);
    entity.animation.source.bell_ticks = 10;
    let mut swung = DynamicGeometry::default();
    render_static(
        &mut store, &content, &world, origin, camera, 0.5, &mut swung,
    );
    assert_eq!(swung.vertices.len(), 8);
    let moved = swung.vertices.iter().zip(&rest.vertices).any(|(a, b)| {
        let d: [f32; 3] = std::array::from_fn(|i| a.position[i] - b.position[i]);
        d.iter().map(|v| v * v).sum::<f32>() > 1e-10
    });
    assert!(moved, "rung bell body must swing away from rest");
}

/// Chest, trapped chest, ender chest and the festive/copper variants all select the one
/// `minecraft:chest#main` rig; the Solaris-authored catalog must supply it even though the
/// activated `Models` carries no explicit entry (regression: the item bake used to demand an
/// explicit `models.rigs` entry and killed the session with "special model rig not activated").
#[test]
fn chest_item_variants_resolve_authored_rig_without_explicit_registry_entry() {
    let models = models_with(Vec::new());
    assert!(models.rigs.is_empty());
    assert!(
        models.has_authored_rig("minecraft:chest#main"),
        "authored catalog must supply the chest rig"
    );
    let authored = models.rig("minecraft:chest#main");
    assert!(!authored.is_placeholder());
    let stack = crate::inventory::Stack::default();
    for texture in ["normal", "trapped", "ender", "christmas"] {
        let mut content = Content::test_cube();
        content.insert_test_texture(&format!("minecraft:entity/chest/{texture}"), test_region(5));
        let special = serde_json::json!({
            "type": "minecraft:chest",
            "texture": format!("minecraft:{texture}"),
        });
        let model = crate::entities::special_item(&models, &content, &stack, &special)
            .unwrap_or_else(|e| panic!("chest item {texture}: {e}"));
        let expected: usize = authored.bones.iter().map(|b| b.quads.len()).sum();
        assert!(expected > 0);
        assert_eq!(model.quads.len(), expected, "{texture}");
        assert!(model.quads.iter().all(|q| q.texture_layer == 5));
    }
}

#[test]
fn copper_statue_states_draw_distinct_poses_in_scene_pass() {
    let position = [258, 65, 135];
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut poses = Vec::new();
    for pose in ["standing", "sitting", "running", "star"] {
        let content = village_content(
            "minecraft:copper_golem_statue",
            "minecraft:copper_golem_statue",
            BTreeMap::from([
                ("facing".into(), "south".into()),
                ("copper_golem_pose".into(), pose.into()),
            ]),
            vec![("minecraft:entity/copper_golem/copper_golem", 9)],
        );
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models_with(vec![])));
        place(&mut store, &content, position);
        let mut geometry = DynamicGeometry::default();
        let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
        assert_eq!(stats.block_entities, 1, "{pose}: {:?}", stats.unrendered);
        assert!(stats.unrendered.is_empty(), "{pose}: {:?}", stats.unrendered);
        assert!(geometry.vertices.iter().all(|v| v.texture_layer == 9));
        poses.push(geometry.vertices.iter().map(|v| v.position).collect::<Vec<_>>());
    }
    for i in 0..poses.len() {
        for j in i + 1..poses.len() {
            assert_ne!(poses[i], poses[j], "statue poses {i} and {j} must differ");
        }
    }
}

#[test]
fn copper_statue_item_uses_complete_texture_resource_location() {
    let models = models_with(Vec::new());
    let stack = crate::inventory::Stack::default();
    let mut content = Content::test_cube();
    content.insert_test_texture("minecraft:entity/copper_golem/copper_golem", test_region(5));
    content.insert_test_texture("minecraft:entity/copper_golem/copper_golem_oxidized", test_region(6));
    for (name, layer) in [("copper_golem", 5), ("copper_golem_oxidized", 6)] {
        let special = serde_json::json!({
            "type": "minecraft:copper_golem_statue",
            "texture": format!("minecraft:textures/entity/copper_golem/{name}.png"),
        });
        let model = crate::entities::special_item(&models, &content, &stack, &special).unwrap();
        assert!(model.quads.iter().all(|quad| quad.texture_layer == layer));
        assert!(!model.quads.is_empty());
    }
}

#[test]
fn bed_item_uses_distinct_head_and_foot_geometry() {
    let models = models_with(Vec::new());
    let stack = crate::inventory::Stack::default();
    let mut content = Content::test_cube();
    content.insert_test_texture("minecraft:entity/bed/red", test_region(5));
    let render = |part| {
        crate::entities::special_item(
            &models,
            &content,
            &stack,
            &serde_json::json!({"type":"minecraft:bed", "texture":"minecraft:red", "part":part}),
        )
        .unwrap()
    };
    let head = render("head");
    let foot = render("foot");
    assert_ne!(head.quads.iter().map(|q| q.positions).collect::<Vec<_>>(),
               foot.quads.iter().map(|q| q.positions).collect::<Vec<_>>());
    assert_ne!(head.quads.iter().map(|q| q.uv).collect::<Vec<_>>(),
               foot.quads.iter().map(|q| q.uv).collect::<Vec<_>>());
}

#[test]
fn trident_special_item_uses_entity_texture() {
    let models = models_with(Vec::new());
    let stack = crate::inventory::Stack::default();
    let mut content = Content::test_cube();
    content.insert_test_texture("minecraft:entity/trident/trident", test_region(5));
    let special = serde_json::json!({"type":"minecraft:trident"});
    let model = crate::entities::special_item(&models, &content, &stack, &special).unwrap();
    assert!(!model.quads.is_empty());
    assert!(model.quads.iter().all(|quad| quad.texture_layer == 5));
}

#[test]
fn banner_item_bakes_colored_flag_and_patterns() {
    let models = models_with(Vec::new());
    let mut content = Content::test_cube();
    content.insert_test_texture("minecraft:entity/banner/banner_base", test_region(5));
    content.insert_test_texture("minecraft:entity/banner/base", test_region(6));
    content.insert_test_texture("minecraft:entity/banner/stripe_bottom", test_region(7));
    let mut stack = crate::inventory::Stack::default();
    stack.components = Arc::new(BTreeMap::from([(
        "minecraft:banner_patterns".into(),
        serde_json::json!([{"pattern":{"asset_id":"minecraft:stripe_bottom"},"color":"blue"}]),
    )]));
    let special = serde_json::json!({"type":"minecraft:banner","color":"red"});
    let model = crate::entities::special_item(&models, &content, &stack, &special).unwrap();
    assert!(model.quads.iter().any(|q| q.texture_layer == 5));
    let red = super::super::render_blocks::dye("red").0;
    let blue = super::super::render_blocks::dye("blue").0;
    let rgb = |v: u32| {
        [
            (v >> 16 & 255) as f32 / 255.,
            (v >> 8 & 255) as f32 / 255.,
            (v & 255) as f32 / 255.,
        ]
    };
    assert!(model.quads.iter().any(|q| q.texture_layer == 6
        && q.tint == crate::protocol::models::TintKind::Constant(rgb(red))));
    assert!(model.quads.iter().any(|q| q.texture_layer == 7
        && q.tint == crate::protocol::models::TintKind::Constant(rgb(blue))));
    assert!(model.quads.iter().filter(|q| q.texture_layer == 6 || q.texture_layer == 7)
        .all(|q| q.entity_material == Some(EntityMaterial::BannerPattern)));
    let mut geometry = DynamicGeometry::default();
    super::super::presentation::append_baked(
        &model, Mat4::IDENTITY, [1., 1.], &mut geometry,
    ).unwrap();
    assert!(!geometry.indices[EntityMaterial::BannerPattern as usize].is_empty());
    assert!(!geometry.indices[EntityMaterial::EntitySolid as usize].is_empty());
}

#[test]
fn shield_item_renders_plain_and_patterned_components() {
    let models = models_with(Vec::new());
    let mut content = Content::test_cube();
    content.insert_test_texture("minecraft:entity/shield/shield_base_nopattern", test_region(10));
    content.insert_test_texture("minecraft:entity/shield/shield_base", test_region(11));
    content.insert_test_texture("minecraft:entity/shield/base", test_region(12));
    content.insert_test_texture("minecraft:entity/shield/stripe_bottom", test_region(13));
    let special = serde_json::json!({"type":"minecraft:shield"});
    let mut stack = crate::inventory::Stack::default();
    let plain = crate::entities::special_item(&models, &content, &stack, &special).unwrap();
    assert!(!plain.quads.is_empty());
    assert!(plain.quads.iter().all(|q| q.texture_layer == 10));
    stack.components = Arc::new(BTreeMap::from([
        ("minecraft:base_color".into(), serde_json::json!("red")),
        ("minecraft:banner_patterns".into(), serde_json::json!([
            {"pattern":{"asset_id":"minecraft:stripe_bottom"},"color":"blue"}
        ])),
    ]));
    let patterned = crate::entities::special_item(&models, &content, &stack, &special).unwrap();
    assert!(patterned.quads.iter().any(|q| q.texture_layer == 11));
    let red = super::super::render_blocks::dye("red").0;
    let blue = super::super::render_blocks::dye("blue").0;
    let rgb = |v: u32| [(v >> 16 & 255) as f32 / 255., (v >> 8 & 255) as f32 / 255., (v & 255) as f32 / 255.];
    assert!(patterned.quads.iter().any(|q| q.texture_layer == 12
        && q.tint == crate::protocol::models::TintKind::Constant(rgb(red))));
    assert!(patterned.quads.iter().any(|q| q.texture_layer == 13
        && q.tint == crate::protocol::models::TintKind::Constant(rgb(blue))));
    let mut geometry = DynamicGeometry::default();
    super::super::presentation::append_baked(&patterned, Mat4::IDENTITY, [1., 1.], &mut geometry).unwrap();
    assert!(!geometry.indices[EntityMaterial::BannerPattern as usize].is_empty());
    assert!(!geometry.indices[EntityMaterial::EntitySolid as usize].is_empty());
}

/// A rig that is neither activated nor authored is a logged placeholder, never an error.
#[test]
fn unknown_special_rig_falls_back_to_placeholder_instead_of_failing() {
    let models = models_with(Vec::new());
    assert!(!models.has_authored_rig("minecraft:no_such_rig#main"));
    assert!(models.rig("minecraft:no_such_rig#main").is_placeholder());
    // Every special-item family's rig key resolves to a real authored model.
    for key in [
        "minecraft:shield#main",
        "minecraft:trident#main",
        "minecraft:chest#main",
        "minecraft:shulker_box#main",
        "minecraft:conduit_shell#main",
        "minecraft:decorated_pot_base#main",
        "minecraft:standing_banner#main",
        "minecraft:bed_head#main",
        "minecraft:skeleton_skull#main",
        "minecraft:wither_skeleton_skull#main",
        "minecraft:zombie_head#main",
        "minecraft:creeper_head#main",
        "minecraft:piglin_head#main",
        "minecraft:dragon_skull#main",
        "minecraft:player_head#main",
        "minecraft:copper_golem#main",
    ] {
        assert!(models.has_authored_rig(key), "{key}");
    }
}

#[test]
fn static_skulls_render_and_report_unresolved_player_profiles() {
    for (block, key, texture_name) in [
        ("minecraft:zombie_head", "minecraft:zombie_head#main", "minecraft:entity/zombie/zombie"),
        ("minecraft:dragon_wall_head", "minecraft:dragon_skull#main", "minecraft:entity/enderdragon/dragon"),
        ("minecraft:piglin_head", "minecraft:piglin_head#main", "minecraft:entity/piglin/piglin"),
        ("minecraft:player_head", "minecraft:player_head#main", "minecraft:entity/player/default"),
    ] {
        let content = village_content(
            block,
            "minecraft:skull",
            BTreeMap::from([
                ("rotation".into(), "4".into()),
                ("facing".into(), "north".into()),
            ]),
            vec![(texture_name, 3)],
        );
        let rig = Rig {
            bones: vec![
                bone("head", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
                bone("jaw", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
                bone("left_ear", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
            ],
            part_by_name: BTreeMap::new(),
        };
        let mut models = models_with(vec![(key.into(), rig)]);
        models.default_skins = (0..7)
            .map(|_| super::super::resources::DefaultSkin {
                texture: texture_name.into(),
                model: super::super::PlayerModel::Wide,
            })
            .collect();
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models));
        let position = [8, 64, -12];
        place(&mut store, &content, position);
        let (origin, camera) = camera_for(position);
        let world = World::new(0, 256);
        let mut geometry = DynamicGeometry::default();
        let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
        assert_eq!(stats.block_entities, 1, "{block}");
        assert!(stats.unrendered.is_empty(), "{block}: {:?}", stats.unrendered);
        assert_eq!(geometry.vertices.len(), 12, "{block}");
        assert_eq!(geometry.indices[EntityMaterial::EntityCutoutZOffset as usize].len(), 18);
        let expected_origin = if block.contains("_wall_") {
            [0.5, 0.25, 0.75]
        } else {
            [0.5, 0., 0.5]
        };
        for (actual, expected) in geometry.vertices[0].position.into_iter().zip(expected_origin) {
            assert!((actual - expected).abs() < 1e-5, "{block}");
        }

        if block == "minecraft:dragon_wall_head" || block == "minecraft:piglin_head" {
            let first = geometry.vertices.iter().map(|v| v.position).collect::<Vec<_>>();
            let source = &mut store.block_entities.get_mut(&position).unwrap().animation.source;
            source.skull_ticks = 3;
            source.skull_active = true;
            let mut animated = DynamicGeometry::default();
            let stats = render_static(&mut store, &content, &world, origin, camera, 0.5, &mut animated);
            assert_eq!(stats.block_entities, 1);
            assert_ne!(first, animated.vertices.iter().map(|v| v.position).collect::<Vec<_>>());
        }
        if block == "minecraft:player_head" {
            store.block_entities.get_mut(&position).unwrap().data =
                Arc::new(serde_json::json!({"profile":{"name":"example"}}));
            let mut profiled = DynamicGeometry::default();
            let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut profiled);
            assert_eq!(stats.block_entities, 0);
            assert!(profiled.vertices.is_empty());
            assert!(stats.unrendered.contains_key("minecraft:skull"));
        }
    }
}

#[test]
fn static_banner_renders_standing_and_wall_with_patterns_and_world_time() {
    for (block, key, props) in [
        (
            "minecraft:white_banner",
            "minecraft:standing_banner",
            BTreeMap::from([("rotation".into(), "0".into())]),
        ),
        (
            "minecraft:red_wall_banner",
            "minecraft:wall_banner",
            BTreeMap::from([("facing".into(), "south".into())]),
        ),
    ] {
        let mut content = village_content(
            block,
            "minecraft:banner",
            props,
            vec![
                ("minecraft:entity/banner/banner_base", 3),
                ("minecraft:entity/banner/base", 4),
                ("minecraft:entity/banner/stripe_bottom", 5),
            ],
        );
        content.set_registries(BTreeMap::from([(
            "minecraft:banner_pattern".into(),
            vec![(
                "minecraft:stripe_bottom".into(),
                serde_json::json!({"asset_id": "minecraft:stripe_bottom"}),
            )],
        )]));
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models_with(vec![(
            format!("{key}#main"),
            Rig {
                bones: vec![
                    bone("pole", [0., 0., 0.], vec![quad(3., 0., 0., 1., 1.)]),
                    bone("flag", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
                ],
                part_by_name: BTreeMap::new(),
            },
        )])));
        let position = [8, 64, -12];
        place(&mut store, &content, position);
        store.block_entities.get_mut(&position).unwrap().data = Arc::new(
            serde_json::json!({"patterns": [{"pattern": "minecraft:stripe_bottom", "color": "blue"}]}),
        );
        let (origin, camera) = camera_for(position);
        let world = World::new(0, 256);
        let mut first = DynamicGeometry::default();
        let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut first);
        assert_eq!(stats.block_entities, 1, "{block}: {:?}", stats.unrendered);
        assert!(stats.unrendered.is_empty());
        assert_eq!(first.vertices.len(), 16);
        assert_eq!(first.vertices.iter().filter(|v| v.texture_layer == 5).count(), 4);
        for i in 0..4 {
            assert_eq!(first.vertices[4 + i].position, first.vertices[8 + i].position);
            assert_eq!(first.vertices[4 + i].position, first.vertices[12 + i].position);
            assert_ne!(first.vertices[i].position, first.vertices[8 + i].position);
        }
        let mut later = DynamicGeometry::default();
        let stats = store.append_static_block_geometry(crate::entities::StaticBlockSceneFrame { content: &content, world: &world, origin, camera, camera_rotation: [0., 0.], radius: 32., scoping: false, text_filtering: false, game_time: 25, partial: 0., sample_light: &|_| Ok(Some([7., 3.])) }, &mut later)
            .unwrap();
        assert_eq!(stats.block_entities, 1);
        assert_ne!(first.vertices[6].position, later.vertices[6].position);
    }
}

#[test]
fn static_shulker_box_renders_and_opening_moves_lid() {
    let content = village_content(
        "minecraft:shulker_box",
        "minecraft:shulker_box",
        BTreeMap::from([("facing".into(), "up".into())]),
        vec![("minecraft:entity/shulker/shulker", 7)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:shulker_box#main".into(),
        Rig {
            bones: vec![
                bone("base", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
                bone("lid", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
            ],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut closed = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut closed);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_eq!(closed.vertices.len(), 8);
    assert!(closed.vertices.iter().all(|vertex| vertex.texture_layer == 7));
    let animation = &mut store.block_entities.get_mut(&position).unwrap().animation;
    animation.progress = 1.;
    animation.previous = 1.;
    let mut opened = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut opened);
    assert_eq!(stats.block_entities, 1);
    assert_eq!(closed.vertices[0].position, opened.vertices[0].position);
    assert_ne!(closed.vertices[5].position, opened.vertices[5].position);
}

#[test]
fn static_decorated_pot_uses_received_sherd_textures() {
    let content = village_content(
        "minecraft:decorated_pot",
        "minecraft:decorated_pot",
        BTreeMap::from([("facing".into(), "north".into())]),
        vec![
            ("minecraft:entity/decorated_pot/decorated_pot_base", 1),
            ("minecraft:entity/decorated_pot/decorated_pot_side", 2),
            ("minecraft:entity/decorated_pot/angler_pottery_pattern", 3),
            ("minecraft:entity/decorated_pot/skull_pottery_pattern", 4),
        ],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![
        (
            "minecraft:decorated_pot_base#main".into(),
            Rig {
                bones: vec![bone("base", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)])],
                part_by_name: BTreeMap::new(),
            },
        ),
        (
            "minecraft:decorated_pot_sides#main".into(),
            Rig {
                bones: ["back", "left", "right", "front"]
                    .into_iter()
                    .map(|name| bone(name, [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]))
                    .collect(),
                part_by_name: BTreeMap::new(),
            },
        ),
    ])));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    store.block_entities.get_mut(&position).unwrap().data = Arc::new(
        serde_json::json!({"sherds": {
            "back": {"id": "minecraft:angler_pottery_sherd"},
            "left": {"id": "minecraft:brick"},
            "right": {"id": "minecraft:brick"},
            "front": {"id": "minecraft:skull_pottery_sherd"}
        }}),
    );
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut geometry = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_eq!(geometry.vertices.len(), 20);
    assert!(geometry.vertices[4..8].iter().all(|vertex| vertex.texture_layer == 3));
    assert!(geometry.vertices[8..16].iter().all(|vertex| vertex.texture_layer == 2));
    assert!(geometry.vertices[16..20].iter().all(|vertex| vertex.texture_layer == 4));
}

#[test]
fn static_enchanting_table_renders_animated_book() {
    let content = village_content(
        "minecraft:enchanting_table",
        "minecraft:enchanting_table",
        BTreeMap::new(),
        vec![("minecraft:entity/enchantment/enchanting_table_book", 3)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:book#main".into(),
        Rig {
            bones: vec![bone("left_lid", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)])],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut first = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut first);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_eq!(first.vertices.len(), 4);
    assert!(first.vertices.iter().all(|vertex| vertex.texture_layer == 3));
    store.block_entities.get_mut(&position).unwrap().animation.source.ticks = 10;
    let mut later = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut later);
    assert_eq!(stats.block_entities, 1);
    assert_ne!(first.vertices[0].position, later.vertices[0].position);
}

#[test]
fn static_conduit_switches_shell_to_active_eye_and_faces_camera() {
    let content = village_content(
        "minecraft:conduit",
        "minecraft:conduit",
        BTreeMap::from([("waterlogged".into(), "false".into())]),
        vec![
            ("minecraft:entity/conduit/base", 1),
            ("minecraft:entity/conduit/cage", 2),
            ("minecraft:entity/conduit/wind", 3),
            ("minecraft:entity/conduit/wind_vertical", 3),
            ("minecraft:entity/conduit/closed_eye", 4),
            ("minecraft:entity/conduit/open_eye", 5),
        ],
    );
    let rig = || Rig {
        bones: vec![bone("part", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)])],
        part_by_name: BTreeMap::new(),
    };
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(
        ["shell", "cage", "wind", "eye"]
            .into_iter()
            .map(|part| (format!("minecraft:conduit#{part}"), rig()))
            .collect(),
    )));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut inactive = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut inactive);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_eq!(inactive.vertices.len(), 4);
    assert!(inactive.vertices.iter().all(|vertex| vertex.texture_layer == 1));
    let source = &mut store.block_entities.get_mut(&position).unwrap().animation.source;
    source.conduit_active = true;
    source.conduit_hunting = true;
    let mut active = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut active);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_eq!(active.vertices.len(), 16);
    assert!(active.vertices[12..].iter().all(|vertex| vertex.texture_layer == 5));
    let mut turned = DynamicGeometry::default();
    let stats = store.append_static_block_geometry(crate::entities::StaticBlockSceneFrame { content: &content, world: &world, origin, camera, camera_rotation: [90., 0.], radius: 32., scoping: false, text_filtering: false, game_time: 0, partial: 0., sample_light: &|_| Ok(Some([7., 3.])) }, &mut turned).unwrap();
    assert_eq!(stats.block_entities, 1);
    assert_ne!(active.vertices[13].position, turned.vertices[13].position);
}

#[test]
fn static_campfire_reports_unavailable_observed_item() {
    let content = village_content(
        "minecraft:campfire",
        "minecraft:campfire",
        BTreeMap::from([("facing".into(), "south".into())]),
        vec![],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    store.block_entities.get_mut(&position).unwrap().data = Arc::new(serde_json::json!({
        "Items": [{"Slot": 0, "id": "test:missing_food", "count": 1}]
    }));
    let (origin, camera) = camera_for(position);
    let mut out = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &World::new(0, 256), origin, camera, 0., &mut out);
    assert!(stats.unrendered["minecraft:campfire"].contains("test:missing_food"));
}

#[test]
fn campfire_painter_places_each_received_slot() {
    let content = village_content(
        "minecraft:campfire",
        "minecraft:campfire",
        BTreeMap::from([("facing".into(), "south".into())]),
        vec![],
    );
    let mut store = super::super::Store::new();
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let block = store.block_entities.get_mut(&position).unwrap();
    block.data = Arc::new(serde_json::json!({
        "Items": [
            {"Slot": 0, "id": "test:food", "count": 1},
            {"Slot": 1, "id": "test:food", "count": 1},
            {"Slot": 2, "id": "test:food", "count": 1},
            {"Slot": 3, "id": "test:food", "count": 1}
        ]
    }));
    let mut placements = Vec::new();
    let mut painter = |_: &Value, context: &str, seed: i32, matrix: Mat4, light: [f32; 2], _: &mut DynamicGeometry| {
        placements.push((context.to_owned(), seed, matrix.transform_point3(Vec3::ZERO), light));
        Ok(())
    };
    items::campfire(block, &content, Mat4::IDENTITY, [7., 3.], &mut painter, &mut DynamicGeometry::default()).unwrap();
    assert_eq!(placements.len(), 4);
    assert!(placements.iter().all(|(context, _, _, light)| context == "fixed" && *light == [7., 3.]));
    for i in 0..4 {
        assert_eq!(placements[i].1, (crate::protocol::pack_pos(position) as i32).wrapping_add(i as i32));
        for j in 0..i {
            assert!((placements[i].2 - placements[j].2).length() > 0.1);
        }
    }
}

#[test]
fn static_campfire_paints_received_food_and_reports_model_errors() {
    let mut content = village_content(
        "minecraft:campfire",
        "minecraft:campfire",
        BTreeMap::from([("facing".into(), "south".into())]),
        vec![],
    );
    content.item_names.insert(1, "test:food".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:food")),
    ]));
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    store.items.insert_test_model("test:food", Content::test_cube().models[0].clone());
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut empty = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut empty);
    assert!(stats.unrendered.is_empty());
    assert!(empty.vertices.is_empty());
    store.block_entities.get_mut(&position).unwrap().data = Arc::new(serde_json::json!({
        "Items": [
            {"Slot": 0, "id": "test:food", "count": 1},
            {"Slot": 1, "id": "test:food", "count": 1},
            {"Slot": 2, "id": "test:food", "count": 1},
            {"Slot": 3, "id": "test:food", "count": 1}
        ]
    }));
    let mut filled = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut filled);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert_eq!(stats.block_entities, 1);
    assert_eq!(filled.vertices.len(), 4 * content.models[0].quads.len() * 4);
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:missing_model")),
    ]));
    let mut fresh = super::super::Store::new();
    fresh.activate_models(Arc::new(models_with(vec![])));
    place(&mut fresh, &content, position);
    fresh.block_entities.get_mut(&position).unwrap().data = store.block_entities[&position].data.clone();
    let stats = render_static(&mut fresh, &content, &world, origin, camera, 0., &mut DynamicGeometry::default());
    assert!(stats.unrendered.contains_key("minecraft:campfire"), "{:?}", stats.unrendered);
}

#[test]
fn static_vault_scene_follows_received_display_item_and_clear() {
    let mut content = village_content("minecraft:vault", "minecraft:vault", BTreeMap::new(), vec![]);
    content.item_names.insert(1, "test:reward".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:reward")),
    ]));
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    store.items.insert_test_model("test:reward", Content::test_cube().models[0].clone());
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);

    let mut empty = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut empty);
    assert_eq!(stats.block_entities, 0);
    assert!(empty.vertices.is_empty());

    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
        "shared_data": {"display_item": {"id": "test:reward", "count": 1}}
    })), Arc::from(&[10, 0][..]), None, &content, 2).unwrap();
    let mut displayed = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0.5, &mut displayed);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert_eq!(stats.block_entities, 1);
    assert!(!displayed.vertices.is_empty());

    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
        "shared_data": {}
    })), Arc::from(&[10, 0][..]), None, &content, 3).unwrap();
    let mut cleared = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut cleared);
    assert_eq!(stats.block_entities, 0);
    assert!(stats.unrendered.is_empty());
    assert!(cleared.vertices.is_empty());
}

#[test]
fn static_shelf_renders_and_clears_received_item_slots() {
    let mut content = village_content(
        "minecraft:oak_shelf",
        "minecraft:shelf",
        BTreeMap::from([("facing".into(), "north".into())]),
        vec![],
    );
    content.item_names.insert(1, "test:food".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:food")),
    ]));
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    store.items.insert_test_model("test:food", Content::test_cube().models[0].clone());
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut centers = Vec::new();
    for slot in [0, 2] {
        store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
            "Items": [{"Slot": slot, "id": "test:food", "count": 1}]
        })), Arc::from(&[10, 0][..]), None, &content, slot + 2).unwrap();
        let mut out = DynamicGeometry::default();
        let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut out);
        assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
        assert_eq!(out.vertices.len(), content.models[0].quads.len() * 4);
        assert!(out.vertices.iter().all(|vertex| vertex.light == [7., 3.]));
        centers.push(out.vertices.iter().map(|v| v.position[0]).sum::<f32>() / out.vertices.len() as f32);
    }
    assert!((centers[0] - centers[1]).abs() > 0.5);
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({"Items": []})),
        Arc::from(&[10, 0][..]), None, &content, 5).unwrap();
    let mut cleared = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut cleared);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert!(cleared.vertices.is_empty());
}

#[test]
fn static_brushable_renders_received_item_with_adjacent_light_and_clears() {
    let mut content = village_content(
        "minecraft:suspicious_sand",
        "minecraft:brushable_block",
        BTreeMap::from([("dusted".into(), "1".into())]),
        vec![],
    );
    content.item_names.insert(1, "test:food".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:food")),
    ]));
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    store.items.insert_test_model("test:food", Content::test_cube().models[0].clone());
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut empty = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut empty);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert!(empty.vertices.is_empty());
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
        "hit_direction": 1, "item": {"id": "test:food", "count": 1}
    })), Arc::from(&[10, 0][..]), None, &content, 2).unwrap();
    let mut out = DynamicGeometry::default();
    let stats = store.append_static_block_geometry(crate::entities::StaticBlockSceneFrame { content: &content, world: &world, origin, camera, camera_rotation: [0., 0.], radius: 32., scoping: false, text_filtering: false, game_time: 0, partial: 0., sample_light: &|point| Ok(Some(if point == [8., 65., -12.] { [11., 4.] } else { [7., 3.] })) }, &mut out).unwrap();
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert_eq!(out.vertices.len(), content.models[0].quads.len() * 4);
    assert!(out.vertices.iter().all(|vertex| vertex.light == [11., 4.]));
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({"hit_direction": 1})),
        Arc::from(&[10, 0][..]), None, &content, 3).unwrap();
    let mut cleared = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut cleared);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert!(cleared.vertices.is_empty());
}

#[test]
fn static_beacon_scene_renders_active_beam_but_not_unpowered_or_pending() {
    let content = village_content(
        "minecraft:beacon",
        "minecraft:beacon",
        BTreeMap::new(),
        vec![("minecraft:entity/beacon/beacon_beam", 7)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    let position = [0, 64, 0];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut geometry = DynamicGeometry::default();

    // A valid scan with no pyramid must not paint a beam.
    {
        let source = &mut store.block_entities.get_mut(&position).unwrap().animation.source;
        source.beam_sections.push(crate::entities::blocks::BeamSection {
            color: 0xffffff,
            height: 1,
        });
    }
    let hidden = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
    assert_eq!(hidden.block_entities, 0);
    assert!(geometry.vertices.is_empty());

    {
        let source = &mut store.block_entities.get_mut(&position).unwrap().animation.source;
        source.beacon_levels = 1;
        source.beam_sections.push(crate::entities::blocks::BeamSection {
            color: 0xff0000,
            height: 1,
        });
        source.pending_world = true;
    }
    let pending = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
    assert_eq!(pending.lighting_pending, 1);
    assert!(geometry.vertices.is_empty());

    store.block_entities.get_mut(&position).unwrap().animation.source.pending_world = false;
    let visible = render_static(&mut store, &content, &world, origin, camera, 0., &mut geometry);
    assert!(visible.unrendered.is_empty(), "{:?}", visible.unrendered);
    assert_eq!(visible.block_entities, 1);
    assert_eq!(geometry.vertices.len(), 64);
    assert!(!geometry.indices[EntityMaterial::BeaconBeamOpaque as usize].is_empty());
    assert!(!geometry.indices[EntityMaterial::BeaconBeamTranslucent as usize].is_empty());

    // The beam extends high above its source, so vertical distance from the
    // beacon must not cull it; horizontal distance still obeys the radius.
    let mut high = DynamicGeometry::default();
    let high_camera = [camera[0], camera[1] + 128., camera[2]];
    let high_stats = render_static(&mut store, &content, &world, origin, high_camera, 0., &mut high);
    assert_eq!(high_stats.block_entities, 1);
    assert_eq!(high.vertices.len(), 64);
    let mut far = DynamicGeometry::default();
    let far_camera = [camera[0] + 40., camera[1], camera[2]];
    let far_stats = render_static(&mut store, &content, &world, origin, far_camera, 0., &mut far);
    assert_eq!(far_stats.block_entities, 0);
    assert!(far.vertices.is_empty());
}

#[test]
fn static_portal_gateway_scene_emits_surfaces_and_respects_world() {
    use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};

    let position = [8, 8, 8];
    let (origin, camera) = camera_for(position);
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    let portal = village_content("minecraft:end_portal", "minecraft:end_portal", BTreeMap::new(), vec![("minecraft:environment/end_sky", 3)]);
    place(&mut store, &portal, position);
    let mut geometry = DynamicGeometry::default();
    let stats = render_static(&mut store, &portal, &World::new(0, 16), origin, camera, 0., &mut geometry);
    assert_eq!(stats.block_entities, 1);
    assert_eq!(geometry.indices[EntityMaterial::EndPortal as usize].len(), 12);
    assert!(geometry.vertices.iter().all(|vertex| vertex.texture_layer == 3));

    let gateway = village_content(
        "minecraft:end_gateway",
        "minecraft:end_gateway",
        BTreeMap::new(),
        vec![("minecraft:environment/end_sky", 3), ("minecraft:entity/end_portal/end_gateway_beam", 7)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![])));
    place(&mut store, &gateway, position);
    store.block_entities.get_mut(&position).unwrap().animation.source.gateway_age = 100;
    let mut pending_geometry = DynamicGeometry::default();
    let pending = render_static(&mut store, &gateway, &World::new(0, 16), origin, camera, 0., &mut pending_geometry);
    assert_eq!(pending.lighting_pending, 1);
    assert!(pending_geometry.vertices.is_empty());

    let uniform = |size| Palette { bits: 0, values: vec![0], words: Vec::new(), size };
    let mut world = World::new(0, 16);
    world.insert([0, 0], Chunk {
        sections: vec![Arc::new(Section {
            blocks: uniform(4096),
            biomes: uniform(64),
            non_empty_block_count: 0,
        })],
        sky: BTreeMap::new(),
        block: BTreeMap::new(),
        block_entities: crate::protocol::world::empty_block_entities(),
        surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
        surface_heightmap_received: false,
    }).unwrap();
    let mut active = DynamicGeometry::default();
    let stats = render_static(&mut store, &gateway, &world, origin, camera, 0., &mut active);
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    assert_eq!(stats.block_entities, 1);
    assert_eq!(active.indices[EntityMaterial::EndGateway as usize].len(), 36);
    assert!(!active.indices[EntityMaterial::BeaconBeamOpaque as usize].is_empty());
    let mut high = DynamicGeometry::default();
    let high_stats = render_static(&mut store, &gateway, &world, origin, [camera[0], camera[1] + 128., camera[2]], 0., &mut high);
    assert_eq!(high_stats.block_entities, 1);
    assert!(!high.indices[EntityMaterial::BeaconBeamOpaque as usize].is_empty());
    let mut extended = DynamicGeometry::default();
    let extended_stats = render_static(&mut store, &gateway, &world, origin, [camera[0] + 200., camera[1], camera[2]], 0., &mut extended);
    assert_eq!(extended_stats.block_entities, 1);
    let mut far = DynamicGeometry::default();
    let far_stats = render_static(&mut store, &gateway, &world, origin, [camera[0] + 257., camera[1], camera[2]], 0., &mut far);
    assert_eq!(far_stats.block_entities, 0);
    assert!(far.vertices.is_empty());

    store.block_entities.get_mut(&position).unwrap().animation.source.gateway_age = 200;
    world.set([8, 7, 8], 1, |state| Ok(state == 0)).unwrap();
    let mut occluded = DynamicGeometry::default();
    let stats = render_static(&mut store, &gateway, &world, origin, camera, 0., &mut occluded);
    assert_eq!(stats.block_entities, 1);
    assert_eq!(occluded.indices[EntityMaterial::EndGateway as usize].len(), 30);
    assert!(occluded.indices[EntityMaterial::BeaconBeamOpaque as usize].is_empty());

    for y in [0, 15] {
        let boundary = [8, y, 8];
        let (boundary_origin, boundary_camera) = camera_for(boundary);
        let mut boundary_store = super::super::Store::new();
        boundary_store.activate_models(Arc::new(models_with(vec![])));
        place(&mut boundary_store, &gateway, boundary);
        boundary_store.block_entities.get_mut(&boundary).unwrap().animation.source.gateway_age = 200;
        let mut out = DynamicGeometry::default();
        let stats = render_static(&mut boundary_store, &gateway, &world, boundary_origin, boundary_camera, 0., &mut out);
        assert_eq!(stats.block_entities, 1, "boundary {y}");
        assert_eq!(out.indices[EntityMaterial::EndGateway as usize].len(), 36, "boundary {y}");
    }
    let edge = [0, 8, 8];
    let (edge_origin, edge_camera) = camera_for(edge);
    let mut edge_store = super::super::Store::new();
    edge_store.activate_models(Arc::new(models_with(vec![])));
    place(&mut edge_store, &gateway, edge);
    let mut edge_out = DynamicGeometry::default();
    let stats = render_static(&mut edge_store, &gateway, &world, edge_origin, edge_camera, 0., &mut edge_out);
    assert_eq!(stats.lighting_pending, 1);
    assert!(edge_out.vertices.is_empty());
}

#[test]
fn static_trial_spawner_uses_received_entity_and_rejects_unsupported_data() {
    use super::super::resources::Family;
    let (mut models, _) = super::super::animation_tests::fixture();
    let schema = models.schemas.get_mut("minecraft:pig").unwrap();
    schema.accessors.insert("DATA_BABY_ID".into(), [16, 8]);
    schema.accessors.insert("DATA_VARIANT_ID".into(), [19, 28]);
    models.families.insert("minecraft:pig".into(), Family {
        rig: "test:pig#main".into(),
        texture: Some("test:pig".into()),
        baby_rig: Some("test:pig_baby#main".into()),
        textures: vec![],
        renderer_class: None,
        materials: BTreeMap::new(),
    });
    for (key, width) in [
        ("test:pig#main", 1.),
        ("test:pig_baby#main", 0.4),
        ("minecraft:warm_pig#main", 0.8),
    ] {
        models.rigs.insert(key.into(), Rig {
            bones: vec![bone("body", [0., 0., 0.], vec![quad(0., 0., 0., width, 1.)])],
            part_by_name: BTreeMap::from([("body".into(), 0)]),
        });
    }
    let mut content = village_content(
        "minecraft:trial_spawner", "minecraft:trial_spawner",
        BTreeMap::from([("trial_spawner_state".into(), "active".into())]),
        vec![("test:pig", 3), ("test:warm", 8)],
    );
    content.entity_names.insert(1, "minecraft:pig".into());
    let mut store = super::super::Store::new();
    store.configure_registry("minecraft:pig_variant", &[
        ("minecraft:temperate".into(), serde_json::json!({"model":"temperate","asset_id":"test:pig"})),
        ("minecraft:warm".into(), serde_json::json!({"model":"warm","asset_id":"test:warm"})),
    ]).unwrap();
    store.activate_models(Arc::new(models));
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let update = |store: &mut super::super::Store, entity: Value, tick| {
        store.apply_block_entity(position, 7,
            Arc::new(serde_json::json!({"spawn_data":{"entity":entity}})),
            Arc::from(&[10, 0][..]), None, &content, tick).unwrap();
    };
    update(&mut store, serde_json::json!({"id":"minecraft:pig"}), 2);
    let mut adult = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut adult);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert!(adult.vertices.iter().all(|vertex| vertex.texture_layer == 3));
    assert!(!adult.vertices.is_empty());

    update(&mut store, serde_json::json!({"id":"minecraft:pig","Age":-1}), 3);
    let mut baby = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut baby);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert_ne!(adult.vertices[1].position, baby.vertices[1].position);

    update(&mut store, serde_json::json!({"id":"minecraft:pig","variant":"minecraft:warm"}), 4);
    let mut warm = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut warm);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert!(warm.vertices.iter().all(|vertex| vertex.texture_layer == 8));

    update(&mut store, serde_json::json!({"id":"minecraft:pig","unhandled_visual":1}), 5);
    let mut unsupported = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut unsupported);
    assert!(stats.unrendered["minecraft:trial_spawner"].contains("unhandled_visual"));
    assert!(unsupported.vertices.is_empty());
    content.insert_test_state_properties(1, BTreeMap::from([("trial_spawner_state".into(), "cooldown".into())]));
    let mut hidden = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut hidden);
    assert_eq!(stats.block_entities, 0);
    assert!(stats.unrendered.is_empty());
    assert!(hidden.vertices.is_empty());
    content.insert_test_state_properties(1, BTreeMap::from([("trial_spawner_state".into(), "active".into())]));
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({"spawn_data":{}})),
        Arc::from(&[10, 0][..]), None, &content, 6).unwrap();
    let mut cleared = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut cleared);
    assert_eq!(stats.block_entities, 0);
    assert!(stats.unrendered.is_empty());
    assert!(cleared.vertices.is_empty());
    store.unload_chunk([position[0].div_euclid(16), position[2].div_euclid(16)]);
    assert!(store.preview_cache.is_empty());
}

#[test]
fn static_spawner_preview_renders_received_equipment_with_shared_painter() {
    use super::super::resources::Family;
    let (mut models, _) = super::super::animation_tests::fixture();
    let schema = models.schemas.remove("minecraft:pig").unwrap();
    models.schemas.insert("minecraft:zombie".into(), schema);
    models.families.insert("minecraft:zombie".into(), Family {
        rig: "test:zombie#main".into(), texture: Some("test:zombie".into()),
        baby_rig: None, textures: vec![], renderer_class: None, materials: BTreeMap::new(),
    });
    let mut arm = bone("right_arm", [0., 0., 0.], vec![quad(0., 0., 0., 0.3, 1.)]);
    arm.role = BoneRole::RightArm;
    models.rigs.insert("test:zombie#main".into(), Rig {
        bones: vec![arm], part_by_name: BTreeMap::from([("right_arm".into(), 0)]),
    });
    let mut content = village_content(
        "minecraft:trial_spawner", "minecraft:trial_spawner",
        BTreeMap::from([("trial_spawner_state".into(), "active".into())]),
        vec![("test:zombie", 3)],
    );
    content.entity_names.insert(1, "minecraft:zombie".into());
    content.item_names.insert(2, "test:weapon".into());
    content.insert_test_item_components(2, BTreeMap::from([
        ("minecraft:max_stack_size".into(), serde_json::json!(64)),
        ("minecraft:item_model".into(), serde_json::json!("test:weapon")),
    ]));
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models));
    store.items.insert_test_model("test:weapon", Content::test_cube().models[0].clone());
    let position = [8, 64, -12];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
        "spawn_data":{"entity":{"id":"minecraft:zombie"}}
    })), Arc::from(&[10, 0][..]), None, &content, 2).unwrap();
    let mut bare = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut bare);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    store.apply_block_entity(position, 7, Arc::new(serde_json::json!({
        "spawn_data":{"entity":{"id":"minecraft:zombie","equipment":{
            "mainhand":{"id":"test:weapon","count":1}
        }}}
    })), Arc::from(&[10, 0][..]), None, &content, 3).unwrap();
    let mut equipped = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut equipped);
    assert_eq!(stats.block_entities, 1, "{:?}", stats.unrendered);
    assert!(equipped.vertices.len() > bare.vertices.len());
}
