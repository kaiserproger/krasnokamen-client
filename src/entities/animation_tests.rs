//! Authored state/rig fixtures; no proprietary model or texture bytes.
use super::*;
use glam::{Mat4, Vec3};
use resources::{Bone, BoneRole, Rig};

pub(super) fn fixture() -> (Models, Entity) {
    let mut models = Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::new(),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: vec![],
        default_skins: vec![],
        poses: BTreeMap::from([(0, "STANDING".into())]),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        armor_models: BTreeMap::new(),
        motion: BTreeMap::new(),
    };
    let transform = Transform {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
        head_yaw: 0.,
    };
    let mut entity = Entity {
        handle: Handle {
            network_id: 41,
            generation: 9,
        },
        uuid: "00000000-0000-0000-0000-000000000041".into(),
        kind: 1,
        type_name: "minecraft:pig".into(),
        position: [0.; 3],
        velocity: [0.; 3],
        transform,
        previous: transform,
        on_ground: true,
        spawn_data: 0,
        observed_tick: 0,
        revision: 1,
        appearance_revision: 1,
        physical: physical::AnimationState::default(),
        binding: None,
        source: None,
        minecart_steps: vec![],
        acceleration_power: None,
        metadata: BTreeMap::new(),
        equipment: std::array::from_fn(|_| None),
        predicted_metadata: BTreeMap::new(),
        predicted_equipment: std::array::from_fn(|_| None),
        predicted_leash: None,
        effects: BTreeMap::new(),
        attributes: BTreeMap::new(),
        passenger_ids: vec![],
        passengers: vec![],
        vehicle: None,
        leash: None,
        leash_holder_id: None,
        animation: None,
        damage_tick: None,
        death_tick: None,
        hurt_direction: 0.,
        submitted_revision: None,
    };
    let facts: render_living::MotionFacts = serde_json::from_value(json!({
        "class":"authored.fixture", "is_mob":true, "walk_uses_y":"never", "methods":{},
        "body_rule":"standard", "walk_rule":"standard",
        "head_rule":{"kind":"fixed","value":75.0},
        "body_head_rule":{"kind":"fixed","value":75.0}
    }))
    .unwrap();
    models.motion.insert(entity.type_name.clone(), facts);
    models.schemas.insert(
        entity.type_name.clone(),
        resources::EntitySchema {
            protocol_id: 1,
            width: 1.,
            fire_immune: false,
            class: "authored.fixture".into(),
            eye_height: 0.8,
            dimension_constants: BTreeMap::new(),
            enum_ids: BTreeMap::new(),
            enum_numbers: BTreeMap::new(),
            hierarchy: vec!["net.minecraft.world.entity.LivingEntity".into()],
            method_owners: BTreeMap::new(),
            dimensions: resources::Dimensions {
                width: 1.,
                height: 1.,
                eye_height: 0.8,
                fixed: false,
                attachments: BTreeMap::new(),
            },
            default_attributes: BTreeMap::from([(
                "minecraft:scale".into(),
                resources::DefaultAttribute { id: 1, value: 1. },
            )]),
            height: 1.,
            accessors: BTreeMap::new(),
        },
    );
    entity.source = Some(source_state::SourceState::new(&entity, &models).unwrap());
    (models, entity)
}

fn tick_walk(models: &Models, entity: &mut Entity, x: f64) {
    let mut source = entity.source.take().unwrap();
    source.living.as_mut().unwrap().tick(
        models,
        entity,
        &render_living::TickInput {
            age: 1,
            position: [x, 0., 0.],
            yaw: 0.,
            alive: true,
            passenger: false,
            baby: false,
            visually_swimming: false,
            in_water: false,
            uses_y_for_walk: false,
            rotation_changed_before_base_tick: false,
            is_mob: true,
            first_passenger_is_mob: false,
            max_head_yaw: 75.,
            max_body_head_yaw: 75.,
            attack_active: false,
            camel_sitting: false,
            camel_visually_sitting: false,
            camel_sitting_down: false,
            camel_pose_transition: false,
            camel_pose_time: 0,
        },
    );
    entity.source = Some(source);
}
fn rig() -> Rig {
    let bones = [
        ("right_hind_leg", BoneRole::RightLeg),
        ("left_hind_leg", BoneRole::LeftLeg),
        ("right_arm", BoneRole::RightArm),
        ("left_arm", BoneRole::LeftArm),
        ("head", BoneRole::Head),
    ]
    .into_iter()
    .map(|(name, role)| Bone {
        parent: None,
        pivot: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role,
        name: name.into(),
        quads: vec![],
        normals: vec![],
    })
    .collect();
    Rig {
        bones,
        part_by_name: BTreeMap::new(),
    }
}

#[test]
fn animation_received_motion_swings_limbs_without_velocity_packets() {
    let (models, mut entity) = fixture();
    let rig = rig();
    let mut idle = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 1., Mat4::IDENTITY, &mut idle);
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    assert_eq!(entity.velocity, [0.; 3]);
    let mut moving = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 2, 1., Mat4::IDENTITY, &mut moving);
    assert_ne!(moving[0], idle[0]);
    assert_ne!(moving[2], idle[2]);
    let right = moving[0].transform_vector3(Vec3::Y);
    let left = moving[1].transform_vector3(Vec3::Y);
    assert!(
        right.z * left.z < 0.,
        "opposite legs must swing in opposite directions"
    );
    let mut halfway = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 2, 0.5, Mat4::IDENTITY, &mut halfway);
    assert_ne!(halfway[0], moving[0]);
}

#[test]
fn animation_spider_walk_moves_all_legs_and_settles_when_idle() {
    let (models, mut entity) = fixture();
    let rig = solaris_models::authored("minecraft:spider#main").unwrap();
    entity.type_name = "minecraft:spider".into();
    let pose = |entity: &Entity| {
        let mut matrices = vec![];
        presentation::evaluate_rig(rig, entity, &models, &crate::protocol::content::Content::test_cube(), 2, 1., Mat4::IDENTITY, &mut matrices);
        matrices
    };
    let idle = pose(&entity);
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    let moving = pose(&entity);
    for index in 0..8 {
        let bone = rig.part_by_name[&format!("leg{index}")];
        assert_ne!(idle[bone], moving[bone], "leg{index} must move");
    }
    for _ in 0..80 {
        tick_walk(&models, &mut entity, 0.4);
    }
    let stopped = pose(&entity);
    for index in 0..8 {
        let bone = rig.part_by_name[&format!("leg{index}")];
        let a = idle[bone].to_cols_array();
        let b = stopped[bone].to_cols_array();
        assert!(a.iter().zip(b).all(|(x, y)| (*x - y).abs() < 1e-5), "leg{index}");
    }
}

#[test]
fn animation_zombie_authored_legs_follow_received_motion() {
    let (models, mut entity) = fixture();
    let rig = solaris_models::authored("minecraft:zombie#main").unwrap();
    entity.type_name = "minecraft:zombie".into();
    let mut idle = vec![];
    presentation::evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 1., Mat4::IDENTITY, &mut idle);
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    let mut moving = vec![];
    presentation::evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 2, 1., Mat4::IDENTITY, &mut moving);
    for name in ["right_leg", "left_leg"] {
        let bone = rig.part_by_name[name];
        assert_ne!(idle[bone], moving[bone], "{name}");
    }
}

#[test]
fn animation_chicken_wings_rest_on_received_ground_and_beat_in_midair() {
    use crate::protocol::{
        content::Content,
        world::{Chunk, Palette, Section, SurfaceHeightmap, World},
    };
    use std::sync::Arc;
    let (mut models, mut entity) = fixture();
    let schema = models.schemas.remove("minecraft:pig").unwrap();
    let motion = models.motion.remove("minecraft:pig").unwrap();
    entity.type_name = "minecraft:chicken".into();
    models.schemas.insert(entity.type_name.clone(), schema);
    models.motion.insert(entity.type_name.clone(), motion);
    // A spawned mob that never moved has received no grounded move packet.
    entity.on_ground = false;
    let content = Content::test_cube();
    let section = |state: u32| {
        Arc::new(Section {
            blocks: Palette { bits: 0, values: vec![state], words: Vec::new(), size: 4096 },
            biomes: Palette { bits: 0, values: vec![0], words: Vec::new(), size: 64 },
            non_empty_block_count: if state == 0 { 0 } else { 4096 },
        })
    };
    // Solid cubes up to y = 16, air above.
    let mut world = World::new(0, 32);
    world
        .insert([0, 0], Chunk {
            sections: vec![section(1), section(0)],
            sky: BTreeMap::new(),
            block: BTreeMap::new(),
            block_entities: crate::protocol::world::empty_block_entities(),
            surface_heightmap: Arc::new(SurfaceHeightmap::new(32).unwrap()),
            surface_heightmap_received: false,
        })
        .unwrap();
    let rig = solaris_models::authored("minecraft:chicken#main").unwrap();
    let wing = rig.bones.iter().position(|bone| bone.role == BoneRole::WingNegativeX).unwrap();
    let wing_at = |entity: &mut Entity, tick: u64| {
        entity.source.as_mut().unwrap().age = tick as i32;
        let mut pose = vec![];
        presentation::evaluate_rig(rig, entity, &models, &crate::protocol::content::Content::test_cube(), tick, 0.5, Mat4::IDENTITY, &mut pose);
        pose[wing]
    };
    let settle_at = |entity: &mut Entity, y: f64| {
        entity.position = [8.5, y, 8.5];
        entity.transform.position = entity.position;
        entity.previous = entity.transform;
        for _ in 0..6 {
            let bounds = physical::bounds(&models, entity, 0, 0.).unwrap();
            source_state::tick(entity, &models, &world, &content, 0, 0, false, false, bounds).unwrap();
        }
    };
    settle_at(&mut entity, 16.);
    let rest = wing_at(&mut entity, 3);
    for tick in [4, 7, 11] {
        assert_eq!(wing_at(&mut entity, tick), rest, "a chicken standing on a block must not flap");
    }
    settle_at(&mut entity, 20.);
    assert_ne!(wing_at(&mut entity, 3), wing_at(&mut entity, 7), "a chicken in midair must flap");
    settle_at(&mut entity, 16.);
    assert_eq!(wing_at(&mut entity, 9), rest, "wings fold again after landing");
}

