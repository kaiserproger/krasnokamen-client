//! Component-driven GUI regression tests; all item numbers/geometry are synthetic.
use super::*;
use crate::inventory::{
    ComponentPatch, ComponentValue, DisplayStack, ItemCost, Recipe, Stack, Tag, Trade, Trades,
};

fn stack(count: i32, components: &[(&str, Value)]) -> Stack {
    let mut stack = Stack::default();
    stack.id = 1;
    stack.count = count;
    stack.components = Arc::new(
        components
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    );
    stack
}

fn damaged(damage: i32) -> Stack {
    stack(
        1,
        &[
            ("minecraft:max_damage", json!(100)),
            ("minecraft:damage", json!(damage)),
            ("minecraft:item_model", json!("test:bar_item")),
        ],
    )
}

fn bundle(items: Vec<Stack>) -> Stack {
    let mut bundle = stack(
        1,
        &[(
            "minecraft:bundle_contents",
            json!(vec![json!({}); items.len()]),
        )],
    );
    bundle.patch = Arc::new(ComponentPatch {
        added: BTreeMap::from([(
            0,
            ComponentValue {
                name: "minecraft:bundle_contents".into(),
                value: Tag::Empty,
                wire_range: 0..0,
                adventure: None,
                hash: None,
                nested_stacks: Arc::from(items),
            },
        )]),
        removed: Vec::new(),
    });
    bundle
}

fn draw(stack: &Stack) -> MeshGeometry {
    let content = Content::test_cube();
    let mut mesh = MeshGeometry::default();
    append_stack_item(
        &content,
        stack,
        &crate::protocol::models::BakedModel::default(),
        [0., 0., 16., 16.],
        Mat4::IDENTITY,
        &mut mesh,
    )
    .unwrap();
    mesh
}

fn width(mesh: &MeshGeometry, quad: usize) -> f32 {
    mesh.vertices[quad * 4 + 1].position[0] - mesh.vertices[quad * 4].position[0]
}

#[test]
fn damaged_stack_emits_shrinking_green_yellow_red_bar_quads() {
    let green = draw(&damaged(10));
    let yellow = draw(&damaged(50));
    let red = draw(&damaged(90));
    for mesh in [&green, &yellow, &red] {
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices[RenderLayer::Translucent as usize].len(), 12);
        assert_eq!(width(mesh, 0), 12.);
        assert_eq!(mesh.vertices[0].position, [2., 13., 0.]);
        assert_eq!(mesh.vertices[2].position, [14., 15., 0.]);
        assert_eq!(mesh.vertices[0].color, [0.18; 3]);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.uv == [-1.; 2] && v.overlay_uv == [-1.; 2])
        );
    }
    assert_eq!(
        [width(&green, 1), width(&yellow, 1), width(&red, 1)],
        [10., 6., 1.]
    );
    assert!(green.vertices[4].color[1] > green.vertices[4].color[0]);
    assert_eq!(yellow.vertices[4].color, [1., 1., 0.]);
    assert!(red.vertices[4].color[0] > red.vertices[4].color[1]);
    assert!(green.vertices[4].color[0] < yellow.vertices[4].color[0]);
    assert!(yellow.vertices[4].color[1] > red.vertices[4].color[1]);
    // At the final pixel boundary, keep the empty track, not a degenerate quad.
    assert_eq!(draw(&damaged(99)).vertices.len(), 4);
}

#[test]
fn undamaged_unbreakable_empty_and_non_damageable_stacks_emit_no_bar() {
    let mut unbreakable = damaged(50);
    Arc::make_mut(&mut unbreakable.components).insert("minecraft:unbreakable".into(), json!({}));
    let mut empty = damaged(50);
    empty.count = 0;
    for stack in [
        damaged(0),
        unbreakable,
        empty,
        stack(1, &[]),
        stack(1, &[("minecraft:damage", json!(50))]),
    ] {
        assert!(draw(&stack).vertices.is_empty());
    }
}

#[test]
fn bundle_fullness_scales_and_changes_blue_to_red_only_when_full() {
    let item = |count, max| stack(count, &[("minecraft:max_stack_size", json!(max))]);
    assert!(draw(&bundle(vec![])).vertices.is_empty());
    let tiny = draw(&bundle(vec![item(1, 64)]));
    let quarter = draw(&bundle(vec![item(16, 64)]));
    let half = draw(&bundle(vec![item(8, 16)]));
    let full = draw(&bundle(vec![item(32, 64), item(8, 16)]));
    assert_eq!(
        [
            width(&tiny, 1),
            width(&quarter, 1),
            width(&half, 1),
            width(&full, 1)
        ],
        [1., 3., 6., 12.]
    );
    assert_eq!(quarter.vertices[4].color, [0.22, 0.48, 0.92]);
    assert_eq!(half.vertices[4].color, quarter.vertices[4].color);
    assert_eq!(full.vertices[4].color, [0.90, 0.18, 0.18]);
    assert_eq!(
        draw(&bundle(vec![item(1, 1)])).vertices[4].color,
        full.vertices[4].color
    );
    let custom = draw(&bundle(vec![item(1, 3), item(2, 3)]));
    assert_eq!(width(&custom, 1), 12.);
    assert_eq!(custom.vertices[4].color, full.vertices[4].color);
}

