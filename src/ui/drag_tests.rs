use super::*;
use crate::inventory::Stack;
use crate::protocol::{Reader, Writer};

fn fixture() -> (Content, InventoryStore, UiState) {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    content.item_names.insert(1, "test:ingredient".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:max_stack_size".into(), json!(64)),
        ("minecraft:item_model".into(), json!("test:ingredient")),
    ]));
    let mut inventory = InventoryStore::default();
    inventory.open_inventory().unwrap();
    inventory.player.slots.fill(Some(Stack::default()));
    inventory.cursor = Some(Stack::default());
    let mut initial = Writer::default();
    initial.var(0);
    initial.var(1);
    initial.var(46);
    for slot in 0..46 {
        if slot == 9 {
            initial.var(1);
            initial.var(1);
            initial.var(0);
            initial.var(0);
        } else {
            initial.var(0);
        }
    }
    initial.var(0);
    inventory.apply(18, &mut Reader::new(&initial.0), &content).unwrap();
    let mut ui = UiState::new_state(&content).unwrap();
    ui.resize(300, 200, 1.0);
    ui.screen = Screen::Menu;
    ui.inventory_instance = inventory.screen_instance;
    ui.inventory_revision = inventory.revision;
    ui.nodes.clear();
    for (slot, x) in [(9, 20.), (1, 60.), (2, 80.), (0, 100.)] {
        ui.add(format!("slot/{slot}"), "slot", String::new(), [x, 20., 16., 16.], true, None, NodeAction::Slot(slot), Paint::Slot(slot as usize)).unwrap();
    }
    ui.dirty = false;
    (content, inventory, ui)
}

fn pointer(ui: &mut UiState, inventory: &mut InventoryStore, content: &Content, phase: PointerPhase, x: f64) -> UiEffects {
    pointer_button(ui, inventory, content, phase, x, PointerButton::Left)
}

fn pointer_button(ui: &mut UiState, inventory: &mut InventoryStore, content: &Content, phase: PointerPhase, x: f64, button: PointerButton) -> UiEffects {
    let mut effects = UiEffects::default();
    ui.execute_inner(UiIntent::Pointer { phase, x, y: 25., button: Some(button), modifiers: Modifiers::default() }, inventory, content, &mut effects).unwrap();
    effects
}

fn deliver(effects: UiEffects, inventory: &mut InventoryStore, content: &Content) -> usize {
    let mut packets = 0;
    for command in effects.commands {
        match command {
            UiCommand::Inventory(intent) => packets += inventory.execute(&intent, content).unwrap().len(),
            UiCommand::InventoryGesture(intents) => packets += inventory.execute_gesture(&intents, content).unwrap().len(),
            _ => {}
        }
    }
    packets
}

fn authoritative(inventory: &mut InventoryStore, content: &Content, state: i32, slot: i16, cursor: bool) {
    let mut wire = Writer::default();
    wire.var(0);
    wire.var(state);
    wire.var(46);
    for i in 0..46 {
        if i == slot {
            wire.var(1); wire.var(1); wire.var(0); wire.var(0);
        } else { wire.var(0); }
    }
    if cursor { wire.var(1); wire.var(1); wire.var(0); wire.var(0); } else { wire.var(0); }
    inventory.apply(18, &mut Reader::new(&wire.0), content).unwrap();
}

#[test]
fn drag_from_inventory_into_grid_and_reconcile_rejection() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    assert_eq!(deliver(down, &mut inventory, &content), 1);
    assert!(inventory.player.slots[9].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);
    pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, 65.);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 65.);
    assert_eq!(deliver(up, &mut inventory, &content), 1);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 1);
    assert!(inventory.cursor.as_ref().unwrap().empty());
    authoritative(&mut inventory, &content, 3, 1, false);
    assert_eq!(inventory.received_menu().slots[1].as_ref().unwrap().count, 1);
    assert!(inventory.received_cursor().unwrap().empty());

    // A server refusal restores the actual cursor and removes the predicted slot.
    authoritative(&mut inventory, &content, 4, -1, true);
    assert!(inventory.player.slots[1].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);
    assert_eq!(inventory.received_cursor().unwrap().count, 1);
}

#[test]
fn rejected_pickup_does_not_issue_a_drop() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    authoritative(&mut inventory, &content, 1, 9, false);
    pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, 65.);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 65.);
    assert_eq!(deliver(up, &mut inventory, &content), 0);
    assert_eq!(inventory.player.slots[9].as_ref().unwrap().count, 1);
    assert!(inventory.player.slots[1].as_ref().unwrap().empty());
    assert!(inventory.cursor.as_ref().unwrap().empty());
}

#[test]
fn delayed_pickup_image_temporarily_overwrites_later_drop_prediction() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 65.);
    deliver(up, &mut inventory, &content);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 1);
    assert!(inventory.cursor.as_ref().unwrap().empty());
    // Server processes the two clicks in order; its pickup image can arrive
    // after the second click has already been predicted locally.
    authoritative(&mut inventory, &content, 2, -1, true);
    assert!(inventory.player.slots[1].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);
    authoritative(&mut inventory, &content, 3, 1, false);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 1);
    assert!(inventory.cursor.as_ref().unwrap().empty());
}

