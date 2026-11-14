//! Crowd composites against the CPU entity path, on the authored zombie and armour rigs.
use super::*;
use crate::{
    api::{DynamicGeometry, Vertex},
    entities::{
        animation_tests, clip_pose::{BakedClip, ChannelSample, ClipLayer, Playback}, presentation::PresentationStats,
        render_living, resources, EntityRenderFrame, Store,
    },
};
use serde_json::json;
use std::collections::BTreeMap;

const ZOMBIE: &str = "minecraft:zombie";
const TICK: u64 = 40;
const PARTIAL: f32 = 0.25;

fn walk(models: &Models, entity: &mut Entity, x: f64) {
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

fn stack(id: u32, components: serde_json::Value) -> Stack {
    let mut stack = Stack::default();
    stack.id = id;
    stack.count = 1;
    stack.components = std::sync::Arc::new(
        components.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    );
    stack
}

/// A walking, attacking, recently hurt zombie in full cloth armour holding a cube item and,
/// in its off hand, a shield (a special item model drawn entity solid).
fn fixture() -> (Store, Content) {
    let (mut models, mut entity) = animation_tests::fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.accessors.insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
    let motion = models.motion.remove("minecraft:pig").unwrap();
    entity.type_name = ZOMBIE.into();
    models.schemas.insert(ZOMBIE.into(), schema);
    models.motion.insert(ZOMBIE.into(), motion);
    models.families.insert(ZOMBIE.into(), resources::Family {
        rig: "minecraft:zombie#main".into(),
        baby_rig: None,
        texture: Some("minecraft:entity/zombie/zombie".into()),
        textures: vec![],
        renderer_class: None,
        materials: BTreeMap::new(),
    });
    let parts: BTreeMap<String, String> = ["head", "chest", "legs", "feet"]
        .into_iter()
        .map(|part| (part.to_string(), format!("minecraft:armor_{part}#main")))
        .collect();
    for key in parts.values() {
        assert!(models.has_authored_rig(key), "{key} is an authored rig");
    }
    models.armor_models.insert(ZOMBIE.into(), resources::ArmorModels { adult: parts.clone(), baby: parts });
    entity.source = Some(super::super::source_state::SourceState::new(&entity, &models).unwrap());
    for step in 1..=6 {
        walk(&models, &mut entity, f64::from(step) * 0.15);
    }
    entity.transform.head_yaw = 25.;
    entity.transform.pitch = -10.;
    entity.animation = Some((0, TICK - 2));
    entity.damage_tick = Some(TICK - 3);
    for (slot, id, part) in [(5, 1, "head"), (4, 2, "chest"), (3, 3, "legs"), (2, 4, "feet")] {
        entity.equipment[slot] = Some(stack(id, json!({
            "minecraft:equippable": {"slot": part, "asset_id": "test:cloth"},
            "minecraft:dyed_color": 0x30_60_90,
        })));
    }
    entity.equipment[0] = Some(stack(5, json!({})));
    entity.equipment[1] = Some(stack(6, json!({})));
    let mut content = Content::test_cube();
    let region = |layer| crate::protocol::content::TextureRegion { uv: [0., 0., 1., 1.], layer, width: 64, height: 64 };
    content.insert_test_texture("minecraft:entity/zombie/zombie", region(0));
    content.insert_test_resource(
        "assets/test/equipment/cloth.json",
        br#"{"layers":{"humanoid":[{"texture":"test:cloth","dyeable":{"color_when_undyed":8405024}}],
            "humanoid_leggings":[{"texture":"test:cloth","dyeable":{"color_when_undyed":8405024}}]}}"#,
    );
    for kind in ["humanoid", "humanoid_leggings"] {
        content.insert_test_texture(&format!("test:entity/equipment/{kind}/cloth"), region(0));
    }
    content.insert_test_texture("minecraft:misc/enchanted_glint_armor", region(0));
    content.item_names.insert(5, "test:held".into());
    content.item_names.insert(6, "test:shield".into());
    content.insert_test_texture("minecraft:entity/shield/shield_base_nopattern", region(0));
    content.insert_test_resource(
        "assets/test/models/item/shield.json",
        br#"{"display":{"thirdperson_righthand":{"rotation":[0,90,0],"translation":[10,6,-4]},
            "thirdperson_lefthand":{"rotation":[0,90,0],"translation":[10,6,12]}}}"#,
    );
    let mut store = Store::new();
    store.items.insert_test_model("test:held", content.models[0].clone());
    store.items.insert_test_graph("test:shield", json!({
        "type": "minecraft:special", "base": "test:item/shield", "model": {"type": "minecraft:shield"},
    }));
    let models = std::sync::Arc::new(models);
    store.items.set_entity_models(std::sync::Arc::clone(&models));
    store.models = Some(models);
    store.ids.insert(41, 0);
    store.spatial.entry([0, 0]).or_default().insert(41);
    store.slots.push(entity);
    (store, content)
}