#[test]
fn animation_creeper_fuse_swells_and_flashes_and_charge_draws_an_aura() {
    use crate::{
        api::{DynamicGeometry, EntityMaterial},
        entities::EntityRenderFrame,
        protocol::content::{Content, TextureRegion},
    };
    use std::sync::Arc;
    let (mut models, mut entity) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    for (name, index) in [("DATA_SHARED_FLAGS_ID", [0, 0]), ("DATA_SWELL_DIR", [16, 1]),
                          ("DATA_IS_POWERED", [17, 8]), ("DATA_IS_IGNITED", [18, 8])] {
        schema.accessors.insert(name.into(), index);
    }
    let motion = models.motion.remove("minecraft:pig").unwrap();
    entity.type_name = "minecraft:creeper".into();
    models.schemas.insert(entity.type_name.clone(), schema);
    models.motion.insert(entity.type_name.clone(), motion);
    models.families.insert(entity.type_name.clone(), resources::Family {
        rig: "minecraft:creeper#main".into(), baby_rig: None,
        texture: Some("minecraft:entity/creeper/creeper".into()), textures: vec![],
        renderer_class: None, materials: BTreeMap::new(),
    });
    entity.source = Some(source_state::SourceState::new(&entity, &models).unwrap());
    let mut content = Content::test_cube();
    for (name, layer) in [("minecraft:entity/creeper/creeper", 1), ("minecraft:entity/creeper/creeper_armor", 2)] {
        content.insert_test_texture(name, TextureRegion { uv: [0., 0., 1., 1.], layer, width: 64, height: 32 });
    }
    let models = Arc::new(models);
    let mut store = Store::new();
    store.models = Some(models.clone());
    store.ids.insert(41, 0);
    store.spatial.entry([0, 0]).or_default().insert(41);
    store.slots.push(entity);
    let set = |store: &mut Store, index: u8, value: Metadata| {
        store.slots[0].metadata.insert(index, value);
    };
    let tick = |store: &mut Store, ticks: usize| {
        for _ in 0..ticks {
            tick_walk(&models, &mut store.slots[0], 0.);
        }
    };
    let render = |store: &mut Store| {
        let mut out = DynamicGeometry::default();
        let stats = presentation::append(store, EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0., 1., -4.], camera_rotation: [0.; 2],
        radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None,
        light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None, }, &mut out).unwrap();
        assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
        out
    };
    let body = |out: &DynamicGeometry| {
        let indices = &out.indices[EntityMaterial::EntityCutout as usize];
        let xs = indices.iter().map(|&i| out.vertices[i as usize].position[0]);
        let (low, high) = xs.fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x), h.max(x)));
        let flashing = indices.iter().any(|&i| out.vertices[i as usize].overlay_uv[0] > 0.);
        (high - low, flashing)
    };
    set(&mut store, 16, Metadata::Int(-1));
    let idle = render(&mut store);
    let (idle_width, idle_flash) = body(&idle);
    assert!(!idle_flash, "an idle creeper must not flash");
    assert!(idle.indices[EntityMaterial::EnergySwirl as usize].is_empty(), "uncharged creepers have no aura");
    set(&mut store, 16, Metadata::Int(1));
    let mut flashes = [false; 2];
    let mut widths = vec![];
    for _ in 0..30 {
        tick(&mut store, 1);
        let (width, flashing) = body(&render(&mut store));
        flashes[usize::from(flashing)] = true;
        widths.push(width);
    }
    assert_eq!(flashes, [true, true], "the fuse must blink white on and off");
    assert!(widths.windows(2).all(|w| w[1] >= w[0] - 1e-4), "the creeper keeps growing while the fuse burns");
    assert!(widths[29] > idle_width * 1.2, "a creeper about to explode is visibly swollen: {widths:?}");
    set(&mut store, 16, Metadata::Int(-1));
    tick(&mut store, 40);
    let (calm_width, calm_flash) = body(&render(&mut store));
    assert!((calm_width - idle_width).abs() < 1e-3 && !calm_flash, "a defused creeper returns to normal");
    set(&mut store, 17, Metadata::Bool(true));
    assert!(!render(&mut store).indices[EntityMaterial::EnergySwirl as usize].is_empty(), "charged aura");
    set(&mut store, 0, Metadata::Byte(32));
    let invisible = render(&mut store);
    assert!(invisible.indices[EntityMaterial::EntityCutout as usize].is_empty());
    assert!(!invisible.indices[EntityMaterial::EnergySwirl as usize].is_empty(), "the aura stays visible");
}

#[test]
fn animation_zombie_family_raises_both_arms() {
    let (mut models, mut entity) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.accessors.insert("DATA_MOB_FLAGS_ID".into(), [15, 0]);
    schema.accessors.insert("DATA_BABY_ID".into(), [16, 8]);
    let rig = rig();
    for name in ["zombie", "husk", "drowned", "giant", "zombie_villager", "zombified_piglin"] {
        entity.type_name = format!("minecraft:{name}");
        models.schemas.insert(entity.type_name.clone(), schema);
        for (flags, baby, held, min_y, max_y) in [
            (0, false, false, 0.15, 0.20),
            (4, false, false, -0.6, -0.4),
            (0, true, true, 0.93, 1.01),
            (0, true, false, 0.15, 0.20),
        ] {
            entity.metadata.insert(15, Metadata::Byte(flags));
            entity.metadata.insert(16, Metadata::Bool(baby));
            entity.equipment[0] = held.then(|| {
                let mut stack = Stack::default();
                stack.id = 1;
                stack.count = 1;
                stack
            });
            let mut pose = vec![];
            presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut pose);
            for arm in [2, 3] {
                let direction = pose[arm].transform_vector3(Vec3::Y);
                assert!((min_y..=max_y).contains(&direction.y), "{name} arm {arm}: {direction:?}");
            }
        }
        schema = models.schemas.remove(&entity.type_name).unwrap();
    }
}

#[test]
fn animation_zombie_idle_arms_bob_and_attack_moves_both() {
    let (models, mut entity) = fixture();
    entity.type_name = "minecraft:zombie".into();
    entity.source = None;
    let rig = rig();
    let pose = |entity: &Entity, tick| {
        let mut out = vec![];
        presentation::evaluate_rig(&rig, entity, &models, &crate::protocol::content::Content::test_cube(), tick, 0., Mat4::IDENTITY, &mut out);
        out
    };
    let idle = pose(&entity, 0);
    let bob = pose(&entity, 20);
    assert_ne!(idle[2], bob[2]);
    assert_ne!(idle[3], bob[3]);
    let before_attack = pose(&entity, 2);
    entity.animation = Some((0, 0));
    let attack = pose(&entity, 2);
    assert_ne!(attack[2], before_attack[2]);
    assert_ne!(attack[3], before_attack[3]);
}

#[test]
fn animation_villager_model_family_swings_legs_at_half_amplitude() {
    let (models, mut entity) = fixture();
    let rig = rig();
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    let swing = |entity: &Entity| {
        let mut matrices = vec![];
        presentation::evaluate_rig(&rig, entity, &models, &crate::protocol::content::Content::test_cube(), 2, 1., Mat4::IDENTITY, &mut matrices);
        matrices[0].transform_vector3(Vec3::Y).z.asin()
    };
    let humanoid = swing(&entity);
    assert!(humanoid.abs() > 0.05, "fixture must actually walk");
    for name in [
        "minecraft:villager",
        "minecraft:wandering_trader",
        "minecraft:witch",
    ] {
        entity.type_name = name.into();
        let family = swing(&entity);
        assert!(
            (family - humanoid * 0.5).abs() < 1e-5,
            "{name} legs {family} vs humanoid {humanoid}"
        );
    }
}

/// A handheld tool in an arm that hangs at the side must point forward out of the fist
/// (source ItemInHandLayer + `rotationXYZ` display transform), not lie along the arm.
#[test]
fn held_tool_points_forward_from_the_fist_not_along_the_arm() {
    use crate::protocol::models::{BakedModel, ModelTransform};
    let mut model = BakedModel::default();
    model.display.insert(
        "thirdperson_righthand".into(),
        ModelTransform {
            rotation: [0., -90., 55.],
            translation: [0., 4. / 16., 0.5 / 16.],
            scale: [0.85; 3],
        },
    );
    // Arm bone frame: +Y runs down the arm, model front is -Z.
    let matrix = presentation::held_item_attachment(Mat4::IDENTITY, 0)
        * presentation::item_matrix(&model, "thirdperson_righthand");
    let handle = matrix.transform_point3(Vec3::new(0.1, 0.1, 0.5));
    let tip = matrix.transform_point3(Vec3::new(0.95, 0.95, 0.5));
    let blade = tip - handle;
    assert!(
        blade.z < -0.9,
        "blade must point forward (-Z), got {blade:?}"
    );
    assert!(blade.x.abs() < 0.05, "no sideways lean, got {blade:?}");
    assert!(
        blade.y.abs() < 0.25,
        "near level, not along the arm, got {blade:?}"
    );
    // The grip sits at the fist, 0.625 down the arm and 1/16 outboard of the right arm.
    assert!(
        (handle.y - 0.51).abs() < 0.05 && handle.x < 0.,
        "{handle:?}"
    );
    let left = presentation::held_item_attachment(Mat4::IDENTITY, 1);
    assert!(left.transform_point3(Vec3::ZERO).x > 0.);
}