#[test]
fn bundle_uses_nested_contents_overhead_and_bees_not_item_names() {
    let nested = bundle(vec![stack(8, &[("minecraft:max_stack_size", json!(64))])]);
    // 1/16 overhead + 8/64 contents = 3/16; 12 * 3/16 rounds to 2.
    let mesh = draw(&bundle(vec![nested]));
    assert_eq!(width(&mesh, 1), 2.);
    let bees = stack(
        1,
        &[
            ("minecraft:max_stack_size", json!(64)),
            ("minecraft:bees", json!([{}])),
        ],
    );
    let mesh = draw(&bundle(vec![bees]));
    assert_eq!(width(&mesh, 1), 12.);
    assert_eq!(mesh.vertices[4].color, [0.90, 0.18, 0.18]);
}

#[test]
fn bar_follows_parent_transform_after_icon_without_inheriting_foil() {
    let mut content = Content::test_cube();
    content.insert_test_texture(
        "minecraft:misc/enchanted_glint_item",
        crate::protocol::content::TextureRegion {
            layer: 5,
            uv: [0., 0., 1., 1.],
            width: 16,
            height: 16,
        },
    );
    let mut stack = damaged(50);
    Arc::make_mut(&mut stack.components)
        .insert("minecraft:enchantment_glint_override".into(), json!(true));
    let mut model = content.models[0].clone();
    model.quads.truncate(1);
    let mut mesh = MeshGeometry::default();
    let parent =
        Mat4::from_translation(Vec3::new(5., 7., 0.)) * Mat4::from_scale(Vec3::new(2., 3., 1.));
    append_stack_item(
        &content,
        &stack,
        &model,
        [10., 20., 32., 32.],
        parent,
        &mut mesh,
    )
    .unwrap();
    assert_eq!(mesh.vertices.len(), 12);
    assert_eq!(mesh.vertices[4].position, [33., 145., 0.]);
    assert_eq!(mesh.vertices[8].position, mesh.vertices[4].position);
    let indices = &mesh.indices[RenderLayer::Translucent as usize];
    assert_eq!([indices[0], indices[6], indices[12]], [0, 4, 8]);
    assert_eq!(mesh.vertices[0].overlay_uv, [-2.; 2]);
    assert!(mesh.vertices[4..].iter().all(|v| v.overlay_uv == [-1.; 2]));
}

fn ui_fixture() -> (Content, InventoryStore, UiState) {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    let inventory = InventoryStore::default();
    let mut ui = UiState::new_state(&content).unwrap();
    ui.resize(300, 200, 1.);
    ui.models
        .insert_test_model("test:bar_item", content.models[0].clone());
    (content, inventory, ui)
}

fn assert_bar(mesh: &MeshGeometry, origin: [f32; 2]) {
    let bar = mesh
        .vertices
        .chunks_exact(4)
        .find(|q| q[0].color == [0.18; 3])
        .expect("real stack paint must emit a bar track");
    assert_eq!(bar[0].position, [origin[0] + 2., origin[1] + 13., 0.]);
    assert_eq!(bar[2].position, [origin[0] + 14., origin[1] + 15., 0.]);
    let fill = mesh
        .vertices
        .chunks_exact(4)
        .find(|q| q[0].color == [1., 1., 0.])
        .expect("real stack paint must emit the half-durability fill");
    assert_eq!(fill[1].position[0] - fill[0].position[0], 6.);
}

