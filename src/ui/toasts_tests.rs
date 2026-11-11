use super::*;
use serde_json::json;

fn advancement(frame: i32) -> Toast {
    Toast::advancement(Arc::new(json!({"text": "Stone Age"})), frame, None)
}

fn entry() -> RecipeEntry {
    RecipeEntry { station: None, result: None }
}

#[test]
fn instance_slides_in_on_the_quadratic_curve_holds_five_seconds_and_slides_out() {
    let mut toasts = ToastManager::default();
    toasts.add(advancement(0));
    // Queued toasts take a slot on the next update; the instance itself first
    // updates (and starts sliding) one update later.
    assert!(toasts.update(1_000));
    assert_eq!(toasts.visible[0].visible_portion, 0.);
    assert_eq!(toasts.visible[0].x(427.), 427.);
    assert!(toasts.sounds.is_empty());
    assert!(!toasts.update(1_100));
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in"]);
    assert!(!toasts.update(1_400));
    assert_eq!(toasts.visible[0].visible_portion, 0.25);
    assert_eq!(toasts.visible[0].x(427.), 387.);
    assert_eq!(toasts.visible[0].y(), 0.);
    toasts.update(1_700);
    assert_eq!(toasts.visible[0].visible_portion, 1.);
    assert_eq!(toasts.visible[0].x(427.), 267.);
    assert_eq!(toasts.visible[0].visible_for, 0);
    toasts.update(6_699);
    assert_eq!(toasts.visible[0].visible_for, 4_999);
    assert_eq!(toasts.sounds.len(), 1);
    // AdvancementToast.DISPLAY_TIME: hidden once fully visible for 5000 ms.
    toasts.update(6_700);
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in", "minecraft:ui.toast.out"]);
    assert_eq!(toasts.visible[0].visible_portion, 1.);
    toasts.update(7_000);
    assert_eq!(toasts.visible[0].visible_portion, 0.75);
    assert!(!toasts.update(7_300));
    assert_eq!(toasts.visible[0].visible_portion, 0.);
    assert!(toasts.update(7_301));
    assert!(toasts.is_idle());
}

#[test]
fn five_slots_fill_top_down_and_the_sixth_waits_for_the_first_free_slot() {
    let mut toasts = ToastManager::default();
    for frame in [0, 2, 0, 0, 0, 0] {
        toasts.add(advancement(frame));
    }
    toasts.update(0);
    let slots: Vec<_> = toasts.visible.iter().map(|t| t.first_slot).collect();
    assert_eq!(slots, [0, 1, 2, 3, 4]);
    assert_eq!(toasts.visible[1].y(), 32.);
    assert_eq!(toasts.visible[4].y(), 128.);
    assert_eq!(toasts.queued.len(), 1);
    toasts.update(100);
    // One transition sound per update, however many toasts changed.
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in"]);
    toasts.update(700);
    toasts.update(5_700);
    toasts.update(6_300);
    assert_eq!(toasts.queued.len(), 1);
    // All five finish together; the waiting toast takes the first free slot.
    assert!(toasts.update(6_301));
    assert_eq!(toasts.visible.len(), 1);
    assert_eq!(toasts.visible[0].first_slot, 0);
    assert_eq!(toasts.visible[0].serial, 6);
    assert!(toasts.queued.is_empty());
}

#[test]
fn challenge_plays_its_sound_once_when_it_takes_a_slot() {
    let mut toasts = ToastManager::default();
    toasts.add(advancement(CHALLENGE));
    toasts.add(advancement(CHALLENGE));
    toasts.add(advancement(2));
    toasts.update(0);
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.challenge_complete"]);
}

#[test]
fn recipe_entries_merge_into_one_toast_and_cycle_over_the_display_time() {
    let mut toasts = ToastManager::default();
    toasts.add_recipe(entry());
    toasts.add_recipe(entry());
    assert_eq!(toasts.queued.len(), 1);
    toasts.update(0);
    toasts.update(0);
    toasts.update(600);
    // A third unlock while on screen joins the visible toast and restarts its clock.
    toasts.add_recipe(entry());
    assert!(toasts.queued.is_empty());
    toasts.update(1_000);
    let displayed = |toasts: &ToastManager| match &toasts.visible[0].toast.kind {
        ToastKind::Recipe { displayed, entries, .. } => (*displayed, entries.len()),
        ToastKind::Advancement { .. } | ToastKind::System { .. } => unreachable!(),
    };
    assert_eq!(displayed(&toasts), (0, 3));
    toasts.update(2_667);
    assert_eq!(displayed(&toasts), (1, 3));
    toasts.update(4_334);
    assert_eq!(displayed(&toasts), (2, 3));
    toasts.update(5_999);
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in"]);
    // Hidden 5000 ms after the last change (at fully-visible time 400).
    toasts.update(6_000);
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in", "minecraft:ui.toast.out"]);
}