#[test]
fn animation_idle_is_not_driven_by_global_clock_or_stale_velocity() {
    let (models, mut entity) = fixture();
    entity.velocity = [0.5, 0., 0.];
    let rig = rig();
    let mut first = vec![];
    let mut later = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 1., Mat4::IDENTITY, &mut first);
    presentation::evaluate_rig(&rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 100, 1., Mat4::IDENTITY, &mut later);
    assert_eq!(first, later);
}

#[test]
fn animation_head_tracks_relative_to_lagged_body() {
    let (models, mut entity) = fixture();
    let living = entity.source.as_mut().unwrap().living.as_mut().unwrap();
    living.body_yaw_old = 20.;
    living.body_yaw = 40.;
    living.head_yaw_old = 60.;
    living.head_yaw = 80.;
    entity.transform.pitch = 25.;
    entity.previous.pitch = 25.;
    let mut matrices = vec![];
    presentation::evaluate_rig(&rig(), &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0.5, Mat4::IDENTITY, &mut matrices);
    let expected =
        Mat4::from_rotation_y(40f32.to_radians()) * Mat4::from_rotation_x(25f32.to_radians());
    for (actual, expected) in matrices[4]
        .to_cols_array()
        .into_iter()
        .zip(expected.to_cols_array())
    {
        assert!((actual - expected).abs() < 1e-5);
    }
}


#[test]
fn rc2_received_sleep_pose_rotates_rig_horizontally() {
    let (mut models, mut entity) = fixture();
    let schema = models.schemas.get_mut(&entity.type_name).unwrap();
    schema.accessors.insert("DATA_POSE".into(), [6, 20]);
    schema.accessors.insert("SLEEPING_POS_ID".into(), [14, 11]);
    // Entity metadata body from the server: pose=2 and optional head block position=(0,0,0).
    let packet = [6, 20, 2, 14, 11, 1, 0, 0, 0, 0, 0, 0, 0, 0, 255];
    entity.metadata = metadata::decode(&mut crate::protocol::Reader::new(&packet), &Content::test_cube(), &models).unwrap();
    models.rigs.insert("authored:rig".into(), Rig {
        bones: vec![Bone {
            parent: None, pivot: Vec3::ZERO, rotation: Vec3::ZERO, scale: Vec3::ONE,
            role: BoneRole::Fixed, name: "body".into(),
            quads: vec![[[0., 0., 0., 0., 0.], [1., 0., 0., 1., 0.],
                         [1., 1., 0., 1., 1.], [0., 1., 0., 0., 1.]]],
            normals: vec![[0., 0., 1.]],
        }],
        part_by_name: BTreeMap::new(),
    });
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:rig".into(),
        texture: crate::protocol::content::TextureRegion { uv: [0., 0., 1., 1.], layer: 0, width: 1, height: 1 },
        style: presentation::RootStyle::Living, layers: None,
        anger: None,
    }));
    models.poses.insert(2, "SLEEPING".into());
    models.schemas.get_mut(&entity.type_name).unwrap().dimension_constants.insert(
        "SLEEPING_DIMENSIONS".into(),
        resources::DimensionConstant::Dimensions(resources::Dimensions {
            width: 0.2, height: 0.2, eye_height: 0.2, fixed: true, attachments: BTreeMap::new(),
        }),
    );
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    let mut axes = Vec::new();
    for direction in [[0, 1], [-1, 0], [0, -1], [1, 0]] {
        let mut geometry = DynamicGeometry::default();
        store.append_geometry(crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|position| {
            assert_eq!(position, [0, 0, 0]);
            Some(direction)
        } }, &mut geometry).unwrap();
        assert_eq!(geometry.vertices.len(), 4);
        assert!(geometry.vertices[0].normal[1] < -0.99, "the model back must face downward");
        let a = Vec3::from_array(geometry.vertices[0].position);
        let b = Vec3::from_array(geometry.vertices[3].position);
        let axis = b - a;
        assert!(axis.y.abs() < 1e-5, "body axis must be horizontal: {axis:?}");
        axes.push(axis);
    }
    assert!(axes[0].dot(axes[2]) < -0.99);
    assert!(axes[1].dot(axes[3]) < -0.99);
    assert!(axes[0].dot(axes[1]).abs() < 1e-5);
}

#[test]
fn rc2_passenger_packet_bends_villager_legs_and_dismount_restores_them() {
    use crate::protocol::{Reader, packet_ids::play::clientbound as cb};
    let (models, mut villager) = fixture();
    let (_, mut seat) = fixture();
    seat.handle.network_id = 42;
    villager.type_name = "minecraft:villager".into();
    let mut store = Store::new();
    store.slots = vec![villager, seat];
    store.ids.insert(41, 0);
    store.ids.insert(42, 1);
    let matrices = |store: &Store| {
        let mut out = Vec::new();
        presentation::evaluate_rig(&rig(), store.get(41).unwrap(), &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut out);
        out
    };
    let standing = matrices(&store);
    // Vehicle 42, one passenger 41: the ordinary SET_PASSENGERS wire body.
    store.apply(cb::SET_PASSENGERS, &mut Reader::new(&[42, 1, 41]), &Content::test_cube(), 0).unwrap();
    let seated = matrices(&store);
    assert!(seated[0].transform_vector3(Vec3::Y).z.abs() > 0.9);
    assert!(seated[1].transform_vector3(Vec3::Y).z.abs() > 0.9);
    store.apply(cb::SET_PASSENGERS, &mut Reader::new(&[42, 0]), &Content::test_cube(), 0).unwrap();
    assert_eq!(matrices(&store), standing);
}
#[test]
fn animation_local_render_slot_is_restored_even_when_extraction_fails() {
    let (_, entity) = fixture();
    let mut store = Store::new();
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    let result = store.append_geometry(crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut DynamicGeometry::default());
    assert!(result.is_err());
    assert!(store.local_avatar.is_some());
    assert!(store.local_render_slot.is_none());
    assert_eq!(store.len(), 0);
    assert!(store.ids.is_empty() && store.spatial.is_empty());
}

#[test]
fn animation_server_interpolation_keeps_ground_height_and_shortest_rotation() {
    let (_, mut entity) = fixture();
    entity.previous.position = [0., 64., 0.];
    entity.transform.position = [0.3, 64., 0.3];
    entity.previous.yaw = 179.;
    entity.transform.yaw = -179.;
    entity.physical.lerp_duration = 3;
    entity.physical.lerp_progress = 1;
    let halfway = entity.presented_transform(10, 0.5);
    assert_eq!(halfway.position, [0.15, 64., 0.15]);
    assert_eq!(halfway.yaw, 180.);
}

#[test]
fn animation_midtick_packet_continues_from_the_displayed_pose_without_snapping() {
    let (_, mut entity) = fixture();
    entity.previous.position = [0., 64., 0.];
    entity.transform.position = [3., 64., 0.];
    entity.physical.lerp_duration = 3;
    entity.physical.lerp_progress = 1;
    let shown = entity.presented_transform(10, 0.5).position[0];
    assert!((shown - 1.5).abs() < 1e-9);
    // A new position packet lands half a tick into tick 10.
    let mut target = entity.transform;
    target.position = [4.5, 64., 0.];
    entity.move_to(target, None, true, 10, false, 0.5);
    let at_arrival = entity.presented_transform(10, 0.5).position[0];
    assert!((at_arrival - shown).abs() < 1e-9, "{at_arrival} vs {shown}");
    // Time since arrival drives the blend across the tick boundary: no jump.
    let before = entity.presented_transform(10, 0.999).position[0];
    entity.physical.lerp_progress = 1;
    let after = entity.presented_transform(11, 0.).position[0];
    assert!(
        after >= before && after - before < 0.02,
        "{before} -> {after}"
    );
    // Duration is measured from arrival, then it settles exactly on target
    // without a sawtooth when the tick counter stops at duration + 1.
    entity.physical.lerp_progress = 3;
    let late = entity.presented_transform(13, 0.25).position[0];
    assert!(late < 4.5);
    entity.physical.lerp_progress = 4;
    for partial in [0., 0.3, 0.99] {
        assert_eq!(entity.presented_transform(14, partial).position[0], 4.5);
    }
    // A snap (teleport) never inherits a phase.
    entity.move_to(target, None, true, 15, true, 0.7);
    assert_eq!(entity.physical.lerp_phase, 0.);
    assert_eq!(entity.presented_transform(15, 0.2).position[0], 4.5);
}

#[test]
fn animation_hurt_flash_is_emitted_and_expires_on_real_rig_vertices() {
    let (mut models, mut entity) = fixture();
    let bone = Bone {
        parent: None,
        pivot: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::Fixed,
        name: "body".into(),
        quads: vec![[
            [0., 0., 0., 0., 0.],
            [1., 0., 0., 1., 0.],
            [1., 1., 0., 1., 1.],
            [0., 1., 0., 0., 1.],
        ]],
        normals: vec![[0., 0., 1.]],
    };
    models.rigs.insert(
        "authored:rig".into(),
        Rig {
            bones: vec![bone],
            part_by_name: BTreeMap::new(),
        },
    );
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:rig".into(),
        texture: crate::protocol::content::TextureRegion {
            uv: [0., 0., 1., 1.],
            layer: 0,
            width: 1,
            height: 1,
        },
        style: presentation::RootStyle::Living,
        layers: None,
        anger: None,
    }));
    entity.damage_tick = Some(10);
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    for (tick, expected) in [(10, [0., 3.]), (19, [0., 3.]), (20, [0., 10.])] {
        let mut geometry = DynamicGeometry::default();
        let stats = store.append_geometry(crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick, partial: 0.5, game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut geometry)
            .unwrap();
        assert_eq!(stats.submitted, 1, "{:?}", stats.unrendered);
        assert_eq!(geometry.vertices.len(), 4);
        assert!(geometry
            .vertices
            .iter()
            .all(|vertex| vertex.overlay_uv == expected));
        assert_eq!(store.len(), 0);
    }
}

