//! Project an observed spawner entity tag into the same presentation state used
//! for received world entities. Unsupported visual data is reported, not guessed.
use super::{Entity, Handle, Metadata, Models, Store, Transform, physical, presentation::variant_registry, render_special};
use crate::protocol::{Result, content::Content};
use serde_json::Value;

pub(super) struct Entry {
    pub(super) revision: u64,
    pub(super) entity: Result<Entity>,
}
use std::collections::BTreeMap;

fn boolean(value: &Value, field: &str) -> Result<bool> {
    value.as_bool().or_else(|| value.as_i64().and_then(|v| match v { 0 => Some(false), 1 => Some(true), _ => None }))
        .ok_or_else(|| format!("spawner entity {field} must be boolean"))
}

fn equipment_item(content: &Content, value: &Value) -> Result<Option<crate::inventory::Stack>> {
    if !value.is_null() {
        let fields = value.as_object().ok_or("spawner equipment item must be a compound")?;
        if let Some(key) = fields.keys().find(|key| !matches!(key.as_str(), "id" | "count" | "components")) {
            return Err(format!("unsupported spawner equipment item field: {key}"));
        }
        if fields.get("count").is_some_and(|count| count.as_i64().is_none()) {
            return Err("spawner equipment item count must be an integer".into());
        }
    }
    render_special::observed_stack(content, value)
}

fn metadata(entity: &mut Entity, models: &Models, name: &str, value: Metadata) -> Result<()> {
    let schema = models.schemas.get(&entity.type_name).ok_or("spawner entity schema unavailable")?;
    let [index, serializer] = *schema.accessors.get(name)
        .ok_or_else(|| format!("{} has no {name} accessor", entity.type_name))?;
    if serializer != value.serializer_id() {
        return Err(format!("{} {name} serializer mismatch", entity.type_name));
    }
    entity.metadata.insert(u8::try_from(index).map_err(|_| "spawner metadata index bound")?, value);
    Ok(())
}


