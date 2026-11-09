use super::*;
use crate::{protocol::{Reader, Writer, content::TextureRegion, packet_ids::play::clientbound as cb}, ui::UiState};

fn receive(ui: &mut UiState, content: &Content, id: i32, color: u8, decorations: bool) {
    let mut packet = Writer::default();
    packet.var(id); packet.byte(0); packet.byte(0);
    packet.byte(1); packet.var(if decorations { 2 } else { 0 });
    if decorations {
        for kind in [0, 1] {
            packet.var(kind); packet.byte(0); packet.byte(4); packet.byte(4); packet.byte(0);
        }
    }
    packet.byte(1); packet.byte(1); packet.byte(10); packet.byte(20);
    packet.var(1); packet.byte(color);
    assert!(ui.apply(cb::MAP_ITEM_DATA, &mut Reader::new(&packet.0), content).unwrap());
}
fn fixture() -> (Models, Entity, Content, Cache, ItemModels, UiState) {
    let (models, entity, mut content, mut cache, items, cube) = super::tests::frame_fixture("minecraft:glow_item_frame");
    content.item_names.insert(2, "minecraft:filled_map".into());
    content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[],"map_decorations":{"minecraft:frame":{"asset_id":"minecraft:frame","show_on_item_frame":true},"minecraft:player_off_map":{"asset_id":"minecraft:player_off_map","show_on_item_frame":false}}}"#.as_slice());
    content.set_registries(BTreeMap::from([("minecraft:map_decoration_type".into(), vec![
        ("minecraft:frame".into(), serde_json::Value::Null),
        ("minecraft:player_off_map".into(), serde_json::Value::Null),
    ])]));
    for (name, layer) in [("frame", 81), ("player_off_map", 82)] {
        content.insert_test_texture(&format!("minecraft:map/decorations/{name}"), TextureRegion {
            layer, uv: [0.1, 0.2, 0.3, 0.4], width: 16, height: 16,
        });
    }
    cache.models.insert("minecraft:block/glow_item_frame".into(), cube.clone());
    let mut map_body = cube;
    for quad in &mut map_body.quads { quad.texture_layer = 77; }
    cache.models.insert("minecraft:block/glow_item_frame_map".into(), map_body);
    let ui = UiState::new_state(&content).unwrap();
    (models, entity, content, cache, items, ui)
}
fn map_stack(id: i32) -> Stack {
    let mut stack = super::tests::held(2);
    std::sync::Arc::make_mut(&mut stack.components).insert("minecraft:map_id".into(), serde_json::json!(id));
    stack
}
fn render(entity: &Entity, models: &Models, content: &Content, cache: &mut Cache, items: &mut ItemModels, ui: &UiState) -> DynamicGeometry {
    let mut out = DynamicGeometry::default();
    append(entity, models, ItemFrameResources { cache, items, maps: ui.map_render_view() }, content,
        p::EntityDrawFrame { origin: [0.; 3], transform: entity.presented_transform(0, 0.), tick: 0, partial: 0., light: [8., 2.], parent: None },
        None, &mut out).unwrap();
    out
}
fn pixels(out: &DynamicGeometry) -> Vec<&crate::api::Vertex> {
    out.vertices.iter().filter(|v| v.uv == [-1.; 2]).collect()
}

#[test]
fn framed_map_uses_received_identity_replacement_and_removal() {
    let (models, mut entity, content, mut cache, mut items, mut ui) = fixture();
    receive(&mut ui, &content, 42, 4, true);
    entity.metadata.insert(9, Metadata::Item(map_stack(42)));
    let first = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert_eq!(pixels(&first).len(), 4);
    assert_eq!(first.vertices.iter().filter(|v| v.texture_layer == 81).count(), 4);
    assert!(first.vertices.iter().all(|v| v.texture_layer != 82), "off-map players do not show in frames");
    assert!(pixels(&first).iter().all(|v| v.light == [15., 13.]));

    // Changing the stack identity must not retain the previous map or invent terrain.
    entity.metadata.insert(9, Metadata::Item(map_stack(43)));
    let unknown = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert!(pixels(&unknown).is_empty());
    assert!(unknown.vertices.iter().all(|v| v.texture_layer != 77 && v.texture_layer != 81));
    receive(&mut ui, &content, 43, 8, false);
    let replacement = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert_eq!(pixels(&replacement).len(), 4);
    assert_ne!(pixels(&replacement)[0].color, pixels(&first)[0].color);
    // A packet updates the same ID in-place: color-zero removes that terrain pixel.
    receive(&mut ui, &content, 43, 0, false);
    let erased = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert!(pixels(&erased).is_empty());
    entity.metadata.insert(9, Metadata::Item(Stack::default()));
    let empty = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert!(pixels(&empty).is_empty());
    assert!(empty.vertices.iter().all(|v| v.texture_layer != 77 && v.texture_layer != 81));
    // UI reset removes received data even if an old entity snapshot is still examined.
    entity.metadata.insert(9, Metadata::Item(map_stack(42)));
    ui.clear_session();
    let reset = render(&entity, &models, &content, &mut cache, &mut items, &ui);
    assert!(pixels(&reset).is_empty());
}

#[test]
fn framed_map_quarter_turns_face_outward_on_all_surfaces() {
    let (models, mut entity, content, mut cache, mut items, mut ui) = fixture();
    receive(&mut ui, &content, 42, 4, true);
    entity.metadata.insert(9, Metadata::Item(map_stack(42)));
    // Hide the body so surface vertices also prove invisible-frame support.
    entity.metadata.insert(0, Metadata::Byte(0x20));
    for direction in 0..6 {
        entity.spawn_data = direction;
        entity.metadata.insert(10, Metadata::Int(0));
        let first = render(&entity, &models, &content, &mut cache, &mut items, &ui);
        assert_eq!(first.vertices.len(), 8);
        let root = frame_root(entity.presented_transform(0, 0.).position, [0.; 3], direction).unwrap();
        let a = Vec3::from_array(first.vertices[0].position);
        let b = Vec3::from_array(first.vertices[1].position);
        let c = Vec3::from_array(first.vertices[2].position);
        assert!((b-a).cross(c-a).dot(root.transform_vector3(Vec3::NEG_Z)) > 0.);
        for rotation in 1..8 {
            entity.metadata.insert(10, Metadata::Int(rotation));
            let rotated = render(&entity, &models, &content, &mut cache, &mut items, &ui);
            let pivot = root.transform_point3(Vec3::new(0., 0., 0.5));
            let turn = root * Mat4::from_rotation_z((rotation % 4) as f32 * std::f32::consts::FRAC_PI_2) * root.inverse();
            for (before, after) in first.vertices.iter().zip(&rotated.vertices) {
                let expected = pivot + turn.transform_vector3(Vec3::from_array(before.position) - pivot);
                assert!((expected - Vec3::from_array(after.position)).length() < 1e-5, "direction={direction}, rotation={rotation}");
            }
        }
    }
}