#[test]
fn unsecure_server_warning_takes_two_slots_and_hides_after_ten_seconds() {
    let title = Arc::new(json!({"translate": "multiplayer.unsecureserver.toast.title"}));
    let message = Arc::new(json!({"translate": "multiplayer.unsecureserver.toast"}));
    // One title line, three message lines of at most 200 pixels.
    let toast = Toast::system(title.clone(), Some(message.clone()), 1, 3, Some(197), 10_000.);
    assert_eq!((toast.width(), toast.height()), (227, 56));
    // Narrow text still keeps the 160-pixel minimum plus the 30-pixel margin.
    assert_eq!(Toast::system(title.clone(), None, 1, 0, Some(90), 10_000.).width(), 190);
    assert_eq!(Toast::system(title.clone(), None, 1, 0, None, 10_000.).width(), 230);
    assert_eq!(Toast::system(title, None, 2, 0, Some(90), 10_000.).height(), 44);
    let mut toasts = ToastManager::default();
    toasts.add(toast);
    toasts.add(advancement(0));
    toasts.update(0);
    // The 56-pixel toast occupies slots 0 and 1; the next one starts at slot 2.
    assert_eq!(toasts.visible[1].first_slot, 2);
    assert_eq!(toasts.visible[1].y(), 64.);
    toasts.update(100);
    toasts.update(700);
    // The advancement toast hides once fully visible for 5000 ms and is gone 600 ms later.
    toasts.update(5_700);
    assert!(toasts.update(6_301));
    assert_eq!(toasts.visible.len(), 1);
    toasts.update(10_699);
    assert_eq!(toasts.visible[0].visible_for, 9_999);
    assert_eq!(toasts.visible[0].visible_portion, 1.);
    assert_eq!(toasts.sounds, ["minecraft:ui.toast.in", "minecraft:ui.toast.out"]);
    // UNSECURE_SERVER_WARNING displays for 10000 ms.
    toasts.update(10_700);
    assert_eq!(toasts.sounds.len(), 3);
    assert_eq!(toasts.visible[0].x(427.), 427. - 227.);
    toasts.update(11_000);
    assert_eq!(toasts.visible[0].visible_portion, 0.75);
    toasts.update(11_301);
    assert!(toasts.is_idle());
}

#[test]
fn progress_map_order_matches_the_java_hash_map() {
    // Orders printed by a JDK HashMap of keys hashing like Identifier
    // (receipts/ClientEntityParity/java-oracle/Order.java, Order2.java).
    assert_eq!(
        progress_map_order(&[
            "minecraft:story/mine_stone",
            "minecraft:story/upgrade_tools",
            "minecraft:story/smelt_iron",
        ]),
        [
            "minecraft:story/upgrade_tools",
            "minecraft:story/mine_stone",
            "minecraft:story/smelt_iron",
        ]
    );
    assert_eq!(
        progress_map_order(&[
            "minecraft:adventure/kill_all_mobs",
            "minecraft:story/cure_zombie_villager",
            "minecraft:story/mine_stone",
            "minecraft:husbandry/froglights",
            "minecraft:nether/all_effects",
            "minecraft:end/respawn_dragon",
            "minecraft:adventure/adventuring_time",
            "minecraft:husbandry/balanced_diet",
            "minecraft:story/enter_the_nether",
            "minecraft:story/root",
            "minecraft:recipes/misc/crafting_table",
            "minecraft:adventure/crafters_crafting_crafters",
            "test:a",
            "test:b",
            "test:c",
            "test:d",
            "test:e",
            "test:a",
        ]),
        [
            "test:d",
            "test:e",
            "test:b",
            "test:c",
            "minecraft:adventure/kill_all_mobs",
            "minecraft:story/enter_the_nether",
            "minecraft:end/respawn_dragon",
            "minecraft:story/root",
            "minecraft:recipes/misc/crafting_table",
            "minecraft:husbandry/balanced_diet",
            "minecraft:story/mine_stone",
            "minecraft:nether/all_effects",
            "minecraft:adventure/crafters_crafting_crafters",
            "minecraft:story/cure_zombie_villager",
            "test:a",
            "minecraft:husbandry/froglights",
            "minecraft:adventure/adventuring_time",
        ]
    );
    // Nine equal hashes in one bin of a 16-slot table: treeifyBin doubles it.
    let colliding = [
        "t:AaAaAaAa", "t:AaAaAaBB", "t:AaAaBBAa", "t:AaAaBBBB", "t:AaBBAaAa",
        "t:AaBBAaBB", "t:AaBBBBAa", "t:AaBBBBBB", "t:BBAaAaAa",
    ];
    let mut ids = colliding.to_vec();
    ids.extend(["minecraft:story/mine_stone", "minecraft:story/upgrade_tools"]);
    let mut expected = colliding.to_vec();
    expected.extend(["minecraft:story/upgrade_tools", "minecraft:story/mine_stone"]);
    assert_eq!(progress_map_order(&ids), expected);
}
