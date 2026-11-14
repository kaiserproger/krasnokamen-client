//! Received humanoid armor uses activated, source-baked per-slot rigs, not item icons.
//! Contracts: local 26.1.2 ArmorStandRenderer, HumanoidArmorLayer,
//! ArmorStandArmorModel and EquipmentLayerRenderer javap (ignored QA receipts).
use super::{presentation as p, Entity, Models};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    inventory::Stack,
    protocol::{
        content::{Content, TextureRegion},
        Result,
    },
};
use glam::Mat4;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Asset {
    layers: BTreeMap<String, Vec<LayerDefinition>>,
}
#[derive(Deserialize)]
struct LayerDefinition {
    texture: String,
    dyeable: Option<Dyeable>,
    #[serde(default)]
    use_player_texture: bool,
}
#[derive(Deserialize)]
struct Dyeable {
    color_when_undyed: Option<i32>,
}
struct Layer {
    texture: TextureRegion,
    // None is an untinted layer; Some(None) draws only when the item is dyed.
    dyeable: Option<Option<i32>>,
    use_player_texture: bool,
}
impl Layer {
    fn color(&self, dyed: i32) -> Option<[f32; 3]> {
        let rgb = match self.dyeable {
            None => -1,
            Some(default) => {
                if dyed != 0 {
                    dyed
                } else {
                    default?
                }
            }
        };
        Some(std::array::from_fn(|i| {
            ((rgb >> (16 - 8 * i)) & 255) as f32 / 255.
        }))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    pub models: &'a Models,
    pub content: &'a Content,
    pub root: Mat4,
    pub tick: u64,
    pub partial: f32,
    pub light: [f32; 2],
}

type EquipmentAssets = BTreeMap<String, BTreeMap<String, Result<Option<Vec<Layer>>>>>;

#[derive(Default)]
pub(super) struct Cache {
    assets: EquipmentAssets,
    transforms: Vec<Mat4>,
}
impl Cache {
    fn activate(&mut self, content: &Content, asset: &str, kind: &str) {
        if self.assets.get(asset).is_some_and(|layers| layers.contains_key(kind)) {
            return;
        }
        if self.assets.len() >= 4096 {
            self.assets.clear();
        }
        self.assets.entry(asset.into()).or_default()
            .insert(kind.into(), load_asset(content, asset, kind));
    }
    fn wings_asset<'a>(&mut self, stack: &'a Stack, content: &Content) -> Result<Option<&'a str>> {
        let Some(asset) = armor_asset(stack, 4) else { return Ok(None) };
        self.activate(content, asset, "wings");
        Ok(self.assets[asset]["wings"].as_ref().map_err(Clone::clone)?
            .as_ref().filter(|layers| !layers.is_empty()).map(|_| asset))
    }

    pub(super) fn append_player_back(
        &mut self,
        e: &Entity,
        profile: Option<super::profiles::ProfileTextures>,
        frame: Frame<'_>,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let Frame { models, content, root, tick, partial, light } = frame;
        let wings = match e.equipment_value(4) {
            Some(stack) => self.wings_asset(stack, content)?,
            None => None,
        };
        let (key, cape) = if wings.is_some() {
            ("minecraft:elytra#main", None)
        } else if let Some(cape) = profile.and_then(|p| p.cape) {
            ("minecraft:cape#main", Some(cape))
        } else {
            return Ok(());
        };
        if !models.has_authored_rig(key) {
            return Err(format!("player back rig not activated: {key}"));
        }
        let rig = models.rig(key);
        p::evaluate_rig(rig, e, models, tick, partial, root, &mut self.transforms);
        let gliding = p::number(models, e, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 128 != 0;
        let living = e.source.as_ref().and_then(|s| s.living.as_ref());
        let sway = living.map_or(0., |s| {
            (s.walk.position(partial) * 0.7).sin() * s.walk.speed(partial).clamp(0., 1.) * 0.12
        });
        // Independently chosen visual motion, not vanilla animation constants.
        for (index, bone) in rig.bones.iter().enumerate() {
            let mut rotation = bone.rotation;
            match bone.name.as_str() {
                "wing_right" => rotation.z += if gliding { 0.95 } else { 0.12 },
                "wing_left" => rotation.z -= if gliding { 0.95 } else { 0.12 },
                "cape" => rotation.x += 0.12 + sway.abs(),
                _ => continue,
            }
            let parent = bone.parent.map_or(root, |i| self.transforms[i]);
            self.transforms[index] = parent
                * super::resources::transform(bone.pivot, rotation, bone.scale);
        }
        let style = |tex| p::RigStyle {
            tex, color: [1.; 3], layer: EntityMaterial::ArmorCutoutNoCull, alpha: 1., light,
        };
        if let Some(asset) = wings {
            let layers = self.assets[asset]["wings"].as_ref().map_err(Clone::clone)?
                .as_ref().ok_or("wings layer disappeared")?;
            let player_texture = profile.and_then(|p| p.elytra.or(p.cape));
            for layer in layers {
                let texture = if layer.use_player_texture {
                    player_texture.unwrap_or(layer.texture)
                } else {
                    layer.texture
                };
                p::emit_rig(rig, &self.transforms, style(texture), out)?;
            }
        } else if let Some(cape) = cape {
            p::emit_rig(rig, &self.transforms, style(cape), out)?;
        }
        Ok(())
    }


    pub(super) fn append(
        &mut self,
        e: &Entity,
        slot: usize,
        stack: &Stack,
        frame: Frame<'_>,
        out: &mut DynamicGeometry,
    ) -> Result<bool> {
        let Frame { models, content, root, tick, partial, light } = frame;
        let Some(asset) = armor_asset(stack, slot) else {
            return Ok(false);
        };
        // Player back layers have their own profile-aware draw hook.
        if e.type_name == "minecraft:player" && slot == 4
            && self.wings_asset(stack, content)?.is_some()
        {
            return Ok(true);
        }
        let stand = e.type_name == "minecraft:armor_stand";
        let baby = !stand && p::boolean(models, e, "DATA_BABY_ID");
        let overlay = animal_overlay(&e.type_name, slot);
        let animal = overlay.is_some();
        if animal && baby {
            return Err("baby animal equipment overlay is not available".into());
        }
        let (key, kind) = if let Some((key, kind)) = overlay {
            (Some(key), kind)
        } else {
            let key = if stand {
                // Small stands scale the whole adult rig about their feet, including equipment.
                armor_rig(slot)
            } else {
                humanoid_rig(models, e, slot, baby)
            };
            let kind = if baby {
                "humanoid_baby"
            } else if slot == 3 {
                "humanoid_leggings"
            } else {
                "humanoid"
            };
            (key, kind)
        };
        let Some(key) = key else {
            return Ok(false);
        };
        if animal && !models.has_authored_rig(key) {
            return Err(format!("animal equipment rig not activated: {key}"));
        }
        let rig = models.rig(key);
        self.activate(content, asset, kind);
        let Some(layers) = self.assets[asset][kind].as_ref().map_err(Clone::clone)? else {
            if animal {
                return Err(format!("equipment asset {asset} has no {kind} layer"));
            }
            // Equipment without this layer type is also omitted by HumanoidArmorLayer.
            return Ok(true);
        };
        p::evaluate_rig(rig, e, models, tick, partial, root, &mut self.transforms);
        let dyed = stack
            .component("minecraft:dyed_color")
            .and_then(serde_json::Value::as_i64)
            .map(|rgb| rgb as i32 | (0xff00_0000u32 as i32))
            .unwrap_or(0);
        let foil = stack
            .component("minecraft:enchantment_glint_override")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| {
                stack
                    .component("minecraft:enchantments")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|v| !v.is_empty())
            });
        let mut glint_pending = foil;
        for layer in layers {
            let Some(color) = layer.color(dyed) else {
                continue;
            };
            p::emit_rig(rig, &self.transforms, p::RigStyle { tex: layer.texture, color, layer: EntityMaterial::ArmorCutoutNoCull, alpha: 1., light }, out)?;
            if glint_pending {
                let texture = content
                    .texture("minecraft:misc/enchanted_glint_armor")
                    .ok_or("armor glint texture not activated")?;
                let start = out.vertices.len();
                p::emit_rig(rig, &self.transforms, p::RigStyle { tex: texture, color, layer: EntityMaterial::ArmorEntityGlint, alpha: 1., light }, out)?;
                // The wrapped material expects local coordinates and an atlas rectangle,
                // not the atlas-space UV and NO_OVERLAY values emitted by the rig.
                let size = [texture.uv[2] - texture.uv[0], texture.uv[3] - texture.uv[1]];
                let scroll = ((tick % 24000) as f32 + partial) / 600.;
                for vertex in &mut out.vertices[start..] {
                    vertex.uv = [
                        (vertex.uv[0] - texture.uv[0]) / size[0] + scroll,
                        (vertex.uv[1] - texture.uv[1]) / size[1],
                    ];
                    vertex.overlay_uv = [texture.uv[0], texture.uv[1]];
                    vertex.overlay_tint = [size[0], size[1], 0.];
                }
                glint_pending = false;
            }
        }
        // Source EquipmentLayerRenderer omits trim for the humanoid-baby layer.
        if !baby && !animal
            && let Some(trim) = stack.component("minecraft:trim")
        {
            let (texture, decal) = trim_texture(content, trim, asset, kind)?;
            p::emit_rig(rig, &self.transforms, p::RigStyle { tex: texture, color: [1.; 3], layer: if decal {
                EntityMaterial::ArmorDecalCutoutNoCull
            } else {
                EntityMaterial::ArmorCutoutNoCull
            }, alpha: 1., light }, out)?;
        }
        Ok(true)
    }
}

