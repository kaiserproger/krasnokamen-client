//! Source-specific nonliving presentation. Shared mesh emission and item baking have one owner.
mod display_snapshot;
pub(super) mod item_frame;
mod items;
mod kernels;
mod painting;
mod projectiles;
mod timeline;
mod world_fx;
use super::{Entity, Handle, Metadata, Models, Store, presentation as p, resources};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    inventory::Stack,
    protocol::{
        Result,
        content::{Content, TextureRegion},
        models::BakedModel,
    },
    ui::{
        items::{ItemContext, ItemModels},
        shaping::TextLayout,
    },
};
pub(crate) use display_snapshot::DisplaySnapshot;
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::collections::BTreeMap;
pub(crate) use timeline::{State, TickEffects};
fn source_sin(angle: f32) -> f32 {
    crate::player::math::sin(f64::from(angle))
}
fn source_cos(angle: f32) -> f32 {
    crate::player::math::cos(f64::from(angle))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handled {
    Unhandled,
    Rendered,
    NoGeometry,
    PendingLighting,
    PendingWorld,
}
struct CachedText {
    revision: u64,
    layout: TextLayout,
}
struct ObservedItem {
    source: Value,
    stack: Option<Stack>,
}
#[derive(Default)]
pub(crate) struct Cache {
    models: BTreeMap<String, BakedModel>,
    text: BTreeMap<Handle, CachedText>,
    observed_items: BTreeMap<usize, ObservedItem>,
    clock_needles: BTreeMap<Handle, std::sync::Arc<std::sync::Mutex<crate::ui::item_dynamic::Needles>>>,
    dynamic_models: BTreeMap<String, Result<bool>>,
    tnt_state: Option<u32>,
}
impl Cache {
    pub fn remove(&mut self, handle: Handle) {
        self.text.remove(&handle);
        self.clock_needles.remove(&handle);
    }
    pub(super) fn item_context(
        &mut self,
        entity: &Entity,
        stacks: [Option<&Stack>; 3],
        content: &Content,
        position: [f64; 3],
        environment: Option<super::ClockEnvironment<'_>>,
    ) -> Result<Option<ItemContext>> {
        let Some((base, time_at)) = environment else { return Ok(None) };
        if !stacks.iter().flatten().any(|stack| !stack.empty()) {
            self.clock_needles.remove(&entity.handle);
            return Ok(None);
        }
        fn dynamic(value: &Value) -> bool {
            match value {
                Value::Object(fields) => {
                    fields.get("property").and_then(Value::as_str).is_some_and(|name| {
                        matches!(name.strip_prefix("minecraft:").unwrap_or(name), "time" | "compass")
                    }) || fields.values().any(dynamic)
                }
                Value::Array(values) => values.iter().any(dynamic),
                _ => false,
            }
        }
        let mut needs_world = false;
        for stack in stacks.into_iter().flatten().filter(|s| !s.empty()) {
            let Some(id) = crate::ui::items::item_model_id(content, stack) else { continue };
            if !self.dynamic_models.contains_key(id) {
                if self.dynamic_models.len() >= 4096 {
                    return Err("dynamic item model cache bound".into());
                }
                let (ns, path) = crate::ui::items::item_model_location(id);
                let result = content.resource_bytes(&format!("assets/{ns}/items/{path}.json"))
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()))
                    .map(|graph| dynamic(&graph));
                self.dynamic_models.insert(id.into(), result);
            }
            needs_world |= *self.dynamic_models[id].as_ref().map_err(Clone::clone)?;
        }
        if !needs_world {
            self.clock_needles.remove(&entity.handle);
            return Ok(None);
        }
        let Some(mut world) = base.world.clone() else {
            return Ok(None);
        };
        let (sun_angle, moon_phase) = time_at(position)?;
        world.position = position;
        world.yaw = entity.transform.yaw;
        world.sun_angle = sun_angle;
        world.moon_phase = moon_phase;
        world.needles = self.clock_needles.entry(entity.handle).or_default().clone();
        let mut context = base.clone();
        context.seed = entity.handle.network_id;
        context.world = Some(world);
        Ok(Some(context))
    }
    fn model<'a>(&'a mut self, content: &Content, name: &str) -> Result<&'a BakedModel> {
        if !self.models.contains_key(name) {
            if self.models.len() >= 512 {
                return Err("special entity cold model cache bound".into());
            }
            self.models.insert(name.into(), content.bake_model(name)?);
        }
        Ok(&self.models[name])
    }
}
fn translation(position: [f64; 3], origin: [f64; 3]) -> Mat4 {
    Mat4::from_translation(Vec3::from_array(std::array::from_fn(|i| {
        (position[i] - origin[i]) as f32
    })))
}
fn textured_quad(
    root: Mat4,
    points: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    texture: TextureRegion,
    color: [f32; 3],
    alpha: f32,
    layer: EntityMaterial,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    p::emit(
        points.map(|point| root.transform_point3(Vec3::from_array(point)).to_array()),
        uv.map(|uv| {
            [
                texture.uv[0] + uv[0] * (texture.uv[2] - texture.uv[0]),
                texture.uv[1] + uv[1] * (texture.uv[3] - texture.uv[1]),
            ]
        }),
        color,
        texture.layer,
        layer,
        alpha,
        light,
        out,
    )
}
pub(crate) use items::{append_observed_item, ObservedItemFrame};