fn render(store: &mut Store, content: &Content, crowd: bool) -> (DynamicGeometry, PresentationStats) {
    store.set_crowd_instancing(crowd);
    let mut out = DynamicGeometry::default();
    let stats = store
        .append_geometry(EntityRenderFrame {
            maps: crate::ui::MapRenderFixture::empty().view(),
            tint_source: &|_, _| Ok([1.; 3]),
            content,
            origin: [0.; 3],
            camera: [0., 1., -4.],
            camera_rotation: [0.; 2],
            radius: 32.,
            tick: TICK,
            partial: PARTIAL,
            game_time: 0,
            clock_environment: None,
            light: &|_| Ok(Some([12., 3.])),
            bed_direction: &|_| None,
        }, &mut out)
        .unwrap();
    assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
    (out, stats)
}

/// The fixture zombie drawn by the CPU path, then as a crowd instance (GPU parity tests).
pub(crate) fn parity_scenes() -> (DynamicGeometry, CrowdScene) {
    let (mut store, content) = fixture();
    let (cpu, _) = render(&mut store, &content, false);
    let (_, stats) = render(&mut store, &content, true);
    assert_eq!(stats.crowd, 1);
    let mut crowd = CrowdScene::default();
    store.take_crowd(&mut crowd);
    (cpu, crowd)
}

/// A `side` x `side` grid of the fixture zombie centred on x = 0 and receding along +z,
/// alternating two composites (with and without boots): CPU geometry translated per
/// copy, and the matching crowd instances.
pub(crate) fn parity_grid(side: usize, spacing: f32) -> (DynamicGeometry, CrowdScene) {
    let (mut store, content) = fixture();
    let (armoured_cpu, _) = render(&mut store, &content, false);
    render(&mut store, &content, true);
    let armoured = store.crowd.instances()[0];
    store.slots[0].equipment[2] = None;
    store.slots[0].appearance_revision += 1;
    let (bare_cpu, _) = render(&mut store, &content, false);
    render(&mut store, &content, true);
    let bare = store.crowd.instances()[0];
    assert_eq!((armoured.words[0], bare.words[0]), (0, 1));
    let mut scene = CrowdScene::default();
    store.take_crowd(&mut scene);
    scene.instances.clear();
    scene.counts = vec![0; scene.meshes.groups.len()];
    scene.bones = 0;
    let mut cpu = DynamicGeometry::default();
    for index in 0..side * side {
        let column = (index % side) as f32 - (side - 1) as f32 / 2.;
        let (x, z) = (column * spacing, (index / side) as f32 * spacing);
        let (mut instance, geometry) = if index % 2 == 0 { (armoured, &armoured_cpu) } else { (bare, &bare_cpu) };
        instance.root[0][3] += x;
        instance.root[2][3] += z;
        instance.words[1] = scene.bones;
        scene.bones += scene.meshes.groups[instance.words[0] as usize].bone_count;
        scene.counts[instance.words[0] as usize] += 1;
        scene.instances.push(instance);
        let base = cpu.vertices.len() as u32;
        cpu.vertices.extend(geometry.vertices.iter().map(|v| Vertex {
            position: [v.position[0] + x, v.position[1], v.position[2] + z],
            ..*v
        }));
        for (material, indices) in geometry.indices.iter().enumerate() {
            cpu.indices[material].extend(indices.iter().map(|index| index + base));
        }
    }
    (cpu, scene)
}