fn trim_holder<'a>(content: &'a Content, value: &'a serde_json::Value, registry: &str) -> Result<&'a serde_json::Value> {
    if let Some(id) = value.as_str() {
        content.registry_entries(registry)?
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, entry)| entry)
            .ok_or_else(|| format!("unknown {registry} holder {id}"))
    } else if value.is_object() {
        Ok(value)
    } else {
        Err(format!("invalid {registry} holder"))
    }
}

fn trim_texture(content: &Content, trim: &serde_json::Value, asset: &str, kind: &str) -> Result<(TextureRegion, bool)> {
    let material = trim_holder(content, &trim["material"], "minecraft:trim_material")?;
    let pattern = trim_holder(content, &trim["pattern"], "minecraft:trim_pattern")?;
    let material_name = material["override_armor_assets"][asset]
        .as_str()
        .or_else(|| material["asset_name"].as_str())
        .ok_or("trim material asset_name missing")?;
    let pattern_id = pattern["asset_id"].as_str().ok_or("trim pattern asset_id missing")?;
    let (namespace, path) = pattern_id.split_once(':').unwrap_or(("minecraft", pattern_id));
    let name = format!("{namespace}:trims/entity/{kind}/{path}_{material_name}");
    // Network registry NBT projects byte booleans to JSON integers; inline holders use bool.
    let decal = pattern["decal"].as_bool().or_else(|| match pattern["decal"].as_i64() {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }).ok_or("trim pattern decal missing or invalid")?;
    Ok((p::texture(content, &name)?, decal))
}

