use super::{
    resources::{self, BoneRole, Rig},
    Entity, Metadata, Models, Store,
};
use crate::{
    api::{DynamicGeometry, EntityMaterial, RenderLayer, Vertex},
    inventory::Stack,
    protocol::{
        content::{Content, TextureRegion},
        models::{BakedModel, BakedQuad, TintKind},
        Result,
    },
};
use glam::{Mat4, Vec3};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_DYNAMIC_VERTICES: usize = 2_000_000;
#[derive(Debug)]
pub(crate) struct Binding {
    pub rig: String,
    pub texture: TextureRegion,
    pub style: RootStyle,
    pub(super) layers: Option<super::villagers::Layers>,
    pub(super) anger: Option<bool>,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum RootStyle {
    Living,
    Boat,
    Minecart,
    Arrow,
    Object,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct PresentationStats {
    pub state_revision: String,
    pub known: usize,
    pub eligible: usize,
    pub submitted: usize,
    pub animated: usize,
    pub block_entities: usize,
    pub vertices: usize,
    /// Disjoint extraction contributions; materials share GPU draw calls.
    pub entity_indices: usize,
    pub block_entity_indices: usize,
    pub lighting_pending: usize,
    pub profile_textures_pending: usize,
    pub unrendered: BTreeMap<String, String>,
}
pub(super) fn value<'a>(models: &Models, e: &'a Entity, name: &str) -> Option<&'a Metadata> {
    models
        .accessor(&e.type_name, name)
        .and_then(|i| e.metadata_value(i))
}
pub(super) fn number(models: &Models, e: &Entity, name: &str) -> Option<i32> {
    value(models, e, name).and_then(Metadata::int)
}
pub(super) fn boolean(models: &Models, e: &Entity, name: &str) -> bool {
    value(models, e, name)
        .and_then(Metadata::boolean)
        .unwrap_or(false)
}

fn anger(models: &Models, e: &Entity, game_time: i64) -> Option<bool> {
    if !matches!(e.type_name.as_str(), "minecraft:bee" | "minecraft:wolf") {
        return None;
    }
    let deadline = match value(models, e, "DATA_ANGER_END_TIME") {
        Some(Metadata::Long(value)) => value.parse::<i64>().ok(),
        _ => None,
    };
    Some(deadline.is_some_and(|end| end != -1 && end > game_time))
}

pub(super) fn binding_stale(models: &Models, e: &Entity, game_time: i64) -> bool {
    e.binding.as_ref().and_then(|binding| binding.as_ref().ok())
        .is_some_and(|binding| binding.anger != anger(models, e, game_time))
}

fn renderer_scale(e: &Entity, models: &Models, partial: f32) -> f32 {
    match e.type_name.as_str() {
        "minecraft:giant" => 6.,
        "minecraft:wither" => {
            let remaining = number(models, e, "DATA_ID_INV").unwrap_or(0);
            2. - if remaining > 0 { (remaining as f32 - partial) / 220. * 0.5 } else { 0. }
        }
        _ => 1.,
    }
}

fn scale_cube_model(matrix: Mat4, feet: Vec3, size: Option<i32>) -> Mat4 {
    let size = size.unwrap_or(1).max(1) as f32;
    Mat4::from_translation(feet)
        * Mat4::from_scale(Vec3::splat(size))
        * Mat4::from_translation(-feet)
        * matrix
}
pub(super) fn texture(content: &Content, name: &str) -> Result<TextureRegion> {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    let path = path.strip_prefix("textures/").unwrap_or(path);
    let path = path.strip_suffix(".png").unwrap_or(path);
    let key;
    let key = if namespace == "minecraft" {
        path
    } else {
        key = format!("{namespace}:{path}");
        &key
    };
    content
        .texture(key)
        .ok_or_else(|| format!("entity texture not activated: {name}"))
}
pub(super) fn sampled(
    light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    point: [f64; 3],
) -> Result<Option<[f32; 2]>> {
    let value = light(point)?;
    if value.is_some_and(|levels| {
        levels
            .iter()
            .any(|v| !v.is_finite() || !(0. ..=15.).contains(v))
    }) {
        return Err("invalid sampled entity lighting".into());
    }
    Ok(value)
}
pub(super) fn entity_light(
    e: &Entity,
    store: &Store,
    models: &Models,
    tick: u64,
    transform: super::Transform,
    dimensions: &resources::Dimensions,
    scale: f32,
    light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
) -> Result<Option<[f32; 2]>> {
    let t = transform;
    if e.type_name == "minecraft:armor_stand"
        && number(models, e, "DATA_CLIENT_FLAGS").unwrap_or(0) & 16 != 0
    {
        let schema = &models.schemas[&e.type_name];
        let small = number(models, e, "DATA_CLIENT_FLAGS").unwrap_or(0) & 1 != 0;
        let dims = if small {
            match schema.dimension_constants.get("BABY_DIMENSIONS") {
                Some(resources::DimensionConstant::Dimensions(d)) => d,
                _ => return Err("small armor stand dimensions missing".into()),
            }
        } else {
            &schema.dimensions
        };
        let t = store.presented_transform(e, tick, 0.)?;
        let half = f64::from(dims.width) * 0.5;
        let low = [
            (t.position[0] - half).floor() as i32,
            t.position[1].floor() as i32,
            (t.position[2] - half).floor() as i32,
        ];
        let high = [
            (t.position[0] + half).floor() as i32,
            (t.position[1] + f64::from(dims.height)).floor() as i32,
            (t.position[2] + half).floor() as i32,
        ];
        let mut best = None;
        let mut brightness = -1.;
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let Some(levels) = sampled(
                        light,
                        [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5],
                    )?
                    else {
                        return Ok(None);
                    };
                    let current = levels[0].max(levels[1]);
                    if current > brightness {
                        brightness = current;
                        best = Some(levels);
                    }
                    if current == 15. {
                        return Ok(Some(levels));
                    }
                }
            }
        }
        return Ok(best);
    }
    sampled(
        light,
        [
            t.position[0],
            t.position[1] + f64::from(dimensions.eye_height * scale),
            t.position[2],
        ],
    )
}
pub(super) fn variant_registry(name: &str) -> Option<&'static str> {
    Some(match name {
        "minecraft:cat" => "minecraft:cat_variant",
        "minecraft:cow" => "minecraft:cow_variant",
        "minecraft:chicken" => "minecraft:chicken_variant",
        "minecraft:pig" => "minecraft:pig_variant",
        "minecraft:frog" => "minecraft:frog_variant",
        "minecraft:wolf" => "minecraft:wolf_variant",
        "minecraft:zombie_nautilus" => "minecraft:zombie_nautilus_variant",
        _ => return None,
    })
}

fn variant<'a>(store: &'a Store, models: &'a Models, e: &Entity) -> Option<&'a Value> {
    let registry = variant_registry(&e.type_name)?;
    let entries = store.registries.get(registry)?;
    let received = value(models, e, "DATA_VARIANT_ID").or_else(|| value(models, e, "DATA_VARIANT"));
    let (name, data) = match received {
        Some(holder) => entries.get(usize::try_from(holder.int()?).ok()?)?,
        // These entities define their synchronized default as temperate-or-any.
        // Unchanged synchronized defaults need not be sent with entity pairing.
        // Resolve that default only inside the received registry, never by a
        // guessed numeric ID or an unadvertised built-in holder.
        None if matches!(
            e.type_name.as_str(),
            "minecraft:chicken" | "minecraft:cow" | "minecraft:pig" | "minecraft:zombie_nautilus"
        ) => entries
            .iter()
            .find(|(name, _)| name == "minecraft:temperate")
            .or_else(|| entries.first())?,
        None => return None,
    };
    if data.is_null() {
        models.builtin_variants.get(registry)?.get(name)
    } else {
        Some(data)
    }
}
pub(super) fn material(
    store: &Store,
    models: &Models,
    e: &Entity,
    content: &Content,
    game_time: i64,
) -> Result<Binding> {
    if e.type_name == "minecraft:player" {
        let default = models.default_skin(Some(&e.uuid))?;
        let profile = store
            .profiles
            .get(&e.uuid)
            .and_then(|profile| profile.textures);
        let model = profile.and_then(|p| p.model).unwrap_or(default.model);
        return Ok(Binding {
            rig: if model == super::PlayerModel::Slim {
                "minecraft:player_slim#main"
            } else {
                "minecraft:player#main"
            }
            .into(),
            texture: match profile.and_then(|p| p.skin) {
                Some(region) => region,
                None => texture(content, &default.texture)?,
            },
            style: RootStyle::Living,
            layers: None,
            anger: None,
        });
    }
    let family = models
        .families
        .get(&e.type_name)
        .ok_or_else(|| format!("no activated rig family for {}", e.type_name))?;
    let baby = boolean(models, e, "DATA_BABY_ID");
    let mut rig = if baby {
        family.baby_rig.as_ref().unwrap_or(&family.rig)
    } else {
        &family.rig
    }
    .clone();
    match e.type_name.as_str() {
        "minecraft:pufferfish" => {
            rig = match number(models, e, "PUFF_STATE").unwrap_or(0) {
                0 => "minecraft:pufferfish_small#main",
                1 => "minecraft:pufferfish_medium#main",
                _ => "minecraft:pufferfish#main",
            }.into();
        }
        "minecraft:tropical_fish" => {
            // Pattern.packedId stores Base in the low byte (the exported enum facts).
            if number(models, e, "DATA_ID_TYPE_VARIANT").unwrap_or(0) & 255 == 1 {
                rig = "minecraft:tropical_fish_large#main".into();
            }
        }
        _ => {}
    }
    let mut selected = if e.type_name == "minecraft:ender_dragon" {
        family
            .materials
            .get("DRAGON_TEXTURE_LOCATION")
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        family.texture.clone()
    };
    if e.type_name == "minecraft:tropical_fish" {
        let key = if number(models, e, "DATA_ID_TYPE_VARIANT").unwrap_or(0) & 255 == 1 {
            "LARGE_TEXTURE"
        } else { "SMALL_TEXTURE" };
        selected = family.materials.get(key).and_then(Value::as_str).map(str::to_owned);
    }
    if let Some(data) = variant(store, models, e) {
        let model = data["model"].as_str();
        if let Some(model) = model && model != "normal" && model != "temperate" {
            let candidate = format!(
                "minecraft:{model}_{}{}#main",
                e.type_name.split(':').nth(1).unwrap_or(""),
                if baby { "_baby" } else { "" }
            );
            if models.has_authored_rig(&candidate) {
                rig = candidate;
            }
        }
        let asset = if baby {
            data["baby_asset_id"].as_str().or(data["asset_id"].as_str())
        } else {
            data["asset_id"].as_str()
        };
        if let Some(asset) = asset {
            selected = Some(asset.to_owned());
        }
        if data["assets"].is_object() {
            let tame = number(models, e, "DATA_FLAGS_ID").unwrap_or(0) & 4 != 0;
            let angry = anger(models, e, game_time).unwrap_or(false);
            let key = if tame {
                "tame"
            } else if angry {
                "angry"
            } else {
                "wild"
            };
            let object = if baby && data["baby_assets"].is_object() {
                &data["baby_assets"]
            } else {
                &data["assets"]
            };
            selected = object[key].as_str().map(str::to_owned);
        }
    }
    if baby
        && let Some(base) = selected.as_ref()
        && let Some(candidate) = family.textures.iter().find(|t| {
            t.ends_with(&format!(
                "{}_baby.png",
                base.rsplit('/')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(".png")
            ))
        })
    {
        selected = Some(candidate.clone());
    }
    if e.type_name == "minecraft:mooshroom" {
        let variant = match number(models, e, "DATA_TYPE").unwrap_or(0) {
            0 => "RED",
            1 => "BROWN",
            other => return Err(format!("unknown mooshroom variant {other}")),
        };
        selected = family.materials["TEXTURES"][variant][if baby { "baby" } else { "adult" }]
            .as_str()
            .map(str::to_owned);
    }
    if e.type_name == "minecraft:sniffer" && baby {
        selected = family.materials.get("SNIFFLET_LOCATION")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    // State-specific materials are selected only from the renderer's exported literal inventory.
    if e.type_name == "minecraft:bee" {
        let flags = number(models, e, "DATA_FLAGS_ID").unwrap_or(0);
        let angry = anger(models, e, game_time).unwrap_or(false);
        let suffix = format!(
            "bee{}{}{}.png",
            if angry { "_angry" } else { "" },
            if flags & 8 != 0 { "_nectar" } else { "" },
            if baby { "_baby" } else { "" }
        );
        selected = family
            .textures
            .iter()
            .find(|t| t.ends_with(&suffix))
            .cloned();
    }
    if e.type_name == "minecraft:ghast" && boolean(models, e, "DATA_IS_CHARGING") {
        selected = family
            .textures
            .iter()
            .find(|t| t.ends_with("ghast_shooting.png"))
            .cloned();
    }

    let style = if e.type_name.ends_with("_boat") || e.type_name.ends_with("_raft") {
        RootStyle::Boat
    } else if e.type_name.ends_with("minecart") {
        RootStyle::Minecart
    } else if matches!(
        e.type_name.as_str(),
        "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:trident"
    ) {
        RootStyle::Arrow
    } else if matches!(
        e.type_name.as_str(),
        "minecraft:end_crystal"
            | "minecraft:leash_knot"
            | "minecraft:shulker_bullet"
            | "minecraft:llama_spit"
            | "minecraft:wither_skull"
            | "minecraft:wind_charge"
            | "minecraft:breeze_wind_charge"
    ) {
        RootStyle::Object
    } else {
        RootStyle::Living
    };
    let resource = selected
        .ok_or_else(|| format!("{} variant material not yet received/resolved", e.type_name))?;
    let texture = texture(content, &resource)?;
    Ok(Binding {
        rig,
        texture,
        style,
        layers: super::villagers::layers(store, models, e, content)?,
        anger: anger(models, e, game_time),
    })
}
// All four Solaris-authored boat/raft rigs share a hull bottom at model Y=18
// pixels. Anchor that surface to the received entity position, not to water.
fn boat_root(position: Vec3, yaw: f32) -> Mat4 {
    Mat4::from_translation(position + Vec3::new(0., 18. / 16., 0.))
        * Mat4::from_rotation_y((180. - yaw).to_radians())
        * Mat4::from_scale(Vec3::new(-1., -1., 1.))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2)
}
fn projectile_root(position: Vec3, yaw: f32, pitch: f32, trident: bool) -> Mat4 {
    let root = Mat4::from_translation(position)
        * Mat4::from_rotation_y((yaw - 90.).to_radians())
        * Mat4::from_rotation_z(pitch.to_radians());
    // The arrow tip is along +X; the authored trident prongs are along -Y.
    if trident { root * Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2) } else { root }
}


pub(super) struct DrawStore<'a> {
    pub(super) transforms: &'a mut Vec<Mat4>,
    pub(super) items: &'a mut crate::ui::items::ItemModels,
    pub(super) equipment_cache: &'a mut super::equipment::Cache,
    pub(super) profiles: &'a BTreeMap<String, super::profiles::PlayerProfile>,
    pub(super) snow_pumpkin_state: &'a mut Option<u32>,
    pub(super) mooshroom_mushroom_states: &'a mut [Option<u32>; 2],
    pub(super) clock_context: Option<&'a crate::ui::items::ItemContext>,
    pub(super) bed_direction: &'a dyn Fn([i32; 3]) -> Option<[i32; 2]>,
}

#[derive(Clone, Copy)]
pub(super) struct EntityDrawFrame {
    pub origin: [f64; 3],
    pub transform: super::Transform,
    pub tick: u64,
    pub partial: f32,
    pub light: [f32; 2],
    pub parent: Option<Mat4>,
}

struct OrbFrame<'a> {
    origin: [f64; 3],
    camera_rotation: [f32; 2],
    tick: u64,
    partial: f32,
    transform: super::Transform,
    light_source: &'a dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
}