/// CPU mirror of `crowd_vertex`: every LOD-0 quad of the instance per crowd material.
fn crowd_quads(meshes: &CrowdMeshes, instance: &CrowdInstance, clips: &ClipLibrary) -> [Vec<[Vertex; 4]>; MATERIALS.len()] {
    let bones = evaluate(meshes, instance, clips);
    let group = &meshes.groups[instance.words[0] as usize];
    std::array::from_fn(|material| {
        let first = group.first_index[material] as usize;
        let indices = &meshes.indices[first..first + group.counts[material][0] as usize];
        indices
            .chunks(6)
            .map(|quad| {
                [quad[0], quad[1], quad[2], quad[4]].map(|index| {
                    let v = meshes.vertices[(group.base_vertex + index as i32) as usize];
                    let m = bones[(v.bone_flags & 0xffff) as usize];
                    let (c0, c1, c2) = (m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate());
                    let cofactor = glam::Mat3::from_cols(c1.cross(c2), c2.cross(c0), c0.cross(c1));
                    let mut normal = cofactor * Vec3::from_array(v.normal);
                    if v.bone_flags & VERTEX_ITEM_NORMAL == 0 {
                        normal *= c0.dot(c1.cross(c2)).signum();
                    }
                    let mut light = [instance.light[0], instance.light[1]];
                    let emission = (v.bone_flags >> VERTEX_EMISSION_SHIFT) as f32;
                    if emission > 0. && light != [-1.; 2] {
                        light = light.map(|level| level.floor().max(emission));
                    }
                    let hurt = instance.light[2] > 0.5 && v.bone_flags & VERTEX_BODY != 0;
                    Vertex {
                        position: m.transform_point3(Vec3::from_array(v.position)).to_array(),
                        color: [v.color[0], v.color[1], v.color[2]],
                        uv: v.uv,
                        overlay_uv: if hurt { [0., 3.] } else { [0., 10.] },
                        overlay_tint: [1.; 3],
                        alpha: v.color[3],
                        texture_layer: v.layer,
                        light,
                        normal: normal.normalize_or_zero().to_array(),
                    }
                })
            })
            .collect()
    })
}

fn cpu_quads(geometry: &DynamicGeometry, material: EntityMaterial) -> Vec<[Vertex; 4]> {
    geometry.indices[material as usize]
        .chunks(6)
        .map(|quad| [quad[0], quad[1], quad[2], quad[4]].map(|index| geometry.vertices[index as usize]))
        .collect()
}

/// Order-independent comparison: both lists sorted by quad centroid and texture.
fn assert_same_quads(label: &str, mut cpu: Vec<[Vertex; 4]>, mut crowd: Vec<[Vertex; 4]>) {
    assert_eq!(cpu.len(), crowd.len(), "{label}: quad count");
    let key = |quad: &[Vertex; 4]| {
        let centroid = quad.iter().fold(Vec3::ZERO, |sum, v| sum + Vec3::from_array(v.position)) / 4.;
        let uv = quad.iter().fold([0.; 2], |sum, v| [sum[0] + v.uv[0], sum[1] + v.uv[1]]);
        [centroid.x, centroid.y, centroid.z, uv[0], uv[1]].map(|value| (value * 1000.).round() as i64)
    };
    cpu.sort_by_key(key);
    crowd.sort_by_key(key);
    let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 2e-4);
    for (index, (a, b)) in cpu.iter().zip(&crowd).enumerate() {
        for (va, vb) in a.iter().zip(b) {
            assert!(close(&va.position, &vb.position), "{label} quad {index} position {:?} != {:?}", va.position, vb.position);
            assert!(close(&va.normal, &vb.normal), "{label} quad {index} normal {:?} != {:?}", va.normal, vb.normal);
            assert!(close(&va.uv, &vb.uv), "{label} quad {index} uv");
            assert!(close(&va.color, &vb.color) && (va.alpha - vb.alpha).abs() < 1e-6, "{label} quad {index} colour");
            assert!(close(&va.light, &vb.light), "{label} quad {index} light");
            assert_eq!(va.overlay_uv, vb.overlay_uv, "{label} quad {index} overlay");
            assert_eq!(va.texture_layer, vb.texture_layer, "{label} quad {index} layer");
        }
    }
}

#[test]
fn crowd_instance_reproduces_cpu_entity_vertices() {
    let (mut store, content) = fixture();
    let (cpu, cpu_stats) = render(&mut store, &content, false);
    assert_eq!((cpu_stats.submitted, cpu_stats.crowd), (1, 0));
    let (crowd, crowd_stats) = render(&mut store, &content, true);
    assert_eq!((crowd_stats.submitted, crowd_stats.crowd), (0, 1), "the zombie moves to the crowd path");
    assert!(crowd.vertices.is_empty(), "no CPU vertices for a crowd entity");
    let registry = &store.crowd;
    assert_eq!(registry.instances().len(), 1);
    let quads = crowd_quads(registry.meshes(), &registry.instances()[0], &clip_pose::library());
    for (material, crowd) in MATERIALS.iter().zip(quads) {
        let cpu = cpu_quads(&cpu, *material);
        if *material != EntityMaterial::ArmorDecalCutoutNoCull {
            assert!(!cpu.is_empty(), "{material:?} is exercised");
        }
        assert_same_quads(&format!("{material:?}"), cpu, crowd);
    }
    // Everything the CPU path drew is accounted for by the crowd materials.
    let drawn: usize = MATERIALS.iter().map(|m| cpu.indices[*m as usize].len()).sum();
    assert_eq!(drawn, cpu.indices.iter().map(Vec::len).sum::<usize>());
}