#[test]
fn first_person_arm_uses_baked_geometry_skin_uvs_and_normals() {
    let (mut models, mut entity) = fixture();
    let bone = Bone {
        parent: None,
        pivot: Vec3::new(-0.3, 0.125, 0.),
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::RightArm,
        name: "right_arm".into(),
        quads: vec![[
            [0., 0.625, 0., 0.1, 0.2],
            [0., 0.625, 0.2, 0.1, 0.4],
            [0.2, 0.625, 0.2, 0.3, 0.4],
            [0.2, 0.625, 0., 0.3, 0.2],
        ]],
        normals: vec![[0., 1., 0.]],
    };
    models.rigs.insert(
        "authored:arm".into(),
        Rig {
            bones: vec![bone],
            part_by_name: BTreeMap::new(),
        },
    );
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:arm".into(),
        texture: crate::protocol::content::TextureRegion {
            uv: [0.25, 0.5, 0.75, 1.],
            layer: 7,
            width: 64,
            height: 64,
        },
        style: presentation::RootStyle::Living,
        layers: None,
        anger: None,
    }));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    let root = Mat4::from_translation(Vec3::new(0.52, -0.9, -0.65))
        * Mat4::from_rotation_z(0.25)
        * Mat4::from_rotation_x(-0.15);
    let mut geometry = DynamicGeometry::default();
    store
        .append_local_arm(&Content::test_cube(), true, root, [15., 0.], &mut geometry)
        .unwrap();
    assert_eq!(geometry.vertices.len(), 4);
    assert_eq!(
        geometry.indices[crate::api::EntityMaterial::EntityCutout as usize].len(),
        6
    );
    let tip = root.transform_point3(Vec3::new(0., 0.625, 0.));
    assert!((Vec3::from_array(geometry.vertices[0].position) - tip).length() < 1e-6);
    assert!(
        tip.y > -0.4,
        "fist must be visible above the offscreen shoulder"
    );
    assert_eq!(geometry.vertices[0].uv, [0.3, 0.6]);
    let normal = root.transform_vector3(Vec3::Y).normalize();
    for vertex in &geometry.vertices {
        assert!((Vec3::from_array(vertex.normal) - normal).length() < 1e-6);
        assert_eq!(vertex.texture_layer, 7);
    }
    // Exercise the actual player empty-hand path, not only a supplied rig root.
    let mut player = crate::player::Player::new();
    player.pose_known = true;
    let mut scene = crate::api::Scene::default();
    player.append_first_person(&Content::test_cube(), &crate::inventory::InventoryStore::default(), &mut store, &mut scene, crate::player::FirstPersonFrame { partial: 1., light: [15., 0.], item_context: &crate::ui::items::ItemContext::default(), ui: None })
        .unwrap();
    let camera = Mat4::look_to_rh(Vec3::ZERO, Vec3::Z, Vec3::Y);
    let tip_normal = camera.transform_vector3(Vec3::from_array(scene.first_person.vertices[0].normal));
    assert!(
        tip_normal.z < -0.5,
        "the fist points away from the camera; the long arm sides, not its end cap, face the viewer",
    );
}

#[test]
fn first_person_missing_local_profile_uses_uuid_skin_like_remote_players() {
    let (mut models, mut entity) = fixture();
    entity.type_name = "minecraft:player".into();
    let mut store = Store::new();
    let mut content = Content::test_cube();
    models.default_skins = (0..18)
        .map(|index| {
            let texture = format!("authored:skin-{index}");
            content.insert_test_texture(&texture, crate::protocol::content::TextureRegion {
                uv: [0., 0., 1., 1.],
                layer: index,
                width: 64,
                height: 64,
            });
            resources::DefaultSkin {
                texture,
                model: if index % 2 == 0 { PlayerModel::Slim } else { PlayerModel::Wide },
            }
        })
        .collect();
    for index in 0..4 {
        entity.uuid = uuid::Uuid::from_u128(index).to_string();
        store.local_uuid = Some(entity.uuid.clone());
        let local = presentation::material(&store, &models, &entity, &content, 0).unwrap();
        store.local_uuid = None;
        let remote = presentation::material(&store, &models, &entity, &content, 0).unwrap();
        assert_eq!(local.texture.layer, index as u32);
        assert_eq!(local.texture.layer, remote.texture.layer);
        assert_eq!(local.rig, remote.rig);
        assert_eq!(local.rig, if index % 2 == 0 {
            "minecraft:player_slim#main"
        } else {
            "minecraft:player#main"
        });
    }
}

#[test]
fn first_person_local_avatar_ignores_empty_equipment_stacks() {
    let (mut models, entity) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.class = "authored.player".into();
    models.schemas.insert("minecraft:player".into(), schema);
    let mut facts = models.motion.remove("minecraft:pig").unwrap();
    facts.is_mob = false;
    facts.body_rule = Some(render_living::BodyRule::Living);
    models.motion.insert("minecraft:player".into(), facts);
    let mut content = Content::test_cube();
    content.entity_names.insert(1, "minecraft:player".into());
    let mut inventory = crate::inventory::InventoryStore::default();
    for slot in [36, 45, 8, 7, 6, 5] {
        inventory.player.slots[slot] = Some(Stack::default());
    }
    let mut player = crate::player::Player::new();
    player.uuid = Some(Arc::from(entity.uuid));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    let world = crate::protocol::world::World::default();
    store
        .sync_local_avatar(&player, &inventory, &world, &content, (1, 1))
        .unwrap();
    assert!(store
        .local_avatar
        .as_ref()
        .unwrap()
        .equipment
        .iter()
        .all(Option::is_none));
    let mut item = Stack::default();
    item.id = 1;
    item.count = 1;
    inventory.player.slots[36] = Some(item);
    store
        .sync_local_avatar(&player, &inventory, &world, &content, (2, 2))
        .unwrap();
    let equipment = &store.local_avatar.as_ref().unwrap().equipment;
    assert_eq!(equipment[0].as_ref().map(|s| s.id), Some(1));
    assert!(equipment[1..].iter().all(Option::is_none));
}

/// The villager family has no per-hand arm bones, only the crossed `arms`: the main-hand item
/// (a witch's potion hangs from the nose) must still be emitted, at that anchor.
#[test]
fn crossed_arms_rigs_emit_the_held_item_at_the_source_anchor() {
    use crate::protocol::content::Content;
    let (models, mut entity) = fixture();
    let content = {
        let mut content = Content::test_cube();
        content.item_names.insert(1, "minecraft:stick".into());
        content.item_names.insert(2, "minecraft:potion".into());
        content
    };
    let cube = Content::test_cube().models[0].clone();
    for (kind, item, anchor) in [
        ("minecraft:villager", 1, "arms"),
        ("minecraft:wandering_trader", 1, "arms"),
        ("minecraft:witch", 1, "arms"),
        ("minecraft:witch", 2, "nose"),
    ] {
        let rig = solaris_models::authored(kind).unwrap();
        let mut items = crate::ui::items::ItemModels::new();
        items.insert_test_model("test:held", cube.clone());
        let mut stack = Stack::default();
        stack.id = item;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            serde_json::json!("test:held"),
        )]));
        entity.type_name = kind.into();
        entity.equipment[0] = Some(stack);
        let mut transforms = vec![];
        presentation::evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut transforms);
        let mut out = DynamicGeometry::default();
        let mut stats = presentation::PresentationStats::default();
        presentation::append_equipment(
            presentation::EquipmentPose { item_context: None, entity: &entity, rig, transforms: &transforms },
            &mut items,
            &mut equipment::Cache::default(),
            equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 0, partial: 0., light: [15., 0.] },
            &mut out,
            &mut stats,
        )
        .unwrap();
        assert!(stats.unrendered.is_empty(), "{kind}: {:?}", stats.unrendered);
        assert_eq!(out.vertices.len(), 24, "{kind} item {item} emitted");
        let centroid = out
            .vertices
            .iter()
            .fold(Vec3::ZERO, |sum, v| sum + Vec3::from_array(v.position))
            / out.vertices.len() as f32;
        let bone = transforms[rig.part_by_name[anchor]].transform_point3(Vec3::ZERO);
        assert!(
            (centroid - bone).length() < 0.45,
            "{kind}: item at {centroid:?} not at {anchor} {bone:?}"
        );
    }
}

#[test]
fn no_ai_mob_head_never_glances_at_the_local_player() {
    let (mut models, mut entity) = fixture();
    models
        .schemas
        .get_mut(&entity.type_name)
        .unwrap()
        .accessors
        .insert("DATA_MOB_FLAGS_ID".into(), [15, 0]);
    assert!(physical::may_glance(&models, &entity), "an AI mob keeps its idle glance");
    entity.metadata.insert(15, Metadata::Byte(1));
    assert!(
        !physical::may_glance(&models, &entity),
        "a summoned NoAI mob keeps its received head yaw like vanilla"
    );
}

