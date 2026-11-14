//! Authored world; 26.3 FallingBlockEntity.tick and Entity.getAirDrag via local javap.
use super::*;
use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap, World};

fn fixture() -> (Store, World, Content) {
    let (mut models, mut entity) = animation_tests::fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.hierarchy.clear();
    schema.dimensions.width = 0.98;
    schema.dimensions.height = 0.98;
    schema.accessors.insert("DATA_NO_GRAVITY".into(), [5, 8]);
    models
        .schemas
        .insert("minecraft:falling_block".into(), schema);
    entity.type_name = "minecraft:falling_block".into();
    entity.source = None;
    entity.on_ground = false;
    entity.transform.position = [8.5, 10., 8.5];
    entity.previous = entity.transform;
    entity.position = entity.transform.position;
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.slots.push(entity);
    store.ids.insert(41, 0);
    store.physical_active.insert(41);
    let palette = |size| Palette {
        bits: 0,
        values: vec![0],
        words: Vec::new(),
        size,
    };
    let mut world = World::new(0, 16);
    world
        .insert(
            [0, 0],
            Chunk {
                sections: vec![Arc::new(Section {
                    blocks: palette(4096),
                    biomes: palette(64),
                    non_empty_block_count: 0,
                })],
                sky: BTreeMap::new(),
                block: BTreeMap::new(),
                block_entities: crate::protocol::world::empty_block_entities(),
                surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                surface_heightmap_received: false,
            },
        )
        .unwrap();
    world.set([8, 2, 8], 1, |state| Ok(state == 0)).unwrap();
    (store, world, Content::test_cube())
}

#[test]
fn falling_block_predicts_between_packets_then_lands_without_moving_received_base() {
    let (mut store, world, content) = fixture();
    let (mut expected, mut velocity) = (10., 0.);
    for tick in 1..=10 {
        store
            .tick_presentation(tick, false, &world, &content, 0)
            .unwrap();
        velocity -= 0.04;
        expected += velocity;
        velocity *= f64::from(0.98_f32);
        let entity = store.get(41).unwrap();
        assert!((entity.presented_transform(tick, 1.).position[1] - expected).abs() < 1e-9);
        assert_eq!(
            entity.position[1], 10.,
            "prediction is not received authority"
        );
    }
    let entity = store.get(41).unwrap();
    let halfway = entity.presented_transform(10, 0.5).position[1];
    assert!(halfway > expected && halfway < entity.previous.position[1]);
    // 26.3 MoveEntityPos includes the path-properties VarInt before the deltas.
    let mut packet = crate::protocol::Writer::default();
    packet.var(41);
    packet.var(0);
    for delta in [0_i16, -4096, 0] {
        packet.0.extend(delta.to_be_bytes());
    }
    store
        .apply(
            crate::protocol::packet_ids::play::clientbound::MOVE_ENTITY_POS,
            &mut crate::protocol::Reader::new(&packet.0),
            &content,
            10,
        )
        .unwrap();
    assert_eq!(
        store.get(41).unwrap().position[1],
        9.,
        "relative move uses received 10, not predicted y"
    );
    assert_eq!(store.get(41).unwrap().transform.position[1], 9.);
    for tick in 11..=60 {
        store
            .tick_presentation(tick, false, &world, &content, 0)
            .unwrap();
    }
    assert!((store.get(41).unwrap().transform.position[1] - 3.).abs() < 1e-9);
    assert_eq!(store.get(41).unwrap().position[1], 9.);
}

#[test]
fn falling_block_freeze_and_no_gravity_preserve_pose_until_a_step() {
    let (mut store, world, content) = fixture();
    store
        .tick_presentation(1, true, &world, &content, 0)
        .unwrap();
    assert_eq!(store.get(41).unwrap().transform.position[1], 10.);
    store
        .tick_presentation(2, false, &world, &content, 0)
        .unwrap();
    assert!((store.get(41).unwrap().transform.position[1] - 9.96).abs() < 1e-9);
    let entity = store.get_mut(41).unwrap();
    entity.metadata.insert(5, Metadata::Bool(true));
    entity.velocity = [0.; 3];
    for tick in 3..=8 {
        store
            .tick_presentation(tick, false, &world, &content, 0)
            .unwrap();
    }
    assert!((store.get(41).unwrap().transform.position[1] - 9.96).abs() < 1e-9);
}

#[test]
fn falling_block_prediction_tracks_horizontal_query_padding_without_accumulating_spans() {
    let (mut store, world, content) = fixture();
    store.get_mut(41).unwrap().velocity = [0.2, 0., 0.];
    for tick in 1..=5 {
        store
            .tick_presentation(tick, false, &world, &content, 0)
            .unwrap();
        assert_eq!(store.motion_spans.len(), 1);
        assert_eq!(store.motion_spans.get(&1), Some(&1));
        assert_eq!(store.get(41).unwrap().position[0], 8.5);
    }
}