pub(super) fn from_nbt(data: &Value, store: &Store, models: &Models, content: &Content, tick: u64) -> Result<Entity> {
    let fields = data.as_object().ok_or("spawner entity tag must be a compound")?;
    let name = fields.get("id").and_then(Value::as_str).ok_or("spawner entity id missing")?;
    if fields.contains_key("equipment") && (fields.contains_key("HandItems") || fields.contains_key("ArmorItems")) {
        return Err("conflicting spawner equipment formats".into());
    }
    if name == "minecraft:player" {
        return Err("spawner player profile preview is unsupported".into());
    }
    let kind = content.entity_names.iter().find_map(|(id, value)| (value == name).then_some(*id))
        .ok_or_else(|| format!("spawner entity type not registered: {name}"))?;
    let schema = models.schemas.get(name).ok_or_else(|| format!("unknown spawner display entity: {name}"))?;
    let transform = Transform { position: [0.; 3], yaw: 0., pitch: 0., head_yaw: 0. };
    let mut entity = Entity {
        handle: Handle { network_id: 0, generation: 0 }, uuid: String::new(), kind,
        type_name: name.to_owned(), position: [0.; 3], velocity: [0.; 3], transform, previous: transform,
        on_ground: false, spawn_data: 0, observed_tick: tick, revision: 0, appearance_revision: 0,
        physical: physical::AnimationState::default(), binding: None, source: None,
        minecart_steps: Vec::new(), acceleration_power: None, metadata: BTreeMap::new(),
        equipment: std::array::from_fn(|_| None), predicted_metadata: BTreeMap::new(),
        predicted_equipment: std::array::from_fn(|_| None), predicted_leash: None,
        effects: BTreeMap::new(), attributes: BTreeMap::new(), passenger_ids: Vec::new(),
        passengers: Vec::new(), vehicle: None, leash: None, leash_holder_id: None,
        animation: None, damage_tick: None, death_tick: None, hurt_direction: 0., submitted_revision: None,
        clip: Default::default(), crowd: None,
    };
    let mut baby = None;
    let mut sheep_color = 0i8;
    let mut sheep_sheared = false;
    let mut sheep_wool = false;
    for (key, value) in fields {
        match key.as_str() {
            "id" => {}
            "Age" => {
                let age = value.as_i64().ok_or("spawner entity Age must be an integer")?;
                let next = age < 0;
                if baby.replace(next).is_some_and(|old| old != next) { return Err("conflicting spawner baby fields".into()); }
            }
            "IsBaby" => {
                let next = boolean(value, key)?;
                if baby.replace(next).is_some_and(|old| old != next) { return Err("conflicting spawner baby fields".into()); }
            }
            "variant" | "Variant" => {
                let registry = variant_registry(name).ok_or_else(|| format!("unsupported spawner {name} variant"))?;
                let selected = value.as_str().ok_or("spawner variant must be a registry identifier")?;
                let entries = store.registries.get(registry).ok_or_else(|| format!("spawner variant registry unavailable: {registry}"))?;
                let id = entries.iter().position(|(entry, _)| entry == selected)
                    .ok_or_else(|| format!("spawner variant not registered: {selected}"))?;
                let [_, serializer] = *schema.accessors.get("DATA_VARIANT_ID").or_else(|| schema.accessors.get("DATA_VARIANT"))
                    .ok_or("spawner variant accessor unavailable")?;
                let accessor = if schema.accessors.contains_key("DATA_VARIANT_ID") { "DATA_VARIANT_ID" } else { "DATA_VARIANT" };
                metadata(&mut entity, models, accessor, Metadata::Registry { serializer, id: u32::try_from(id).map_err(|_| "spawner variant id bound")? })?;
            }
            "Color" if name == "minecraft:sheep" => {
                sheep_color = i8::try_from(value.as_i64().ok_or("sheep Color must be an integer")?).map_err(|_| "sheep Color bound")?;
                if !(0..=15).contains(&sheep_color) { return Err("sheep Color bound".into()); }
                sheep_wool = true;
            }
            "Sheared" if name == "minecraft:sheep" => { sheep_sheared = boolean(value, key)?; sheep_wool = true; }
            "Pose" => {
                let pose = value.as_str().ok_or("spawner Pose must be a name")?.to_ascii_uppercase();
                let id = models.poses.iter().find_map(|(id, name)| (name == &pose).then_some(*id))
                    .ok_or_else(|| format!("unknown spawner pose {pose}"))?;
                metadata(&mut entity, models, "DATA_POSE", Metadata::Pose(id))?;
            }
            "Rotation" => {
                let rotation = value.as_array().filter(|v| v.len() == 2).ok_or("spawner Rotation requires two numbers")?;
                let yaw = rotation[0].as_f64().ok_or("invalid spawner yaw")? as f32;
                let pitch = rotation[1].as_f64().ok_or("invalid spawner pitch")? as f32;
                if !yaw.is_finite() || !pitch.is_finite() { return Err("non-finite spawner rotation".into()); }
                entity.transform.yaw = yaw; entity.transform.pitch = pitch; entity.transform.head_yaw = yaw;
                entity.previous = entity.transform;
            }
            "HandItems" | "ArmorItems" => {
                let (base, length) = if key == "HandItems" { (0, 2) } else { (2, 4) };
                let items = value.as_array().filter(|items| items.len() == length).ok_or_else(|| format!("invalid {key} list"))?;
                for (slot, item) in items.iter().enumerate() {
                    entity.equipment[base + slot] = equipment_item(content, item)?;
                }
            }
            "equipment" => {
                let slots = value.as_object().ok_or("spawner equipment must be a compound")?;
                for (slot, item) in slots {
                    let index = match slot.as_str() { "mainhand" => 0, "offhand" => 1, "feet" => 2, "legs" => 3, "chest" => 4, "head" => 5, "body" => 6, "saddle" => 7, _ => return Err(format!("unsupported spawner equipment slot: {slot}")) };
                    entity.equipment[index] = equipment_item(content, item)?;
                }
            }
            // These fields affect AI/physics, not the snapshot rendered in the cage.
            "NoAI" | "Silent" | "NoGravity" | "PersistenceRequired" | "Invulnerable" => { boolean(value, key)?; }
            _ => return Err(format!("unsupported spawner entity field: {key}")),
        }
    }
    if let Some(baby) = baby { metadata(&mut entity, models, "DATA_BABY_ID", Metadata::Bool(baby))?; }
    if sheep_wool { metadata(&mut entity, models, "DATA_WOOL_ID", Metadata::Byte(sheep_color | if sheep_sheared { 16 } else { 0 }))?; }
    if let Some(selected) = fields.get("variant").or_else(|| fields.get("Variant")).and_then(Value::as_str) {
        let registry = variant_registry(name).ok_or("spawner variant registry unavailable")?;
        let (_, data) = store.registries.get(registry).and_then(|entries| entries.iter().find(|(key, _)| key == selected))
            .ok_or("spawner variant is not registered")?;
        let data = if data.is_null() {
            models.builtin_variants.get(registry).and_then(|values| values.get(selected))
                .ok_or("spawner variant data unavailable")?
        } else { data };
        if let Some(model) = data["model"].as_str().filter(|model| !matches!(*model, "normal" | "temperate")) {
            let key = format!("minecraft:{model}_{}{}#main", name.split(':').nth(1).unwrap_or(""),
                if baby.unwrap_or(false) { "_baby" } else { "" });
            if !models.has_authored_rig(&key) {
                return Err(format!("spawner variant rig not activated: {key}"));
            }
        }
    }
    Ok(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_age_variant_pose_and_equipment_and_rejects_unknown_fields() {
        let (mut models, _) = super::super::animation_tests::fixture();
        let schema = models.schemas.get_mut("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_BABY_ID".into(), [16, 8]);
        schema.accessors.insert("DATA_VARIANT_ID".into(), [19, 28]);
        schema.accessors.insert("DATA_POSE".into(), [6, 20]);
        models.poses.insert(5, "CROUCHING".into());
        let mut content = Content::test_cube();
        content.entity_names.insert(1, "minecraft:pig".into());
        content.item_names.insert(2, "test:hand_item".into());
        content.insert_test_item_components(2, BTreeMap::from([
            ("minecraft:max_stack_size".into(), json!(64)),
            ("minecraft:item_model".into(), json!("test:hand_item")),
        ]));
        let mut store = Store::new();
        store.configure_registry("minecraft:pig_variant", &[
            ("minecraft:temperate".into(), json!({"model":"temperate"})),
            ("minecraft:warm".into(), json!({"model":"temperate"})),
        ]).unwrap();
        let data = json!({
            "id":"minecraft:pig", "Age":-5, "variant":"minecraft:warm",
            "Pose":"crouching", "Rotation":[30.0, 10.0],
            "equipment":{"mainhand":{"id":"test:hand_item","count":1}}
        });
        let entity = from_nbt(&data, &store, &models, &content, 42).unwrap();
        assert!(matches!(entity.metadata.get(&16), Some(Metadata::Bool(true))));
        assert!(matches!(entity.metadata.get(&19), Some(Metadata::Registry { serializer:28, id:1 })));
        assert!(matches!(entity.metadata.get(&6), Some(Metadata::Pose(5))));
        assert_eq!(entity.equipment[0].as_ref().unwrap().id, 2);
        assert_eq!(entity.transform.yaw, 30.);
        assert_eq!(entity.transform.pitch, 10.);
        assert!(from_nbt(&json!({"id":"minecraft:pig","unknown_visual":1}), &store, &models, &content, 0)
            .unwrap_err().contains("unknown_visual"));
        assert!(from_nbt(&json!({"id":"minecraft:pig","Age":-1,"IsBaby":false}), &store, &models, &content, 0)
            .unwrap_err().contains("conflicting"));
        assert!(from_nbt(&json!({"id":"minecraft:missing"}), &store, &models, &content, 0)
            .unwrap_err().contains("not registered"));
    }
}