#[test]
fn armor_stand_renders_feet_on_plate_and_respects_part_flags() {
    let (mut models, mut entity) = fixture();
    let mut schema = models.schemas.remove(&entity.type_name).unwrap();
    schema.accessors.insert("DATA_CLIENT_FLAGS".into(), [15, 0]);
    schema.dimension_constants.insert(
        "BABY_DIMENSIONS".into(),
        resources::DimensionConstant::Dimensions(schema.dimensions.clone()),
    );
    entity.type_name = "minecraft:armor_stand".into();
    entity.source = None;
    models.schemas.insert(entity.type_name.clone(), schema);
    entity.binding = Some(Ok(presentation::Binding {
        rig: "minecraft:armor_stand#renderer_main".into(),
        texture: crate::protocol::content::TextureRegion {
            uv: [0., 0., 1., 1.],
            layer: 0,
            width: 64,
            height: 64,
        },
        style: presentation::RootStyle::Living,
        layers: None,
        anger: None,
    }));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    let mut render = |flags: i8| {
        store.local_avatar.as_mut().unwrap().metadata.insert(15, Metadata::Byte(flags));
        let mut geometry = DynamicGeometry::default();
        store.append_geometry(crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut geometry)
            .unwrap();
        geometry
    };
    let plate = |v: &crate::api::Vertex| {
        v.position[0].abs() > 0.3 && v.position[2].abs() > 0.3 && v.position[1] < 0.08
    };
    let arm = |v: &crate::api::Vertex| {
        v.position[0].abs() > 0.25
            && v.position[2].abs() < 0.1
            && (0.6..1.2).contains(&v.position[1])
    };
    let default = render(0);
    assert!(default.vertices.iter().any(plate), "plate should be at foot level");
    assert!(!default.vertices.iter().any(arm), "arms are hidden by default");
    assert!(render(4).vertices.iter().any(arm), "ShowArms renders the arms");
    assert!(
        !render(8).vertices.iter().any(plate),
        "NoBasePlate hides the plate"
    );
    let height = |geometry: &DynamicGeometry| {
        let (min, max) = geometry.vertices.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(min, max), v| (min.min(v.position[1]), max.max(v.position[1])),
        );
        max - min
    };
    let small = render(1);
    assert!(
        (height(&small) - height(&default) * 0.5).abs() < 0.01,
        "small height {}, normal height {}",
        height(&small),
        height(&default)
    );
}