#[test]
fn every_retained_stack_paint_uses_shared_decoration_in_real_renderer() {
    let (content, mut inventory, mut ui) = ui_fixture();
    let stack = damaged(50);
    inventory.player.slots[9] = Some(stack.clone());
    inventory.player.slots[36] = Some(stack.clone());
    ui.creative_stacks.insert(7, stack.clone());
    let cost = ItemCost {
        stack: stack.clone(),
        components: Arc::default(),
    };
    inventory.player.trades = Some(Arc::new(Trades {
        offers: vec![Trade {
            buy_a: cost.clone(),
            buy_b: Some(cost),
            sell: stack.clone(),
            out_of_stock: false,
            uses: 0,
            max_uses: 1,
            xp: 0,
            special_price: 0,
            price_multiplier: 0.,
            demand: 0,
        }],
        level: 1,
        xp: 0,
        show_progress: false,
        can_restock: false,
        selected: 0,
    }));
    inventory.recipes.insert(
        4,
        Recipe {
            id: 4,
            display: Arc::new(json!({"result":{"type":"minecraft:item_stack"}})),
            group: None,
            category: "test".into(),
            ingredients: None,
            flags: 0,
            display_stacks: vec![DisplayStack {
                display_path: "result".into(),
                stack: stack.clone(),
            }],
            ingredient_items: None,
        },
    );
    let cases = [
        ("slot/9", Paint::Slot(9), true),
        ("hud/slot/0", Paint::Slot(36), true),
        ("creative/7", Paint::Creative(7), false),
        ("trade/a", Paint::TradeItem { offer: 0, part: 0 }, false),
        ("trade/b", Paint::TradeItem { offer: 0, part: 1 }, false),
        (
            "trade/result",
            Paint::TradeItem { offer: 0, part: 2 },
            false,
        ),
        ("recipe/4", Paint::RecipeDisplay(4), false),
        (
            "variants",
            Paint::Variants(Arc::new(vec![stack.clone()]), false),
            false,
        ),
        ("ghost", Paint::Variants(Arc::new(vec![stack]), true), false),
    ];
    for (id, paint, inset) in cases {
        ui.nodes.clear();
        let bounds = if inset {
            [9., 19., 18., 18.]
        } else {
            [10., 20., 16., 16.]
        };
        ui.add(
            id.into(),
            "image",
            String::new(),
            bounds,
            true,
            None,
            NodeAction::None,
            paint,
        )
        .unwrap();
        let nodes = std::mem::take(&mut ui.nodes);
        let mut mesh = MeshGeometry::default();
        ui.render_nodes(
            &content,
            &inventory,
            &mut mesh,
            Mat4::IDENTITY,
            &nodes,
            None,
        )
        .unwrap();
        assert_bar(&mesh, [10., 20.]);
    }
    // Container menus select active-menu slots, not similarly numbered player slots.
    inventory.active = Some(inventory.player.clone());
    inventory.player.slots[9] = Some(Stack::default());
    ui.nodes.clear();
    ui.add(
        "container/slot/9".into(),
        "image",
        String::new(),
        [9., 19., 18., 18.],
        true,
        None,
        NodeAction::None,
        Paint::Slot(9),
    )
    .unwrap();
    let nodes = std::mem::take(&mut ui.nodes);
    let mut mesh = MeshGeometry::default();
    ui.render_nodes(
        &content,
        &inventory,
        &mut mesh,
        Mat4::IDENTITY,
        &nodes,
        None,
    )
    .unwrap();
    assert_bar(&mesh, [10., 20.]);
}

#[test]
fn cursor_decoration_is_rendered_in_inventory_and_creative() {
    for screen in [Screen::Menu, Screen::Creative] {
        let (content, mut inventory, mut ui) = ui_fixture();
        inventory.open_inventory().unwrap();
        inventory.cursor = Some(damaged(50));
        ui.inventory_instance = inventory.screen_instance;
        ui.inventory_revision = inventory.revision;
        ui.selected_slot = inventory.selected_slot;
        ui.screen = screen;
        ui.nodes.clear();
        ui.dirty = false;
        ui.pointer = [40., 40.];
        let mut mesh = MeshGeometry::default();
        ui.render(&content, &inventory, &mut mesh).unwrap();
        assert_bar(&mesh, [32., 32.]);
    }
}

#[test]
fn slot_and_cursor_counts_are_appended_over_the_bar() {
    for cursor in [false, true] {
        let (mut content, mut inventory, mut ui) = ui_fixture();
        // Empty, Solaris-authored font definition exercises ordinary missing-glyph
        // geometry without loading any game assets.
        content.insert_test_font("minecraft:default", &json!({"providers":[]}));
        content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
        ui.text = TextRenderer::load(&content).unwrap().into();
        inventory.open_inventory().unwrap();
        ui.inventory_instance = inventory.screen_instance;
        ui.inventory_revision = inventory.revision;
        ui.selected_slot = inventory.selected_slot;
        ui.screen = Screen::Menu;
        ui.nodes.clear();
        ui.dirty = false;
        ui.pointer = [18., 28.];
        if !cursor {
            ui.add(
                "slot/9".into(),
                "image",
                String::new(),
                [9., 19., 18., 18.],
                true,
                None,
                NodeAction::None,
                Paint::Slot(9),
            )
            .unwrap();
        }
        let mut meshes = Vec::new();
        for count in [1, 2] {
            let mut stack = damaged(50);
            stack.count = count;
            if cursor {
                inventory.cursor = Some(stack);
            } else {
                inventory.player.slots[9] = Some(stack);
            }
            let mut mesh = MeshGeometry::default();
            ui.render(&content, &inventory, &mut mesh).unwrap();
            assert_bar(&mesh, [10., 20.]);
            meshes.push(mesh);
        }
        let icon_and_bar = &meshes[0];
        let counted = &meshes[1];
        let prefix = icon_and_bar.vertices.len();
        assert!(
            counted.vertices.len() > prefix,
            "count must emit text geometry"
        );
        assert_eq!(icon_and_bar.vertices[prefix - 4].color, [1., 1., 0.]);
        assert_eq!(counted.vertices[prefix - 4].color, [1., 1., 0.]);
        assert!(
            counted.vertices[prefix..]
                .iter()
                .any(|v| v.color == [1.; 3])
        );
        let indices = &counted.indices[RenderLayer::Translucent as usize];
        let count_start = icon_and_bar.indices[RenderLayer::Translucent as usize].len();
        assert!(indices[count_start..].iter().all(|&i| i as usize >= prefix));
    }
}