#[test]
fn composites_are_shared_and_follow_appearance_changes() {
    let (mut store, content) = fixture();
    render(&mut store, &content, true);
    let revision = store.crowd.meshes().revision;
    render(&mut store, &content, true);
    assert_eq!(store.crowd.meshes().revision, revision, "an unchanged appearance reuses its composite");
    assert_eq!(store.crowd.meshes().groups.len(), 1);
    let boots = store.slots[0].equipment[2].take().unwrap();
    store.slots[0].appearance_revision += 1;
    render(&mut store, &content, true);
    assert_eq!(store.crowd.meshes().groups.len(), 2, "removed boots make a new composite");
    assert_eq!(store.crowd.instances()[0].words[0], 1);
    // Equal boots in a new allocation resolve to the first composite again.
    store.slots[0].equipment[2] = Some(stack(boots.id, serde_json::Value::Object(
        boots.components.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    )));
    store.slots[0].appearance_revision += 1;
    render(&mut store, &content, true);
    assert_eq!(store.crowd.meshes().groups.len(), 2);
    assert_eq!(store.crowd.instances()[0].words[0], 0);
}

#[test]
fn clip_layers_drive_bones_carry_and_grip_like_the_cpu_pose() {
    let (mut store, content) = fixture();
    render(&mut store, &content, true);
    let sample = |rotation: [f32; 3], weight: f32, offset: [f32; 3]| ChannelSample { rotation, weight, offset, _pad: 0. };
    let frame = |t: f32| {
        let mut frame = [ChannelSample::default(); clip_pose::CHANNELS];
        frame[Channel::Body as usize] = sample([0.3 * t, 0.1, 0.], 0.4, [0., 0.05 * t, 0.]);
        frame[Channel::RightArm as usize] = sample([-1.2 * t, 0.2, 0.1], 1., [0., 0., 0.02]);
        frame[Channel::LeftLeg as usize] = sample([0.5 * t, 0., 0.], 0.7, [0.; 3]);
        frame[Channel::Head as usize] = sample([0., 0.4 * t, 0.], 0.2, [0.; 3]);
        frame[Channel::RightItem as usize] = sample([0.6 * t, 0., 0.2], 0., [0., 0.04, 0.]);
        frame[Channel::LeftItem as usize] = sample([0., 0.3, 0.], 0., [0.; 3]);
        frame
    };
    let library = ClipLibrary::new(vec![BakedClip {
        name: "swing".into(),
        playback: Playback::Loop,
        frames: vec![frame(0.), frame(1.), frame(0.5)],
    }])
    .unwrap();
    let entity = &mut store.slots[0];
    entity.clip.layers = [
        ClipLayer { clip: 1, start_tick: TICK - 1, rate: 1., weight: 0.8, mirror: false },
        ClipLayer { clip: 1, start_tick: TICK - 2, rate: 0.5, weight: 0.5, mirror: true },
    ];
    let entity = &store.slots[0];
    let models = store.models.clone().unwrap();
    let Some(Ok(bound)) = entity.binding.as_ref() else { panic!("bound") };
    let frame = p::EntityDrawFrame {
        origin: [0.; 3],
        transform: entity.presented_transform(TICK, PARTIAL),
        tick: TICK,
        partial: PARTIAL,
        light: [12., 3.],
        parent: None,
    };
    let instance = instance(entity, &models, &content, bound, frame, &|_| None).unwrap();
    assert_ne!(instance.words[3], 0, "both layers are active");
    let gpu = evaluate(store.crowd.meshes(), &instance, &library);
    let root = p::root_matrix(entity, &models, bound, frame, &|_| None);
    let pose = p::role_pose(entity, &models, &content, TICK, PARTIAL);
    let main = models.rig(&bound.rig);
    let mut expected = Vec::new();
    let mut transforms = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for key in ["", "chest", "legs", "feet", "head"] {
        let rig = if key.is_empty() { main } else { models.rig(models.armor_rig(ZOMBIE, false, key).unwrap()) };
        if seen.contains(&(std::ptr::from_ref(rig) as usize)) {
            continue;
        }
        seen.push(std::ptr::from_ref(rig) as usize);
        p::evaluate_posed_rig(rig, entity, &models, (TICK, PARTIAL), &pose, &library, root, &mut transforms);
        expected.extend(transforms.iter().copied());
    }
    // The held item hangs from the main rig's right arm through the item grip, the shield
    // from the left arm through the left grip.
    let arm = main.bones.iter().position(|bone| bone.role == BoneRole::RightArm).unwrap();
    let left = main.bones.iter().position(|bone| bone.role == BoneRole::LeftArm).unwrap();
    let layers = entity.clip.active(TICK, PARTIAL);
    let grip = library.grip(&layers, 0).expect("the right item channel is sampled");
    expected.push(expected[arm] * grip);
    let left_grip = library.grip(&layers, 1).expect("the left item channel is sampled");
    expected.push(expected[left] * left_grip);
    assert_eq!(gpu.len(), expected.len());
    for (index, (a, b)) in gpu.iter().zip(&expected).enumerate() {
        assert!(a.abs_diff_eq(*b, 1e-4), "bone {index}: {a:?} != {b:?}");
    }
    // Without the clip the right arm keeps its vanilla pose.
    let plain = evaluate(store.crowd.meshes(), &CrowdInstance { words: [instance.words[0], 0, instance.words[2], 0], ..instance }, &library);
    assert!(!plain[arm].abs_diff_eq(gpu[arm], 1e-3));
}