#[test]
fn world_clock_frame_and_drop_change_model_but_stone_does_not() {
    use crate::ui::item_dynamic::ItemWorldContext;
    use crate::ui::items::ItemContext;
    use std::cell::Cell;

    let (mut models, _) = fixture();
    for kind in ["minecraft:item_frame", "minecraft:item"] {
        let (mut fixture_models, _) = fixture();
        let mut schema = fixture_models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_ITEM".into(), [9, 7]);
        schema.accessors.insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
        models.schemas.insert(kind.into(), schema);
    }
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    let mut content = Content::test_cube();
    content.item_names.insert(1, "minecraft:clock".into());
    content.item_names.insert(2, "minecraft:stone".into());
    for (name, layer) in [("test:early", 11), ("test:late", 22), ("minecraft:stone", 33)] {
        let mut model = content.models[0].clone();
        for quad in &mut model.quads { quad.texture_layer = layer; }
        store.items.insert_test_model(name, model);
    }
    let clock_graph = json!({
        "type":"minecraft:range_dispatch", "property":"minecraft:time", "source":"daytime",
        "fallback":{"type":"minecraft:model","model":"test:early"},
        "entries":[{"threshold":0.125,"model":{"type":"minecraft:model","model":"test:late"}}]
    });
    content.insert_test_resource("assets/minecraft/items/clock.json", json!({"model":clock_graph.clone()}).to_string().as_bytes());
    content.insert_test_resource("assets/minecraft/items/stone.json", br#"{"model":{"type":"minecraft:model","model":"minecraft:stone"}}"#);
    store.items.insert_test_graph("minecraft:clock", clock_graph.clone());
    store.items.insert_test_graph("clock", clock_graph);
    for (id, kind, item) in [(1, "minecraft:item_frame", 1), (2, "minecraft:item", 1), (3, "minecraft:item", 2)] {
        let (_, mut entity) = fixture();
        entity.handle = Handle { network_id: id, generation: id as u64 };
        entity.type_name = kind.into();
        entity.spawn_data = 2;
        entity.binding = Some(Err("item uses item renderer".into()));
        if id == 2 {
            entity.transform.position[0] = 2.;
            entity.previous.position[0] = 2.;
        }
        entity.metadata.insert(0, Metadata::Byte(0x20));
        let mut stack = Stack::default();
        stack.id = item;
        stack.count = 1;
        if id == 1 {
            Arc::make_mut(&mut stack.components).insert("minecraft:item_model".into(), json!("clock"));
        }
        entity.metadata.insert(9, Metadata::Item(stack));
        store.ids.insert(id, store.slots.len());
        store.slots.push(entity);
        store.spatial.entry([0, 0]).or_default().insert(id);
    }
    let mut context = ItemContext {
        world: Some(ItemWorldContext {
            dimension: "minecraft:overworld".into(), position: [0.; 3], yaw: 0.,
            game_tick: 0, sun_angle: 0., moon_phase: 0., spawn: None, death: None,
            needles: Arc::default(),
        }),
        ..Default::default()
    };
    let frame_angle = Cell::new(0.);
    let ground_angle = Cell::new(0.);
    let fail_frame = Cell::new(false);
    let at = |point: [f64; 3]| {
        if point[0] == 0. && fail_frame.get() {
            return Err("test time sample failure".into());
        }
        Ok((if point[0] == 0. { frame_angle.get() } else { ground_angle.get() }, 0.))
    };
    let mut render = |context: &ItemContext| {
        let mut geometry = DynamicGeometry::default();
        let stats = store.append_geometry(crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderView::empty(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: Some((context, &at)), light: &|_| Ok(Some([15., 15.])), bed_direction: &|_| None }, &mut geometry).unwrap();
        let count = |layer| geometry.vertices.iter().filter(|v| v.texture_layer == layer).count();
        (count(11), count(22), count(33), stats.unrendered.get("minecraft:item_frame").cloned())
    };
    let before = render(&context);
    assert!(before.0 > 0 && before.1 == 0 && before.2 > 0 && before.3.is_none(), "{before:?}");
    assert_eq!(before.0, before.2 * 2, "clock frame and drop must both render");
    frame_angle.set(90.);
    for tick in 1..=100 {
        context.world.as_mut().unwrap().game_tick = tick;
        let after = render(&context);
        if tick == 1 { assert_eq!(after, before, "clock wobble should not jump to its target"); }
    }
    let frame_changed = render(&context);
    assert_eq!((frame_changed.0, frame_changed.1, frame_changed.2), (before.2, before.2, before.2),
        "unqualified frame clock must change independently of dropped clock");
    ground_angle.set(90.);
    for tick in 101..=200 {
        context.world.as_mut().unwrap().game_tick = tick;
        render(&context);
    }
    let both_changed = render(&context);
    assert_eq!((both_changed.0, both_changed.1, both_changed.2), (0, before.0, before.2));
    fail_frame.set(true);
    context.world.as_mut().unwrap().game_tick = 201;
    let failed_frame = render(&context);
    assert_eq!((failed_frame.0, failed_frame.1, failed_frame.2), (0, before.2, before.2));
    assert!(failed_frame.3.is_some_and(|reason| reason.contains("test time sample failure")));
}

fn action_fixture(kind: &str) -> (Models, Entity, Content) {
    let (mut models, mut e) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    for (name, accessor) in [
        ("DATA_LIVING_ENTITY_FLAGS", [8, 0]), ("DATA_PLAYER_MAIN_HAND", [15, 42]),
        ("DATA_BABY_ID", [16, 8]), ("DATA_SPELL_CASTING_ID", [17, 0]),
        ("IS_CHARGING_CROSSBOW", [18, 8]), ("DATA_POSE", [6, 20]),
        ("ARMADILLO_STATE", [19, 1]),
    ] { schema.accessors.insert(name.into(), accessor); }
    let motion = models.motion.remove("minecraft:pig").unwrap();
    e.type_name = kind.into();
    models.schemas.insert(kind.into(), schema);
    models.motion.insert(kind.into(), motion);
    e.metadata.insert(15, Metadata::Arm(1));
    e.source = Some(source_state::SourceState::new(&e, &models).unwrap());
    (models, e, Content::test_cube())
}

fn action_stack(id: u32, components: serde_json::Value) -> Stack {
    let mut stack = Stack::default();
    stack.id = id;
    stack.count = 1;
    stack.components = Arc::new(components.as_object().unwrap().iter()
        .map(|(k, v)| (k.clone(), v.clone())).collect());
    stack
}

fn action_age(models: &Models, e: &mut Entity, content: &Content, age: i32) {
    let mut source = e.source.take().unwrap();
    source.age = age;
    source.living.as_mut().unwrap().prepare_item_use(models, e, content, age).unwrap();
    e.source = Some(source);
}

fn action_pose(models: &Models, e: &Entity, content: &Content, rig: &Rig) -> Vec<Mat4> {
    let mut out = Vec::new();
    presentation::evaluate_rig(rig, e, models, content, 0, 0., Mat4::IDENTITY, &mut out);
    out
}

#[test]
fn animation_use_actions_follow_logical_hand_and_do_not_follow_item_model() {
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    content.item_names.insert(1, "minecraft:bow".into());
    content.item_names.insert(2, "minecraft:bread".into());
    let rig = rig();
    e.equipment[0] = Some(action_stack(1, json!({"minecraft:item_model":"minecraft:bread"})));
    e.metadata.insert(8, Metadata::Byte(1));
    action_age(&models, &mut e, &content, 1);
    let right_bow = action_pose(&models, &e, &content, &rig);
    for index in [2, 3] {
        assert!(right_bow[index].transform_vector3(Vec3::Y).z < -0.8, "both arms aim");
    }
    e.metadata.insert(15, Metadata::Arm(0));
    let left_bow = action_pose(&models, &e, &content, &rig);
    for (a, b) in [(2, 3), (3, 2)] {
        let right = right_bow[a].transform_vector3(Vec3::Y);
        let left = left_bow[b].transform_vector3(Vec3::Y);
        assert!((Vec3::new(-right.x, right.y, right.z) - left).length() < 1e-5);
    }
    e.equipment[0] = Some(action_stack(2, json!({
        "minecraft:consumable":{}, "minecraft:item_model":"minecraft:bow"
    })));
    action_age(&models, &mut e, &content, 2);
    let eating = action_pose(&models, &e, &content, &rig);
    assert!(eating[3].transform_vector3(Vec3::Y).y > 0.9, "third-person food uses ITEM, not an invented bow pose");
    assert_eq!(eating[2], Mat4::IDENTITY, "empty opposite hand is not raised");
    e.equipment[0] = Some(action_stack(2, json!({"minecraft:blocks_attacks":{}})));
    let shield = action_pose(&models, &e, &content, &rig);
    let across_chest = shield[3].transform_vector3(Vec3::Y);
    assert!(across_chest.z < -0.6 && across_chest.x.abs() > 0.2);
    e.metadata.insert(8, Metadata::Byte(0));
    action_age(&models, &mut e, &content, 3);
    let stopped = action_pose(&models, &e, &content, &rig);
    assert!(stopped[3].transform_vector3(Vec3::Y).y > 0.9, "release restores held-item rest");
}

#[test]
fn animation_offhand_use_and_attack_mirror_the_selected_main_arm() {
    let (models, mut e, content) = action_fixture("minecraft:player");
    e.equipment[1] = Some(action_stack(1, json!({"minecraft:blocks_attacks":{}})));
    e.metadata.insert(8, Metadata::Byte(3));
    let rig = rig();
    for (main, active_arm, resting_arm) in [(1, 3, 2), (0, 2, 3)] {
        e.metadata.insert(15, Metadata::Arm(main));
        let pose = action_pose(&models, &e, &content, &rig);
        assert!(pose[active_arm].transform_vector3(Vec3::Y).z < -0.6);
        assert_eq!(pose[resting_arm], Mat4::IDENTITY);
    }
    e.metadata.insert(8, Metadata::Byte(0));
    e.equipment[1] = None;
    e.animation = Some((0, 0));
    let mut attack = Vec::new();
    presentation::evaluate_rig(&rig, &e, &models, &content, 3, 0., Mat4::IDENTITY, &mut attack);
    assert!(attack[3].transform_vector3(Vec3::Y).z < -0.8);
    assert_eq!(attack[2], Mat4::IDENTITY);
}

#[test]
fn animation_crossbow_charge_hold_release_and_identity_reset() {
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    content.item_names.insert(1, "minecraft:crossbow".into());
    e.equipment[0] = Some(action_stack(1, json!({})));
    e.metadata.insert(8, Metadata::Byte(1));
    action_age(&models, &mut e, &content, 10);
    let rig = rig();
    let start = action_pose(&models, &e, &content, &rig);
    action_age(&models, &mut e, &content, 22);
    let pulling = action_pose(&models, &e, &content, &rig);
    assert_ne!(start[3], pulling[3], "support arm pulls back while charging");
    assert_eq!(start[2], pulling[2], "holding arm stays braced");
    e.observed_tick = 999;
    assert_eq!(pulling, action_pose(&models, &e, &content, &rig), "movement receipt does not restart use");
    e.metadata.insert(8, Metadata::Byte(0));
    e.equipment[0] = Some(action_stack(1, json!({"minecraft:charged_projectiles":[{"id":"minecraft:arrow"}]})));
    action_age(&models, &mut e, &content, 23);
    let hold = action_pose(&models, &e, &content, &rig);
    assert_ne!(pulling[2], hold[2]);
    assert!(hold[2].transform_vector3(Vec3::Y).z < -0.8);
    e.equipment[0] = Some(action_stack(1, json!({})));
    let unloaded = action_pose(&models, &e, &content, &rig);
    assert!(unloaded[2].transform_vector3(Vec3::Y).y > 0.9);
    e.metadata.insert(8, Metadata::Byte(1));
    action_age(&models, &mut e, &content, 24);
    assert_eq!(start, action_pose(&models, &e, &content, &rig));
    e.source = Some(source_state::SourceState::new(&e, &models).unwrap());
    assert_eq!(e.source.as_ref().unwrap().living.as_ref().unwrap().item_use_ticks(0.), 0.);
}

#[test]
fn animation_casting_is_distinct_from_crossbow_and_clears_on_received_spell_end() {
    let (models, mut e, mut content) = action_fixture("minecraft:evoker");
    content.item_names.insert(1, "minecraft:crossbow".into());
    e.equipment[0] = Some(action_stack(1, json!({})));
    e.metadata.insert(18, Metadata::Bool(true));
    action_age(&models, &mut e, &content, 10);
    let rig = rig();
    let charging = action_pose(&models, &e, &content, &rig);
    e.metadata.insert(18, Metadata::Bool(false));
    e.metadata.insert(17, Metadata::Byte(1));
    let casting = action_pose(&models, &e, &content, &rig);
    assert!(casting[2].transform_vector3(Vec3::Y).x < -0.6);
    assert!(casting[3].transform_vector3(Vec3::Y).x > 0.6);
    assert_ne!(charging[2], casting[2]);
    e.metadata.insert(17, Metadata::Byte(0));
    let stopped = action_pose(&models, &e, &content, &rig);
    assert!(stopped[2].transform_vector3(Vec3::Y).y > 0.9);
    assert_eq!(stopped[3], Mat4::IDENTITY);
}

#[test]
fn animation_roleless_named_parts_move_without_erasing_authored_orientation() {
    for (kind, part) in [
        ("cod", "tail"), ("salmon", "body_rear"), ("tropical_fish", "tail"),
        ("squid", "tentacle1"), ("glow_squid", "tentacle1"), ("ghast", "tentacle_0"),
        ("blaze", "stick0"), ("guardian", "tail"), ("elder_guardian", "tail"),
        ("dolphin", "tail"), ("silverfish", "segment0"), ("endermite", "head"),
        ("nautilus", "body"), ("zombie_nautilus", "body"),
    ] {
        let (models, mut e, content) = action_fixture(&format!("minecraft:{kind}"));
        e.velocity = [0.1, 0., 0.];
        e.source.as_mut().unwrap().living.as_mut().unwrap().in_water = true;
        let rig = solaris_models::authored(&format!("minecraft:{kind}#main")).unwrap();
        let bone = rig.part_by_name[part];
        e.source.as_mut().unwrap().age = 1;
        let first = action_pose(&models, &e, &content, rig);
        e.source.as_mut().unwrap().age = 9;
        let later = action_pose(&models, &e, &content, rig);
        assert_ne!(first[bone], later[bone], "{kind}:{part}");
        assert!(later[bone].to_cols_array().iter().all(|v| v.is_finite()));
        if kind.ends_with("squid") {
            // Squid movement bends around X only; the asset's UV yaw remains in the matrix.
            let rest_yaw = Mat4::from_rotation_y(rig.bones[bone].rotation.y).transform_vector3(Vec3::X);
            assert!((later[bone].transform_vector3(Vec3::X) - rest_yaw).length() < 1e-4);
        }
    }
}

#[test]
fn animation_modern_action_timers_change_parts_and_return_to_rest() {
    use render_living::Modern;
    for kind in ["frog", "warden", "rabbit", "breeze", "copper_golem", "creaking", "sniffer"] {
        let (models, mut e, content) = action_fixture(&format!("minecraft:{kind}"));
        let rig = solaris_models::authored(&format!("minecraft:{kind}#main")).unwrap();
        e.source.as_mut().unwrap().age = 5;
        let idle = action_pose(&models, &e, &content, rig);
        match &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            Modern::Frog { jump, .. } => jump.start(0),
            Modern::Warden { attack, .. } => attack.start(0),
            Modern::Rabbit { hop, .. } => hop.start(0),
            Modern::Breeze { shoot, .. } => shoot.start(0),
            Modern::CopperGolem { interactions, .. } => interactions[0].start(0),
            Modern::Creaking { attack, .. } => attack.start(0),
            Modern::Sniffer { dig, .. } => dig.start(0),
            _ => panic!("fixture modern kind"),
        }
        let action = action_pose(&models, &e, &content, rig);
        assert_ne!(idle, action, "{kind} timer must reach actual part matrices");
        if let Modern::CopperGolem { interactions, .. } = &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            for timer in interactions { timer.stop(); }
        }
        e.source.as_mut().unwrap().age = 200;
        assert_eq!(idle, action_pose(&models, &e, &content, rig), "{kind} action ends or leaves its looping state");
    }
}