#[test]
fn right_drag_whole_virtual_result_then_place_one() {
    let (content, mut inventory, mut ui) = fixture();
    let mut result = inventory.player.slots[9].as_ref().unwrap().clone();
    result.count = 4;
    inventory.player.slots[0] = Some(result);
    let down = pointer_button(&mut ui, &mut inventory, &content, PointerPhase::Down, 105., PointerButton::Right);
    assert_eq!(deliver(down, &mut inventory, &content), 1);
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 4);
    let up = pointer_button(&mut ui, &mut inventory, &content, PointerPhase::Up, 65., PointerButton::Right);
    assert_eq!(deliver(up, &mut inventory, &content), 1);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 1);
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 3);
}

#[test]
fn modified_click_does_not_issue_a_second_release_action() {
    for (button, shift) in [(PointerButton::Left, true), (PointerButton::Middle, false)] {
        let (content, mut inventory, mut ui) = fixture();
        inventory.cursor = inventory.player.slots[9].clone();
        let mut down = UiEffects::default();
        ui.execute_inner(UiIntent::Pointer {
            phase: PointerPhase::Down, x: 65., y: 25., button: Some(button),
            modifiers: Modifiers { shift, ..Modifiers::default() },
        }, &mut inventory, &content, &mut down).unwrap();
        assert_eq!(down.commands.len(), 1);
        pointer_button(&mut ui, &mut inventory, &content, PointerPhase::Move, 85., button);
        let up = pointer_button(&mut ui, &mut inventory, &content, PointerPhase::Up, 85., button);
        assert_eq!(deliver(up, &mut inventory, &content), 0);
    }
}

#[test]
fn direct_release_transfers_without_intermediate_move() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 65.);
    assert_eq!(deliver(up, &mut inventory, &content), 1);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 1);
    assert!(inventory.cursor.as_ref().unwrap().empty());
}

#[test]
fn initial_click_stays_on_cursor_but_release_outside_drops() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 25.);
    assert_eq!(deliver(up, &mut inventory, &content), 0);
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);

    // Start with a fresh pickup; releasing outside sends the normal outside
    // PICKUP packet, which is the server's authoritative drop path.
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 290.);
    assert_eq!(deliver(up, &mut inventory, &content), 1);
    assert!(inventory.cursor.as_ref().unwrap().empty());
}

#[test]
fn changed_authoritative_cursor_cancels_pending_release() {
    let (mut content, mut inventory, mut ui) = fixture();
    content.item_names.insert(2, "test:replacement".into());
    content.insert_test_item_components(2, BTreeMap::from([
        ("minecraft:max_stack_size".into(), json!(64)),
    ]));
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let mut wire = Writer::default();
    wire.var(1); wire.var(2); wire.var(0); wire.var(0);
    inventory.apply(crate::protocol::packet_ids::play::clientbound::SET_CURSOR_ITEM, &mut Reader::new(&wire.0), &content).unwrap();
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 65.);
    assert_eq!(deliver(up, &mut inventory, &content), 0);
    assert!(inventory.player.slots[1].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().id, 2);
}

#[test]
fn preheld_cursor_quick_crafts_over_traversed_slots_after_resync() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 25.);
    deliver(up, &mut inventory, &content);
    inventory.cursor.as_mut().unwrap().count = 4;
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 65.);
    assert_eq!(deliver(down, &mut inventory, &content), 0);
    pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, 65.);
    pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, 85.);
    // Unrelated authoritative update with the same carried stack is not a
    // reason to lose the gesture's screen identity or act on a replacement.
    let mut wire = Writer::default();
    wire.var(4); wire.var(1); wire.var(0); wire.var(0);
    inventory.apply(crate::protocol::packet_ids::play::clientbound::SET_CURSOR_ITEM, &mut Reader::new(&wire.0), &content).unwrap();
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 85.);
    assert_eq!(deliver(up, &mut inventory, &content), 4);
    assert_eq!(inventory.player.slots[1].as_ref().unwrap().count, 2);
    assert_eq!(inventory.player.slots[2].as_ref().unwrap().count, 2);
    assert!(inventory.cursor.as_ref().unwrap().empty());
}

#[test]
fn drag_to_output_keeps_carried_item() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, 105.);
    let up = pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, 105.);
    assert_eq!(deliver(up, &mut inventory, &content), 1);
    assert!(inventory.player.slots[0].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);
    authoritative(&mut inventory, &content, 2, -1, true);
    assert!(inventory.player.slots[0].as_ref().unwrap().empty());
    assert_eq!(inventory.cursor.as_ref().unwrap().count, 1);
}

#[test]
fn held_cursor_is_rendered_and_disappears_after_drop() {
    let (content, mut inventory, mut ui) = fixture();
    let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, 25.);
    deliver(down, &mut inventory, &content);
    ui.models.insert_test_model("test:ingredient", content.models[0].clone());
    ui.nodes.clear();
    ui.inventory_revision = inventory.revision;
    ui.dirty = false;
    let mut held = MeshGeometry::default();
    ui.render(&content, &inventory, &mut held).unwrap();
    assert!(!held.vertices.is_empty(), "held item must have visible geometry");
    inventory.cursor = Some(Stack::default());
    let mut empty = MeshGeometry::default();
    ui.render(&content, &inventory, &mut empty).unwrap();
    assert!(empty.vertices.is_empty(), "empty cursor must not draw an item");
}