// The sprite and value tiers are specified by the 26.1.2 orb renderer.
fn append_experience_orb(
    entity: &Entity,
    store: &Store,
    models: &Models,
    content: &Content,
    frame: OrbFrame<'_>,
    out: &mut DynamicGeometry,
) -> Result<bool> {
    let OrbFrame { origin, camera_rotation, tick, partial, transform, light_source } = frame;
    let (dimensions, scale) = models.dimensions(entity)?;
    let Some(mut light) = entity_light(entity, store, models, tick, transform, dimensions, scale, light_source)? else {
        return Ok(false);
    };
    light[1] = (light[1] + 7.).min(15.);
    let texture = texture(content, "entity/experience/experience_orb")?;
    let value = number(models, entity, "DATA_VALUE").unwrap_or(0);
    let icon = [3, 7, 17, 37, 73, 149, 307, 617, 1237, 2477]
        .into_iter().filter(|threshold| value >= *threshold).count();
    let u = (icon % 4) as f32 * 0.25;
    let v = (icon / 4) as f32 * 0.25;
    let uv = [[u, v + 0.25], [u + 0.25, v + 0.25], [u + 0.25, v], [u, v]]
        .map(|p| [
            texture.uv[0] + p[0] * (texture.uv[2] - texture.uv[0]),
            texture.uv[1] + p[1] * (texture.uv[3] - texture.uv[1]),
        ]);
    let [yaw, pitch] = camera_rotation;
    let rotation = glam::Quat::from_euler(
        glam::EulerRot::YXZ, -(yaw - 180.).to_radians(), -pitch.to_radians(), 0.,
    );
    let position = Vec3::from_array(std::array::from_fn(|i| (transform.position[i] - origin[i]) as f32));
    let matrix = Mat4::from_translation(position + Vec3::Y * 0.1)
        * Mat4::from_quat(rotation) * Mat4::from_scale(Vec3::splat(0.3));
    let positions = [[-0.5, -0.25, 0.], [0.5, -0.25, 0.], [0.5, 0.75, 0.], [-0.5, 0.75, 0.]]
        .map(|p| matrix.transform_point3(Vec3::from_array(p)).to_array());
    let age = entity.source.as_ref().map_or(0., |source| source.age as f32) + partial;
    let phase = age * 0.5;
    let color = [
        ((crate::player::math::sin(f64::from(phase)) + 1.) * 0.5 * 255.) as u8 as f32 / 255.,
        1.,
        ((crate::player::math::sin(f64::from(phase + 4.1887903)) + 1.) * 0.1 * 255.) as u8 as f32 / 255.,
    ];
    let first = out.vertices.len();
    emit(positions, uv, color, texture.layer, EntityMaterial::EntityTranslucentCullItemTarget,
        128. / 255., light, out)?;
    let normal = (rotation * Vec3::Y).to_array();
    for vertex in &mut out.vertices[first..] {
        vertex.normal = normal;
    }
    Ok(true)
}

/// Shared post-draw layer: special renderers and ordinary rigs obey the same fire flag.
fn append_entity_fire(
    entity: &Entity,
    models: &Models,
    content: &Content,
    origin: [f64; 3],
    camera_yaw: f32,
    transform: super::Transform,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let flags = number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0);
    if flags & 1 == 0 || flags & 32 != 0 || entity.type_name == "minecraft:falling_block" {
        return Ok(());
    }
    let (dimensions, scale) = models.dimensions(entity)?;
    let position = transform.position;
    let center = Vec3::from_array(std::array::from_fn(|i| (position[i] - origin[i]) as f32));
    let facing = Mat4::from_rotation_y((180. - camera_yaw).to_radians());
    let width = dimensions.width * scale * 1.4;
    let height = dimensions.height * scale;
    let textures = [texture(content, "block/fire_0")?, texture(content, "block/fire_1")?];
    let mut y = 0.;
    let mut half = width * 0.5;
    let mut row = 0;
    while y < height && half > 0.001 {
        let tex = textures[row % 2];
        let [u0, v0, u1, v1] = tex.uv;
        let z = width * 0.5 + 0.02 + row as f32 * 0.001;
        let positions = [
            Vec3::new(-half, y, z), Vec3::new(half, y, z),
            Vec3::new(half, y + width, z), Vec3::new(-half, y + width, z),
        ].map(|p| (center + facing.transform_vector3(p)).to_array());
        emit(positions, [[u0, v1], [u1, v1], [u1, v0], [u0, v0]], [1.; 3],
            tex.layer, EntityMaterial::EntityFire, 1., [15., 15.], out)?;
        y += width * 0.45;
        half *= 0.9;
        row += 1;
    }
    Ok(())
}

pub fn append(
    store: &mut Store,
    frame: super::EntityRenderFrame<'_>,
    out: &mut DynamicGeometry,
) -> Result<PresentationStats> {
    let super::EntityRenderFrame {
        content, origin, camera, camera_rotation, radius, tick, partial, game_time,
        clock_environment, maps, tint_source, light: light_source, bed_direction,
    } = frame;
    if !radius.is_finite() || radius <= 0. || radius > 4096. {
        return Err("entity presentation radius bound".into());
    }
    let models = store
        .models
        .clone()
        .ok_or("entity model cache not activated")?;
    let mut stats = PresentationStats {
        state_revision: store.revision.to_string(),
        known: store.len(),
        ..Default::default()
    };
    store.frame_partial = partial.clamp(0., 1.);
    store.frame_tick = tick;
    // Mounted bodies can occupy a different raw bucket from their seat.
    let area = crate::player::Bounds {
        min: camera.map(|value| value - f64::from(radius)),
        max: camera.map(|value| value + f64::from(radius)),
    };
    let mut visible = std::mem::take(&mut store.visible_ids);
    visible.clear();
    visible.extend(store.candidates(area).map(|entity| entity.handle.network_id));
    for index in 0..visible.len() + usize::from(store.local_render_slot.is_some()) {
        let slot = if index == visible.len() {
            store.local_render_slot.unwrap()
        } else {
            store.ids[&visible[index]]
        };
        let entity = &store.slots[slot];
        let transform = match store.presented_transform(entity, tick, partial) {
            Ok(transform) => transform,
            Err(reason) => {
                stats.unrendered.insert(entity.type_name.clone(), reason);
                continue;
            }
        };
        let distance = transform.position
            .iter()
            .zip(camera)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>();
        if distance > f64::from(radius) * f64::from(radius) {
            continue;
        }
        stats.eligible += 1;
        // Text displays are world text drawn by the nameplate pass.
        if matches!(
            entity.type_name.as_str(),
            "minecraft:marker" | "minecraft:interaction" | "minecraft:text_display"
        ) {
            continue;
        }
        if entity.type_name == "minecraft:experience_orb" {
            match append_experience_orb(entity, store, &models, content, OrbFrame { origin, camera_rotation, tick, partial, transform, light_source }, out) {
                Ok(true) => {
                    append_entity_fire(entity, &models, content, origin, camera_rotation[0], transform, out)?;
                    stats.animated += 1;
                    stats.submitted += 1;
                    store.slots[slot].submitted_revision = Some(store.revision);
                }
                Ok(false) => stats.lighting_pending += 1,
                Err(reason) => { stats.unrendered.insert(entity.type_name.clone(), reason); }
            }
            continue;
        }
        let item = value(&models, entity, "DATA_ITEM")
            .or_else(|| value(&models, entity, "DATA_ITEM_STACK_ID"));
        let stack = match item { Some(Metadata::Item(stack)) => Some(stack), _ => None };
        let stacks = [stack.or_else(|| entity.equipment_value(0)),
            entity.equipment_value(1), entity.equipment_value(5)];
        let clock_context = match store.special_render_cache.item_context(
            entity, stacks, content, transform.position, clock_environment,
        ) {
            Ok(context) => context,
            Err(reason) => {
                stats.unrendered.insert(entity.type_name.clone(), format!("item context: {reason}"));
                continue;
            }
        };
        let start = out.vertices.len();
        match super::render_special::append(store, slot, &models, content, origin,
            tick, partial, transform, clock_context.as_ref(), maps, light_source, tint_source, out) {
            Ok(super::render_special::Handled::Unhandled) => {}
            Ok(handled @ (super::render_special::Handled::Rendered | super::render_special::Handled::NoGeometry)) => {
                if matches!(handled, super::render_special::Handled::Rendered) {
                    append_entity_fire(&store.slots[slot], &models, content, origin, camera_rotation[0], transform, out)?;
                }
                if out.vertices.len() > start {
                    stats.submitted += 1;
                    store.slots[slot].submitted_revision = Some(store.revision);
                }
                continue;
            }
            Ok(super::render_special::Handled::PendingLighting) => {
                stats.lighting_pending += 1;
                continue;
            }
            Ok(super::render_special::Handled::PendingWorld) => continue,
            Err(reason) => {
                stats.unrendered.insert(store.slots[slot].type_name.clone(), reason);
                continue;
            }
        }
        let entity = &store.slots[slot];
        let (dimensions, scale) = match models.dimensions(entity) {
            Ok(value) => value,
            Err(reason) => {
                stats.unrendered.insert(entity.type_name.clone(), reason);
                continue;
            }
        };
        let Some(light) = entity_light(
            entity,
            store,
            &models,
            tick,
            transform,
            dimensions,
            scale,
            light_source,
        )?
        else {
            stats.lighting_pending += 1;
            continue;
        };
        if entity.type_name == "minecraft:player"
            && store.profiles.get(&entity.uuid).is_some_and(|p| {
                p.textures.is_none() && p.properties.iter().any(|p| p.name == "textures")
            })
        {
            stats.profile_textures_pending += 1;
        }
        if binding_stale(&models, &store.slots[slot], game_time) {
            store.slots[slot].binding = None;
        }
        if store.slots[slot].binding.is_none() {
            let binding = material(store, &models, &store.slots[slot], content, game_time);
            store.slots[slot].binding = Some(binding);
        }
        let entity = &store.slots[slot];
        let mut draw_store = DrawStore {
            transforms: &mut store.transforms,
            items: &mut store.items,
            equipment_cache: &mut store.equipment_cache,
            profiles: &store.profiles,
            snow_pumpkin_state: &mut store.snow_pumpkin_state,
            mooshroom_mushroom_states: &mut store.mooshroom_mushroom_states,
            clock_context: clock_context.as_ref(),
            bed_direction,
        };
        draw_entity(entity, &models, content, EntityDrawFrame { origin, transform, tick, partial, light, parent: None },
            &mut draw_store, out, &mut stats)?;
        if entity.binding.as_ref().is_some_and(Result::is_ok) {
            append_entity_fire(entity, &models, content, origin, camera_rotation[0], transform, out)?;
        }
        if out.vertices.len() > start {
            stats.submitted += 1;
            store.slots[slot].submitted_revision = Some(store.revision);
        }
    }
    visible.clear();
    store.visible_ids = visible;
    stats.vertices = out.vertices.len();
    Ok(stats)
}
pub(super) fn draw_entity(
    entity: &Entity,
    models: &Models,
    content: &Content,
    frame: EntityDrawFrame,
    store: &mut DrawStore<'_>,
    out: &mut DynamicGeometry,
    stats: &mut PresentationStats,
) -> Result<()> {
    let EntityDrawFrame { origin, transform, tick, partial, light, parent } = frame;
    let start = out.vertices.len();
        match entity.binding.as_ref().unwrap() {
            Ok(binding) => {
                let rig = models.rig(&binding.rig);
                let tex = binding.texture;
                let t = transform;
                let body_yaw = entity
                    .source
                    .as_ref()
                    .and_then(|source| source.living.as_ref())
                    .map_or(t.yaw, |living| living.body(partial));
                let relative = Vec3::new(
                    (t.position[0] - origin[0]) as f32,
                    (t.position[1] - origin[1]) as f32,
                    (t.position[2] - origin[2]) as f32,
                );
                let mut matrix = match binding.style {
                    RootStyle::Living => {
                        Mat4::from_translation(relative)
                            * Mat4::from_rotation_y((180. - body_yaw).to_radians())
                            * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                            * Mat4::from_translation(Vec3::new(0., -1.501, 0.))
                    }
                    RootStyle::Arrow => projectile_root(
                        relative, t.yaw, t.pitch, entity.type_name == "minecraft:trident",
                    ),
                    RootStyle::Boat => boat_root(relative, t.yaw),
                    RootStyle::Minecart => {
                        Mat4::from_translation(relative + Vec3::new(0., 0.375, 0.))
                            * Mat4::from_rotation_y((180. - t.yaw).to_radians())
                            * Mat4::from_rotation_z(-t.pitch.to_radians())
                            * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                            // Authored rigs place feet at y=24 pixels. Convert
                            // that baseline around the cart's 6-pixel roll pivot.
                            * Mat4::from_translation(Vec3::new(0., -18. / 16., 0.))
                    }
                    RootStyle::Object => {
                        Mat4::from_translation(relative) * Mat4::from_rotation_y(t.yaw.to_radians())
                    }
                };
                let pose = number(models, entity, "DATA_POSE").unwrap_or(0);
                if pose == 2 && matches!(binding.style, RootStyle::Living) {
                    // Lay the rig along the bed axis, with its head at the received bed head.
                    let direction = match value(models, entity, "SLEEPING_POS_ID") {
                        Some(Metadata::OptionalBlockPosition(Some(position))) => (store.bed_direction)(*position),
                        _ => None,
                    };
                    let (offset, yaw) = match direction {
                        Some([x, z]) => {
                            let angle: f32 = match (x, z) {
                                (0, 1) => 0., (0, -1) => 180.,
                                (-1, 0) => -90., (1, 0) => 90.,
                                _ => body_yaw,
                            };
                            let schema = &models.schemas[&entity.type_name];
                            let eye = match (boolean(models, entity, "DATA_BABY_ID"), schema.dimension_constants.get("BABY_DIMENSIONS")) {
                                (true, Some(resources::DimensionConstant::Dimensions(d))) => d.eye_height,
                                _ => schema.dimensions.eye_height,
                            };
                            (Vec3::new(-(x as f32) * eye, 0., -(z as f32) * eye), angle)
                        }
                        None => (Vec3::ZERO, body_yaw),
                    };
                    matrix = Mat4::from_translation(relative + offset)
                        * Mat4::from_rotation_y(yaw.to_radians())
                        * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
                        * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                        * Mat4::from_translation(Vec3::new(0., -1.501, 0.));
                }
                if pose == 3 || pose == 1 || pose == 4 {
                    matrix = Mat4::from_translation(relative + Vec3::new(0., 0.35, 0.))
                        * Mat4::from_rotation_y((180. - t.yaw).to_radians())
                        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                        * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                        * Mat4::from_translation(Vec3::new(0., -1.501, 0.));
                }
                let renderer_scale = renderer_scale(entity, models, partial);
                if renderer_scale != 1. {
                    matrix = Mat4::from_translation(relative)
                        * Mat4::from_scale(Vec3::splat(renderer_scale))
                        * Mat4::from_translation(-relative)
                        * matrix;
                }
                if matches!(
                    entity.type_name.as_str(),
                    "minecraft:slime" | "minecraft:magma_cube"
                ) {
                    matrix = scale_cube_model(
                        matrix,
                        relative,
                        number(models, entity, "ID_SIZE"),
                    );
                }
                if entity.type_name == "minecraft:armor_stand"
                    && number(models, entity, "DATA_CLIENT_FLAGS").unwrap_or(0) & 1 != 0
                {
                    matrix = Mat4::from_translation(relative)
                        * Mat4::from_scale(Vec3::splat(0.5))
                        * Mat4::from_translation(-relative)
                        * matrix;
                }
                let swell = super::creeper::swell(entity, partial);
                if swell > 0. {
                    matrix = super::creeper::swollen(matrix, relative, swell);
                }
                if matches!(binding.style, RootStyle::Living)
                    && entity.type_name != "minecraft:armor_stand"
                    && entity.source.as_ref().is_some_and(|s| s.living.is_some())
                {
                    // Landing squash + idle breathing, scaled about the feet.
                    let breathe = if entity.death_tick.is_none() && pose == 0 {
                        crate::motion::breathing(
                            ((tick % 1200) as f32 + partial) * 0.05,
                            entity.handle.network_id as f32 * 1.618,
                        )
                    } else {
                        0.
                    };
                    let s = breathe - entity.physical.motion.squash(tick, partial);
                    if s != 0. {
                        let xz = 1. - s * 0.5;
                        matrix = Mat4::from_translation(relative)
                            * Mat4::from_scale(Vec3::new(xz, 1. + s, xz))
                            * Mat4::from_translation(-relative)
                            * matrix;
                    }
                    // Short backward lean when struck; the hit flash rides along.
                    if let (Some(damage), None) = (entity.damage_tick, entity.death_tick) {
                        let lean =
                            crate::motion::hurt_lean(tick.saturating_sub(damage) as f32 + partial);
                        if lean > 0. {
                            let (sin, cos) = body_yaw.to_radians().sin_cos();
                            matrix = Mat4::from_translation(relative)
                                * Mat4::from_axis_angle(
                                    Vec3::new(cos, 0., sin),
                                    -lean.to_radians(),
                                )
                                * Mat4::from_translation(-relative)
                                * matrix;
                        }
                    }
                }
                if let Some(death) = entity.death_tick {
                    let a = ((tick.saturating_sub(death) as f32 + partial) / 20.).clamp(0., 1.);
                    matrix *= Mat4::from_rotation_z(a.sqrt() * std::f32::consts::FRAC_PI_2);
                }
                if let Some(parent) = parent { matrix = parent * matrix; }
                let invisible =
                    number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 32 != 0;
                evaluate_rig(rig, entity, models, content, tick, partial, matrix, store.transforms);
                if !invisible {
                    emit_rig_parts(
                        rig,
                        store.transforms,
                        RigStyle { tex, color: [1.; 3], layer: EntityMaterial::EntityCutout, alpha: 1., light },
                        (entity.type_name == "minecraft:armor_stand")
                            .then(|| number(models, entity, "DATA_CLIENT_FLAGS").unwrap_or(0)),
                        entity.type_name == "minecraft:end_crystal"
                            && value(models, entity, "DATA_SHOW_BOTTOM")
                                .and_then(Metadata::boolean) == Some(false),
                        Some((entity, models, tick, partial)),
                        out,
                    )?;
                    super::villagers::append(
                        binding.layers.as_ref(),
                        rig,
                        store.transforms,
                        light,
                        out,
                    )?;
                    if entity.type_name == "minecraft:snow_golem"
                        && number(models, entity, "DATA_PUMPKIN_ID").unwrap_or(16) & 16 != 0
                    {
                        let result = (|| {
                            let state = match *store.snow_pumpkin_state {
                                Some(state) => state,
                                None => {
                                    let state = content.resolve_block_state(
                                        "minecraft:carved_pumpkin",
                                        &BTreeMap::new(),
                                    )?;
                                    *store.snow_pumpkin_state = Some(state);
                                    state
                                }
                            };
                            let model = content
                                .model_for(state)?
                                .ok_or("carved pumpkin block model unavailable")?;
                            let head = rig.bones.iter().zip(store.transforms.iter())
                                .find_map(|(bone, matrix)| (bone.role == BoneRole::Head).then_some(*matrix))
                                .ok_or("snow golem has no head bone")?;
                            append_snow_pumpkin(model, head, light, out)
                        })();
                        if let Err(reason) = result {
                            stats.unrendered.insert(entity.type_name.clone(), reason);
                        }
                    }
                    if entity.type_name == "minecraft:mooshroom"
                        && !boolean(models, entity, "DATA_BABY_ID")
                    {
                        let result = (|| {
                            let brown = number(models, entity, "DATA_TYPE").unwrap_or(0) == 1;
                            let index = usize::from(brown);
                            let state = match store.mooshroom_mushroom_states[index] {
                                Some(state) => state,
                                None => {
                                    let block = if brown { "minecraft:brown_mushroom" } else { "minecraft:red_mushroom" };
                                    let state = content.resolve_block_state(block, &BTreeMap::new())?;
                                    store.mooshroom_mushroom_states[index] = Some(state);
                                    state
                                }
                            };
                            let mushroom = content.model_for(state)?
                                .ok_or("mooshroom mushroom block model unavailable")?;
                            let head = rig.bones.iter().zip(store.transforms.iter())
                                .find_map(|(bone, transform)| (bone.role == BoneRole::Head).then_some(*transform))
                                .ok_or("mooshroom has no head bone")?;
                            append_mooshroom_mushrooms(mushroom, matrix, head, light, out)
                        })();
                        if let Err(reason) = result {
                            stats.unrendered.insert(entity.type_name.clone(), reason);
                        }
                    }
                    let hurt = matches!(binding.style, RootStyle::Living)
                        && (entity
                            .damage_tick
                            .is_some_and(|damage| tick.saturating_sub(damage) < 10)
                            || entity.death_tick.is_some());
                    let white = super::creeper::flash(swell);
                    if hurt || white > 0. {
                        let overlay = if hurt { [0., 3.] } else { [white, 10.] };
                        for vertex in &mut out.vertices[start..] {
                            vertex.overlay_uv = overlay;
                        }
                    }
                }
                stats.animated += 1;
                if !invisible && entity.type_name == "minecraft:player" {
                    let profile = store.profiles.get(&entity.uuid).and_then(|p| p.textures);
                    if let Err(reason) = store.equipment_cache.append_player_back(entity, profile,
                        super::equipment::Frame { models, content, root: matrix, tick, partial, light }, out)
                    {
                        stats.unrendered.insert("minecraft:player:back".into(), reason);
                    }
                }
                append_equipment(
                    EquipmentPose { entity, rig, transforms: store.transforms, item_context: store.clock_context },
                    store.items,
                    store.equipment_cache,
                    super::equipment::Frame { models, content, root: matrix, tick, partial, light },
                    out,
                    stats,
                )?;
                if entity.type_name == "minecraft:creaking" {
                    let result = (|| {
                        let head = rig.bones.iter().zip(store.transforms.iter())
                            .find_map(|(bone, transform)| (bone.role == BoneRole::Head).then_some(*transform))
                            .ok_or("creaking has no head bone")?;
                        super::creaking_eyes::append(
                            models, content, entity, head, store.transforms, light, out,
                        )
                    })();
                    if let Err(reason) = result {
                        stats.unrendered.insert(format!("{}:eyes", entity.type_name), reason);
                    }
                }
                if !invisible {
                    if matches!(entity.type_name.as_str(), "minecraft:cat" | "minecraft:wolf")
                        && number(models, entity, "DATA_FLAGS_ID").unwrap_or(0) & 4 != 0
                    {
                        let baby = boolean(models, entity, "DATA_BABY_ID");
                        let path = match (entity.type_name.as_str(), baby) {
                            ("minecraft:cat", false) => "minecraft:entity/cat/cat_collar",
                            ("minecraft:cat", true) => "minecraft:entity/cat/cat_collar_baby",
                            ("minecraft:wolf", false) => "minecraft:entity/wolf/wolf_collar",
                            _ => "minecraft:entity/wolf/wolf_collar_baby",
                        };
                        let result = content
                            .texture(path)
                            .ok_or_else(|| format!("entity texture not activated: {path}"))
                            .and_then(|collar| {
                                let color = super::sheep::dye_color(
                                    number(models, entity, "DATA_COLLAR_COLOR").unwrap_or(14),
                                );
                                if baby && entity.type_name == "minecraft:wolf" {
                                    static PUPPY_COLLAR: std::sync::LazyLock<Option<&'static Rig>> =
                                        std::sync::LazyLock::new(|| {
                                            super::solaris_models::authored("minecraft:wolf_baby_collar#main")
                                        });
                                    let collar_rig = PUPPY_COLLAR.ok_or("missing puppy collar rig")?;
                                    let body = rig.bones.iter().position(|bone| bone.name == "body")
                                        .ok_or("missing puppy body")?;
                                    let transforms = [Mat4::IDENTITY, store.transforms[body]];
                                    emit_rig(collar_rig, &transforms, crate::entities::presentation::RigStyle { tex: collar, color, layer: EntityMaterial::EntityCutout, alpha: 1., light }, out)
                                } else {
                                    emit_rig(rig, store.transforms, crate::entities::presentation::RigStyle { tex: collar, color, layer: EntityMaterial::EntityCutout, alpha: 1., light }, out)
                                }
                            });
                        if let Err(reason) = result {
                            stats.unrendered.insert(format!("{}:collar", entity.type_name), reason);
                        }
                    }
                    super::sheep::append(
                        entity,
                        models,
                        content,
                        super::sheep::CoatFrame {
                            baby: boolean(models, entity, "DATA_BABY_ID"),
                            base_rig: rig, tick, partial, root: matrix, light,
                        },
                        store.transforms,
                        out,
                    )?;
                    if entity.type_name == "minecraft:drowned"
                        && let Err(reason) = super::drowned_outer::append_drowned_outer(
                            models, content, entity, super::drowned_outer::Pose { tick, partial, root: matrix },
                            store.transforms, light, out,
                        )
                    {
                        stats.unrendered.insert(format!("{}:outer", entity.type_name), reason);
                    }
                    if entity.type_name == "minecraft:slime" {
                        let shell = super::solaris_models::authored("minecraft:slime_outer#main")
                            .ok_or("missing authored slime outer rig")?;
                        evaluate_rig(shell, entity, models, content, tick, partial, matrix, store.transforms);
                        emit_rig(shell, store.transforms, crate::entities::presentation::RigStyle { tex, color: [1.; 3], layer: EntityMaterial::EntityTranslucent, alpha: 1., light }, out)?;
                    }
                    if entity.type_name == "minecraft:breeze" {
                        let wind = super::solaris_models::authored("minecraft:breeze_wind#main")
                            .ok_or("missing authored breeze wind rig")?;
                        let texture = texture(content, "minecraft:entity/breeze/breeze_wind")?;
                        evaluate_rig(wind, entity, models, content, tick, partial, matrix, store.transforms);
                        let scroll = ((tick % 2000) as f32 + partial) * 0.01;
                        emit_wrapped(wind, store.transforms, WrappedStyle {
                            tex: texture, scroll: [scroll, 0.], color: [1.; 3],
                            layer: EntityMaterial::BreezeWind, light,
                        }, out)?;
                    }
                }
                // minecraft.wiki: the charged aura stays visible on an invisible creeper.
                if entity.type_name == "minecraft:creeper"
                    && let Err(reason) = super::creeper::append_charge(
                        entity, models, content, (tick, partial, matrix), store.transforms, out,
                    )
                {
                    stats.unrendered.insert(format!("{}:charge", entity.type_name), reason);
                }
            }
            Err(reason) => {
                if !append_item_entity(
                    entity,
                    models,
                    store,
                    content,
                    frame,
                    out,
                    stats,
                )? {
                    stats
                        .unrendered
                        .insert(entity.type_name.clone(), reason.clone());
                }
            }
        }
    Ok(())
}