#[test]
fn held_equipment_uses_anatomical_side_and_borrowed_dynamic_model_context() {
    use crate::ui::{item_dynamic::ItemWorldContext, items::{ItemContext, ItemModels}};
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    content.item_names.insert(1, "test:clock".into());
    let graph = json!({
        "type":"minecraft:range_dispatch", "property":"minecraft:time", "source":"daytime", "wobble":false,
        "fallback":{"type":"minecraft:model","model":"test:early"},
        "entries":[{"threshold":0.4,"model":{"type":"minecraft:model","model":"test:late"}}]
    });
    content.insert_test_resource("assets/test/items/clock.json", json!({"model":graph.clone()}).to_string().as_bytes());
    let mut items = ItemModels::new();
    for (name, layer) in [("test:early", 31), ("test:late", 32)] {
        let mut model = content.models[0].clone();
        for quad in &mut model.quads {
            quad.texture_layer = layer;
            for point in &mut quad.positions { for axis in point { *axis -= 0.5; } }
        }
        items.insert_test_model(name, model);
    }
    items.insert_test_graph("test:clock", graph);
    e.equipment[0] = Some(action_stack(1, json!({})));
    let mut rig = rig();
    rig.bones[2].pivot.x = -0.5;
    rig.bones[3].pivot.x = 0.5;
    let mut context = ItemContext {
        world: Some(ItemWorldContext {
            dimension: "minecraft:overworld".into(), position: [0.;3], yaw: 0.,
            game_tick: 0, sun_angle: 0., moon_phase: 0., spawn: None, death: None,
            needles: Arc::default(),
        }), ..Default::default()
    };
    let mut cache = render_special::Cache::default();
    let sample = std::cell::Cell::new(0.);
    let at = |_: [f64;3]| Ok((sample.get(), 0.));
    let mut equipment = equipment::Cache::default();
    let mut render = |e: &Entity, base: &ItemContext| {
        let state = cache.item_context(e, [e.equipment_value(0), None, None], &content,
            e.position, Some((base, &at))).unwrap().unwrap();
        let transforms = action_pose(&models, e, &content, &rig);
        let mut out = DynamicGeometry::default();
        let mut stats = presentation::PresentationStats::default();
        presentation::append_equipment(
            presentation::EquipmentPose { entity: e, rig: &rig, transforms: &transforms, item_context: Some(&state) },
            &mut items, &mut equipment,
            equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY,
                tick: 0, partial: 0., light: [15.;2] }, &mut out, &mut stats,
        ).unwrap();
        assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
        assert!(!out.vertices.is_empty());
        let center_x = out.vertices.iter().map(|v| v.position[0]).sum::<f32>() / out.vertices.len() as f32;
        (center_x, out.vertices[0].texture_layer)
    };
    let (right, early) = render(&e, &context);
    assert!(right < 0. && early == 31);
    e.metadata.insert(15, Metadata::Arm(0));
    sample.set(180.);
    context.world.as_mut().unwrap().game_tick = 1;
    let (left, late) = render(&e, &context);
    assert!(left > 0. && late == 32, "left-handed held clock must both move sides and advance model");
    assert!((left + right).abs() < 1e-4, "hand anchors mirror: {left} {right}");
    e.equipment[0] = None;
    let transforms = action_pose(&models, &e, &content, &rig);
    let mut removed = DynamicGeometry::default();
    presentation::append_equipment(
        presentation::EquipmentPose { entity: &e, rig: &rig, transforms: &transforms, item_context: None },
        &mut ItemModels::new(), &mut equipment::Cache::default(),
        equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY,
            tick: 0, partial: 0., light: [15.;2] }, &mut removed, &mut presentation::PresentationStats::default(),
    ).unwrap();
    assert!(removed.vertices.is_empty(), "equipment removal clears held geometry");
}

#[test]
fn animation_modern_baby_adult_and_identity_changes_do_not_reuse_action_timers() {
    use render_living::Modern;
    for baby in [false, true] {
        let (models, mut e, content) = action_fixture("minecraft:camel");
        e.metadata.insert(16, Metadata::Bool(baby));
        let rig = solaris_models::authored(if baby { "minecraft:camel_baby#main" } else { "minecraft:camel#main" }).unwrap();
        let standing = action_pose(&models, &e, &content, rig);
        if let Modern::Camel { sit_pose, .. } = &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            sit_pose.start(0);
        }
        let sitting = action_pose(&models, &e, &content, rig);
        for name in ["body", "right_hind_leg", "left_hind_leg"] {
            assert_ne!(standing[rig.part_by_name[name]], sitting[rig.part_by_name[name]], "{baby}:{name}");
        }
        e.uuid = "00000000-0000-0000-0000-000000000099".into();
        e.source = Some(source_state::SourceState::new(&e, &models).unwrap());
        assert_eq!(standing, action_pose(&models, &e, &content, rig));
    }
}

#[test]
fn animation_crossbow_enchantment_update_preserves_elapsed_use_and_reuses_shared_effects() {
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    content.item_names.insert(1, "minecraft:crossbow".into());
    content.set_registries(BTreeMap::from([("minecraft:enchantment".into(), vec![
        ("test:quick".into(), json!({"effects":{"minecraft:crossbow_charge_time":{
            "type":"minecraft:add", "value":-0.75
        }}})),
        ("test:random".into(), json!({"effects":{"minecraft:crossbow_charge_time":{
            "type":"minecraft:remove_binomial", "chance":0.5
        }}})),
    ])]));
    e.equipment[0] = Some(action_stack(1, json!({})));
    e.metadata.insert(8, Metadata::Byte(1));
    action_age(&models, &mut e, &content, 10);
    action_age(&models, &mut e, &content, 22);
    assert!((e.source.as_ref().unwrap().living.as_ref().unwrap().crossbow_pull(0.) - 0.48).abs() < 1e-5);
    e.equipment[0] = Some(action_stack(1, json!({"minecraft:enchantments":{"test:quick":1}})));
    action_age(&models, &mut e, &content, 22);
    let living = e.source.as_ref().unwrap().living.as_ref().unwrap();
    assert_eq!(living.item_use_ticks(0.), 12., "component refresh is not a new use action");
    assert_eq!(living.crossbow_pull(0.), 1., "updated fast charge is already fully drawn");
    let random = action_stack(1, json!({"minecraft:enchantments":{"test:random":1}}));
    let mut draws = 0;
    let fast = crate::player::crossbow_charge_ticks(&random, &content, &mut |sample| {
        assert!(matches!(sample, crate::player::EffectSample::Uniform));
        draws += 1;
        0.
    }).unwrap();
    assert_eq!(fast, 1.);
    assert_eq!(draws, 2, "same binomial draws as the gameplay effect owner");
    let slow = crate::player::crossbow_charge_ticks(&random, &content, &mut |_| 0.75).unwrap();
    assert_eq!(slow, 25.);
}

#[test]
fn received_aquatic_shape_state_selects_part_identity_and_matching_texture() {
    let (mut models, mut e, mut content) = action_fixture("minecraft:tropical_fish");
    models.schemas.get_mut(&e.type_name).unwrap().accessors.insert("DATA_ID_TYPE_VARIANT".into(), [23, 1]);
    models.families.insert(e.type_name.clone(), resources::Family {
        rig: "minecraft:tropical_fish#main".into(), baby_rig: None,
        texture: Some("test:large".into()), textures: vec![], renderer_class: None,
        materials: BTreeMap::from([
            ("SMALL_TEXTURE".into(), json!("test:small")),
            ("LARGE_TEXTURE".into(), json!("test:large")),
        ]),
    });
    for (texture, layer) in [("test:small", 1), ("test:large", 2)] {
        content.insert_test_texture(texture, crate::protocol::content::TextureRegion {
            layer, uv:[0.,0.,1.,1.], width:32, height:32,
        });
    }
    let store = Store::new();
    for (packed, large) in [(0, false), (256, false), (1, true), (1281, true), (512, false)] {
        e.metadata.insert(23, Metadata::Int(packed));
        let binding = presentation::material(&store, &models, &e, &content, 0).unwrap();
        assert_eq!(binding.rig, if large { "minecraft:tropical_fish_large#main" } else { "minecraft:tropical_fish#main" });
        assert_eq!(binding.texture.layer, if large { 2 } else { 1 });
    }
    let schema = models.schemas.remove(&e.type_name).unwrap();
    e.type_name = "minecraft:pufferfish".into();
    models.schemas.insert(e.type_name.clone(), schema);
    models.schemas.get_mut(&e.type_name).unwrap().accessors.insert("PUFF_STATE".into(), [24, 1]);
    models.families.insert(e.type_name.clone(), resources::Family {
        rig: "minecraft:pufferfish#main".into(), baby_rig: None, texture:Some("test:small".into()),
        textures:vec![], renderer_class:None, materials:BTreeMap::new(),
    });
    for (puff, expected) in [(0, "pufferfish_small"), (1, "pufferfish_medium"), (2, "pufferfish"), (0, "pufferfish_small")] {
        e.metadata.insert(24, Metadata::Int(puff));
        assert_eq!(presentation::material(&store, &models, &e, &content, 0).unwrap().rig,
            format!("minecraft:{expected}#main"));
    }
}

