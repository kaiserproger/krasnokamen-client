//! `minecraft_attack_entity_once` with a piercing weapon: the client's spear
//! arm queues a STAB player-action (wire 8) that clears the queue and swings,
//! but `interact_checked` only counted ATTACK or player-action 0|2|7 as
//! "started", so the tool reported `ordinary entity attack was not dispatched`.
//! Recorded from receipts/G2/live-g2l/hit-test-diag (15 identical swings:
//! `attack not started; ... main_hand=minecraft:iron_spear
//! (piercing_weapon=true); ... queued_commands=[player_action#41 action=8,
//! swing#46]`).
use super::*;
use crate::{
    entities::{Handle, Models},
    player::Hit,
    protocol::{
        Reader, Writer,
        content::Content,
        world::{Chunk, Palette, Section, SurfaceHeightmap},
    },
};

const SPEAR_ENTITY: u32 = 3;
const TARGET_ID: i32 = 77;

fn models() -> Models {
    let schema = serde_json::from_value(json!({
        "protocol_id": 1, "width": 0.6, "height": 1.95, "eye_height": 1.62,
        "fire_immune": false, "class": "net.minecraft.world.entity.LivingEntity",
        "dimension_constants": {}, "enum_ids": {}, "enum_numbers": {},
        "hierarchy": ["net.minecraft.world.entity.Entity", "net.minecraft.world.entity.LivingEntity"],
        "method_owners": {"isPickable": "net.minecraft.world.entity.LivingEntity"},
        "dimensions": {"width": 0.6, "height": 1.95, "eye_height": 1.62,
            "fixed": false, "attachments": {}},
        "default_attributes": {"minecraft:scale": {"id": 0, "value": 1.0}}, "accessors": {}
    }))
    .unwrap();
    Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::from([("test:soldier".into(), schema)]),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: Vec::new(),
        default_skins: Vec::new(),
        poses: BTreeMap::from([(0, "STANDING".into())]),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        armor_models: BTreeMap::new(),
        motion: BTreeMap::new(),
    }
}

/// Player eye at (4.5, 5.62, 4.5); a soldier standing 2.5 blocks ahead on open
/// ground, well inside entity-interaction reach, with nothing between.
fn session() -> (Session, Handle) {
    let mut session = Session::new_mode("attack-dispatch-test".into(), false);
    session.world = World::new(0, 16);
    let palette = |size| Palette {
        bits: 0,
        values: vec![0],
        words: Vec::new(),
        size,
    };
    session
        .world
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
    let mut content = Content::test_cube();
    content
        .entity_names
        .insert(SPEAR_ENTITY, "test:soldier".into());
    content
        .item_names
        .insert(SPEAR_ENTITY, "minecraft:iron_spear".into());
    let mut packet = Writer::default();
    packet.var(TARGET_ID);
    packet.0.extend([0u8; 16]);
    packet.var(SPEAR_ENTITY as i32);
    for coordinate in [4.5, 4., 7.] {
        packet.f64(coordinate);
    }
    packet.0.extend([0u8; 5]);
    session
        .entities
        .apply(1, &mut Reader::new(&packet.0), &content, 1)
        .unwrap();
    session.entities.models = Some(Arc::new(models()));
    session.content = Some(content);
    session.phase = "play".into();
    session.player.position = [4.5, 4., 4.5];
    session.player.pose_known = true;
    // A spear in the selected hotbar slot: same component the live client arms on.
    let mut spear = crate::inventory::Stack::default();
    spear.id = SPEAR_ENTITY;
    spear.count = 1;
    spear.components = Arc::new(BTreeMap::from([(
        "minecraft:piercing_weapon".to_owned(),
        json!({}),
    )]));
    session.inventory.player.slots[36] = Some(spear);
    session.inventory.selected_slot = 0;
    session.player.game_mode = 0;
    let _peer = super::mining_start_tests::connect(&mut session);
    let handle = session.entities.get(TARGET_ID).unwrap().handle;
    (session, handle)
}

#[test]
fn spear_attack_counts_its_stab_as_started() {
    let (mut session, handle) = session();
    assert!(
        session.aim_at_entity(handle, true).unwrap().is_none(),
        "open-ground soldier 2.5 blocks ahead must be aimable"
    );
    let result = session
        .interact_entity(handle, crate::player::InteractionIntent::Attack)
        .unwrap()
        .expect("aimed spear attack must dispatch, not crosshair-mismatch");
    assert_eq!(
        result["started"].as_bool(),
        Some(true),
        "queued STAB must count as started: {result}"
    );
    // NOTE: `interact_entity` flushes the queue into the loopback peer, so the
    // STAB packet itself is asserted through the recorded diag transcript
    // (hit-test-diag: `queued_commands=[player_action#41 action=8, swing#46]`);
    // here `started == true` is the signal the fix restores.
    let target = session
        .player
        .interaction_context(
            &session.world,
            session.content.as_ref().unwrap(),
            &session.inventory,
            &session.entities,
            session.world_tick,
        )
        .unwrap()
        .target;
    assert!(
        matches!(target, Some(Hit::Entity { handle: hit, .. }) if hit == handle),
        "same raycast still resolves to the target after the swing"
    );
}