/// Cavalry: a seated passenger's legs take the riding pitch on both paths.
#[test]
fn riding_passengers_pose_their_legs_like_the_cpu_pose() {
    let (mut store, content) = fixture();
    render(&mut store, &content, true);
    store.slots[0].vehicle = Some(crate::entities::Handle { network_id: 7, generation: 1 });
    let entity = &store.slots[0];
    let models = store.models.clone().unwrap();
    let Some(Ok(bound)) = entity.binding.as_ref() else { panic!("bound") };
    let frame = p::EntityDrawFrame {
        origin: [0.; 3],
        transform: entity.presented_transform(TICK, PARTIAL),
        tick: TICK,
        partial: PARTIAL,
        light: [12., 3.],
        parent: None,
    };
    let instance = instance(entity, &models, &content, bound, frame, &|_| None).unwrap();
    assert_ne!(instance.words[2] & INSTANCE_RIDING, 0);
    let library = clip_pose::library();
    let gpu = evaluate(store.crowd.meshes(), &instance, &library);
    let root = p::root_matrix(entity, &models, bound, frame, &|_| None);
    let pose = p::role_pose(entity, &models, &content, TICK, PARTIAL);
    assert!(pose.riding);
    let main = models.rig(&bound.rig);
    let mut transforms = Vec::new();
    p::evaluate_posed_rig(main, entity, &models, (TICK, PARTIAL), &pose, &library, root, &mut transforms);
    let leg = main.bones.iter().position(|bone| bone.role == BoneRole::RightLeg).unwrap();
    for (index, expected) in transforms.iter().enumerate() {
        assert!(gpu[index].abs_diff_eq(*expected, 1e-4), "bone {index}: {:?} != {expected:?}", gpu[index]);
    }
    let standing = evaluate(store.crowd.meshes(), &CrowdInstance { words: [instance.words[0], 0, instance.words[2] & !INSTANCE_RIDING, 0], ..instance }, &library);
    assert!(!standing[leg].abs_diff_eq(gpu[leg], 1e-3), "the riding flag moves the legs");
}