/// Equipment slots are logical hands; rigs and display transforms are anatomical sides.
fn left_main_arm(models: &Models, e: &Entity) -> bool {
    number(models, e, "DATA_PLAYER_MAIN_HAND").map_or_else(
        || number(models, e, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 2 != 0,
        |arm| arm == 0,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArmAction { Item, Block, Bow, CrossbowCharge, CrossbowHold, Trident, Spear, Spyglass, Horn, Brush }

fn use_action(content: &Content, stack: &Stack) -> ArmAction {
    // Presentation-model overrides never change an item's behavior.
    match content.item_names.get(&stack.id).map(String::as_str) {
        Some("minecraft:bow") => return ArmAction::Bow,
        Some("minecraft:crossbow") => return ArmAction::CrossbowCharge,
        Some("minecraft:trident") => return ArmAction::Trident,
        Some("minecraft:spyglass") => return ArmAction::Spyglass,
        Some("minecraft:goat_horn") => return ArmAction::Horn,
        Some("minecraft:brush") => return ArmAction::Brush,
        _ => {}
    }
    // Item subclasses own their animations; plain Item checks consumable before blocking/kinetic.
    if let Some(consumable) = stack.component("minecraft:consumable") {
        return match consumable["animation"].as_str().unwrap_or("eat") {
            "block" => ArmAction::Block, "bow" => ArmAction::Bow,
            "crossbow" => ArmAction::CrossbowCharge, "trident" => ArmAction::Trident,
            "spyglass" => ArmAction::Spyglass, "toot_horn" => ArmAction::Horn,
            "brush" => ArmAction::Brush, "spear" => ArmAction::Spear,
            _ => ArmAction::Item,
        };
    }
    if stack.component("minecraft:blocks_attacks").is_some() {
        return ArmAction::Block;
    }
    if stack.component("minecraft:kinetic_weapon").is_some() {
        return ArmAction::Spear;
    }
    ArmAction::Item
}

/// Independent angular fits to local HumanoidModel/AnimationUtils observations.
/// Eating/drinking retains ITEM, as AvatarRenderer does; aimed actions use both arms.
fn equipped_arm_rotations(
    e: &Entity, models: &Models, content: &Content, head: Vec3,
    gait: [f32; 2], age: f32, partial: f32,
) -> [Vec3; 2] {
    let mut arms = gait.map(|x| Vec3::new(x, 0., 0.));
    let main = usize::from(left_main_arm(models, e));
    let flags = number(models, e, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
    let charging = boolean(models, e, "IS_CHARGING_CROSSBOW")
        || boolean(models, e, "DATA_IS_CHARGING_CROSSBOW");
    let using = flags & 1 != 0 || charging;
    let active = usize::from(flags & 2 != 0);
    let mut two_handed = None;
    for hand in 0..2 {
        let arm = main ^ hand;
        let Some(stack) = e.equipment_value(hand).filter(|s| !s.empty()) else { continue };
        arms[arm].x = arms[arm].x * 0.5 - 18_f32.to_radians();
        let action = if using && active == hand {
            use_action(content, stack)
        } else if content.item_names.get(&stack.id).is_some_and(|n| n == "minecraft:crossbow")
            && stack.component("minecraft:charged_projectiles")
                .and_then(Value::as_array).is_some_and(|p| !p.is_empty())
            && e.animation.is_none()
        {
            ArmAction::CrossbowHold
        } else { ArmAction::Item };
        let side = if arm == 0 { 1. } else { -1. };
        match action {
            ArmAction::Item => {}
            ArmAction::Block => {
                arms[arm].x = gait[arm] * 0.5 - 54_f32.to_radians()
                    + head.x.clamp(-80_f32.to_radians(), 25_f32.to_radians());
                arms[arm].y = -side * 30_f32.to_radians()
                    + head.y.clamp(-30_f32.to_radians(), 30_f32.to_radians());
            }
            ArmAction::Trident => arms[arm].x = gait[arm] * 0.5 - std::f32::consts::PI,
            ArmAction::Spear => {
                let ticks = e.source.as_ref().and_then(|s| s.living.as_ref())
                    .map_or(0., |s| s.item_use_ticks(partial));
                let delay = stack.component("minecraft:kinetic_weapon")
                    .and_then(|k| k["delay_ticks"].as_f64()).unwrap_or(0.) as f32;
                let raised = if ticks <= 0. { 0. } else if delay == 0. { 1. }
                    else { (ticks / delay).clamp(0., 1.) };
                arms[arm].x = (head.x - 45_f32.to_radians() - raised * 30_f32.to_radians())
                    .clamp(-120_f32.to_radians(), 30_f32.to_radians());
                arms[arm].y = (head.y - side * 6_f32.to_radians())
                    .clamp(-60_f32.to_radians(), 60_f32.to_radians());
            }
            ArmAction::Spyglass => {
                arms[arm].x = (head.x - 110_f32.to_radians()
                    - if number(models, e, "DATA_POSE") == Some(5) { 15_f32.to_radians() } else { 0. })
                    .clamp(-2.4, 3.3);
                arms[arm].y = head.y - side * 15_f32.to_radians();
            }
            ArmAction::Horn => {
                arms[arm].x = head.x.clamp(-1.2, 1.2) - 85_f32.to_radians();
                arms[arm].y = head.y - side * 30_f32.to_radians();
            }
            ArmAction::Brush => arms[arm].x = gait[arm] * 0.5 - 36_f32.to_radians(),
            action => {
                if two_handed.is_none() || using && active == hand {
                    two_handed = Some((arm, action));
                }
            }
        }
    }
    if let Some((hold, action)) = two_handed {
        let other = hold ^ 1;
        let side = if hold == 0 { 1. } else { -1. };
        match action {
            ArmAction::Bow => {
                arms[hold] = Vec3::new(head.x - std::f32::consts::FRAC_PI_2, head.y - side * 0.1, 0.);
                arms[other] = Vec3::new(head.x - std::f32::consts::FRAC_PI_2, head.y + side * 0.5, 0.);
            }
            ArmAction::CrossbowHold => {
                arms[hold] = Vec3::new(head.x - 84_f32.to_radians(), head.y - side * 0.3, 0.);
                arms[other] = Vec3::new(head.x - 86_f32.to_radians(), head.y + side * 0.6, 0.);
            }
            ArmAction::CrossbowCharge => {
                let progress = e.source.as_ref().and_then(|s| s.living.as_ref())
                    .map_or(0., |s| s.crossbow_pull(partial));
                arms[hold] = Vec3::new(-56_f32.to_radians(), -side * 0.8, 0.);
                arms[other] = Vec3::new(-(56. + 34. * progress).to_radians(), side * (0.4 + 0.45 * progress), 0.);
            }
            _ => {}
        }
    }
    if number(models, e, "DATA_SPELL_CASTING_ID").is_some_and(|spell| spell != 0) {
        let wave = (age * 0.65).cos() * 14_f32.to_radians();
        arms = [Vec3::new(wave, 0., 135_f32.to_radians()), Vec3::new(wave, 0., -135_f32.to_radians())];
    }
    arms
}

/// Procedural rigs have named anatomical parts even when no humanoid roles apply.
/// Preserve authored pivots, scales and yaw (notably the squid texture orientation).
fn procedural_part(
    bone: &resources::Bone, e: &Entity, models: &Models, age: f32, moving: f32,
    rotation: &mut Vec3, pivot: &mut Vec3,
) {
    let name = bone.name.as_str();
    let water = e.source.as_ref().and_then(|s| s.living.as_ref()).is_some_and(|s| s.in_water);
    let numbered = |prefix: &str| name.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok());
    match e.type_name.as_str() {
        "minecraft:cod" | "minecraft:tropical_fish" if matches!(name, "tail" | "tail_fin") => {
            rotation.y += (age * 0.6).sin() * if water { -0.45 } else { -0.65 };
        }
        "minecraft:salmon" if matches!(name, "body_rear" | "body_back" | "tail") => {
            rotation.y += (age * if water { 0.6 } else { 1. }).sin()
                * if water { -0.25 } else { -0.33 };
        }
        "minecraft:pufferfish" if matches!(name, "right_fin" | "left_fin" | "right_blue_fin" | "left_blue_fin") => {
            let side = if name.starts_with("right") { 1. } else { -1. };
            rotation.z += side * (-0.2 + (age * 0.2).sin() * 0.4);
        }
        "minecraft:squid" | "minecraft:glow_squid" if numbered("tentacle").is_some() => {
            rotation.x += if water { ((age * 0.12).sin() * 0.5 + 0.5).powi(2) * 1.2 } else { 0. };
        }
        "minecraft:ghast" | "minecraft:happy_ghast" if numbered("tentacle_").is_some() || numbered("tentacle").is_some() => {
            let index = numbered("tentacle_").or_else(|| numbered("tentacle")).unwrap();
            rotation.x += 0.4 + (age * 0.3 + index as f32).sin() * 0.2;
        }
        "minecraft:blaze" if numbered("stick").is_some() => {
            let index = numbered("stick").unwrap();
            let turn = age * match index / 4 { 0 => -0.3, 1 => 0.1, _ => -0.16 };
            let rest = Vec3::new(bone.pivot.x, 0., bone.pivot.z);
            let orbit = Mat4::from_rotation_y(turn).transform_vector3(rest);
            pivot.x = orbit.x;
            pivot.z = orbit.z;
            pivot.y += (age * 0.25 + index as f32 * 0.5).cos() / 16.;
        }
        "minecraft:guardian" | "minecraft:elder_guardian" => {
            if let Some(index) = numbered("spike") {
                let extended = !boolean(models, e, "DATA_ID_MOVING");
                *pivot *= (if extended { 1. } else { 0.5 }) + (age * 1.5 + index as f32).cos() * 0.01;
            } else if matches!(name, "tail" | "tail_tip" | "tail0" | "tail1" | "tail2") {
                let speed = if boolean(models, e, "DATA_ID_MOVING") { 0.4 } else { 0.1 };
                rotation.y += (age * speed).sin() * if name == "tail" || name == "tail0" { 0.16 } else { 0.32 };
            }
        }
        "minecraft:dolphin" if moving > 1e-4 || e.velocity.iter().any(|v| v.abs() > 1e-4) => {
            match name {
                "body" => rotation.x -= 0.05 + (age * 0.3).cos() * 0.05,
                "tail" => rotation.x -= (age * 0.3).cos() * 0.1,
                "fluke_left" | "fluke_right" | "tail_fin" => rotation.x -= (age * 0.3).cos() * 0.2,
                _ => {}
            }
        }
        "minecraft:nautilus" | "minecraft:zombie_nautilus" => {
            if name == "root" { pivot.y += (age * 0.2).sin() * 0.03; }
            if name == "body" || name.ends_with("mouth") || numbered("arm").is_some() {
                rotation.x += (age * 0.2).sin() * (0.1 + moving * 0.2);
            }
        }
        "minecraft:silverfish" | "minecraft:endermite" => {
            let index = numbered("segment").or_else(|| match name {
                "head" => Some(0), "body" => Some(1), "tail" => Some(2),
                "layer0" => Some(2), "layer1" => Some(4), "layer2" => Some(1), _ => None,
            });
            if let Some(i) = index {
                let strength = (i as f32 - 2.).abs();
                let wave = age * 0.9 + i as f32 * 0.47;
                rotation.y += wave.cos() * (1. + strength) * 0.03;
                pivot.x += wave.sin() * strength * 0.02;
            }
        }
        _ => {}
    }
}

pub(super) fn evaluate_rig(
    rig: &Rig,
    e: &Entity,
    models: &Models,
    content: &Content,
    tick: u64,
    partial: f32,
    root: Mat4,
    out: &mut Vec<Mat4>,
) {
    out.clear();
    out.reserve(rig.bones.len());
    let living = e.source.as_ref().and_then(|source| source.living.as_ref());
    // Relative-position packets do not carry useful velocity for locomotion.
    // The source owner advances WalkRule from interpolated position deltas.
    let phase = living.map_or(0., |state| state.walk.position(partial) * 0.6662);
    let amplitude = living.map_or(0., |state| state.walk.speed(partial));
    // VillagerModel (villager, wandering trader, witch) swings legs at half the humanoid amplitude.
    let leg_swing = if matches!(
        e.type_name.as_str(),
        "minecraft:villager" | "minecraft:wandering_trader" | "minecraft:witch"
    ) {
        0.5
    } else {
        1.
    };
    let t = e.presented_transform(tick, partial);
    let body_yaw = living.map_or(t.yaw, |state| state.body(partial));
    let head_yaw = living.map_or(t.head_yaw, |state| state.head(partial));
    let crouch = number(models, e, "DATA_POSE") == Some(5);
    let head = Vec3::new(t.pitch.to_radians(),
        ((head_yaw - body_yaw + 180.).rem_euclid(360.) - 180.).clamp(-85., 85.).to_radians(), 0.);
    let age = e.source.as_ref().map_or(tick as f32, |s| s.age as f32) + partial;
    let arms = equipped_arm_rotations(e, models, content, head,
        [(phase + std::f32::consts::PI).cos() * amplitude, phase.cos() * amplitude], age, partial);
    let attack = e
        .animation
        .map(|(_, start)| (tick.saturating_sub(start) as f32 + partial) / 6.)
        .filter(|f| *f < 1.);
    let pet_sitting = matches!(e.type_name.as_str(), "minecraft:cat" | "minecraft:wolf")
        && number(models, e, "DATA_FLAGS_ID").unwrap_or(0) & 1 != 0;
    let pet_baby = pet_sitting && boolean(models, e, "DATA_BABY_ID");
    let zombie_arms = matches!(
        e.type_name.as_str(),
        "minecraft:zombie" | "minecraft:husk" | "minecraft:drowned" | "minecraft:giant"
            | "minecraft:zombie_villager" | "minecraft:zombified_piglin"
    );
    let zombie_arms_raised = zombie_arms && (!boolean(models, e, "DATA_BABY_ID")
        || e.equipment_value(0).is_none_or(|stack| stack.empty()));
    let aggressive = zombie_arms && number(models, e, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 4 != 0;
    let wing_beat = if super::render_living::folds_wings(&e.type_name) {
        living.map_or(0., |state| state.wings(partial))
    } else {
        1.
    };
    let wave = (age * 0.3).sin();
    let wing_flap = if super::render_living::folds_wings(&e.type_name) {
        (wave + 1.) * 0.7 * wing_beat
    } else { wave * 0.3 };
    for bone in &rig.bones {
        let mut rotation = bone.rotation;
        let mut pivot = bone.pivot;
        let mut scale = bone.scale;
        match bone.role {
            BoneRole::Head => {
                let (look_yaw, look_pitch, weight) = e.physical.motion.look(partial);
                let mut yaw_rel =
                    ((head_yaw - body_yaw + 180.).rem_euclid(360.) - 180.).clamp(-85., 85.);
                let mut pitch = t.pitch;
                if weight > 0. {
                    // Idle mobs glance at a nearby player; weight eases in/out per tick.
                    let desired = crate::motion::angle_delta(body_yaw, look_yaw).clamp(
                        -crate::motion::LOOK_HEAD_LIMIT,
                        crate::motion::LOOK_HEAD_LIMIT,
                    );
                    yaw_rel = crate::motion::lerp(yaw_rel, desired, weight);
                    pitch = crate::motion::lerp(pitch, look_pitch, weight);
                }
                rotation.x += pitch.to_radians();
                rotation.y += yaw_rel.to_radians();
            }
            BoneRole::RightLeg => rotation.x += phase.cos() * 1.4 * amplitude * leg_swing,
            BoneRole::LeftLeg => {
                rotation.x += (phase + std::f32::consts::PI).cos() * 1.4 * amplitude * leg_swing
            }
            BoneRole::RightArm | BoneRole::LeftArm => {
                let arm = usize::from(bone.role == BoneRole::LeftArm);
                rotation += arms[arm];
                if number(models, e, "DATA_SPELL_CASTING_ID").is_some_and(|spell| spell != 0) {
                    rotation = arms[arm];
                    pivot.z = 0.;
                }
                let attack_hand = e.animation.map_or(0, |(action, _)| usize::from(action == 3));
                if (usize::from(left_main_arm(models, e)) ^ attack_hand) == arm
                    && let Some(a) = attack
                {
                    rotation.x -= (a * std::f32::consts::PI).sin() * 1.2;
                }
            }
            BoneRole::WingNegativeX => rotation.z += wing_flap,
            BoneRole::WingPositiveX => rotation.z -= wing_flap,
            // Source `WitchModel.setupAnim`: a held item pulls the nose forward and up.
            BoneRole::Nose if e.equipment_value(0).is_some() => {
                pivot = Vec3::new(0., 1., -1.5) / 16.;
                rotation.x = -0.9;
            }
            BoneRole::Nose | BoneRole::CrossedArms | BoneRole::Fixed => {}
        }
        if zombie_arms_raised && matches!(bone.role, BoneRole::RightArm | BoneRole::LeftArm) {
            // Observed vanilla idle silhouette is 80 degrees forward, not horizontal.
            rotation = Vec3::ZERO;
            let side = if matches!(bone.role, BoneRole::RightArm) { 1. } else { -1. };
            let bob = (age * 0.07).sin() * 0.05 * side;
            rotation.x = if aggressive { -120_f32.to_radians() } else { -80_f32.to_radians() } + bob;
            rotation.y = -side * 0.1;
            rotation.z = side * ((age * 0.09).cos() + 1.) * 0.05;
            if let Some(a) = attack {
                let swing = (a * std::f32::consts::PI).sin();
                let recover = ((1. - (1. - a).powi(2)) * std::f32::consts::PI).sin();
                rotation.x += swing * 1.1 - recover * 0.35;
                rotation.y += side * swing * 0.6;
            }
        }
        if e.type_name == "minecraft:drowned" && aggressive
            && matches!(bone.role, BoneRole::RightArm | BoneRole::LeftArm)
            && (bone.role == BoneRole::LeftArm) == left_main_arm(models, e)
            && e.equipment_value(0).and_then(|s| content.item_names.get(&s.id))
                .is_some_and(|name| name == "minecraft:trident")
        {
            rotation.x = rotation.x * 0.5 - std::f32::consts::PI;
            rotation.y = 0.;
        }
        if matches!(e.type_name.as_str(), "minecraft:spider" | "minecraft:cave_spider") {
            // Solaris-authored alternating leg swing, driven by received
            // displacement and eased to the authored rest pose when stopped.
            let leg = match bone.name.as_str() {
                "leg0" | "leg4" => Some((1., 1.)),
                "leg1" | "leg5" => Some((-1., -1.)),
                "leg2" | "leg6" => Some((1., -1.)),
                "leg3" | "leg7" => Some((-1., 1.)),
                _ => None,
            };
            if let Some((side, phase_sign)) = leg {
                let swing = phase.sin() * amplitude * phase_sign;
                rotation.y += side * swing * 0.3;
                rotation.z += side * swing * 0.15;
            }
        }
        if pet_sitting {
            // These are Solaris-authored poses for the embedded rigs: lower the
            // hindquarters and fold the rear legs while the head stays upright.
            match bone.name.as_str() {
                "body" => {
                    rotation.x -= 15_f32.to_radians();
                    if e.type_name == "minecraft:wolf" && !pet_baby {
                        pivot.y += 2. / 16.;
                    }
                }
                "mane" if !pet_baby => {
                    rotation.x -= 15_f32.to_radians();
                    pivot.y += 1. / 16.;
                }
                "left_hind_leg" | "right_hind_leg" => {
                    pivot.y += if pet_baby { 2. / 16. } else { 6. / 16. };
                    rotation.x = -std::f32::consts::FRAC_PI_2;
                }
                "left_front_leg" | "right_front_leg" => rotation.x = bone.rotation.x,
                "tail" => {
                    pivot.y += if pet_baby { 1. / 16. } else { 4. / 16. };
                    rotation.x += 20_f32.to_radians();
                }
                _ => {}
            }
        }
        if crouch && bone.name == "body" {
            rotation.x += 0.5;
            pivot.y += 0.2;
        }
        if e.vehicle.is_some() {
            match bone.role {
                BoneRole::RightLeg | BoneRole::LeftLeg => rotation.x = -1.4137167,
                _ => {}
            }
        }
        if e.type_name == "minecraft:armor_stand" {
            let field = match bone.name.as_str() {
                "head" => Some("DATA_HEAD_POSE"),
                "body" => Some("DATA_BODY_POSE"),
                "left_arm" => Some("DATA_LEFT_ARM_POSE"),
                "right_arm" => Some("DATA_RIGHT_ARM_POSE"),
                "left_leg" => Some("DATA_LEFT_LEG_POSE"),
                "right_leg" => Some("DATA_RIGHT_LEG_POSE"),
                _ => None,
            };
            if let Some(Metadata::Rotations(r)) = field.and_then(|field| value(models, e, field)) {
                rotation = Vec3::from_array(*r) * std::f32::consts::PI / 180.;
            }
        }
        procedural_part(bone, e, models, age, amplitude, &mut rotation, &mut pivot);
        if let Some(state) = living {
            state.apply_part(models, e, bone, age, partial, &mut pivot, &mut rotation, &mut scale);
        }
        let parent = bone.parent.map(|i| out[i]).unwrap_or(root);
        if e.type_name == "minecraft:end_crystal"
            && matches!(bone.name.as_str(), "glass_outer" | "glass_inner" | "core")
        {
            let age = e.source.as_ref().map_or(tick as f32, |source| {
                (source.age + source.special.crystal_offset) as f32
            }) + partial;
            let wave = (age * 0.2).sin() * 0.5 + 0.5;
            pivot.y += (wave * wave + wave) * 0.2;
            let turn = Mat4::from_rotation_y((age * 3.).to_radians());
            let tilt = Mat4::from_axis_angle(Vec3::new(1., 0., 1.).normalize(), std::f32::consts::FRAC_PI_3);
            let rotation = match bone.name.as_str() {
                "glass_outer" => turn * tilt,
                "glass_inner" => turn * tilt * tilt * turn,
                _ => turn * tilt * tilt * turn * tilt * turn,
            };
            out.push(parent * Mat4::from_translation(pivot) * rotation
                * Mat4::from_scale(scale));
        } else {
            out.push(parent * resources::transform(pivot, rotation, scale));
        }
    }
}
pub(super) struct RigStyle {
    pub tex: TextureRegion,
    pub color: [f32; 3],
    pub layer: EntityMaterial,
    pub alpha: f32,
    pub light: [f32; 2],
}

pub(super) fn emit_rig(
    rig: &Rig,
    transforms: &[Mat4],
    style: RigStyle,
    out: &mut DynamicGeometry,
) -> Result<()> {
    emit_rig_parts(rig, transforms, style, None, false, None, out)
}

/// A texture layer that repeats within its own atlas rectangle (breeze wind, energy
/// swirl), scrolled by `scroll` texture widths/heights.
pub(super) struct WrappedStyle {
    pub tex: TextureRegion,
    pub scroll: [f32; 2],
    pub color: [f32; 3],
    pub layer: EntityMaterial,
    pub light: [f32; 2],
}
pub(super) fn emit_wrapped(
    rig: &Rig,
    transforms: &[Mat4],
    style: WrappedStyle,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let WrappedStyle { tex, scroll, color, layer, light } = style;
    let start = out.vertices.len();
    emit_rig(rig, transforms, RigStyle { tex, color, layer, alpha: 1., light }, out)?;
    let size = [tex.uv[2] - tex.uv[0], tex.uv[3] - tex.uv[1]];
    for vertex in &mut out.vertices[start..] {
        vertex.uv = [
            (vertex.uv[0] - tex.uv[0]) / size[0] + scroll[0],
            (vertex.uv[1] - tex.uv[1]) / size[1] + scroll[1],
        ];
        vertex.overlay_uv = [tex.uv[0], tex.uv[1]];
        vertex.overlay_tint = [size[0], size[1], 0.];
    }
    Ok(())
}

fn emit_rig_parts(
    rig: &Rig,
    transforms: &[Mat4],
    style: RigStyle,
    stand_flags: Option<i32>,
    hide_crystal_base: bool,
    entity: Option<(&Entity, &Models, u64, f32)>,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let RigStyle { tex, color, layer, alpha, light } = style;
    if rig.bones.len() != transforms.len() {
        return Err("entity rig transform cardinality mismatch".into());
    }
    // A missing model must be unmistakable: placeholder boxes are tinted magenta.
    let color = if rig.is_placeholder() {
        [1., 0., 1.]
    } else {
        color
    };
    for (bone, matrix) in rig.bones.iter().zip(transforms) {
        if entity.is_some_and(|(e, models, tick, partial)| {
            let age = e.source.as_ref().map_or(tick as f32, |s| s.age as f32) + partial;
            e.source.as_ref().and_then(|s| s.living.as_ref())
                .is_some_and(|s| !s.part_visible(models, e, &bone.name, age))
        }) {
            continue;
        }
        if hide_crystal_base && bone.name == "base" {
            continue;
        }
        if stand_flags.is_some_and(|flags| match bone.name.as_str() {
            "left_arm" | "right_arm" => flags & 4 == 0,
            "base_plate" => flags & 8 != 0,
            _ => false,
        }) {
            continue;
        }
        let normal_matrix = matrix.inverse().transpose();
        for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
            let positions = quad.map(|v| {
                matrix
                    .transform_point3(Vec3::new(v[0], v[1], v[2]))
                    .to_array()
            });
            let uv = quad.map(|v| {
                [
                    tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                    tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                ]
            });
            emit(positions, uv, color, tex.layer, layer, alpha, light, out)?;
            let normal = normal_matrix
                .transform_vector3(Vec3::from_array(*normal))
                .normalize_or_zero()
                .to_array();
            let first = out.vertices.len() - 4;
            for vertex in &mut out.vertices[first..] {
                vertex.normal = normal;
            }
        }
    }
    Ok(())
}
pub(crate) fn emit(
    positions: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    color: [f32; 3],
    texture_layer: u32,
    layer: EntityMaterial,
    alpha: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    if out.vertices.len() + 4 > MAX_DYNAMIC_VERTICES {
        return Err("dynamic entity geometry byte/vertex budget reached".into());
    }
    let first = u32::try_from(out.vertices.len()).map_err(|_| "dynamic geometry index overflow")?;
    let normal = (Vec3::from_array(positions[1]) - Vec3::from_array(positions[0]))
        .cross(Vec3::from_array(positions[2]) - Vec3::from_array(positions[0]))
        .normalize_or_zero()
        .to_array();
    for i in 0..4 {
        out.vertices.push(Vertex {
            position: positions[i],
            color,
            uv: uv[i],
            // OverlayTexture.NO_OVERLAY uses integer texels, not atlas UVs.
            overlay_uv: [0., 10.],
            overlay_tint: [1.; 3],
            alpha,
            texture_layer,
            light,
            normal,
        });
    }
    out.indices[layer as usize].extend([first, first + 1, first + 2, first + 2, first + 3, first]);
    Ok(())
}
pub(super) fn append_baked(
    model: &BakedModel,
    matrix: Mat4,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    for q in &model.quads {
        let color = if let TintKind::Constant(color) = q.tint {
            color
        } else {
            [1.; 3]
        };
        emit(
            q.positions
                .map(|p| matrix.transform_point3(Vec3::from_array(p)).to_array()),
            q.uv,
            color,
            q.texture_layer,
            q.entity_material.unwrap_or(match q.layer {
                RenderLayer::Opaque | RenderLayer::Cutout => EntityMaterial::ItemCutout,
                RenderLayer::Translucent => EntityMaterial::ItemTranslucent,
            }),
            q.alpha,
            q.light_with_emission(light),
            out,
        )?;
    }
    Ok(())
}
pub(super) fn append_baked_stack(
    model: &BakedModel,
    matrix: Mat4,
    light: [f32; 2],
    content: &Content,
    stack: &crate::inventory::Stack,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let foil = crate::ui::items::foil_texture(content, stack)?;
    let first = out.vertices.len();
    append_baked(model, matrix, light, out)?;
    if let Some(texture) = foil {
        for (quad, vertices) in model.quads.iter().zip(out.vertices[first..].chunks_exact_mut(4)) {
            if quad.entity_material.is_none() || matches!(quad.entity_material,
                Some(EntityMaterial::ItemCutout | EntityMaterial::ItemTranslucent)) {
                crate::ui::items::mark_foil(vertices, texture);
            }
        }
    }
    Ok(())
}


fn append_snow_pumpkin(
    model: &BakedModel,
    head: Mat4,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    // The authored head pivot is at the top of its eight-pixel cube.
    let matrix = head
        * Mat4::from_translation(Vec3::new(0., 0.25, 0.))
        * Mat4::from_rotation_y(std::f32::consts::PI)
        * Mat4::from_scale(Vec3::new(0.625, -0.625, -0.625))
        * Mat4::from_translation(Vec3::splat(-0.5));
    append_transformed_block(model, matrix, light, out)
}

fn append_mooshroom_mushrooms(
    model: &BakedModel,
    root: Mat4,
    head: Mat4,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let shape = Mat4::from_scale(Vec3::new(-1., -1., 1.))
        * Mat4::from_translation(Vec3::splat(-0.5));
    let back = root * Mat4::from_translation(Vec3::new(0.2, -0.35, 0.5));
    let back_turn = Mat4::from_rotation_y((-48_f32).to_radians());
    append_transformed_block(model, back * back_turn * shape, light, out)?;
    append_transformed_block(
        model,
        back * Mat4::from_rotation_y(42_f32.to_radians())
            * Mat4::from_translation(Vec3::new(0.1, 0., -0.6))
            * back_turn * shape,
        light,
        out,
    )?;
    append_transformed_block(
        model,
        head * Mat4::from_translation(Vec3::new(0., -0.7, -0.2))
            * Mat4::from_rotation_y((-78_f32).to_radians()) * shape,
        light,
        out,
    )
}

fn append_transformed_block(
    model: &BakedModel,
    matrix: Mat4,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let seed = crate::protocol::models::position_seed([0; 3]);
    append_block_quads(model, seed, matrix, light, |quad| Ok(match quad.tint {
        TintKind::Constant(color) => color,
        _ => [1.; 3],
    }), out)
}
/// Emits the block model parts chosen by `seed` at `matrix`, colouring each quad by `tint`.
pub(super) fn append_block_quads(
    model: &BakedModel,
    seed: i64,
    matrix: Mat4,
    light: [f32; 2],
    tint: impl Fn(&BakedQuad) -> Result<[f32; 3]>,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let mut result = Ok(());
    model.for_each_quad_seeded(seed, |quad| {
        if result.is_err() {
            return;
        }
        result = tint(quad).and_then(|color| emit(
            quad.positions.map(|p| matrix.transform_point3(Vec3::from_array(p)).to_array()),
            quad.uv,
            color,
            quad.texture_layer,
            quad.entity_material.unwrap_or(match quad.layer {
                RenderLayer::Opaque | RenderLayer::Cutout => EntityMaterial::ItemCutout,
                RenderLayer::Translucent => EntityMaterial::ItemTranslucent,
            }),
            quad.alpha,
            quad.light_with_emission(light),
            out,
        ));
    });
    result
}
pub(super) fn item_matrix(model: &BakedModel, context: &str) -> Mat4 {
    crate::ui::items::display_matrix(model, context)
}
/// Source `ItemInHandLayer` pose from an arm (or head) bone transform: rotate X -90 and
/// Y 180, then translate (+-1/16, 0.125, -0.625). Folded through the two rotations
/// that is (-+1/16, 0.625, -0.125) in the bone frame; heads take no sideways shift.
pub(super) fn held_item_attachment(bone: Mat4, slot: usize) -> Mat4 {
    let side = match slot {
        0 => -1.,
        1 => 1.,
        _ => 0.,
    } / 16.;
    bone * Mat4::from_translation(Vec3::new(side, 0.625, -0.125))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_y(std::f32::consts::PI)
}
/// Source `CrossedArmsItemLayer.applyTranslation`: from the arms bone, rotate X 0.75 rad,
/// scale 1.07, translate (0, 0.13, -0.34), then flip X by pi.
pub(super) fn crossed_arms_attachment(arms: Mat4) -> Mat4 {
    arms * Mat4::from_rotation_x(0.75)
        * Mat4::from_scale(Vec3::splat(1.07))
        * Mat4::from_translation(Vec3::new(0., 0.13, -0.34))
        * Mat4::from_rotation_x(std::f32::consts::PI)
}
/// Source `WitchItemLayer.applyTranslation` for a held potion: from the nose bone, translate
/// (1/16, 0.25, 0), then Z 180, X 140, Z 10 and X 180 degrees.
pub(super) fn witch_potion_attachment(nose: Mat4) -> Mat4 {
    nose * Mat4::from_translation(Vec3::new(0.0625, 0.25, 0.))
        * Mat4::from_rotation_z(std::f32::consts::PI)
        * Mat4::from_rotation_x(140f32.to_radians())
        * Mat4::from_rotation_z(10f32.to_radians())
        * Mat4::from_rotation_x(std::f32::consts::PI)
}
pub(super) struct EquipmentPose<'a> {
    pub entity: &'a Entity,
    pub rig: &'a Rig,
    pub transforms: &'a [Mat4],
    pub item_context: Option<&'a crate::ui::items::ItemContext>,
}

pub(super) fn append_equipment(
    pose: EquipmentPose<'_>,
    items: &mut crate::ui::items::ItemModels,
    equipment: &mut super::equipment::Cache,
    frame: super::equipment::Frame<'_>,
    out: &mut DynamicGeometry,
    stats: &mut PresentationStats,
) -> Result<()> {
    let EquipmentPose { entity: e, rig, transforms, item_context } = pose;
    let super::equipment::Frame { models, content, partial, light, .. } = frame;
    for slot in [4, 3, 2, 5, 0, 1, 6, 7] {
        let Some(stack) = e.equipment_value(slot) else {
            continue;
        };
        match equipment.append(e, slot, stack, frame, out) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(reason) => {
                stats
                    .unrendered
                    .insert(format!("{}:equipment:{slot}", e.type_name), reason);
                continue;
            }
        }
        // Villager-family rigs hold only the main-hand item, across the crossed arms (or on
        // the witch's nose for a potion), displayed as a ground item (`HoldingEntityRenderState`).
        if rig.bones.iter().any(|b| b.role == BoneRole::CrossedArms) && slot < 2 {
            if slot == 0 {
                let potion = e.type_name == "minecraft:witch"
                    && content.item_names.get(&stack.id).map(String::as_str)
                        == Some("minecraft:potion");
                let role = if potion {
                    BoneRole::Nose
                } else {
                    BoneRole::CrossedArms
                };
                let Some(bone) = rig
                    .bones
                    .iter()
                    .zip(transforms)
                    .find_map(|(b, m)| (b.role == role).then_some(*m))
                else {
                    stats.unrendered.insert(
                        format!("{}:equipment:0", e.type_name),
                        "rig has no matching equipment attachment".into(),
                    );
                    continue;
                };
                let state = crate::ui::items::ItemContext {
                    display: "ground".into(),
                    ..item_context.cloned().unwrap_or_default()
                };
                match items.model_with(content, stack, &state) {
                    Ok(model) => {
                        let attachment = if potion {
                            witch_potion_attachment(bone)
                        } else {
                            crossed_arms_attachment(bone)
                        };
                        append_baked_stack(model, attachment * item_matrix(model, "ground"), light, content, stack, out)?;
                    }
                    Err(reason) => {
                        stats
                            .unrendered
                            .insert(format!("{}:equipment:0", e.type_name), reason);
                    }
                }
            }
            continue;
        }
        let anatomical_hand = slot ^ usize::from(left_main_arm(models, e));
        let target = match slot {
            0 | 1 => if anatomical_hand == 0 { BoneRole::RightArm } else { BoneRole::LeftArm },
            5 => BoneRole::Head,
            _ => {
                stats.unrendered.insert(
                    format!("{}:equipment:{slot}", e.type_name),
                    "armor/body/saddle layer requires item equippable model attachment".into(),
                );
                continue;
            }
        };
        let Some((bone, matrix)) = rig
            .bones
            .iter()
            .zip(transforms)
            .find(|(b, _)| b.role == target)
        else {
            stats.unrendered.insert(
                format!("{}:equipment:{slot}", e.type_name),
                "rig has no matching equipment attachment".into(),
            );
            continue;
        };
        let _ = bone;
        let context = if slot < 2 && anatomical_hand == 0 {
            "thirdperson_righthand"
        } else if slot < 2 {
            "thirdperson_lefthand"
        } else {
            "head"
        };
        let flags = number(models, e, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
        let state = crate::ui::items::ItemContext {
            display: context.into(),
            using_item: flags & 1 != 0 && ((flags & 2 != 0) == (slot == 1)),
            use_ticks: e.source.as_ref().and_then(|s| s.living.as_ref())
                .map_or(0., |s| s.item_use_ticks(partial)),
            crossbow_pull: e.source.as_ref().and_then(|s| s.living.as_ref())
                .map_or(0., |s| s.crossbow_pull(partial)),
            ..item_context.cloned().unwrap_or_default()
        };
        match items.model_with(content, stack, &state) {
            Ok(model) => {
                let bone = if e.type_name == "minecraft:vex" && slot < 2 {
                    let side = if anatomical_hand == 0 { 0.046875 } else { -0.046875 };
                    *matrix
                        * Mat4::from_scale(Vec3::splat(0.55))
                        * Mat4::from_translation(Vec3::new(side, -0.15625, 0.078125))
                } else {
                    *matrix
                };
                let attachment = held_item_attachment(bone, if slot < 2 { anatomical_hand } else { slot });
                append_baked_stack(model, attachment * item_matrix(model, context), light, content, stack, out)?;
            }
            Err(reason) => {
                stats
                    .unrendered
                    .insert(format!("{}:equipment:{slot}", e.type_name), reason);
            }
        }
    }
    Ok(())
}
fn append_item_entity(
    e: &Entity,
    models: &Models,
    store: &mut DrawStore<'_>,
    content: &Content,
    frame: EntityDrawFrame,
    out: &mut DynamicGeometry,
    stats: &mut PresentationStats,
) -> Result<bool> {
    let EntityDrawFrame { origin, tick, partial, light, .. } = frame;
    let items = &mut *store.items;
    let clock_context = store.clock_context;
    let accessor = match e.type_name.as_str() {
        "minecraft:item" => "DATA_ITEM",
        "minecraft:item_display" => "DATA_ITEM_STACK_ID",
        _ => return Ok(false),
    };
    let Some(Metadata::Item(stack)) = value(models, e, accessor) else {
        return Ok(false);
    };
    if stack.empty() {
        return Ok(true);
    }
    let context = if e.type_name == "minecraft:item" {
        "ground"
    } else {
        "fixed"
    };
    let t = frame.transform;
    let mut pos = Vec3::new(
        (t.position[0] - origin[0]) as f32,
        (t.position[1] - origin[1]) as f32,
        (t.position[2] - origin[2]) as f32,
    );
    let age = tick as f32 + partial;
    if context == "ground" {
        pos.y += (age / 10.).sin() * 0.1 + 0.1;
    }
    let model = if let Some(clock) = clock_context {
        let mut state = clock.clone();
        state.display = context.into();
        items.model_with(content, stack, &state)
    } else {
        items.model(content, stack, context)
    };
    match model {
        Ok(model) => {
            let matrix = Mat4::from_translation(pos)
                * Mat4::from_rotation_y(if context == "ground" {
                    age / 20.
                } else {
                    t.yaw.to_radians()
                });
            append_baked_stack(model, matrix * item_matrix(model, context), light, content, stack, out)?;
        }
        Err(e) => {
            stats.unrendered.insert(e.clone(), e);
        }
    }
    Ok(true)
}
pub fn special_item(
    models: &Models,
    content: &Content,
    stack: &Stack,
    special: &Value,
) -> Result<BakedModel> {
    let kind = special["type"]
        .as_str()
        .ok_or("special item missing type")?
        .trim_start_matches("minecraft:");
    let shield_patterned = kind == "shield"
        && (stack.component("minecraft:base_color").is_some()
            || stack.component("minecraft:banner_patterns")
                .and_then(Value::as_array).is_some_and(|patterns| !patterns.is_empty()));
    let (rig, texture_name) = match kind {
        "shield" => ("minecraft:shield#main", if shield_patterned {
            "entity/shield/shield_base"
        } else {
            "entity/shield/shield_base_nopattern"
        }.to_owned()),
        "trident" => ("minecraft:trident#main", "entity/trident/trident".to_owned()),
        "chest" => (
            "minecraft:chest#main",
            format!(
                "entity/chest/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("normal")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "shulker_box" => (
            "minecraft:shulker_box#main",
            format!(
                "entity/shulker/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("shulker")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "conduit" => (
            "minecraft:conduit_shell#main",
            "entity/conduit/base".to_owned(),
        ),
        "decorated_pot" => (
            "minecraft:decorated_pot_base#main",
            "entity/decorated_pot/decorated_pot_base".to_owned(),
        ),
        "banner" => (
            "minecraft:standing_banner#main",
            "entity/banner/banner_base".to_owned(),
        ),
        "bed" => (
            match special["part"].as_str() {
                Some("head") => "minecraft:bed_head#main",
                Some("foot") => "minecraft:bed_foot#main",
                _ => return Err("bed item part must be head or foot".into()),
            },
            format!(
                "entity/bed/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("red")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "head" | "player_head" => {
            let kind = special["kind"].as_str().unwrap_or("player");
            match kind {
                "skeleton" => (
                    "minecraft:skeleton_skull#main",
                    "entity/skeleton/skeleton".to_owned(),
                ),
                "wither_skeleton" => (
                    "minecraft:wither_skeleton_skull#main",
                    "entity/skeleton/wither_skeleton".to_owned(),
                ),
                "zombie" => (
                    "minecraft:zombie_head#main",
                    "entity/zombie/zombie".to_owned(),
                ),
                "creeper" => (
                    "minecraft:creeper_head#main",
                    "entity/creeper/creeper".to_owned(),
                ),
                "piglin" => (
                    "minecraft:piglin_head#main",
                    "entity/piglin/piglin".to_owned(),
                ),
                "dragon" => (
                    "minecraft:dragon_skull#main",
                    "entity/enderdragon/dragon".to_owned(),
                ),
                "player" => (
                    "minecraft:player_head#main",
                    "entity/player/wide/steve".to_owned(),
                ),
                _ => return Err(format!("unknown special head family {kind}")),
            }
        }
        "copper_golem_statue" => (
            "minecraft:copper_golem#main",
            special["texture"]
                .as_str()
                .ok_or("copper golem statue item texture missing")?
                .to_owned(),
        ),
        _ => return Err(format!("unknown special item render family {kind}")),
    };
    // Solaris-authored rigs live in the embedded catalog, not in `models.rigs`; `Models::rig`
    // resolves both. A key with neither renders the placeholder box (logged once) rather than
    // failing the GUI/hand item bake and with it the session.
    if !models.has_authored_rig(rig) {
        static REPORTED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
        if REPORTED
            .lock()
            .map_or(true, |mut seen| seen.insert(rig.to_owned()))
        {
            eprintln!("special model rig not activated: {rig}; using placeholder");
        }
    }
    let rig = models.rig(rig);
    let tex = texture(content, &texture_name)?;
    let mut model = BakedModel::default();
    let mut banner_layers = Vec::new();
    if kind == "banner" || shield_patterned {
        let color = if shield_patterned {
            stack.component("minecraft:base_color").and_then(Value::as_str).unwrap_or("white")
        } else {
            special["color"].as_str().unwrap_or("white")
        };
        let rgb = |name: &str| {
            let c = super::render_blocks::dye(name).0;
            [
                (c >> 16 & 255) as f32 / 255.,
                (c >> 8 & 255) as f32 / 255.,
                (c & 255) as f32 / 255.,
            ]
        };
        let family = if shield_patterned { "shield" } else { "banner" };
        banner_layers.push((texture(content, &format!("entity/{family}/base"))?, rgb(color)));
        if let Some(patterns) = stack
            .component("minecraft:banner_patterns")
            .and_then(Value::as_array)
        {
            for pattern in patterns.iter().take(16) {
                let holder = &pattern["pattern"];
                let asset = if let Some(id) = holder.as_str() {
                    content
                        .registry_entries("minecraft:banner_pattern")?
                        .iter()
                        .find(|(name, _)| name == id)
                        .and_then(|(_, value)| value["asset_id"].as_str())
                        .ok_or("banner pattern asset missing")?
                } else {
                    holder["asset_id"]
                        .as_str()
                        .ok_or("banner pattern asset missing")?
                };
                let (namespace, path) = asset.split_once(':').unwrap_or(("minecraft", asset));
                let name = format!("{namespace}:entity/{family}/{path}");
                banner_layers.push((
                    texture(content, &name)?,
                    rgb(pattern["color"].as_str().unwrap_or("white")),
                ));
            }
        }
    }
    // Bed halves already use the half-local frame expected by the item graph.
    // Banners instead use the standing model's feet-at-24 frame.
    let item_root = if kind == "banner" {
        Mat4::from_translation(Vec3::new(0., -1.5, 0.))
    } else if kind == "chest" {
        // The rig is centered in X/Z and uses +Y down; item coordinates use +Y up.
        Mat4::from_translation(Vec3::new(0.5, 1.125, 0.5))
            * Mat4::from_scale(Vec3::new(1., -1., 1.))
    } else {
        Mat4::IDENTITY
    };
    let mut matrices = Vec::with_capacity(rig.bones.len());
    for bone in &rig.bones {
        let matrix = bone.parent.map(|i| matrices[i]).unwrap_or(item_root)
            * resources::transform(bone.pivot, bone.rotation, bone.scale);
        matrices.push(matrix);
        let reflected = kind == "chest" && matrix.determinant() < 0.;
        for q in &bone.quads {
            let mut positions = q.map(|v| {
                matrix
                    .transform_point3(Vec3::new(v[0], v[1], v[2]))
                    .to_array()
            });
            let mut uv = q.map(|v| {
                [
                    tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                    tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                ]
            });
            if reflected {
                positions.swap(1, 3);
                uv.swap(1, 3);
            }
            model.quads.push(BakedQuad {
                positions,
                uv,
                tint: TintKind::None,
                tint_index: None,
                cullface: None,
                face: 1,
                shade: true,
                shade_direction: None,
                gui_light_side: false,
                ambient_occlusion: false,
                light_emission: 0,
                layer: RenderLayer::Cutout,
                entity_material: matches!(kind, "banner" | "shield" | "bed" | "chest" | "decorated_pot").then_some(EntityMaterial::EntitySolid),
                alpha: 1.,
                texture_layer: tex.layer,
            });
            if (kind == "banner" && bone.name == "flag")
                || (shield_patterned && bone.name == "plate")
            {
                let p = positions.map(Vec3::from_array);
                let normal = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                for (index, (layer, color)) in banner_layers.iter().enumerate() {
                    model.quads.push(BakedQuad {
                        positions: p
                            .map(|v| (v + normal * (index + 1) as f32 * 0.0001).to_array()),
                        uv: q.map(|v| {
                            [
                                layer.uv[0] + v[3] * (layer.uv[2] - layer.uv[0]),
                                layer.uv[1] + v[4] * (layer.uv[3] - layer.uv[1]),
                            ]
                        }),
                        tint: TintKind::Constant(*color),
                        tint_index: None,
                        cullface: None,
                        face: 1,
                        shade: true,
                        shade_direction: None,
                        gui_light_side: false,
                        ambient_occlusion: false,
                        light_emission: 0,
                        layer: RenderLayer::Translucent,
                        entity_material: Some(EntityMaterial::BannerPattern),
                        alpha: 1.,
                        texture_layer: layer.layer,
                    });
                }
            }
        }
    }
    if kind == "decorated_pot" {
        let sides = models.rig("minecraft:decorated_pot_sides#main");
        let decorations = stack.component("minecraft:pot_decorations").and_then(Value::as_array);
        matrices.clear();
        for bone in &sides.bones {
            let matrix = bone.parent.map(|i| matrices[i]).unwrap_or(item_root)
                * resources::transform(bone.pivot, bone.rotation, bone.scale);
            matrices.push(matrix);
            let Some(index) = ["back", "left", "right", "front"].iter()
                .position(|side| *side == bone.name) else { continue };
            let item = decorations.and_then(|values| values.get(index))
                .and_then(Value::as_str).unwrap_or("minecraft:brick");
            let name = if let Some(pattern) = item.strip_prefix("minecraft:")
                .and_then(|name| name.strip_suffix("_pottery_sherd"))
            {
                format!("entity/decorated_pot/{pattern}_pottery_pattern")
            } else {
                "entity/decorated_pot/decorated_pot_side".to_owned()
            };
            let tex = texture(content, &name)?;
            for quad in &bone.quads {
                model.quads.push(BakedQuad {
                    positions: quad.map(|v| matrix.transform_point3(Vec3::new(v[0], v[1], v[2])).to_array()),
                    uv: quad.map(|v| [
                        tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                        tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                    ]),
                    tint: TintKind::None,
                    tint_index: None,
                    cullface: None,
                    face: 1,
                    shade: true,
                    shade_direction: None,
                    gui_light_side: false,
                    ambient_occlusion: false,
                    light_emission: 0,
                    layer: RenderLayer::Cutout,
                    entity_material: Some(EntityMaterial::EntitySolid),
                    alpha: 1.,
                    texture_layer: tex.layer,
                });
            }
        }
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::super::animation_tests::fixture;
    use super::*;

    #[test]
    fn wolf_tame_material_precedes_unexpired_anger() {
        use super::super::resources::Family;
        let (mut models, mut wolf) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_FLAGS_ID".into(), [18, 0]);
        schema.accessors.insert("DATA_ANGER_END_TIME".into(), [22, 2]);
        schema.accessors.insert("DATA_VARIANT_ID".into(), [23, 25]);
        wolf.type_name = "minecraft:wolf".into();
        wolf.metadata.insert(23, Metadata::Registry { serializer: 25, id: 0 });
        wolf.metadata.insert(22, Metadata::Long("1051".into()));
        models.schemas.insert(wolf.type_name.clone(), schema);
        models.families.insert(wolf.type_name.clone(), Family {
            rig: "test:wolf#main".into(), baby_rig: None, texture: None, textures: vec![],
            renderer_class: None, materials: BTreeMap::new(),
        });
        let mut store = Store::new();
        store.registries.insert("minecraft:wolf_variant".into(), vec![("test:wolf".into(),
            serde_json::json!({"assets":{"wild":"test:wild","angry":"test:angry","tame":"test:tame"}}))]);
        let mut content = Content::test_cube();
        for (name, layer) in [("test:wild", 1), ("test:angry", 2), ("test:tame", 3)] {
            content.insert_test_texture(name, TextureRegion { uv: [0., 0., 1., 1.], layer, width: 1, height: 1 });
        }
        assert_eq!(material(&store, &models, &wolf, &content, 1050).unwrap().texture.layer, 2);
        assert_eq!(material(&store, &models, &wolf, &content, 1051).unwrap().texture.layer, 1);
        wolf.metadata.insert(18, Metadata::Byte(4));
        assert_eq!(material(&store, &models, &wolf, &content, 1050).unwrap().texture.layer, 3);
    }

    #[test]
    fn received_bee_anger_deadline_changes_cached_rendered_texture() {
        use super::super::{cb, resources::{Bone, BoneRole, Family, Rig}};
        use crate::protocol::Reader;
        use std::sync::Arc;

        let (mut models, mut entity) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_BABY_ID".into(), [16, 8]);
        schema.accessors.insert("DATA_FLAGS_ID".into(), [18, 0]);
        schema.accessors.insert("DATA_ANGER_END_TIME".into(), [19, 2]);
        entity.type_name = "minecraft:bee".into();
        entity.source = None;
        models.schemas.insert(entity.type_name.clone(), schema);
        let names = ["bee.png", "bee_angry.png", "bee_nectar.png", "bee_angry_nectar.png",
            "bee_baby.png", "bee_angry_baby.png", "bee_nectar_baby.png", "bee_angry_nectar_baby.png"];
        models.families.insert(entity.type_name.clone(), Family {
            rig: "test:bee#main".into(), baby_rig: Some("test:bee#main".into()),
            texture: None, textures: names.iter().map(|name| format!("minecraft:entity/bee/{name}")).collect(),
            renderer_class: None, materials: BTreeMap::new(),
        });
        models.rigs.insert("test:bee#main".into(), Rig {
            bones: vec![Bone {
                parent: None, pivot: Vec3::ZERO, rotation: Vec3::ZERO, scale: Vec3::ONE, role: BoneRole::Fixed,
                name: "body".into(), quads: vec![[[0., 0., 0., 0., 0.], [1., 0., 0., 1., 0.],
                    [1., 1., 0., 1., 1.], [0., 1., 0., 0., 1.]]], normals: vec![[0., 0., 1.]],
            }], part_by_name: BTreeMap::new(),
        });
        let mut content = Content::test_cube();
        for (index, name) in names.iter().enumerate() {
            let key = format!("minecraft:entity/bee/{}", name.trim_end_matches(".png"));
            content.insert_test_texture(&key, TextureRegion { uv: [0., 0., 1., 1.],
                layer: index as u32 + 1, width: 64, height: 64 });
        }
        let mut store = Store::new();
        store.models = Some(Arc::new(models));
        store.ids.insert(41, 0);
        store.spatial.entry([0, 0]).or_default().insert(41);
        store.slots.push(entity);
        let update = |store: &mut Store, deadline: i64, flags: u8, baby: bool| {
            let mut packet = vec![41, 18, 0, flags, 16, 8, u8::from(baby), 19, 2];
            let mut value = deadline as u64;
            loop {
                let byte = (value & 127) as u8;
                value >>= 7;
                packet.push(byte | if value == 0 { 0 } else { 128 });
                if value == 0 { break; }
            }
            packet.push(255);
            store.apply(cb::SET_ENTITY_DATA, &mut Reader::new(&packet), &content, 1).unwrap();
        };
        let render = |store: &mut Store, time| {
            let mut out = DynamicGeometry::default();
            let stats = append(store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: time, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut out).unwrap();
            assert_eq!(stats.submitted, 1, "{:?}", stats.unrendered);
            assert!(!out.vertices.is_empty());
            let layer = out.vertices[0].texture_layer;
            assert!(out.vertices.iter().all(|vertex| vertex.texture_layer == layer));
            layer
        };
        update(&mut store, 1051, 8, false);
        assert_eq!(render(&mut store, 1050), 4);
        assert_eq!(render(&mut store, 1051), 3);
        assert_eq!(render(&mut store, 1052), 3);
        update(&mut store, 1051, 0, false);
        assert_eq!(render(&mut store, 1052), 1);
        update(&mut store, i64::from(i32::MAX) + 100, 8, true);
        assert_eq!(render(&mut store, 1052), 8);
        update(&mut store, -1, 8, true);
        assert_eq!(render(&mut store, 1052), 7);
        update(&mut store, 2000, 0, false);
        assert_eq!(render(&mut store, 1052), 2);
        assert_eq!(render(&mut store, 2000), 1);
        assert_eq!(render(&mut store, 1999), 2);
    }

    #[test]
    fn burning_flag_adds_fire_quads_only_for_visible_entities() {
        let (mut models, mut entity) = fixture();
        models.schemas.get_mut("minecraft:pig").unwrap().accessors
            .insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
        entity.binding = Some(Ok(Binding {
            rig: "minecraft:pig#renderer_main".into(),
            texture: TextureRegion { uv: [0., 0., 1., 1.], layer: 1, width: 64, height: 32 },
            style: RootStyle::Living,
            layers: None,
            anger: None,
        }));
        let id = entity.handle.network_id;
        let mut store = Store::new();
        store.models = Some(std::sync::Arc::new(models));
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        store.slots.push(entity);
        let mut content = Content::test_cube();
        for (name, layer) in [("minecraft:block/fire_0", 42), ("minecraft:block/fire_1", 43)] {
            content.insert_test_texture(name, TextureRegion {
                uv: [0., 0., 1., 1.], layer, width: 16, height: 16,
            });
        }
        for (flags, burning) in [(0, false), (1, true), (32, false), (33, false), (0, false)] {
            store.slots[0].metadata.insert(0, Metadata::Byte(flags));
            let mut geometry = DynamicGeometry::default();
            append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut geometry).unwrap();
            let fire: Vec<_> = geometry.vertices.iter()
                .filter(|v| v.texture_layer == 42 || v.texture_layer == 43).collect();
            assert_eq!(!fire.is_empty(), burning, "flags {flags}");
            assert_eq!(!geometry.indices[EntityMaterial::EntityFire as usize].is_empty(), burning);
            if burning {
                assert!(fire.iter().any(|v| v.texture_layer == 42));
                assert!(fire.iter().any(|v| v.texture_layer == 43));
                assert!(fire.iter().all(|v| v.light == [15., 15.]));
            }
        }
    }

    #[test]
    fn received_pet_sitting_flag_poses_and_restores_adult_and_baby_rigs() {
        for (kind, baby, key) in [
            ("minecraft:cat", false, "minecraft:cat#renderer_main"),
            ("minecraft:cat", true, "minecraft:cat_baby#main"),
            ("minecraft:wolf", false, "minecraft:wolf#renderer_main"),
            ("minecraft:wolf", true, "minecraft:wolf_baby#main"),
        ] {
            let (mut models, mut entity) = fixture();
            let mut schema = models.schemas.remove("minecraft:pig").unwrap();
            schema.accessors.insert("DATA_FLAGS_ID".into(), [18, 0]);
            schema.accessors.insert("DATA_BABY_ID".into(), [16, 8]);
            entity.type_name = kind.into();
            models.schemas.insert(kind.into(), schema);
            entity.metadata.insert(16, Metadata::Bool(baby));
            entity.metadata.insert(18, Metadata::Byte(4));
            let rig = super::super::solaris_models::authored(key).unwrap();
            let mut standing = Vec::new();
            evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut standing);
            entity.metadata.insert(18, Metadata::Byte(5));
            let mut sitting = Vec::new();
            evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut sitting);
            for name in ["body", "left_hind_leg", "right_hind_leg", "tail"] {
                let i = rig.bones.iter().position(|bone| bone.name == name).unwrap();
                assert_ne!(standing[i].to_cols_array(), sitting[i].to_cols_array(), "{key} {name}");
            }
            let head = rig.bones.iter().position(|bone| bone.name == "head").unwrap();
            assert_eq!(standing[head].to_cols_array(), sitting[head].to_cols_array(), "{key} head");
            entity.metadata.insert(18, Metadata::Byte(4));
            let mut restored = Vec::new();
            evaluate_rig(rig, &entity, &models, &crate::protocol::content::Content::test_cube(), 0, 0., Mat4::IDENTITY, &mut restored);
            assert_eq!(standing.iter().map(|m| m.to_cols_array()).collect::<Vec<_>>(),
                restored.iter().map(|m| m.to_cols_array()).collect::<Vec<_>>(), "{key}");
        }
    }

    #[test]
    fn goat_and_sheep_rigs_have_no_overlapping_same_facing_coplanar_faces() {
        use super::super::solaris_models::{BoneSpec, ModelFile};
        use glam::{Mat4, Vec3};
        // Rest-pose cube corners in pixels; bit 0/1/2 of the index select the x/y/z maximum.
        fn corners(bone: &BoneSpec, parent: Mat4, out: &mut Vec<[Vec3; 8]>) {
            let [rx, ry, rz] = bone.rotation.map(f32::to_radians);
            let m = parent
                * Mat4::from_translation(Vec3::from(bone.pivot))
                * Mat4::from_rotation_x(rx)
                * Mat4::from_rotation_y(ry)
                * Mat4::from_rotation_z(rz);
            for cube in &bone.cubes {
                let lo = Vec3::from(cube.origin) - Vec3::splat(cube.inflate);
                let size = Vec3::from(cube.size) + Vec3::splat(2. * cube.inflate);
                out.push(std::array::from_fn(|k| {
                    let bit = |b: usize| ((k >> b) & 1) as f32;
                    m.transform_point3(lo + size * Vec3::new(bit(0), bit(1), bit(2)))
                }));
            }
            for child in &bone.bones {
                corners(child, m, out);
            }
        }
        // (corner, edge u, edge v, outward normal) of every non-degenerate face.
        fn faces(c: &[Vec3; 8]) -> Vec<(Vec3, Vec3, Vec3, Vec3)> {
            let centre = c.iter().sum::<Vec3>() / 8.;
            [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]]
                .iter()
                .filter_map(|f| {
                    let (p0, e1, e2) = (c[f[0]], c[f[1]] - c[f[0]], c[f[3]] - c[f[0]]);
                    if e1.length() < 1e-4 || e2.length() < 1e-4 {
                        return None;
                    }
                    let mut n = e1.cross(e2).normalize();
                    if n.dot((c[f[0]] + c[f[2]]) / 2. - centre) < 0. {
                        n = -n;
                    }
                    Some((p0, e1, e2, n))
                })
                .collect()
        }
        for (name, source) in [
            ("goat", include_str!("../../assets/solaris/models/entity/goat.json")),
            ("goat_baby", include_str!("../../assets/solaris/models/entity/goat_baby.json")),
            ("sheep", include_str!("../../assets/solaris/models/entity/sheep.json")),
            ("sheep_baby", include_str!("../../assets/solaris/models/entity/sheep_baby.json")),
            ("sheep_wool", include_str!("../../assets/solaris/models/entity/sheep_wool.json")),
            ("sheep_wool_baby", include_str!("../../assets/solaris/models/entity/sheep_wool_baby.json")),
        ] {
            let model = ModelFile::parse(source).unwrap();
            let mut cubes = Vec::new();
            for bone in &model.bones {
                corners(bone, Mat4::IDENTITY, &mut cubes);
            }
            for (i, a) in cubes.iter().enumerate() {
                for (j, b) in cubes.iter().enumerate().skip(i + 1) {
                    for (a0, a1, a2, an) in faces(a) {
                        for (b0, b1, b2, bn) in faces(b) {
                            // Same-facing faces on one plane that overlap z-fight; opposite-facing
                            // contact faces are occluded by their own cubes.
                            if an.dot(bn) < 1. - 1e-4 || an.dot(b0 - a0).abs() > 1e-3 {
                                continue;
                            }
                            for u in [0.1, 0.3, 0.5, 0.7, 0.9] {
                                for v in [0.1, 0.3, 0.5, 0.7, 0.9] {
                                    let p = a0 + a1 * u + a2 * v - b0;
                                    let (s, t) = (p.dot(b1) / b1.dot(b1), p.dot(b2) / b2.dot(b2));
                                    assert!(
                                        !((1e-4..1. - 1e-4).contains(&s) && (1e-4..1. - 1e-4).contains(&t)),
                                        "{name}: cubes {i} and {j} share a coplanar face facing {an:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn allay_head_face_uv_contains_both_atlas_eyes() {
        use super::super::solaris_models::Face;
        let model = super::super::solaris_models::ModelFile::parse(include_str!(
            "../../assets/solaris/models/entity/allay.json"
        )).unwrap();
        let head = model.bones.iter().find(|bone| bone.name == "head").unwrap();
        let cube = &head.cubes[0];
        let [u0, v0, u1, v1] = cube.faces[&Face::Front];
        // Pixel centers independently observed in the runtime allay texture.
        for (u, v) in [(6.5, 7.5), (8.5, 7.5), (6.5, 8.5), (8.5, 8.5)] {
            assert!(u0 < u && u < u1 && v0 < v && v < v1);
        }
    }

    #[test]
    fn authored_trident_prongs_join_crossbar_at_shaft_tip() {
        let model = super::super::solaris_models::ModelFile::parse(include_str!(
            "../../assets/solaris/models/entity/trident.json"
        )).unwrap();
        let pole = model.bones.iter().find(|bone| bone.name == "pole").unwrap();
        let head = model.bones.iter().find(|bone| bone.name == "head").unwrap();
        let bar = &head.cubes[0];
        let pole_tip = pole.pivot[1] + pole.cubes[0].origin[1];
        let bar_bottom = head.pivot[1] + bar.origin[1];
        assert_eq!(head.cubes.len(), 4);
        assert!(bar_bottom > pole_tip);
        for prong in &head.cubes[1..] {
            assert_eq!(head.pivot[1] + prong.origin[1], pole_tip);
            assert_eq!(head.pivot[1] + prong.origin[1] + prong.size[1], bar_bottom);
            assert!(prong.origin[0] >= bar.origin[0]);
            assert!(prong.origin[0] + prong.size[0] <= bar.origin[0] + bar.size[0]);
        }
        assert!(bar.origin[0] < 0. && bar.origin[0] + bar.size[0] > 0.);
        // The default box unwrap samples transparent texels, visually severing the prongs.
        // This rectangle is opaque in the trident entity texture.
        use super::super::solaris_models::Face;
        for face in [Face::Up, Face::Down, Face::Front, Face::Back, Face::Left, Face::Right] {
            assert_eq!(bar.faces.get(&face), Some(&[0., 8., 4., 10.]), "{face:?}");
        }
    }

    #[test]
    fn trident_and_arrow_authored_axes_follow_same_projectile_orientation() {
        for (key, bone_name, axis, other) in [
            ("minecraft:arrow#renderer/model", "shaft", 0, 1),
            ("minecraft:trident#renderer/model", "pole", 1, 0),
        ] {
            let rig = super::super::solaris_models::authored(key).unwrap();
            let bone = rig.bones.iter().find(|bone| bone.name == bone_name).unwrap();
            let span = |index: usize| {
                let (lo, hi) = bone.quads.iter().flatten().map(|vertex| vertex[index])
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
                hi - lo
            };
            assert!(span(axis) > span(other) * 4., "{key}");
        }
        for yaw in [0., 90., 180., 270.] {
            for pitch in [0., 45., 270.] {
                let arrow = projectile_root(Vec3::ZERO, yaw, pitch, false).transform_vector3(Vec3::X);
                let trident = projectile_root(Vec3::ZERO, yaw, pitch, true).transform_vector3(-Vec3::Y);
                assert!((arrow - trident).length() < 1e-5, "{yaw} {pitch}");
            }
        }
    }

    #[test]
    fn decorated_pot_item_bakes_all_sides_and_received_sherds() {
        let (models, _) = fixture();
        let mut content = Content::test_cube();
        for (name, layer) in [
            ("minecraft:entity/decorated_pot/decorated_pot_base", 40),
            ("minecraft:entity/decorated_pot/decorated_pot_side", 41),
            ("minecraft:entity/decorated_pot/angler_pottery_pattern", 42),
        ] {
            content.insert_test_texture(name, TextureRegion {
                layer, uv: [0., 0., 1., 1.], width: 32, height: 32,
            });
        }
        let special = serde_json::json!({"type":"minecraft:decorated_pot"});
        let mut stack = Stack::default();
        let sides = super::super::solaris_models::authored("minecraft:decorated_pot_sides#main").unwrap();
        let side_count: usize = sides.bones.iter().map(|bone| bone.quads.len()).sum();
        let back_count = sides.bones.iter().find(|bone| bone.name == "back").unwrap().quads.len();
        assert!(back_count > 0 && side_count > back_count);
        let plain = special_item(&models, &content, &stack, &special).unwrap();
        assert_eq!(plain.quads.iter().filter(|quad| quad.texture_layer == 41).count(), side_count);
        stack.components = std::sync::Arc::new(BTreeMap::from([(
            "minecraft:pot_decorations".into(),
            serde_json::json!(["minecraft:angler_pottery_sherd", "minecraft:brick", "minecraft:brick", "minecraft:brick"]),
        )]));
        let patterned = special_item(&models, &content, &stack, &special).unwrap();
        assert_eq!(patterned.quads.iter().filter(|quad| quad.texture_layer == 42).count(), back_count);
        assert_eq!(patterned.quads.iter().filter(|quad| quad.texture_layer == 41).count(), side_count - back_count);
        assert!(patterned.quads.iter().any(|quad| quad.texture_layer == 40));
        assert!(patterned.quads.iter().all(|quad| quad.entity_material == Some(EntityMaterial::EntitySolid)));
    }


    #[test]
    fn boat_and_raft_hulls_anchor_to_received_position() {
        let position = Vec3::new(5.5, 159.8, -3.5);
        for key in [
            "minecraft:oak_boat#renderer/model",
            "minecraft:oak_chest_boat#renderer/model",
            "minecraft:bamboo_raft#renderer/model",
            "minecraft:bamboo_chest_raft#renderer/model",
        ] {
            let rig = super::super::solaris_models::authored(key).unwrap();
            let bottom = rig.bones.iter().find(|bone| bone.name == "bottom").unwrap();
            for yaw in [0., 90., 180., 270.] {
                let mut matrices = Vec::new();
                for bone in &rig.bones {
                    matrices.push(
                        bone.parent.map(|i| matrices[i]).unwrap_or_else(|| boat_root(position, yaw))
                            * resources::transform(bone.pivot, bone.rotation, bone.scale),
                    );
                }
                let matrix = matrices[rig.bones.iter().position(|bone| bone.name == "bottom").unwrap()];
                let (min_y, max_y) = bottom.quads.iter().flatten().fold(
                    (f32::INFINITY, f32::NEG_INFINITY),
                    |(min_y, max_y), vertex| {
                        let y = matrix.transform_point3(Vec3::new(vertex[0], vertex[1], vertex[2])).y;
                        (min_y.min(y), max_y.max(y))
                    },
                );
                assert!((min_y - position.y).abs() < 1e-4, "{key}: {min_y}");
                assert!((max_y - position.y - 2. / 16.).abs() < 1e-4, "{key}: {max_y}");
                if key.contains("_boat#") {
                    for name in ["left_wall", "right_wall", "bow", "stern"] {
                        let index = rig.bones.iter().position(|bone| bone.name == name).unwrap();
                        let wall_min = rig.bones[index].quads.iter().flatten()
                            .map(|vertex| matrices[index].transform_point3(
                                Vec3::new(vertex[0], vertex[1], vertex[2])
                            ).y)
                            .fold(f32::INFINITY, f32::min);
                        assert!((wall_min - max_y).abs() < 1e-4, "{key} {name}: {wall_min} vs {max_y}");
                    }
                }
            }
        }
    }

    #[test]
    fn experience_orb_received_value_emits_camera_facing_textured_geometry() {
        let (mut models, mut entity) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_VALUE".into(), [8, 1]);
        schema.dimensions.eye_height = 0.425;
        models.schemas.insert("minecraft:experience_orb".into(), schema);
        entity.type_name = "minecraft:experience_orb".into();
        let id = entity.handle.network_id;
        let mut store = Store::new();
        store.models = Some(std::sync::Arc::new(models));
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        store.slots.push(entity);
        let mut content = Content::test_cube();
        content.insert_test_texture("minecraft:entity/experience/experience_orb",
            TextureRegion { layer: 42, uv: [0., 0., 1., 1.], width: 64, height: 64 });
        for (value, u, v) in [(1, 0., 0.), (7, 0.5, 0.), (2477, 0.5, 0.5)] {
            store.slots[0].metadata.insert(8, Metadata::Int(value));
            let mut geometry = DynamicGeometry::default();
            let stats = append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [180., 0.], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([10., 2.])), bed_direction: &|_| None }, &mut geometry).unwrap();
            assert_eq!(stats.submitted, 1, "{:?}", stats.unrendered);
            assert!(stats.unrendered.is_empty());
            assert_eq!(geometry.vertices.len(), 4);
            assert_eq!(geometry.indices[EntityMaterial::EntityTranslucentCullItemTarget as usize].len(), 6);
            assert!(geometry.vertices.iter().all(|p| p.texture_layer == 42 && p.light == [10., 9.]));
            assert!(geometry.vertices.iter().all(|p| Vec3::from_array(p.normal).dot(Vec3::Y) > 0.999));
            let vertex = &geometry.vertices[0];
            assert!((vertex.position[0] + 0.15).abs() < 1e-5);
            assert!((vertex.position[1] - 0.025).abs() < 1e-5);
            assert!((vertex.uv[0] - u).abs() < 1e-5 && (vertex.uv[1] - v - 0.25).abs() < 1e-5);
        }
        let mut rotated = DynamicGeometry::default();
        let stats = append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [90., 0.], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([10., 2.])), bed_direction: &|_| None }, &mut rotated).unwrap();
        assert_eq!(stats.submitted, 1);
        assert!(rotated.vertices.iter().all(|p| p.position[0].abs() < 1e-5));
        assert!((rotated.vertices[0].position[2].abs() - 0.15).abs() < 1e-5);
        let mut pitched = DynamicGeometry::default();
        append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [90., 30.], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([10., 2.])), bed_direction: &|_| None }, &mut pitched).unwrap();
        let expected_normal = glam::Quat::from_euler(
            glam::EulerRot::YXZ, 90f32.to_radians(), -30f32.to_radians(), 0.,
        ) * Vec3::Y;
        assert!(pitched.vertices.iter().all(|p| Vec3::from_array(p.normal).dot(expected_normal) > 0.999));
        let mut geometry = DynamicGeometry::default();
        let stats = append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [180., 0.], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(None), bed_direction: &|_| None }, &mut geometry).unwrap();
        assert_eq!(stats.lighting_pending, 1);
        assert!(geometry.vertices.is_empty());
    }

    #[test]
    fn received_painting_uses_special_renderer_without_family_rig() {
        let (mut models, mut entity) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_PAINTING_VARIANT_ID".into(), [9, 34]);
        models.schemas.insert("minecraft:painting".into(), schema);
        entity.type_name = "minecraft:painting".into();
        entity.spawn_data = 3;
        entity.metadata.insert(9, Metadata::Registry { serializer: 34, id: 0 });
        let id = entity.handle.network_id;
        let mut store = Store::new();
        store.models = Some(std::sync::Arc::new(models));
        store.registries.insert(
            "minecraft:painting_variant".into(),
            vec![("minecraft:test".into(), serde_json::json!({
                "width": 4, "height": 4, "asset_id": "minecraft:test"
            }))],
        );
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        store.slots.push(entity);
        let mut content = Content::test_cube();
        let texture = TextureRegion { layer: 42, uv: [0., 0., 1., 1.], width: 16, height: 16 };
        content.insert_test_texture("minecraft:painting/test", texture);
        content.insert_test_texture("minecraft:painting/back", TextureRegion { layer: 43, ..texture });
        // The add-entity position is the hanging block anchor (vanilla Painting placement):
        // even sizes shift half a block left and up; the 1/16 mesh backs onto the wall.
        for (direction, axis, expected, lateral_min) in [(2, 2, 0.9375, -2.), (3, 2, 0.0625, -1.), (4, 0, 0.9375, -1.), (5, 0, 0.0625, -2.)] {
            store.slots[0].spawn_data = direction;
            let mut geometry = DynamicGeometry::default();
            let samples = std::cell::RefCell::new(Vec::new());
            let stats = append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &content, origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|point| { samples.borrow_mut().push(point); Ok(Some([15., 0.])) }, bed_direction: &|_| None }, &mut geometry).unwrap();
            assert_eq!(stats.submitted, 1);
            assert!(stats.unrendered.is_empty());
            assert_eq!(samples.borrow().len(), 16);
            assert!(samples.borrow().iter().all(|point| point[axis] == 0.));
            let front: Vec<_> = geometry.vertices.iter().filter(|v| v.texture_layer == 42).collect();
            assert!(!front.is_empty());
            assert!(front.iter().all(|v| (v.position[axis] - expected).abs() < 1e-5));
            let lateral_axis = if axis == 0 { 2 } else { 0 };
            let min = front.iter().map(|v| v.position[lateral_axis]).fold(f32::INFINITY, f32::min);
            let max = front.iter().map(|v| v.position[lateral_axis]).fold(f32::NEG_INFINITY, f32::max);
            assert!((min - lateral_min).abs() < 1e-5 && (max - lateral_min - 4.).abs() < 1e-5);
            let min_y = front.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            let max_y = front.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
            assert!((min_y + 1.).abs() < 1e-5 && (max_y - 3.).abs() < 1e-5);
        }
    }

    #[test]
    fn giant_renders_six_times_larger_at_the_same_feet() {
        use super::super::resources::{Bone, BoneRole, Rig};
        let (mut models, mut entity) = fixture();
        let (mut giant_models, _) = fixture();
        models.schemas.insert("minecraft:giant".into(), giant_models.schemas.remove("minecraft:pig").unwrap());
        models.rigs.insert("test:rig".into(), Rig {
            bones: vec![Bone {
                parent: None, pivot: Vec3::ZERO, rotation: Vec3::ZERO, scale: Vec3::ONE,
                role: BoneRole::Fixed, name: "body".into(),
                quads: vec![[[0., 0., 0., 0., 0.], [1., 0., 0., 1., 0.],
                    [1., 1., 0., 1., 1.], [0., 1., 0., 0., 1.]]],
                normals: vec![[0., 0., 1.]],
            }],
            part_by_name: BTreeMap::new(),
        });
        entity.source = None;
        entity.binding = Some(Ok(Binding {
            rig: "test:rig".into(),
            texture: TextureRegion { uv: [0., 0., 1., 1.], layer: 1, width: 64, height: 32 },
            style: RootStyle::Living, layers: None, anger: None,
        }));
        let id = entity.handle.network_id;
        let mut store = Store::new();
        store.models = Some(std::sync::Arc::new(models));
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        store.slots.push(entity);
        let render = |store: &mut Store| {
            let mut geometry = DynamicGeometry::default();
            let stats = append(store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut geometry).unwrap();
            assert!(stats.unrendered.is_empty(), "{:?}", stats.unrendered);
            geometry.vertices.iter().map(|v| v.position[1]).collect::<Vec<_>>()
        };
        let normal = render(&mut store);
        store.slots[0].type_name = "minecraft:giant".into();
        let giant = render(&mut store);
        assert!(!normal.is_empty());
        assert_eq!(normal.len(), giant.len());
        for (normal_y, giant_y) in normal.iter().zip(&giant) {
            assert!((giant_y - 6. * normal_y).abs() < 1e-5);
        }
    }

    #[test]
    fn slime_shell_is_translucent_and_hidden_with_the_entity() {
        for kind in ["minecraft:slime", "minecraft:magma_cube"] {
            let (mut models, mut entity) = fixture();
            let mut schema = models.schemas.remove("minecraft:pig").unwrap();
            schema.accessors.insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
            schema.accessors.insert("ID_SIZE".into(), [1, 1]);
            models.schemas.insert(kind.into(), schema);
            entity.type_name = kind.into();
            entity.binding = Some(Ok(Binding {
                rig: format!("{kind}#main"),
                texture: TextureRegion { uv: [0., 0., 1., 1.], layer: 1, width: 64, height: 32 },
                style: RootStyle::Living,
                layers: None,
                anger: None,
            }));
            let id = entity.handle.network_id;
            let mut store = Store::new();
            store.models = Some(std::sync::Arc::new(models));
            store.ids.insert(id, 0);
            store.spatial.entry([0, 0]).or_default().insert(id);
            store.slots.push(entity);
            for size in [1, 4] {
                store.slots[0].metadata.insert(1, Metadata::Int(size));
                for invisible in [false, true] {
                    store.slots[0].metadata.insert(0, Metadata::Byte(if invisible { 32 } else { 0 }));
                    let mut geometry = DynamicGeometry::default();
                    let stats = append(&mut store, crate::entities::EntityRenderFrame { maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|_, _| Ok([1.; 3]), content: &Content::test_cube(), origin: [0.; 3], camera: [0.; 3], camera_rotation: [0.; 2], radius: 32., tick: 0, partial: 0., game_time: 0, clock_environment: None, light: &|_| Ok(Some([15., 0.])), bed_direction: &|_| None }, &mut geometry).unwrap();
                    assert!(stats.unrendered.is_empty(), "{kind}: {:?}", stats.unrendered);
                    let cutout = &geometry.indices[EntityMaterial::EntityCutout as usize];
                    let translucent = &geometry.indices[EntityMaterial::EntityTranslucent as usize];
                    assert_eq!(cutout.is_empty(), invisible, "{kind} size {size}");
                    assert_eq!(translucent.is_empty(), invisible || kind == "minecraft:magma_cube",
                        "{kind} size {size}");
                }
            }
        }
    }

    #[test]
    fn cube_model_size_scales_geometry_about_entity_feet() {
        let feet = Vec3::new(3., 7., -2.);
        let root = Mat4::from_translation(feet);
        let point = Vec3::new(0.25, 0.5, -0.25);
        for size in [None, Some(1)] {
            let transformed = scale_cube_model(root, feet, size).transform_point3(point);
            assert!((transformed - (feet + point)).abs().max_element() < 1e-6);
        }
        let large = scale_cube_model(root, feet, Some(4));
        assert!((large.transform_point3(Vec3::ZERO) - feet).abs().max_element() < 1e-6);
        assert!((large.transform_point3(point) - (feet + point * 4.)).abs().max_element() < 1e-6);
    }

    #[test]
    fn snow_golem_pumpkin_tracks_the_head_and_encloses_it() {
        let content = Content::test_cube();
        let model = &content.models[0];
        let mut normal = DynamicGeometry::default();
        let mut tilted = DynamicGeometry::default();
        append_snow_pumpkin(model, Mat4::IDENTITY, [15., 0.], &mut normal).unwrap();
        let rotation = Mat4::from_rotation_z(0.4);
        append_snow_pumpkin(model, rotation, [15., 0.], &mut tilted).unwrap();
        let (min, max) = normal.vertices.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), vertex| {
                let point = Vec3::from_array(vertex.position);
                (min.min(point), max.max(point))
            },
        );
        assert!((min - Vec3::new(-0.3125, -0.0625, -0.3125)).abs().max_element() < 1e-5);
        assert!((max - Vec3::new(0.3125, 0.5625, 0.3125)).abs().max_element() < 1e-5);
        assert_eq!(normal.vertices.len(), tilted.vertices.len());
        for (a, b) in normal.vertices.iter().zip(&tilted.vertices) {
            let expected = rotation.transform_point3(Vec3::from_array(a.position));
            assert!((expected - Vec3::from_array(b.position)).abs().max_element() < 1e-5);
        }
    }

    #[test]
    fn chest_item_faces_remain_outward_and_keep_uvs_after_reflection() {
        let (models, _) = fixture();
        let mut content = Content::test_cube();
        content.insert_test_texture("minecraft:entity/chest/normal", TextureRegion {
            layer: 42, uv: [0., 0., 1., 1.], width: 64, height: 64,
        });
        let item = special_item(
            &models, &content, &Stack::default(),
            &serde_json::json!({"type":"minecraft:chest","texture":"normal"}),
        ).unwrap();
        let rig = models.rig("minecraft:chest#main");
        let root = Mat4::from_translation(Vec3::new(0.5, 1.125, 0.5))
            * Mat4::from_scale(Vec3::new(1., -1., 1.));
        let mut matrices = Vec::new();
        let mut emitted = item.quads.iter();
        for bone in &rig.bones {
            let matrix = bone.parent.map(|i| matrices[i]).unwrap_or(root)
                * resources::transform(bone.pivot, bone.rotation, bone.scale);
            matrices.push(matrix);
            for source in &bone.quads {
                let quad = emitted.next().unwrap();
                let p = quad.positions.map(Vec3::from_array);
                let actual = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
                let a = Vec3::new(source[0][0], source[0][1], source[0][2]);
                let b = Vec3::new(source[1][0], source[1][1], source[1][2]);
                let c = Vec3::new(source[2][0], source[2][1], source[2][2]);
                let normal = matrix.inverse().transpose().transform_vector3((b - a).cross(c - a)).normalize();
                assert!(actual.dot(normal) > 0.99);
                for vertex in source {
                    let pos = matrix.transform_point3(Vec3::new(vertex[0], vertex[1], vertex[2]));
                    assert!((0..4).any(|i| {
                        (p[i] - pos).abs().max_element() < 1e-5
                            && (quad.uv[i][0] - vertex[3]).abs() < 1e-5
                            && (quad.uv[i][1] - vertex[4]).abs() < 1e-5
                    }));
                }
            }
        }
        assert!(emitted.next().is_none());
    }

    #[test]
    fn chest_item_lid_is_above_body_in_item_space() {
        let (models, _) = fixture();
        let mut content = Content::test_cube();
        content.insert_test_texture("minecraft:entity/chest/normal", TextureRegion {
            layer: 42, uv: [0., 0., 1., 1.], width: 64, height: 64,
        });
        let special = serde_json::json!({"type":"minecraft:chest","texture":"normal"});
        let item = special_item(&models, &content, &Stack::default(), &special).unwrap();
        let y_bounds = |quads: &[BakedQuad]| {
            quads.iter().flat_map(|q| q.positions).map(|p| p[1])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), y| (lo.min(y), hi.max(y)))
        };
        let (body_bottom, body_top) = y_bounds(&item.quads[..6]);
        let (lid_bottom, lid_top) = y_bounds(&item.quads[6..12]);
        assert!(body_bottom >= 0. && lid_bottom >= body_top - 1e-6);
        assert!(lid_top > body_top && lid_top <= 1.);
    }

    #[test]
    fn bed_item_graph_joins_both_full_half_models() {
        let (models, _) = fixture();
        let mut content = Content::test_cube();
        content.insert_test_texture(
            "minecraft:entity/bed/red",
            TextureRegion {
                layer: 42,
                uv: [0., 0., 1., 1.],
                width: 64,
                height: 64,
            },
        );
        let stack = Stack::default();
        let q = std::f32::consts::FRAC_1_SQRT_2;
        // 26.1.2 red_bed special-node transforms, before the common display transform.
        let rotation = Mat4::from_quat(glam::Quat::from_xyzw(0., -q, q, 0.));
        for (part, graph_z, expected_z) in [
            ("head", 1., [0., 1.]),
            ("foot", 0., [-1., 0.]),
        ] {
            let special = serde_json::json!({"type":"minecraft:bed","part":part,"texture":"red"});
            let model = special_item(&models, &content, &stack, &special).unwrap();
            let graph = Mat4::from_translation(Vec3::new(1., 0.5625, graph_z)) * rotation;
            let mut low = Vec3::splat(f32::INFINITY);
            let mut high = Vec3::splat(f32::NEG_INFINITY);
            for quad in &model.quads {
                for position in quad.positions {
                    let p = graph.transform_point3(Vec3::from_array(position));
                    low = low.min(p);
                    high = high.max(p);
                }
            }
            let expected_low = Vec3::new(0., 0., expected_z[0]);
            let expected_high = Vec3::new(1., 0.5625, expected_z[1]);
            assert!((low - expected_low).abs().max_element() < 1e-5, "{part}: {low:?}");
            assert!((high - expected_high).abs().max_element() < 1e-5, "{part}: {high:?}");
        }
    }

    #[test]
    fn breeze_wind_emits_wrapped_animated_texture_layer() {
        let rig = super::super::solaris_models::authored("minecraft:breeze_wind#main").unwrap();
        let transforms = vec![Mat4::IDENTITY; rig.bones.len()];
        let tex = TextureRegion {
            layer: 42,
            uv: [0.25, 0.125, 0.5, 0.375],
            width: 128,
            height: 128,
        };
        let mut first = DynamicGeometry::default();
        let mut second = DynamicGeometry::default();
        let style = |scroll| WrappedStyle {
            tex, scroll: [scroll, 0.], color: [1.; 3], layer: EntityMaterial::BreezeWind, light: [15., 15.],
        };
        emit_wrapped(rig, &transforms, style(0.), &mut first).unwrap();
        emit_wrapped(rig, &transforms, style(0.25), &mut second).unwrap();
        assert!(!first.indices[EntityMaterial::BreezeWind as usize].is_empty());
        assert_eq!(first.vertices.len(), second.vertices.len());
        for (a, b) in first.vertices.iter().zip(&second.vertices) {
            assert_eq!(a.texture_layer, 42);
            assert_eq!(a.overlay_uv, [0.25, 0.125]);
            assert_eq!(a.overlay_tint[..2], [0.25, 0.25]);
            assert!((b.uv[0] - a.uv[0] - 0.25).abs() < 0.0001);
            assert_eq!(a.uv[1], b.uv[1]);
        }
    }

    #[test]
    fn pig_uses_received_default_variant_only_when_metadata_is_absent() {
        let (mut models, mut pig) = fixture();
        models.schemas.get_mut("minecraft:pig").unwrap().accessors
            .insert("DATA_VARIANT_ID".into(), [19, 0]);
        let mut store = Store::default();
        let warm = serde_json::json!({"asset_id":"test:warm"});
        let temperate = serde_json::json!({"asset_id":"test:temperate"});
        store.registries.insert("minecraft:pig_variant".into(), vec![
            ("minecraft:warm".into(), warm.clone()),
            ("minecraft:temperate".into(), temperate.clone()),
        ]);
        assert_eq!(variant(&store, &models, &pig), Some(&temperate));
        pig.metadata.insert(19, Metadata::Int(0));
        assert_eq!(variant(&store, &models, &pig), Some(&warm));
        pig.metadata.insert(19, Metadata::Int(99));
        assert_eq!(variant(&store, &models, &pig), None);
    }

    #[test]
    fn zombie_nautilus_uses_received_default_without_masking_explicit_variant() {
        let (mut models, mut entity) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_VARIANT_ID".into(), [19, 0]);
        entity.type_name = "minecraft:zombie_nautilus".into();
        models.schemas.insert(entity.type_name.clone(), schema);
        let mut store = Store::default();
        assert_eq!(variant(&store, &models, &entity), None);
        let warm = serde_json::json!({"asset_id":"test:warm"});
        let temperate = serde_json::json!({"asset_id":"test:temperate"});
        store.registries.insert(
            "minecraft:zombie_nautilus_variant".into(),
            vec![
                ("minecraft:warm".into(), warm.clone()),
                ("minecraft:temperate".into(), temperate.clone()),
            ],
        );
        assert_eq!(variant(&store, &models, &entity), Some(&temperate));
        entity.metadata.insert(19, Metadata::Int(0));
        assert_eq!(variant(&store, &models, &entity), Some(&warm));
        entity.metadata.insert(19, Metadata::Int(99));
        assert_eq!(variant(&store, &models, &entity), None);
    }

    #[test]
    fn chicken_uses_received_default_without_masking_explicit_variant() {
        let (mut models, mut entity) = fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_VARIANT_ID".into(), [19, 0]);
        entity.type_name = "minecraft:chicken".into();
        models.schemas.insert(entity.type_name.clone(), schema);
        let mut store = Store::default();
        assert_eq!(variant(&store, &models, &entity), None);
        let warm = serde_json::json!({"asset_id":"test:warm"});
        let temperate = serde_json::json!({"asset_id":"test:temperate"});
        store.registries.insert(
            "minecraft:chicken_variant".into(),
            vec![
                ("minecraft:warm".into(), warm.clone()),
                ("minecraft:temperate".into(), temperate.clone()),
            ],
        );
        assert_eq!(variant(&store, &models, &entity), Some(&temperate));
        entity.metadata.insert(19, Metadata::Int(0));
        assert_eq!(variant(&store, &models, &entity), Some(&warm));
        entity.metadata.insert(19, Metadata::Int(99));
        assert_eq!(variant(&store, &models, &entity), None);
        entity.metadata.insert(19, Metadata::Int(-1));
        assert_eq!(variant(&store, &models, &entity), None);
        entity.metadata.remove(&19);
        store.registries.get_mut("minecraft:chicken_variant").unwrap().pop();
        assert_eq!(variant(&store, &models, &entity), Some(&warm));
    }

    #[test]
    fn dragon_uses_its_base_texture_when_family_has_no_default_texture() {
        let (mut models, mut dragon) = fixture();
        dragon.type_name = "minecraft:ender_dragon".into();
        let facts: Value = serde_json::from_str(include_str!(
            "../../assets/solaris/entities/entity-facts.json"
        ))
        .unwrap();
        let family = serde_json::from_value(facts["families"]["minecraft:ender_dragon"].clone())
            .unwrap();
        models.families.insert(dragon.type_name.clone(), family);
        let mut content = Content::test_cube();
        content.insert_test_texture(
            "minecraft:entity/enderdragon/dragon",
            TextureRegion {
                layer: 42,
                uv: [0., 0., 1., 1.],
                width: 256,
                height: 256,
            },
        );
        let binding = material(&Store::default(), &models, &dragon, &content, 0).unwrap();
        assert_eq!(binding.texture.layer, 42);
    }
}
