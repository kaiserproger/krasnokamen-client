use super::*;
use crate::{api::{DynamicGeometry, EntityMaterial}, protocol::{Writer, content::TextureRegion}};

fn status(session: &mut Session, entity: i32, event: u8) -> Result<()> {
    let mut packet = Writer::default();
    packet.i32(entity); packet.byte(event);
    session.apply(Phase::Play, cb::ENTITY_EVENT, &packet.0)
}

#[test]
fn received_permission_is_local_and_deop_disconnect_rejoin_clear_authority() {
    let mut session = super::tests::session_with_received_cube([0, 3, 0]);
    session.player.entity_id = 7;
    session.player.game_mode = 1;
    assert_eq!(session.player.permission_level, None, "creative does not grant operator authority");
    status(&mut session, 8, 28).unwrap();
    assert_eq!(session.player.permission_level, None, "another player's status is not our authority");
    for (event, level) in [(26, 2), (28, 4), (25, 1), (24, 0)] {
        status(&mut session, 7, event).unwrap();
        assert_eq!(session.player.permission_level, Some(level));
    }
    status(&mut session, 7, 23).unwrap();
    status(&mut session, 7, 29).unwrap();
    assert_eq!(session.player.permission_level, Some(0), "unrelated events preserve received deop");
    status(&mut session, 7, 27).unwrap();
    session.disconnect();
    assert_eq!(session.player.permission_level, None);
    // Install the next connection's normal received-state fixture without a socket.
    // Authority must remain unknown until that connection receives its own event.
    session.content = super::tests::session_with_received_cube([0, 3, 0]).content;
    session.player.entity_id = 9;
    status(&mut session, 7, 28).unwrap();
    assert_eq!(session.player.permission_level, None);
    status(&mut session, 9, 24).unwrap();
    assert_eq!(session.player.permission_level, Some(0));
}

fn impact(session: &mut Session, event: i32, data: i32) {
    let mut packet = Writer::default();
    packet.i32(event); packet.i64(protocol::pack_pos([0, 3, 0]));
    packet.i32(data); packet.byte(0);
    session.apply(Phase::Play, cb::LEVEL_EVENT, &packet.0).unwrap();
}
fn geometry(session: &Session) -> DynamicGeometry {
    let mut out = DynamicGeometry::default();
    session.block_effects.append(&session.world, [0.; 3], 0., 0., 0., &mut out).unwrap();
    out
}

#[test]
fn received_potion_impact_uses_event_rgb_and_effect_family_until_expiry() {
    let mut session = super::tests::session_with_received_cube([0, 3, 0]);
    session.presentation = true;
    let content = session.content.as_mut().unwrap();
    for (prefix, base) in [("effect", 100), ("spell", 200)] {
        for i in 0..8 {
            content.insert_test_texture(&format!("minecraft:particle/{prefix}_{}", 7-i), TextureRegion {
                layer: base+i, uv: [0.1, 0.2, 0.3, 0.4], width: 16, height: 16,
            });
        }
    }
    content.insert_test_texture("minecraft:item/splash_potion", TextureRegion {
        layer: 300, uv: [0., 0., 1., 1.], width: 16, height: 16,
    });
    impact(&mut session, 2002, 0xff0000);
    assert_eq!(session.events.back().unwrap()["presentation_supported"], true);
    assert!(geometry(&session).vertices.is_empty(), "unknown light does not fabricate a visible frame");
    let mut chunk = session.world.chunk([0, 0]).unwrap().clone();
    chunk.sky.insert(1, Arc::new(vec![0xff; 2048]));
    chunk.block.insert(1, Arc::new(vec![0; 2048]));
    session.world.insert([0, 0], chunk).unwrap();
    // Initial radial particles may occupy adjacent chunks: install received light there too.
    for key in [[-1, 0], [0, -1], [-1, -1]] {
        let chunk = session.world.chunk([0, 0]).unwrap().clone();
        session.world.insert(key, chunk).unwrap();
    }
    let red = geometry(&session);
    assert_eq!(red.vertices.len(), 108 * 4);
    let spells: Vec<_> = red.vertices.iter().filter(|v| v.texture_layer == 100).collect();
    assert_eq!(spells.len(), 100 * 4);
    assert!(spells.iter().all(|v| (0.75..=1.).contains(&v.color[0]) && v.color[1] == 0. && v.color[2] == 0.));
    assert!(spells.iter().all(|v| (2. < v.position[1]) && v.position[1] < 4.));
    assert_eq!(red.indices[EntityMaterial::EntityTranslucent as usize].len(), 100 * 6);
    // High bits do not make a valid received RGB color a negative block-state ID.
    impact(&mut session, 2007, 0xff00ff00_u32 as i32);
    let both = geometry(&session);
    let green: Vec<_> = both.vertices.iter().filter(|v| v.texture_layer == 200).collect();
    assert_eq!(green.len(), 100 * 4);
    assert!(green.iter().all(|v| v.color[0] == 0. && (0.75..=1.).contains(&v.color[1]) && v.color[2] == 0.));
    let before = both.vertices.len();
    impact(&mut session, 2003, 0x0000ff);
    assert_eq!(session.events.back().unwrap()["presentation_supported"], false);
    assert_eq!(geometry(&session).vertices.len(), before);
    for _ in 0..4 { session.block_effects.tick(&session.world, session.content.as_ref().unwrap()); }
    assert!(geometry(&session).vertices.iter().any(|v| (101..=107).contains(&v.texture_layer)), "sprite animation advances on ticks");
    for _ in 0..40 { session.block_effects.tick(&session.world, session.content.as_ref().unwrap()); }
    assert!(geometry(&session).vertices.is_empty());
    impact(&mut session, 2002, 0);
    let black = geometry(&session);
    let spells: Vec<_> = black.vertices.iter().filter(|v| v.texture_layer == 100).collect();
    assert_eq!(spells.len(), 400);
    assert!(spells.iter().all(|v| v.color == [0.; 3]), "zero RGB is black, not an unknown/default potion");
    session.presentation = false;
    impact(&mut session, 2007, 0xffffff);
    assert_eq!(session.events.back().unwrap()["presentation_supported"], false);
    assert_eq!(geometry(&session).vertices.len(), black.vertices.len());
    session.block_effects.clear();
    assert!(geometry(&session).vertices.is_empty(), "dimension/disconnect clear removes all impacts");
}
