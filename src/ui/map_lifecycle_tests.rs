use super::*;
use crate::protocol::{Writer, content::TextureRegion};

fn fixture() -> (Content, UiState) {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[],"map_decorations":{"minecraft:frame":{"asset_id":"minecraft:frame","show_on_item_frame":true},"minecraft:banner_red":{"asset_id":"minecraft:banner_red","show_on_item_frame":true},"minecraft:player_off_map":{"asset_id":"minecraft:player_off_map","show_on_item_frame":false}}}"#.as_slice());
    content.set_registries(BTreeMap::from([("minecraft:map_decoration_type".into(), vec![
        ("minecraft:frame".into(), Value::Null), ("minecraft:banner_red".into(), Value::Null),
        ("minecraft:player_off_map".into(), Value::Null),
    ])]));
    for (name, layer) in [("map_background", 5), ("map_background_checkerboard", 6),
        ("decorations/frame", 7), ("decorations/banner_red", 8), ("decorations/player_off_map", 9)] {
        content.insert_test_texture(&format!("minecraft:map/{name}"), TextureRegion {
            layer, uv: [0., 0., 1., 1.], width: 16, height: 16,
        });
    }
    let ui = UiState::new_state(&content).unwrap();
    (content, ui)
}
fn packet(id: i32, decorations: Option<&[(i32, i8, i8, u8)]>, pixel: Option<u8>) -> Writer {
    let mut packet = Writer::default();
    packet.var(id); packet.byte(0); packet.byte(0);
    packet.byte(u8::from(decorations.is_some()));
    if let Some(decorations) = decorations {
        packet.var(decorations.len() as i32);
        for &(kind, x, y, rotation) in decorations {
            packet.var(kind); packet.byte(x as u8); packet.byte(y as u8); packet.byte(rotation); packet.byte(0);
        }
    }
    if let Some(color) = pixel {
        packet.byte(1); packet.byte(1); packet.byte(10); packet.byte(20); packet.var(1); packet.byte(color);
    } else { packet.byte(0); }
    packet
}
fn receive(ui: &mut UiState, content: &Content, packet: &Writer) -> Result<bool> {
    ui.apply(cb::MAP_ITEM_DATA, &mut Reader::new(&packet.0), content)
}
fn held(ui: &UiState, content: &Content, id: i32) -> DynamicGeometry {
    let mut out = DynamicGeometry::default();
    ui.append_held_map(id, content, glam::Mat4::IDENTITY, [15.; 2], &mut out).unwrap();
    out
}

#[test]
fn received_map_marker_replacements_remove_old_icons_and_omitted_updates_preserve_them() {
    let (content, mut ui) = fixture();
    receive(&mut ui, &content, &packet(42, Some(&[(0, -128, 127, 15), (2, 127, 0, 0)]), Some(4))).unwrap();
    let first = held(&ui, &content, 42);
    assert_eq!(first.vertices.iter().filter(|v| v.texture_layer == 7).count(), 4);
    assert_eq!(first.vertices.iter().filter(|v| v.texture_layer == 9).count(), 4);
    let pixels = ui.maps()[&42].colors.clone();
    // A decoration-only update is a replacement, not an append; banners replace stale frame/player icons.
    receive(&mut ui, &content, &packet(42, Some(&[(1, 20, -40, 4)]), None)).unwrap();
    let banner = held(&ui, &content, 42);
    assert!(banner.vertices.iter().all(|v| v.texture_layer != 7 && v.texture_layer != 9));
    let icon: Vec<_> = banner.vertices.iter().filter(|v| v.texture_layer == 8).collect();
    assert_eq!(icon.len(), 4);
    let center = icon.iter().fold([0.; 2], |sum, v| [sum[0] + v.position[0] / 4., sum[1] + v.position[1] / 4.]);
    assert!((center[0] - 73.5).abs() < 1e-5 && (center[1] - 43.5).abs() < 1e-5);
    assert_eq!(*ui.maps()[&42].colors, *pixels);
    // Pixel-only packet carries no decoration update: keep the banner, do not clear it.
    receive(&mut ui, &content, &packet(42, None, Some(8))).unwrap();
    assert_eq!(held(&ui, &content, 42).vertices.iter().filter(|v| v.texture_layer == 8).count(), 4);
    assert_eq!(ui.maps()[&42].colors[20 * 128 + 10], 8);
    receive(&mut ui, &content, &packet(42, Some(&[]), None)).unwrap();
    assert!(ui.maps()[&42].decorations.is_empty());
    assert!(held(&ui, &content, 42).vertices.iter().all(|v| v.texture_layer != 8));
}

#[test]
fn unknown_map_ids_and_rejected_marker_packets_never_borrow_another_maps_state() {
    let (content, mut ui) = fixture();
    receive(&mut ui, &content, &packet(42, Some(&[(1, 0, 0, 0)]), Some(4))).unwrap();
    let unknown = held(&ui, &content, 43);
    assert_eq!(unknown.vertices.len(), 4);
    assert!(unknown.vertices.iter().all(|v| v.texture_layer == 5));
    assert!(!ui.maps().contains_key(&43));
    let revision = ui.maps()[&42].revision;
    assert!(receive(&mut ui, &content, &packet(42, Some(&[(900, 0, 0, 0)]), None)).is_err());
    assert_eq!(ui.maps()[&42].revision, revision);
    assert_eq!(held(&ui, &content, 42).vertices.iter().filter(|v| v.texture_layer == 8).count(), 4);
    ui.clear_session();
    assert!(ui.maps().is_empty());
    assert_eq!(held(&ui, &content, 42).vertices.len(), 4);
}