#[expect(clippy::too_many_arguments, reason = "Shares the scene extraction inputs without a second render-state owner")]
pub(crate) fn append(
    store: &mut Store,
    slot: usize,
    models: &Models,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    clock_context: Option<&ItemContext>,
    maps: crate::ui::MapRenderView<'_>,
    light_source: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    tint_source: &dyn Fn(crate::protocol::models::TintKind, [i32; 3]) -> Result<[f32; 3]>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let entity = &store.slots[slot];
    if !matches!(entity.type_name.as_str(),
        "minecraft:painting" | "minecraft:item_frame" | "minecraft:glow_item_frame"
        | "minecraft:tnt" | "minecraft:falling_block" | "minecraft:lightning_bolt")
        && entity.leash_value().is_none() && entity.leash_holder_id.is_none()
    {
        return Ok(Handled::Unhandled);
    }
    let vertex_start = out.vertices.len();
    let index_starts = out.indices.each_ref().map(Vec::len);
    let result = (|| {
        let entity = &store.slots[slot];
        if entity.type_name == "minecraft:lightning_bolt" {
            return world_fx::append_bolt(entity, origin, tick, partial, out);
        }
        if !matches!(entity.type_name.as_str(),
            "minecraft:painting" | "minecraft:item_frame" | "minecraft:glow_item_frame"
            | "minecraft:tnt" | "minecraft:falling_block") {
            return world_fx::append_leash(entity, store, models, origin, tick, partial, light_source, out);
        }
        if store.slots[slot].type_name == "minecraft:painting" {
            return painting::append(&store.slots[slot], store, models,
                content, origin, tick, partial, light_source, out);
        }
        let entity = &store.slots[slot];
        let (dimensions, scale) = models.dimensions(entity)?;
        let Some(light) = p::entity_light(entity, models, tick, partial,
            dimensions, scale, light_source)? else {
            return Ok(Handled::PendingLighting);
        };
        if matches!(entity.type_name.as_str(), "minecraft:tnt" | "minecraft:falling_block") {
            return world_fx::append_block(store, slot, models, content,
                p::EntityDrawFrame { origin, tick, partial, light, parent: None }, tint_source, out);
        }
        item_frame::append(entity, models, item_frame::ItemFrameResources {
            cache: &mut store.special_render_cache, items: &mut store.items, maps,
        }, content, p::EntityDrawFrame { origin, tick, partial, light, parent: None }, clock_context, out)
    })();
    if result.is_err() {
        out.vertices.truncate(vertex_start);
        for (indices, start) in out.indices.iter_mut().zip(index_starts) {
            indices.truncate(start);
        }
    }
    result
}
pub(super) use items::observed_stack;