fn load_asset(content: &Content, asset: &str, kind: &str) -> Result<Option<Vec<Layer>>> {
    let (ns, path) = asset.split_once(':').unwrap_or(("minecraft", asset));
    let bytes = content.resource_bytes(&format!("assets/{ns}/equipment/{path}.json"))?;
    let mut asset: Asset = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let Some(definitions) = asset.layers.remove(kind) else {
        return Ok(None);
    };
    let mut activated = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let (ns, path) = definition
            .texture
            .split_once(':')
            .unwrap_or(("minecraft", &definition.texture));
        activated.push(Layer {
            texture: p::texture(content, &format!("{ns}:entity/equipment/{kind}/{path}"))?,
            dyeable: definition.dyeable.map(|v| v.color_when_undyed),
            use_player_texture: definition.use_player_texture,
        });
    }
    Ok(Some(activated))
}

fn humanoid_rig<'a>(models: &'a Models, e: &Entity, slot: usize, baby: bool) -> Option<&'a str> {
    let part = match slot {
        2 => "feet",
        3 => "legs",
        4 => "chest",
        5 => "head",
        _ => return None,
    };
    models.armor_rig(&e.type_name, baby, part)
}

pub(super) fn armor_asset(stack: &Stack, slot: usize) -> Option<&str> {
    let name = match slot {
        2 => "feet",
        3 => "legs",
        4 => "chest",
        5 => "head",
        6 => "body",
        7 => "saddle",
        _ => return None,
    };
    let equippable = stack.component("minecraft:equippable")?;
    (equippable["slot"].as_str()? == name).then_some(())?;
    equippable["asset_id"].as_str()
}
fn armor_rig(slot: usize) -> Option<&'static str> {
    Some(match slot {
        2 => "minecraft:armor_stand#boots",
        3 => "minecraft:armor_stand#leggings",
        4 => "minecraft:armor_stand#chestplate",
        5 => "minecraft:armor_stand#helmet",
        _ => return None,
    })
}