#[test]
fn detail_levels_drop_thin_item_quads_then_trims() {
    let template = Content::test_cube().models[0].quads[0].clone();
    let quad = |positions: [[f32; 3]; 4]| crate::protocol::models::BakedQuad { positions, ..template.clone() };
    let mut model = BakedModel::default();
    model.quads.push(quad([[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]]));
    model.quads.push(quad([[0., 0., 0.1], [1., 0., 0.1], [1., 1., 0.1], [0., 1., 0.1]]));
    for x in 0..4 {
        let x = x as f32 / 4.;
        model.quads.push(quad([[x, 0., 0.], [x, 0., 0.0625], [x, 1., 0.0625], [x, 1., 0.]]));
    }
    let main = super::super::solaris_models::authored("minecraft:zombie#main").unwrap();
    let held = Held { slot: 0, hand: 0, bone: 3, context: "thirdperson_righthand", state: ItemContext::default(), shape: 1 };
    let texture = crate::protocol::content::TextureRegion { uv: [0., 0., 1., 1.], layer: 0, width: 16, height: 16 };
    let parts = [
        Part::Rig { rig: main, texture, color: [1.; 3], material: EntityMaterial::EntityCutout, body: true },
        Part::Rig { rig: main, texture, color: [1.; 3], material: EntityMaterial::ArmorDecalCutoutNoCull, body: false },
        Part::Item { held: &held, model: &model },
    ];
    let mut meshes = CrowdMeshes::default();
    meshes.append(build(&parts, main, 0.).unwrap());
    let group = &meshes.groups[0];
    let at = |material: EntityMaterial| group.counts[MATERIALS.iter().position(|m| *m == material).unwrap()];
    let body = at(EntityMaterial::EntityCutout);
    assert!(body[0] > 0 && body == [body[0]; LODS], "the body keeps every level");
    let trim = at(EntityMaterial::ArmorDecalCutoutNoCull);
    assert_eq!(trim, [body[0], body[0], 0], "trims leave at level 2");
    assert_eq!(at(EntityMaterial::ItemCutout), [36, 12, 12], "extrusion edges leave at level 1");
    // The kept quads come first: a level draws a prefix of its range.
    let items = MATERIALS.iter().position(|m| *m == EntityMaterial::ItemCutout).unwrap();
    let first = group.first_index[items] as usize;
    let attachment = p::held_item_attachment(Mat4::IDENTITY, 0) * p::item_matrix(&model, "thirdperson_righthand");
    let faces: Vec<Vec3> = model.quads[..2]
        .iter()
        .flat_map(|quad| quad.positions.map(|v| attachment.transform_point3(Vec3::from_array(v))))
        .collect();
    for &index in &meshes.indices[first..first + 12] {
        let position = Vec3::from_array(meshes.vertices[(group.base_vertex + index as i32) as usize].position);
        assert!(faces.iter().any(|face| face.abs_diff_eq(position, 1e-5)), "a face quad vertex");
    }
    // The bound covers every vertex under any pose.
    assert!(group.radius > 1.5 && group.radius < 6., "radius {}", group.radius);
}

#[test]
fn ineligible_entities_stay_on_the_cpu_path() {
    let fallback = |stats: &PresentationStats, reason| stats.crowd_fallback.get(ZOMBIE).and_then(|reasons| reasons.get(reason)).copied();
    // Invisible: the CPU path still draws its equipment.
    let (mut store, content) = fixture();
    store.slots[0].metadata.insert(0, crate::entities::Metadata::Byte(32));
    let (_, stats) = render(&mut store, &content, true);
    assert_eq!(stats.crowd, 0);
    assert_eq!(fallback(&stats, "invisible or glowing"), Some(1));
    // Enchanted armour needs the glint material.
    let (mut store, content) = fixture();
    let mut boots = store.slots[0].equipment[2].take().unwrap();
    let mut components = (*boots.components).clone();
    components.insert("minecraft:enchantment_glint_override".into(), json!(true));
    boots.components = std::sync::Arc::new(components);
    store.slots[0].equipment[2] = Some(boots);
    let (cpu, stats) = render(&mut store, &content, true);
    assert_eq!((stats.crowd, stats.submitted), (0, 1));
    assert_eq!(fallback(&stats, "enchantment glint"), Some(1));
    assert!(!cpu.vertices.is_empty());
    // A routed entity records no fallback; kinds outside the crowd set are never tried.
    let (mut store, content) = fixture();
    let (_, stats) = render(&mut store, &content, true);
    assert_eq!((stats.crowd, stats.crowd_fallback.len()), (1, 0));
    assert_eq!(kind("minecraft:creeper"), None);
    assert_eq!(kind(ZOMBIE), Some(ZOMBIE));
}

#[test]
fn gpu_layouts_match_the_wgsl_structs() {
    assert_eq!(std::mem::size_of::<CrowdVertex>(), 56);
    assert_eq!(std::mem::size_of::<CrowdBone>(), 64);
    assert_eq!(std::mem::size_of::<CrowdInstance>(), 144);
    assert_eq!(std::mem::size_of::<ChannelSample>(), 32);
}