#[test]
fn modern_visibility_reaches_submitted_geometry_for_tongue_croak_and_shell() {
    use render_living::Modern;
    for kind in ["frog", "armadillo"] {
        let (mut models, mut e, content) = action_fixture(&format!("minecraft:{kind}"));
        let parts: &[&str] = if kind == "frog" { &["body","tongue","croaking_body"] } else { &["body","head","cube"] };
        let model = json!({
            "schema":1, "id":"test_pose", "texture":{"path":"test.png","size":[16,16]},
            "bones":parts.iter().map(|name| json!({
                "name":name,"cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]
            })).collect::<Vec<_>>()
        });
        let rig = solaris_models::ModelFile::parse(&model.to_string()).unwrap().bake().unwrap();
        models.rigs.insert("test:pose#main".into(), rig);
        e.binding = Some(Ok(presentation::Binding {
            rig:"test:pose#main".into(),
            texture:crate::protocol::content::TextureRegion { layer:9,uv:[0.,0.,1.,1.],width:16,height:16 },
            style:presentation::RootStyle::Living, layers:None, anger:None,
        }));
        let models = Arc::new(models);
        let mut store = Store::new();
        store.models = Some(models);
        store.ids.insert(41,0);
        store.spatial.entry([0,0]).or_default().insert(41);
        store.slots.push(e);
        let render = |store: &mut Store| {
            let mut out = DynamicGeometry::default();
            let stats = store.append_geometry(crate::entities::EntityRenderFrame {
                maps:crate::ui::MapRenderView::empty(), tint_source:&|_,_|Ok([1.;3]),
                content:&content,origin:[0.;3],camera:[0.;3],camera_rotation:[0.;2],
                radius:32.,tick:0,partial:0.,game_time:0,clock_environment:None,
                light:&|_|Ok(Some([15.;2])),bed_direction:&|_|None,
            }, &mut out).unwrap();
            assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
            assert!(out.vertices.iter().all(|v| v.position.iter().all(|n| n.is_finite())));
            out.vertices.len()
        };
        let idle = render(&mut store);
        let e = &mut store.slots[0];
        e.source.as_mut().unwrap().age = 5;
        match &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            Modern::Frog { tongue, .. } => tongue.start(0),
            Modern::Armadillo { .. } => { e.metadata.insert(19,Metadata::Int(2)); }
            _ => unreachable!(),
        }
        let active = render(&mut store);
        assert_eq!((idle, active), if kind == "frog" { (24,48) } else { (48,24) });
        let e = &mut store.slots[0];
        if let Modern::Frog { tongue, croak, .. } = &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            tongue.stop(); croak.start(0);
            assert_eq!(render(&mut store), 48, "croaking throat geometry is conditional");
        } else { e.metadata.insert(19,Metadata::Int(0)); }
        store.slots[0].source.as_mut().unwrap().age = 100;
        assert_eq!(render(&mut store), idle, "ending the action restores exact visible part set");
    }
}

#[test]
fn animation_aimed_and_single_hand_use_actions_have_distinct_silhouettes() {
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    let rig = rig();
    e.metadata.insert(8, Metadata::Byte(1));
    let mut directions = Vec::new();
    for (id, item, components) in [
        (1, "minecraft:trident", json!({})),
        (2, "minecraft:spyglass", json!({})),
        (3, "minecraft:goat_horn", json!({})),
        (4, "minecraft:brush", json!({})),
        (5, "test:spear", json!({"minecraft:kinetic_weapon":{"delay_ticks":10}})),
    ] {
        content.item_names.insert(id, item.into());
        e.equipment[0] = Some(action_stack(id, components));
        action_age(&models, &mut e, &content, id as i32 * 20);
        action_age(&models, &mut e, &content, id as i32 * 20 + 10);
        let pose = action_pose(&models, &e, &content, &rig);
        directions.push(pose[2].transform_vector3(Vec3::Y));
        assert_eq!(pose[3], Mat4::IDENTITY, "{item} does not commandeer the empty opposite arm");
    }
    assert!(directions[0].y < -0.95, "trident is raised overhead");
    assert!(directions[1].y < 0. && directions[1].z < -0.8, "spyglass reaches the eye");
    assert!(directions[2].y > 0. && directions[2].z < -0.8, "horn is held at the mouth");
    assert!(directions[3].y > 0.7 && directions[3].z < -0.5, "brush extends down and forward");
    assert!(directions[4].y > 0.1 && directions[4].z < -0.9, "spear is braced forward, not thrown overhead");
    for (i, a) in directions.iter().enumerate() {
        for b in &directions[i+1..] { assert!((*a-*b).length() > 0.1); }
    }
}

#[test]
fn animation_drowned_trident_pose_takes_precedence_on_the_actual_main_arm() {
    let (mut models, mut e, mut content) = action_fixture("minecraft:drowned");
    let schema = models.schemas.get_mut(&e.type_name).unwrap();
    schema.accessors.remove("DATA_PLAYER_MAIN_HAND");
    schema.accessors.insert("DATA_MOB_FLAGS_ID".into(), [22, 0]);
    content.item_names.insert(1, "minecraft:trident".into());
    e.equipment[0] = Some(action_stack(1, json!({})));
    let rig = rig();
    for (flags, holding, other) in [(4, 2, 3), (6, 3, 2)] {
        e.metadata.insert(22, Metadata::Byte(flags));
        let pose = action_pose(&models, &e, &content, &rig);
        assert!(pose[holding].transform_vector3(Vec3::Y).z > 0.6, "trident arm is cocked behind the head");
        assert!(pose[other].transform_vector3(Vec3::Y).z < -0.6, "other arm retains aggressive zombie pose");
    }
}

#[test]
fn animation_bat_rest_and_axolotl_play_dead_are_reversible_for_babies_and_adults() {
    use render_living::Modern;
    let (models, mut bat, content) = action_fixture("minecraft:bat");
    let bat_rig = solaris_models::authored("minecraft:bat#main").unwrap();
    let flight = action_pose(&models, &bat, &content, bat_rig);
    if let Modern::Bat { rest, .. } = &mut bat.source.as_mut().unwrap().living.as_mut().unwrap().modern {
        rest.start(0);
    }
    let hanging = action_pose(&models, &bat, &content, bat_rig);
    assert_ne!(flight[0], hanging[0], "hanging reverses the body's orientation");
    for wing in ["left_wing", "right_wing"] {
        assert_ne!(flight[bat_rig.part_by_name[wing]], hanging[bat_rig.part_by_name[wing]], "resting wings fold");
    }
    if let Modern::Bat { rest, .. } = &mut bat.source.as_mut().unwrap().living.as_mut().unwrap().modern {
        rest.stop();
    }
    assert_eq!(flight, action_pose(&models, &bat, &content, bat_rig));
    for baby in [false, true] {
        let (models, mut e, content) = action_fixture("minecraft:axolotl");
        e.metadata.insert(16, Metadata::Bool(baby));
        let rig = solaris_models::authored(if baby { "minecraft:axolotl_baby#main" } else { "minecraft:axolotl#main" }).unwrap();
        let idle = action_pose(&models, &e, &content, rig);
        if let Modern::Axolotl(state) = &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            for age in 0..10 { state.tick(age, baby, true, true, false, false); }
        }
        let dead = action_pose(&models, &e, &content, rig);
        assert_ne!(dead[0], idle[0], "{baby}: playing dead rolls the body");
        if let Modern::Axolotl(state) = &mut e.source.as_mut().unwrap().living.as_mut().unwrap().modern {
            for age in 10..21 { state.tick(age, baby, false, true, false, false); }
        }
        let alive = action_pose(&models, &e, &content, rig);
        for index in [0, rig.part_by_name["body"]] {
            assert!(alive[index].to_cols_array().iter().zip(idle[index].to_cols_array())
                .all(|(a,b)| (*a-b).abs() < 1e-5), "{baby}: recovery restores posture");
        }
    }
}

#[test]
fn animation_wither_invulnerability_scale_reaches_visible_geometry() {
    let (mut models, mut e, content) = action_fixture("minecraft:wither");
    models.schemas.get_mut(&e.type_name).unwrap().accessors.insert("DATA_ID_INV".into(), [22, 1]);
    e.binding = Some(Ok(presentation::Binding {
        rig:"minecraft:wither#main".into(), style:presentation::RootStyle::Living,
        texture:crate::protocol::content::TextureRegion { layer:9,uv:[0.,0.,1.,1.],width:64,height:64 },
        layers:None,anger:None,
    }));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.ids.insert(41,0);
    store.spatial.entry([0,0]).or_default().insert(41);
    store.slots.push(e);
    let render_width = |store: &mut Store| {
        let mut out = DynamicGeometry::default();
        let stats = store.append_geometry(crate::entities::EntityRenderFrame {
            maps:crate::ui::MapRenderView::empty(),tint_source:&|_,_|Ok([1.;3]),
            content:&content,origin:[0.;3],camera:[0.;3],camera_rotation:[0.;2],
            radius:32.,tick:0,partial:0.,game_time:0,clock_environment:None,
            light:&|_|Ok(Some([15.;2])),bed_direction:&|_|None,
        }, &mut out).unwrap();
        assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
        let (low, high) = out.vertices.iter().fold((f32::INFINITY,f32::NEG_INFINITY),
            |(low,high),v| (low.min(v.position[0]),high.max(v.position[0])));
        high-low
    };
    let adult = render_width(&mut store);
    assert!(adult > 1., "the visible wither body must actually be drawn");
    for (invulnerability, ratio) in [(220,0.75),(110,0.875),(0,1.)] {
        store.slots[0].metadata.insert(22,Metadata::Int(invulnerability));
        let width = render_width(&mut store);
        assert!((width/adult-ratio).abs() < 1e-4, "{invulnerability}: {width}/{adult}");
    }
}

#[test]
fn animation_kinetic_weapon_uses_the_component_zero_delay_default() {
    let (models, mut e, mut content) = action_fixture("minecraft:player");
    content.item_names.insert(1, "test:spear".into());
    e.metadata.insert(8, Metadata::Byte(1));
    e.equipment[0] = Some(action_stack(1, json!({"minecraft:kinetic_weapon":{}})));
    action_age(&models, &mut e, &content, 1);
    action_age(&models, &mut e, &content, 2);
    let rig = rig();
    let immediate = action_pose(&models, &e, &content, &rig)[2].transform_vector3(Vec3::Y);
    assert!(immediate.z < -0.9, "omitted delay is zero, not an invented ten-tick windup");
    e.equipment[0] = Some(action_stack(1, json!({"minecraft:kinetic_weapon":{"delay_ticks":10}})));
    action_age(&models, &mut e, &content, 2);
    let delayed = action_pose(&models, &e, &content, &rig)[2].transform_vector3(Vec3::Y);
    assert!(delayed.z > -0.8, "an explicit delay retains its authored windup");
    assert!(immediate.is_finite() && delayed.is_finite());
}