fn animal_overlay(species: &str, slot: usize) -> Option<(&'static str, &'static str)> {
    Some(match (species, slot) {
        ("minecraft:horse", 6) => ("minecraft:horse_body_equipment#main", "horse_body"),
        ("minecraft:wolf", 6) => ("minecraft:wolf_body_equipment#main", "wolf_body"),
        ("minecraft:llama" | "minecraft:trader_llama", 6) =>
            ("minecraft:llama_body_equipment#main", "llama_body"),
        ("minecraft:happy_ghast", 6) =>
            ("minecraft:happy_ghast_body_equipment#main", "happy_ghast_body"),
        ("minecraft:nautilus" | "minecraft:zombie_nautilus", 6) =>
            ("minecraft:nautilus_body_equipment#main", "nautilus_body"),
        ("minecraft:nautilus" | "minecraft:zombie_nautilus", 7) =>
            ("minecraft:nautilus_saddle_equipment#main", "nautilus_saddle"),
        ("minecraft:horse", 7) => ("minecraft:horse_saddle_equipment#main", "horse_saddle"),
        ("minecraft:pig", 7) => ("minecraft:pig_saddle_equipment#main", "pig_saddle"),
        ("minecraft:strider", 7) => ("minecraft:strider_saddle_equipment#main", "strider_saddle"),
        ("minecraft:camel", 7) => ("minecraft:camel_saddle_equipment#main", "camel_saddle"),
        ("minecraft:camel_husk", 7) => ("minecraft:camel_husk_saddle_equipment#main", "camel_husk_saddle"),
        ("minecraft:donkey", 7) => ("minecraft:donkey_saddle_equipment#main", "donkey_saddle"),
        ("minecraft:mule", 7) => ("minecraft:mule_saddle_equipment#main", "mule_saddle"),
        ("minecraft:skeleton_horse", 7) =>
            ("minecraft:skeleton_horse_saddle_equipment#main", "skeleton_horse_saddle"),
        ("minecraft:zombie_horse", 7) =>
            ("minecraft:zombie_horse_saddle_equipment#main", "zombie_horse_saddle"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn humanoid_fixture(kind: &str) -> (super::super::Store, Models, Content, Cache) {
        use super::super::resources::{ArmorModels, Bone, BoneRole, Dimensions, EntitySchema, Rig};
        use crate::protocol::{Reader, Writer};
        use glam::Vec3;

        let mut content = Content::test_cube();
        content.entity_names.insert(1, kind.into());
        content.insert_test_resource("assets/test/equipment/cloth.json", br#"{"layers":{}}"#);
        let slots = [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")];
        for (index, (_, name)) in slots.iter().enumerate() {
            let id = index as u32 + 1;
            content.item_names.insert(id, format!("test:{name}"));
            content.insert_test_item_components(
                id,
                BTreeMap::from([(
                    "minecraft:equippable".into(),
                    serde_json::json!({"slot":name,"asset_id":"test:cloth"}),
                )]),
            );
        }
        let mut store = super::super::Store::new();
        let mut spawn = Writer::default();
        spawn.var(4);
        spawn.0.extend([0u8; 16]);
        spawn.var(1);
        for _ in 0..3 {
            spawn.f64(0.);
        }
        spawn.0.extend([0u8; 5]);
        store
            .apply(1, &mut Reader::new(&spawn.0), &content, 1)
            .unwrap();
        let mut equipment = Writer::default();
        equipment.var(4);
        for (index, (slot, _)) in slots.iter().enumerate() {
            equipment.byte(*slot | if index + 1 < slots.len() { 0x80 } else { 0 });
            equipment.var(1);
            equipment.var(index as i32 + 1);
            equipment.var(0);
            equipment.var(0);
        }
        store
            .apply(104, &mut Reader::new(&equipment.0), &content, 2)
            .unwrap();
        let mut rigs = BTreeMap::new();
        let mut sets = BTreeMap::new();
        for prefix in ["adult", "baby"] {
            let mut parts = BTreeMap::new();
            for (_, part) in slots {
                let key = format!("test:{prefix}/{part}");
                let (name, role) = match part {
                    "head" => ("head", BoneRole::Head),
                    "chest" => ("right_arm", BoneRole::RightArm),
                    _ => ("right_leg", BoneRole::RightLeg),
                };
                rigs.insert(
                    key.clone(),
                    Rig {
                        bones: vec![Bone {
                            parent: None,
                            pivot: Vec3::ZERO,
                            rotation: Vec3::ZERO,
                            scale: Vec3::ONE,
                            role,
                            name: name.into(),
                            quads: vec![[
                                [0., 0., 0., 0., 0.],
                                [1., 0., 0., 1., 0.],
                                [1., 1., 0., 1., 1.],
                                [0., 1., 0., 0., 1.],
                            ]],
                            normals: vec![[0., 0., 1.]],
                        }],
                        part_by_name: BTreeMap::from([(name.into(), 0)]),
                    },
                );
                parts.insert(part.to_string(), key);
            }
            sets.insert(prefix, parts);
        }
        let schema = EntitySchema {
            protocol_id: 1,
            width: 0.6,
            height: 1.8,
            eye_height: 1.6,
            fire_immune: false,
            class: "test.Humanoid".into(),
            hierarchy: vec![],
            dimension_constants: BTreeMap::new(),
            enum_ids: BTreeMap::new(),
            enum_numbers: BTreeMap::new(),
            method_owners: BTreeMap::new(),
            dimensions: Dimensions {
                width: 0.6,
                height: 1.8,
                eye_height: 1.6,
                fixed: false,
                attachments: BTreeMap::new(),
            },
            default_attributes: BTreeMap::new(),
            accessors: BTreeMap::from([("DATA_BABY_ID".into(), [0, 8])]),
        };
        let models = Models {
            rigs,
            schemas: BTreeMap::from([(kind.into(), schema)]),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: vec![],
            default_skins: vec![],
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            armor_models: BTreeMap::from([(
                kind.into(),
                ArmorModels {
                    adult: sets.remove("adult").unwrap(),
                    baby: sets.remove("baby").unwrap(),
                },
            )]),
            motion: BTreeMap::new(),
        };
        let mut cache = Cache::default();
        cache.assets.insert(
            "test:cloth".into(),
            [
                ("humanoid", 2, 32),
                ("humanoid_leggings", 3, 32),
                ("humanoid_baby", 4, 64),
            ]
            .into_iter()
            .map(|(kind, layer, height)| {
                (
                    kind.into(),
                    Ok(Some(vec![Layer {
                        texture: TextureRegion {
                            layer,
                            uv: [0., 0., 1., 1.],
                            width: 64,
                            height,
                        },
                        dyeable: Some(Some(0x804020)),
                        use_player_texture: false,
                    }])),
                )
            })
            .collect(),
        );
        (store, models, content, cache)
    }

    include!("equipment_tests.rs");

    #[test]
    fn horse_received_body_and_saddle_layers_render_and_strip_independently() {
        use crate::protocol::{Reader, Writer};
        use serde_json::json;

        let (mut store, models, mut content, mut cache) = humanoid_fixture("minecraft:horse");
        for (id, slot, asset) in [(5, "body", "test:horse_armor"), (6, "saddle", "test:horse_saddle")] {
            content.item_names.insert(id, format!("test:{slot}"));
            content.insert_test_item_components(id, BTreeMap::from([(
                "minecraft:equippable".into(), json!({"slot":slot,"asset_id":asset}),
            )]));
        }
        content.insert_test_resource("assets/test/equipment/horse_armor.json",
            br#"{"layers":{"horse_body":[{"texture":"test:armor","dyeable":{"color_when_undyed":16777215}}]}}"#);
        content.insert_test_resource("assets/test/equipment/horse_saddle.json",
            br#"{"layers":{"horse_saddle":[{"texture":"test:saddle"}]}}"#);
        for (name, layer) in [("test:entity/equipment/horse_body/armor", 10), ("test:entity/equipment/horse_saddle/saddle", 11)] {
            content.insert_test_texture(name, TextureRegion { layer, uv:[0.,0.,1.,1.], width:64, height:64 });
        }
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor",
            TextureRegion { layer: 12, uv:[0.,0.,1.,1.], width:64, height:64 });
        let mut equipment = Writer::default();
        equipment.var(4);
        for (slot, id) in [(6, 5), (7, 6)] {
            equipment.byte(slot | if slot == 6 { 0x80 } else { 0 });
            equipment.var(1);
            equipment.var(id);
            equipment.var(0);
            equipment.var(0);
        }
        store.apply(crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT, &mut Reader::new(&equipment.0), &content, 2).unwrap();
        let entity = store.get(4).unwrap();
        for (slot, expected_layer) in [(6, 10), (7, 11)] {
            let mut out = DynamicGeometry::default();
            assert!(cache.append(entity, slot, entity.equipment_value(slot).unwrap(),
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut out).unwrap());
            assert!(!out.vertices.is_empty());
            assert!(out.vertices.iter().all(|v| v.texture_layer == expected_layer));
        }
        let mut dyed = entity.equipment_value(6).unwrap().clone();
        let components = Arc::make_mut(&mut dyed.components);
        components.insert("minecraft:dyed_color".into(), json!(0xff0000));
        components.insert("minecraft:enchantment_glint_override".into(), json!(true));
        let mut tinted = DynamicGeometry::default();
        assert!(cache.append(entity, 6, &dyed,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut tinted).unwrap());
        assert!(!tinted.indices[EntityMaterial::ArmorEntityGlint as usize].is_empty());
        assert!(tinted.vertices.iter().any(|v| v.texture_layer == 10 && v.color == [1., 0., 0.]));
        let mut invalid = entity.equipment_value(6).unwrap().clone();
        Arc::make_mut(&mut invalid.components).insert("minecraft:equippable".into(),
            json!({"slot":"body","asset_id":"test:missing"}));
        assert!(cache.append(entity, 6, &invalid,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut DynamicGeometry::default()).is_err());
        Arc::make_mut(&mut invalid.components).insert("minecraft:equippable".into(),
            json!({"slot":"body","asset_id":"test:horse_saddle"}));
        assert!(cache.append(entity, 6, &invalid,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut DynamicGeometry::default()).is_err());
        store.get_mut(4).unwrap().metadata.insert(0, super::super::Metadata::Bool(true));
        let baby = store.get(4).unwrap();
        assert!(cache.append(baby, 7, baby.equipment_value(7).unwrap(),
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut DynamicGeometry::default()).is_err());
        store.get_mut(4).unwrap().metadata.insert(0, super::super::Metadata::Bool(false));

        let mut stripped = Writer::default();
        stripped.var(4);
        stripped.byte(6);
        stripped.var(0);
        store.apply(crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT, &mut Reader::new(&stripped.0), &content, 3).unwrap();
        let entity = store.get(4).unwrap();
        assert!(entity.equipment_value(6).is_none());
        let mut saddle = DynamicGeometry::default();
        assert!(cache.append(entity, 7, entity.equipment_value(7).unwrap(),
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 3, partial: 0., light: [15.; 2] }, &mut saddle).unwrap());
        assert!(!saddle.vertices.is_empty());
        store.get_mut(4).unwrap().type_name = "minecraft:cow".into();
        let entity = store.get(4).unwrap();
        assert!(!cache.append(entity, 7, entity.equipment_value(7).unwrap(),
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 3, partial: 0., light: [15.; 2] }, &mut DynamicGeometry::default()).unwrap());
    }

    #[test]
    fn horse_overlay_joints_follow_authored_horse_pose() {
        use super::super::{animation_tests, render_living};
        let (models, mut entity) = animation_tests::fixture();
        entity.type_name = "minecraft:horse".into();
        let mut source = entity.source.take().unwrap();
        for x in [0.2, 0.4] {
            source.living.as_mut().unwrap().tick(&models, &entity, &render_living::TickInput {
                age: 1, position: [x, 0., 0.], yaw: 0., alive: true, passenger: false,
                baby: false, visually_swimming: false, in_water: false, uses_y_for_walk: false,
                rotation_changed_before_base_tick: false, is_mob: true,
                first_passenger_is_mob: false, max_head_yaw: 75., max_body_head_yaw: 75.,
                attack_active: false, camel_sitting: false, camel_visually_sitting: false,
                camel_sitting_down: false, camel_pose_transition: false, camel_pose_time: 0,
            });
        }
        entity.source = Some(source);
        for overlay_key in ["minecraft:horse_body_equipment#main", "minecraft:horse_saddle_equipment#main"] {
            let horse = models.rig("minecraft:horse#main");
            let overlay = models.rig(overlay_key);
            let mut base_transforms = Vec::new();
            let mut overlay_transforms = Vec::new();
            p::evaluate_rig(horse, &entity, &models, 2, 1., Mat4::IDENTITY, &mut base_transforms);
            p::evaluate_rig(overlay, &entity, &models, 2, 1., Mat4::IDENTITY, &mut overlay_transforms);
            for (index, bone) in overlay.bones.iter().enumerate() {
                let base = horse.part_by_name[&bone.name];
                assert_eq!(overlay_transforms[index], base_transforms[base], "joint {}", bone.name);
            }
        }
    }

    #[test]
    fn received_wolf_armor_emits_layers_and_strip_removes_them() {
        use crate::protocol::{Reader, Writer};
        use serde_json::json;

        let (mut store, models, mut content, mut cache) = humanoid_fixture("minecraft:wolf");
        for slot in 2..=5 {
            store.apply(crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT, &mut Reader::new(&[4, slot, 0]), &content, 3).unwrap();
        }
        content.item_names.insert(5, "test:wolf_armor".into());
        content.insert_test_item_components(5, BTreeMap::from([(
            "minecraft:equippable".into(), json!({"slot":"body","asset_id":"test:wolf_armor"}),
        )]));
        content.insert_test_resource("assets/test/equipment/wolf_armor.json", br#"{
            "layers":{"wolf_body":[{"texture":"test:base"},{"texture":"test:overlay","dyeable":{}}]}
        }"#);
        for (name, layer) in [("test:entity/equipment/wolf_body/base", 10),
                              ("test:entity/equipment/wolf_body/overlay", 11)] {
            content.insert_test_texture(name, TextureRegion {
                layer, uv: [0., 0., 1., 1.], width: 64, height: 32,
            });
        }
        let mut packet = Writer::default();
        packet.var(4);
        packet.byte(6);
        packet.var(1);
        packet.var(5);
        packet.var(0);
        packet.var(0);
        store.apply(crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT, &mut Reader::new(&packet.0), &content, 4).unwrap();
        let entity = store.get(4).unwrap();
        assert_eq!(entity.equipment[6].as_ref().unwrap().id, 5);
        let rig = models.rig("minecraft:wolf#main");
        let mut transforms = Vec::new();
        p::evaluate_rig(rig, entity, &models, 4, 0., Mat4::IDENTITY, &mut transforms);
        let mut out = DynamicGeometry::default();
        let mut stats = p::PresentationStats::default();
        let mut items = crate::ui::items::ItemModels::new();
        p::append_equipment(p::EquipmentPose { entity, rig, transforms: &transforms }, &mut items, &mut cache,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 4, partial: 0., light: [15.; 2] }, &mut out, &mut stats).unwrap();
        assert!(out.vertices.iter().any(|vertex| vertex.texture_layer == 10));
        assert!(!out.vertices.iter().any(|vertex| vertex.texture_layer == 11));
        assert!(stats.unrendered.is_empty());
        let mut dyed = entity.equipment_value(6).unwrap().clone();
        Arc::make_mut(&mut dyed.components).insert("minecraft:dyed_color".into(), json!(0xff0000));
        let mut colored = DynamicGeometry::default();
        assert!(cache.append(entity, 6, &dyed,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 4, partial: 0., light: [15.; 2] }, &mut colored).unwrap());
        assert!(colored.vertices.iter().any(|v| v.texture_layer == 11 && v.color == [1., 0., 0.]));

        store.apply(crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT, &mut Reader::new(&[4, 6, 0]), &content, 5).unwrap();
        let entity = store.get(4).unwrap();
        assert!(entity.equipment_value(6).is_none());
        let mut stripped = DynamicGeometry::default();
        let mut stats = p::PresentationStats::default();
        p::append_equipment(p::EquipmentPose { entity, rig, transforms: &transforms }, &mut items, &mut cache,
            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 5, partial: 0., light: [15.; 2] }, &mut stripped, &mut stats).unwrap();
        assert!(stripped.vertices.is_empty());
        assert!(stats.unrendered.is_empty());
    }

    #[test]
    fn worn_trim_uses_received_registry_assets_slot_and_baby_rules() {
        use serde_json::json;
        let (mut store, models, mut content, mut cache) = humanoid_fixture("minecraft:zombie");
        content.set_registries(BTreeMap::from([
            ("minecraft:trim_material".into(), vec![("test:metal".into(), json!({
                "asset_name":"gold", "override_armor_assets":{"test:cloth":"gold_darker"}
            }))]),
            ("minecraft:trim_pattern".into(), vec![("test:pattern".into(), json!({
                "asset_id":"minecraft:sentry", "decal":0
            }))]),
        ]));
        for (name, layer) in [
            ("minecraft:trims/entity/humanoid/sentry_gold_darker", 10),
            ("minecraft:trims/entity/humanoid_leggings/sentry_gold_darker", 11),
            ("minecraft:trims/entity/humanoid/sentry_gold", 12),
        ] {
            content.insert_test_texture(name, TextureRegion {
                layer, uv: [0., 0., 1., 1.], width: 64, height: 32,
            });
        }
        let entity = store.get(4).unwrap();
        for (slot, expected) in [(4, 10), (3, 11)] {
            let mut stack = entity.equipment_value(slot).unwrap().clone();
            Arc::make_mut(&mut stack.components).insert(
                "minecraft:trim".into(),
                json!({"material":"test:metal","pattern":"test:pattern"}),
            );
            let mut out = DynamicGeometry::default();
            assert!(cache.append(entity, slot, &stack,
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut out).unwrap());
            assert_eq!(out.vertices.len(), 8);
            assert!(out.vertices[4..].iter().all(|v| v.texture_layer == expected));
        }
        let material = json!({"asset_name":"gold"});
        let decal = json!({"asset_id":"minecraft:sentry","decal":true});
        let mut stack = entity.equipment_value(4).unwrap().clone();
        Arc::make_mut(&mut stack.components).insert(
            "minecraft:trim".into(), json!({"material":material,"pattern":decal}),
        );
        let mut out = DynamicGeometry::default();
        cache.append(entity, 4, &stack, crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut out).unwrap();
        assert_eq!(out.indices[EntityMaterial::ArmorDecalCutoutNoCull as usize].len(), 6);
        assert!(out.vertices[4..].iter().all(|v| v.texture_layer == 12));
        let byte_decal = json!({"material":{"asset_name":"gold"},
            "pattern":{"asset_id":"minecraft:sentry","decal":1}});
        assert!(trim_texture(&content, &byte_decal, "test:cloth", "humanoid").unwrap().1);
        let invalid_decal = json!({"material":{"asset_name":"gold"},
            "pattern":{"asset_id":"minecraft:sentry","decal":2}});
        assert!(trim_texture(&content, &invalid_decal, "test:cloth", "humanoid").is_err());
        store.get_mut(4).unwrap().metadata.insert(0, super::super::Metadata::Bool(true));
        let mut baby_out = DynamicGeometry::default();
        cache.append(store.get(4).unwrap(), 4, &stack, crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut baby_out).unwrap();
        assert_eq!(baby_out.vertices.len(), 4);
    }

    #[test]
    fn p1_humanoid_armor_received_slots_render_player_zombie_skeleton_and_piglin_families() {
        for kind in [
            "minecraft:player",
            "minecraft:zombie",
            "minecraft:husk",
            "minecraft:drowned",
            "minecraft:zombie_villager",
            "minecraft:skeleton",
            "minecraft:stray",
            "minecraft:bogged",
            "minecraft:parched",
            "minecraft:wither_skeleton",
            "minecraft:piglin",
            "minecraft:piglin_brute",
            "minecraft:zombified_piglin",
        ] {
            let (mut store, models, content, mut cache) = humanoid_fixture(kind);
            for (slot, part) in [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")] {
                let entity = store.get(4).unwrap();
                assert_eq!(
                    humanoid_rig(&models, entity, slot, false),
                    Some(format!("test:adult/{part}").as_str())
                );
                let mut out = DynamicGeometry::default();
                assert!(
                    cache
                        .append(
                            entity,
                            slot,
                            entity.equipment_value(slot).unwrap(),
                            Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [3., 7.] },
                            &mut out,
                        )
                        .unwrap(),
                    "{kind}:{part}"
                );
                assert_eq!(out.vertices.len(), 4, "{kind}:{part}");
                assert_eq!(
                    out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
                    6
                );
                assert!(out
                    .vertices
                    .iter()
                    .all(|v| v.texture_layer == if slot == 3 { 3 } else { 2 }));
                store
                    .apply(
                        104,
                        &mut crate::protocol::Reader::new(&[4, slot as u8, 0]),
                        &content,
                        3,
                    )
                    .unwrap();
                assert!(store.get(4).unwrap().equipment_value(slot).is_none());
            }
        }
    }

    #[test]
    fn p1_humanoid_armor_babies_use_source_baby_models_and_texture_for_all_slots() {
        for kind in [
            "minecraft:zombie",
            "minecraft:husk",
            "minecraft:drowned",
            "minecraft:zombie_villager",
            "minecraft:piglin",
            "minecraft:zombified_piglin",
        ] {
            let (mut store, models, content, mut cache) = humanoid_fixture(kind);
            store
                .get_mut(4)
                .unwrap()
                .metadata
                .insert(0, super::super::Metadata::Bool(true));
            let entity = store.get(4).unwrap();
            for (slot, part) in [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")] {
                assert_eq!(
                    humanoid_rig(&models, entity, slot, true),
                    Some(format!("test:baby/{part}").as_str())
                );
                let mut out = DynamicGeometry::default();
                assert!(cache
                    .append(
                        entity,
                        slot,
                        entity.equipment_value(slot).unwrap(),
                        Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                        &mut out,
                    )
                    .unwrap());
                assert_eq!(out.vertices.len(), 4);
                assert!(out.vertices.iter().all(|v| v.texture_layer == 4));
            }
        }
    }

    #[test]
    fn worn_glint_uses_its_atlas_rectangle_and_animated_coordinates() {
        let (store, models, mut content, mut cache) = humanoid_fixture("minecraft:zombie");
        let texture = TextureRegion {
            layer: 7,
            uv: [0.25, 0.5, 0.375, 0.75],
            width: 64,
            height: 64,
        };
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor", texture);
        let entity = store.get(4).unwrap();
        let mut stack = entity.equipment_value(4).unwrap().clone();
        Arc::make_mut(&mut stack.components).insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(true),
        );
        let mut first = DynamicGeometry::default();
        let mut later = DynamicGeometry::default();
        for (tick, out) in [(0, &mut first), (120, &mut later)] {
            cache.append(entity, 4, &stack, crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick, partial: 0., light: [15.; 2] }, out).unwrap();
        }
        let indices = &first.indices[EntityMaterial::ArmorEntityGlint as usize];
        assert_eq!(indices.len(), 6);
        let a = &first.vertices[indices[0] as usize];
        let b = &later.vertices[later.indices[EntityMaterial::ArmorEntityGlint as usize][0] as usize];
        assert_eq!(a.texture_layer, texture.layer);
        assert_eq!(a.overlay_uv, [0.25, 0.5]);
        assert_eq!(a.overlay_tint[..2], [0.125, 0.25]);
        assert_eq!(a.uv, [0., 0.]);
        assert_ne!(a.uv, b.uv);
        assert_eq!(a.position, first.vertices[0].position);

        Arc::make_mut(&mut stack.components).insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(false),
        );
        let mut disabled = DynamicGeometry::default();
        cache.append(entity, 4, &stack, crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 0, partial: 0., light: [15.; 2] }, &mut disabled).unwrap();
        assert!(disabled.indices[EntityMaterial::ArmorEntityGlint as usize].is_empty());
    }

    #[test]
    fn p1_humanoid_armor_black_dye_white_overlay_and_only_first_layer_glint() {
        let (store, models, mut content, mut cache) = humanoid_fixture("minecraft:player");
        let layers = cache
            .assets
            .get_mut("test:cloth")
            .unwrap()
            .get_mut("humanoid")
            .unwrap()
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap();
        layers.push(Layer {
            texture: layers[0].texture,
            dyeable: None,
            use_player_texture: false,
        });
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor", layers[0].texture);
        let entity = store.get(4).unwrap();
        let mut stack = entity.equipment_value(4).unwrap().clone();
        let components = Arc::make_mut(&mut stack.components);
        components.insert("minecraft:dyed_color".into(), serde_json::json!(0));
        components.insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(true),
        );
        let mut out = DynamicGeometry::default();
        assert!(cache
            .append(
                entity,
                4,
                &stack,
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                &mut out
            )
            .unwrap());
        assert_eq!(out.vertices.len(), 12);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            12
        );
        assert_eq!(
            out.indices[EntityMaterial::ArmorEntityGlint as usize].len(),
            6
        );
        assert!(out.vertices[..8].iter().all(|v| v.color[..3] == [0.; 3]));
        assert!(out.vertices[8..].iter().all(|v| v.color[..3] == [1.; 3]));
    }

    #[test]
    fn p1_humanoid_armor_uses_received_pose_and_preserves_nonarmor_fallbacks() {
        let (mut store, mut models, content, mut cache) = humanoid_fixture("minecraft:zombie");
        let entity = store.get(4).unwrap();
        let mut before = DynamicGeometry::default();
        cache.append(entity, 5, entity.equipment_value(5).unwrap(), crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut before)
            .unwrap();
        store.get_mut(4).unwrap().transform.pitch = 45.;
        let entity = store.get(4).unwrap();
        let mut after = DynamicGeometry::default();
        cache.append(entity, 5, entity.equipment_value(5).unwrap(), crate::entities::equipment::Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] }, &mut after)
            .unwrap();
        assert_ne!(before.vertices[2].position, after.vertices[2].position);
        let mut fallback = DynamicGeometry::default();
        assert!(!cache
            .append(
                entity,
                3,
                entity.equipment_value(4).unwrap(),
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                &mut fallback
            )
            .unwrap());
        models.armor_models.clear();
        assert!(!cache
            .append(
                entity,
                4,
                entity.equipment_value(4).unwrap(),
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                &mut fallback
            )
            .unwrap());
        assert!(fallback.vertices.is_empty());
    }

    #[test]
    fn p1_armor_layer_requires_matching_equippable_slot_and_asset() {
        let mut stack = Stack::default();
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:equippable".into(),
            serde_json::json!({"slot":"chest", "asset_id":"test:plate"}),
        )]));
        assert_eq!(armor_asset(&stack, 4), Some("test:plate"));
        for slot in [0, 1, 2, 3, 5, 6, 7] {
            assert_eq!(armor_asset(&stack, slot), None);
        }
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:equippable".into(),
            serde_json::json!({"slot":"head"}),
        )]));
        assert_eq!(armor_asset(&stack, 5), None);
    }

    #[test]
    fn p1_armor_layer_dye_and_undyed_overlay_colors() {
        let texture = TextureRegion {
            layer: 0,
            uv: [0., 0., 1., 1.],
            width: 64,
            height: 32,
        };
        let plain = Layer {
            texture,
            dyeable: None,
            use_player_texture: false,
        };
        assert_eq!(plain.color(0x123456), Some([1.; 3]));
        let leather = Layer {
            texture,
            dyeable: Some(Some(0x804020)),
            use_player_texture: false,
        };
        assert_eq!(
            leather.color(0),
            Some([128. / 255., 64. / 255., 32. / 255.])
        );
        assert_eq!(leather.color(0x00ff00), Some([0., 1., 0.]));
        let only_dyed = Layer {
            texture,
            dyeable: Some(None),
            use_player_texture: false,
        };
        assert_eq!(only_dyed.color(0), None);
        assert_eq!(only_dyed.color(0xff0000), Some([1., 0., 0.]));
    }

    #[test]
    fn p1_received_armor_stand_equipment_emits_armor_and_clears_on_removal() {
        use super::super::{
            resources::{Bone, BoneRole, Rig},
            Store,
        };
        use crate::protocol::{Reader, Writer};
        use glam::Vec3;
        let mut content = Content::test_cube();
        content
            .entity_names
            .insert(1, "minecraft:armor_stand".into());
        content.item_names.insert(1, "test:plate".into());
        content.insert_test_item_components(
            1,
            BTreeMap::from([(
                "minecraft:equippable".into(),
                serde_json::json!({"slot":"chest", "asset_id":"test:plate"}),
            )]),
        );
        let mut store = Store::new();
        let mut spawn = Writer::default();
        spawn.var(4);
        spawn.0.extend([0u8; 16]);
        spawn.var(1);
        for coordinate in [0., 0., 0.] {
            spawn.f64(coordinate);
        }
        spawn.0.extend([0u8; 5]);
        store
            .apply(1, &mut Reader::new(&spawn.0), &content, 1)
            .unwrap();
        // Local source javap: target VarInt, ordinal CHEST=4, count/item/empty patch.
        store
            .apply(104, &mut Reader::new(&[4, 4, 1, 1, 0, 0]), &content, 2)
            .unwrap();
        let observed = store.observation(None, 512);
        assert_eq!(observed["entities"][0]["network_id"], 4);
        assert_eq!(observed["entities"][0]["equipment"][4]["count"], 1);
        assert_eq!(observed["entities"][0]["origin"], "server_observed");
        let rig = Rig {
            bones: vec![Bone {
                parent: None,
                pivot: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::ONE,
                role: BoneRole::Fixed,
                name: "body".into(),
                quads: vec![[
                    [0., 0., 0., 0., 0.],
                    [1., 0., 0., 1., 0.],
                    [1., 1., 0., 1., 1.],
                    [0., 1., 0., 0., 1.],
                ]],
                normals: vec![[0., 0., 1.]],
            }],
            part_by_name: BTreeMap::new(),
        };
        let models = Models {
            rigs: BTreeMap::from([("minecraft:armor_stand#chestplate".into(), rig)]),
            schemas: BTreeMap::new(),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            armor_models: BTreeMap::new(),
            motion: BTreeMap::new(),
        };
        let texture = TextureRegion {
            layer: 0,
            uv: [0., 0., 1., 1.],
            width: 64,
            height: 32,
        };
        let mut cache = Cache::default();
        cache.assets.insert(
            "test:plate".into(),
            BTreeMap::from([(
                "humanoid".into(),
                Ok(Some(vec![Layer {
                    texture,
                    dyeable: None,
                    use_player_texture: false,
                }])),
            )]),
        );
        let entity = store.get(4).unwrap();
        let mut out = DynamicGeometry::default();
        assert!(cache
            .append(
                entity,
                4,
                entity.equipment_value(4).unwrap(),
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                &mut out
            )
            .unwrap());
        assert_eq!(out.vertices.len(), 4);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            6
        );
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor", texture);
        let mut glinted = entity.equipment_value(4).unwrap().clone();
        Arc::make_mut(&mut glinted.components).insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(true),
        );
        cache
            .assets
            .get_mut("test:plate")
            .unwrap()
            .get_mut("humanoid")
            .unwrap()
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .push(Layer {
                texture,
                dyeable: None,
                use_player_texture: false,
            });
        out = DynamicGeometry::default();
        assert!(cache
            .append(
                entity,
                4,
                &glinted,
                Frame { models: &models, content: &content, root: Mat4::IDENTITY, tick: 2, partial: 0., light: [15.; 2] },
                &mut out
            )
            .unwrap());
        assert_eq!(out.vertices.len(), 12);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            12
        );
        assert_eq!(
            out.indices[EntityMaterial::ArmorEntityGlint as usize].len(),
            6
        );
        store
            .apply(104, &mut Reader::new(&[4, 4, 0]), &content, 3)
            .unwrap();
        assert!(store.get(4).unwrap().equipment_value(4).is_none());
        store.equipment_cache = cache;
        store.activate_models(Arc::new(models));
        assert!(store.equipment_cache.assets.is_empty());
        assert!(store.equipment_cache.transforms.is_empty());
    }
}
