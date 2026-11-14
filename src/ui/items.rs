//! Resource item graphs evaluated against actual stack and presentation context.
use crate::{
    api::{EntityMaterial, RenderLayer},
    entities,
    inventory::Stack,
    protocol::{
        Result,
        content::Content,
        models::{BakedModel, TintKind},
    },
};
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct ItemContext {
    pub display: String,
    /// Exact ItemModelResolver seed supplied by the Source presentation owner.
    pub seed: i32,
    pub using_item: bool,
    pub use_ticks: f32,
    pub crossbow_pull: f32,
    pub fishing_cast: bool,
    pub dimension: String,
    pub daytime: f32,
    pub random_time: f32,
    pub spawn_compass: f32,
    pub lodestone_compass: f32,
    pub recovery_compass: f32,
    pub local_date: String,
    pub selected_bundle: Option<usize>,
    pub(crate) world: Option<super::item_dynamic::ItemWorldContext>,
}
impl Default for ItemContext {
    fn default() -> Self {
        Self {
            display: "gui".into(),
            seed: 0,
            using_item: false,
            use_ticks: 0.,
            crossbow_pull: 0.,
            fishing_cast: false,
            dimension: "minecraft:overworld".into(),
            daytime: 0.,
            random_time: 0.,
            spawn_compass: 0.,
            lodestone_compass: 0.,
            recovery_compass: 0.,
            local_date: String::new(),
            selected_bundle: None,
            world: None,
        }
    }
}
/// The stack override is authoritative, including an explicit false.
pub(crate) fn has_foil(stack: &Stack) -> bool {
    stack
        .component("minecraft:enchantment_glint_override")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            stack
                .component("minecraft:enchantments")
                .and_then(Value::as_object)
                .is_some_and(|entries| !entries.is_empty())
        })
}

/// Mark item vertices for a silhouette-masked sample of the item glint atlas.
/// Negative overlay coordinates are otherwise unused by item materials.
pub(crate) fn mark_foil(
    vertices: &mut [crate::api::Vertex],
    texture: crate::protocol::content::TextureRegion,
) {
    for vertex in vertices {
        vertex.overlay_uv = [-2. - texture.uv[0], -2. - texture.uv[1]];
        vertex.overlay_tint = [
            texture.uv[2] - texture.uv[0],
            texture.uv[3] - texture.uv[1],
            texture.layer as f32,
        ];
    }
}

pub(crate) fn foil_texture(
    c: &Content,
    stack: &Stack,
) -> Result<Option<crate::protocol::content::TextureRegion>> {
    if has_foil(stack) {
        Ok(Some(
            c.texture("minecraft:misc/enchanted_glint_item")
                .ok_or("item glint texture not activated")?,
        ))
    } else {
        Ok(None)
    }
}
/// Logs a failed item-model bake once per item/error (never per frame).
pub(crate) fn report_item_model_failure(c: &Content, s: &Stack, error: &str) {
    static REPORTED: std::sync::Mutex<std::collections::BTreeSet<String>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let item = c.item_names.get(&s.id).map_or("<unknown>", String::as_str);
    let line = format!("item {item}: {error}");
    if REPORTED
        .lock()
        .map_or(true, |mut seen| seen.insert(line.clone()))
    {
        eprintln!("item model unavailable, drawing empty icon: {line}");
    }
}
pub(crate) fn item_model_id<'a>(content: &'a Content, stack: &'a Stack) -> Option<&'a str> {
    stack
        .components
        .get("minecraft:item_model")
        .and_then(Value::as_str)
        .or_else(|| content.item_names.get(&stack.id).map(String::as_str))
}

pub(crate) fn item_model_location(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}
/// `swap_animation_scale` from the item definition (26.3): scales the
/// hotbar-switch equip animation of the newly selected item. Default 1.
pub(crate) fn swap_animation_scale(content: &Content, stack: &Stack) -> f32 {
    let Some(id) = item_model_id(content, stack) else {
        return 1.;
    };
    let (ns, path) = item_model_location(id);
    let Ok(bytes) = content.resource_bytes(&format!("assets/{ns}/items/{path}.json")) else {
        return 1.;
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value["swap_animation_scale"].as_f64())
        .map(|v| v as f32)
        .unwrap_or(1.)
}

/// The presentation-context inputs one selection read on its graph path. A selection is a
/// function of the definition, the stack, the display context and exactly these inputs, so a
/// caller that keys them (and the stack and `ItemModels::generation`) may reuse its result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reads(u8);
impl Reads {
    pub(crate) const USING_ITEM: Self = Self(1);
    pub(crate) const USE_TICKS: Self = Self(2);
    pub(crate) const CROSSBOW_PULL: Self = Self(4);
    /// Inputs a caller cannot key cheaply (world needles, time, dimension, date, bundle
    /// selection, fishing cast): select again every frame.
    pub(crate) const VOLATILE: Self = Self(8);
    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Default)]
pub struct ItemModels {
    definitions: BTreeMap<String, Arc<Value>>,
    baked: BTreeMap<String, BakedModel>,
    /// Retained selections with their geometry shape (see `model_shape`).
    selected: BTreeMap<
        (u32, usize, String, Vec<usize>),
        (Arc<BTreeMap<String, Value>>, BakedModel, [f32; 6], u64),
    >,
    /// Shape of each distinct selected geometry, keyed by everything entity emission reads.
    shapes: BTreeMap<Vec<u32>, u64>,
    entities: Option<Arc<entities::Models>>,
    /// Changes whenever retained resources are released (`generation`).
    generation: u64,
    /// Graph evaluations so far (tests prove memoized callers skip them).
    #[cfg(test)]
    selections: usize,
}
/// Shape identities are process-unique, so a cleared cache can never alias an old one.
static NEXT_SHAPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// Generations are process-unique too: a new `ItemModels` never repeats an old one.
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
impl ItemModels {
    pub fn new() -> Self {
        Self::default()
    }
    /// Identity of the resources selections are made from: results a caller retained (shapes,
    /// models it compared) stay valid while this is unchanged.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    #[cfg(test)]
    pub(crate) fn selections(&self) -> usize {
        self.selections
    }
    fn release(&mut self) {
        self.generation = NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn set_entity_models(&mut self, models: Arc<entities::Models>) {
        self.entities = Some(models);
        self.selected.clear();
        self.release();
    }
    /// Refresh the listed-player snapshot used to resolve player-head skins;
    /// profiled heads depend on it, so the selection cache is cleared with it.
    pub fn set_entity_profiles(
        &mut self,
        profiles: Arc<BTreeMap<String, entities::PlayerProfile>>,
    ) {
        self.profiles = Some(profiles);
        self.selected.clear();
    }
    /// Registers `model` under `id` with a one-node item graph (test fixtures only).
    #[cfg(test)]
    pub(crate) fn insert_test_model(&mut self, id: &str, model: BakedModel) {
        self.definitions.insert(
            id.into(),
            Arc::new(serde_json::json!({"model":{"type":"minecraft:model","model":id}})),
        );
        self.baked.insert(id.into(), model);
    }
    #[cfg(test)]
    pub(crate) fn insert_test_graph(&mut self, id: &str, graph: Value) {
        self.definitions
            .insert(id.into(), Arc::new(serde_json::json!({"model": graph})));
    }
    pub fn clear(&mut self) {
        self.definitions.clear();
        self.baked.clear();
        self.selected.clear();
        // Texture layers and UVs in the keys refer to the released atlas.
        self.shapes.clear();
        self.release();
    }
    /// GUI/icon variant of `model_with`: an item whose model cannot be baked is logged once per
    /// item/error and drawn as an empty icon. A single bad item must never end the session.
    pub fn icon_model<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> &'a BakedModel {
        static EMPTY: std::sync::LazyLock<BakedModel> =
            std::sync::LazyLock::new(BakedModel::default);
        match self.model_with(c, s, ctx) {
            Ok(model) => model,
            Err(error) => {
                report_item_model_failure(c, s, &error);
                &EMPTY
            }
        }
    }
    pub fn model<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        display: &str,
    ) -> Result<&'a BakedModel> {
        let ctx = ItemContext {
            display: display.into(),
            ..ItemContext::default()
        };
        self.model_with(c, s, &ctx)
    }
    pub fn model_with<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<&'a BakedModel> {
        self.model_with_bounds(c, s, ctx).map(|(model, _)| model)
    }
    pub fn model_with_bounds<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<(&'a BakedModel, [f32; 6])> {
        self.select(c, s, ctx)
            .map(|selected| (&selected.1, selected.2))
    }
    /// `model_with` plus the selection's shape and the context inputs it read: equal shapes
    /// mean identical entity geometry (display transform of `ctx`, quad positions, UVs,
    /// colours, layers, materials, alpha and emission), whichever stack or graph produced it.
    pub(crate) fn model_shape<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<(&'a BakedModel, u64)> {
        self.select(c, s, ctx)
            .map(|selected| (&selected.1, selected.3))
    }
    #[expect(clippy::type_complexity, reason = "the retained selection entry")]
    fn select<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<(
        &'a (Arc<BTreeMap<String, Value>>, BakedModel, [f32; 6], u64),
        Reads,
    )> {
        if s.empty() {
            return Err("empty stack has no item model".into());
        }
        #[cfg(test)]
        {
            self.selections += 1;
        }
        let definition = self.definition(c, s)?;
        let graph = &definition["model"];
        let mut nodes = Vec::new();
        let mut reads = Reads::default();
        evaluate(graph, s, ctx, 0, Mat4::IDENTITY, &mut nodes, &mut reads)?;
        // The selected resource-node identities capture dynamic branches; a node also fixes its
        // graph transformation (one path from the root). The geometry reads the stack itself
        // only through special models, nested bundle items and stack tints: only then is the
        // stack part of the key, its immutable component handle retained alongside, so address
        // reuse cannot alias a different stack. Equal stacks in separate allocations (every
        // received equipment stack) otherwise share one selection.
        let mut identities = Vec::with_capacity(nodes.len() + 1);
        identities.push(ctx.seed as u32 as usize);
        identities.extend(nodes.iter().map(|(node, _)| *node as *const Value as usize));
        let contains_selected_item = nodes.iter().any(|(node, _)| {
            node["type"]
                .as_str()
                .is_some_and(|ty| short(ty) == "bundle/selected_item")
        });
        if contains_selected_item {
            identities.extend([
                ctx.selected_bundle.unwrap_or(usize::MAX),
                ctx.using_item as usize,
                ctx.fishing_cast as usize,
                ctx.use_ticks.to_bits() as usize,
                ctx.crossbow_pull.to_bits() as usize,
                ctx.daytime.to_bits() as usize,
                ctx.random_time.to_bits() as usize,
                ctx.spawn_compass.to_bits() as usize,
                ctx.lodestone_compass.to_bits() as usize,
                ctx.recovery_compass.to_bits() as usize,
            ]);
            identities.extend(ctx.dimension.bytes().map(usize::from));
            identities.push(usize::MAX);
            identities.extend(ctx.local_date.bytes().map(usize::from));
        }
        let reads_stack = nodes.iter().any(|(node, _)| node_reads_stack(node));
        let key = (
            s.id,
            if reads_stack {
                Arc::as_ptr(&s.components) as usize
            } else {
                0
            },
            ctx.display.clone(),
            identities,
        );
        // Nested selected items can change their dynamic branch within a tick.
        if contains_selected_item {
            self.selected.remove(&key);
        }
        if !self.selected.contains_key(&key) {
            let mut result = BakedModel::default();
            for (node, transformation) in nodes {
                let mut model = match short(node["type"].as_str().ok_or("item graph type missing")?)
                {
                    "model" => {
                        let model_id = node["model"]
                            .as_str()
                            .ok_or("item graph model ID missing")?;
                        if !self.baked.contains_key(model_id) {
                            self.baked.insert(model_id.into(), c.bake_model(model_id)?);
                        }
                        self.baked[model_id].clone()
                    }
                    "special" => {
                        let models = self
                            .entities
                            .as_ref()
                            .ok_or("entity model resources not activated")?;
                        let mut special = entities::special_item(
                            models,
                            c,
                            s,
                            &node["model"],
                            self.profiles.as_deref(),
                        )?;
                        let base = c.bake_model(
                            node["base"].as_str().ok_or("special item base missing")?,
                        )?;
                        special.display = base.display;
                        special
                    }
                    "bundle/selected_item" => {
                        let nested = crate::inventory::bundle_item(
                            s,
                            ctx.selected_bundle.ok_or("bundle_selection_required")?,
                            c,
                        )?;
                        let nested_context = ItemContext {
                            selected_bundle: None,
                            ..ctx.clone()
                        };
                        self.model_with(c, &nested, &nested_context)?.clone()
                    }
                    _ => return Err("invalid evaluated item node".into()),
                };
                let tints = node["tints"].as_array();
                for q in &mut model.quads {
                    // Vanilla leaves a layer untinted when its index has no tint source.
                    // Potion declares only layer0; its generated bottle layer1 has no tint.
                    q.tint = match q
                        .tint_index
                        .and_then(|index| tints.and_then(|tints| tints.get(index as usize)))
                    {
                        Some(tint) => TintKind::Constant(tint_color(c, tint, s)?),
                        None if short(node["type"].as_str().unwrap_or("")) == "special" => q.tint,
                        None => TintKind::None,
                    };
                }
                if transformation != Mat4::IDENTITY {
                    apply_graph_transform(&mut model, transformation);
                }
                if ctx.display == "gui"
                    && short(node["type"].as_str().unwrap_or("")) == "special"
                    && matches!(
                        short(node["model"]["type"].as_str().unwrap_or("")),
                        "head" | "player_head"
                    )
                {
                    sort_gui_special_head(&mut model);
                }
                result.quads.extend(model.quads);
                result.display.extend(model.display);
            }
            // Bound retained dynamic variants: old geometry can be recreated from immutable resources.
            if self.selected.len() >= 4096 {
                self.selected.clear();
            }
            if self.shapes.len() >= 4096 {
                self.shapes.clear();
            }
            let display = display_matrix(&result, &ctx.display);
            let bounds = model_bounds(&result, display);
            let shape = *self
                .shapes
                .entry(shape_key(&result, display))
                .or_insert_with(|| NEXT_SHAPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            return Ok(self.selected.entry(key).or_insert((
                Arc::clone(&s.components),
                result,
                bounds,
                shape,
            )));
        }
        Ok((&self.selected[&key], reads))
    }
    key
}
/// Whether an evaluated node's geometry reads the stack: special models (banner patterns,
/// profiles), nested bundle items and stack-sourced tints do; plain models with constant or
/// grass tints are fixed by the node.
fn node_reads_stack(node: &Value) -> bool {
    short(node["type"].as_str().unwrap_or("")) != "model"
        || node["tints"].as_array().is_some_and(|tints| {
            tints.iter().any(|tint| {
                !matches!(
                    short(tint["type"].as_str().unwrap_or("")),
                    "constant" | "grass"
                )
            })
        })
}
/// Everything entity emission (`presentation::append_baked`) reads from a selection in one
/// display context, as bits: equal keys draw identical triangles.
fn shape_key(model: &BakedModel, display: Mat4) -> Vec<u32> {
    let mut key = Vec::with_capacity(16 + model.quads.len() * 25);
    key.extend(display.to_cols_array().map(f32::to_bits));
    for q in &model.quads {
        let color = match q.tint {
            TintKind::Constant(color) => color,
            _ => [1.; 3],
        };
        let material = q.entity_material.unwrap_or(match q.layer {
            RenderLayer::Opaque | RenderLayer::Cutout => EntityMaterial::ItemCutout,
            RenderLayer::Translucent => EntityMaterial::ItemTranslucent,
        });
        key.extend(
            q.positions
                .iter()
                .flatten()
                .chain(q.uv.iter().flatten())
                .chain(&color)
                .map(|v| v.to_bits()),
        );
        key.extend([
            q.alpha.to_bits(),
            q.texture_layer,
            material as u32,
            u32::from(q.light_emission),
        ]);
    }
    key
}
// GUI overlays have no depth buffer. Keep the cutout material and its foil
// semantics, but paint special head faces far-to-near in GUI display space.
fn sort_gui_special_head(model: &mut BakedModel) {
    let display = display_matrix(model, "gui");
    let depth = |q: &crate::protocol::models::BakedQuad| {
        q.positions
            .iter()
            .map(|p| display.transform_point3(Vec3::from_array(*p)).z)
            .sum::<f32>()
            * 0.25
    };
    // Heads have few quads; stable insertion sort preserves coplanar skin layers
    // without allocating a second list of geometry.
    for i in 1..model.quads.len() {
        let mut j = i;
        while j > 0
            && depth(&model.quads[j])
                .total_cmp(&depth(&model.quads[j - 1]))
                .is_lt()
        {
            model.quads.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// Source `ItemTransform.apply`: translate, rotate by the quaternion
/// `rotationXYZ(rx, ry, rz)` (matrix `Rx * Ry * Rz`, so a vertex is turned about Z
/// first and X last), scale, then recentre the unit-cube model on the origin.
/// `ItemDisplayContext.leftHand()` is true for both left-hand contexts, first and
/// third person, and negates the translation X and the Y/Z rotations.
pub(crate) fn display_matrix(model: &BakedModel, context: &str) -> Mat4 {
    if let Some(t) = model.display.get(context) {
        let mirrored = matches!(context, "thirdperson_lefthand" | "firstperson_lefthand");
        let translation = Vec3::from_array(t.translation)
            * if mirrored {
                Vec3::new(-1., 1., 1.)
            } else {
                Vec3::ONE
            };
        let rotation = Vec3::from_array(t.rotation)
            * if mirrored {
                Vec3::new(1., -1., -1.)
            } else {
                Vec3::ONE
            };
        Mat4::from_translation(translation)
            * Mat4::from_rotation_x(rotation.x.to_radians())
            * Mat4::from_rotation_y(rotation.y.to_radians())
            * Mat4::from_rotation_z(rotation.z.to_radians())
            * Mat4::from_scale(Vec3::from_array(t.scale))
            * Mat4::from_translation(Vec3::splat(-0.5))
    } else {
        Mat4::from_translation(Vec3::splat(-0.5))
    }
}
fn model_bounds(model: &BakedModel, matrix: Mat4) -> [f32; 6] {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for quad in &model.quads {
        for position in &quad.positions {
            let point = matrix.transform_point3(Vec3::from_array(*position));
            low = low.min(point);
            high = high.max(point);
        }
    }
    if model.quads.is_empty() {
        [0.; 6]
    } else {
        [low.x, low.y, low.z, high.x, high.y, high.z]
    }
}
fn short(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}
fn evaluate<'a>(
    v: &'a Value,
    s: &Stack,
    c: &ItemContext,
    depth: u8,
    parent_transform: Mat4,
    out: &mut Vec<(&'a Value, Mat4)>,
    reads: &mut Reads,
) -> Result<()> {
    if depth > 64 || out.len() > 256 {
        return Err("item graph resource bound".into());
    }
    let transformation = if v["transformation"].is_null() {
        parent_transform
    } else {
        parent_transform * graph_transform(&v["transformation"])?
    };
    let kind = short(v["type"].as_str().ok_or("item graph type missing")?);
    if matches!(
        kind,
        "condition" | "select" | "range_dispatch" | "bundle/selected_item"
    ) {
        reads.insert(context_reads(kind, v));
    }
    match kind {
        "model" | "special" => out.push((v, transformation)),
        "empty" => {}
        "composite" => {
            for node in v["models"]
                .as_array()
                .ok_or("composite item models missing")?
            {
                evaluate(node, s, c, depth + 1, transformation, out, reads)?;
            }
        }
        "condition" => {
            let b = condition(v, s, c)?;
            evaluate(
                &v[if b { "on_true" } else { "on_false" }],
                s,
                c,
                depth + 1,
                transformation,
                out,
                reads,
            )?;
        }
        "select" => {
            let selected = selection(v, s, c)?;
            let mut chosen = &v["fallback"];
            for case in v["cases"].as_array().ok_or("item select cases missing")? {
                let when = &case["when"];
                if when.as_str() == Some(selected.as_str())
                    || when
                        .as_array()
                        .is_some_and(|xs| xs.iter().any(|x| x.as_str() == Some(selected.as_str())))
                {
                    chosen = &case["model"];
                    break;
                }
            }
            if !chosen.is_null() {
                evaluate(chosen, s, c, depth + 1, transformation, out, reads)?;
            }
        }
        "range_dispatch" => {
            let n = number(v, s, c)? * v["scale"].as_f64().unwrap_or(1.) as f32;
            let mut chosen = &v["fallback"];
            let mut threshold = f32::NEG_INFINITY;
            for entry in v["entries"]
                .as_array()
                .ok_or("item range entries missing")?
            {
                let t = entry["threshold"]
                    .as_f64()
                    .ok_or("item range threshold missing")? as f32;
                if t <= n && t >= threshold {
                    threshold = t;
                    chosen = &entry["model"];
                }
            }
            if !chosen.is_null() {
                evaluate(chosen, s, c, depth + 1, transformation, out, reads)?;
            }
        }
        "bundle/selected_item" => {
            if c.selected_bundle.is_some() {
                out.push((v, transformation));
            }
        }
        other => return Err(format!("unsupported item graph node {other}")),
    }
    Ok(())
}
/// The context inputs a `condition`, `select`, `range_dispatch` or `bundle/selected_item`
/// node reads (`condition`, `selection`, `number`); stack components and the display
/// context are keyed by every caller.
fn context_reads(kind: &str, v: &Value) -> Reads {
    match (kind, v["property"].as_str().map(short)) {
        ("condition", Some("using_item")) => Reads::USING_ITEM,
        ("condition", Some("has_component" | "broken"))
        | ("select", Some("display_context" | "block_state" | "trim_material" | "charge_type")) => {
            Reads::default()
        }
        ("range_dispatch", Some("use_duration" | "use_cycle")) => {
            Reads(Reads::USING_ITEM.0 | Reads::USE_TICKS.0)
        }
        ("range_dispatch", Some("crossbow/pull")) => {
            Reads(Reads::USING_ITEM.0 | Reads::CROSSBOW_PULL.0)
        }
        _ => Reads::VOLATILE,
    }
}
fn component<'a>(s: &'a Stack, k: &str) -> Option<&'a Value> {
    s.component(k)
}
fn condition(v: &Value, s: &Stack, c: &ItemContext) -> Result<bool> {
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item condition property missing")?,
        ) {
            "using_item" => c.using_item,
            "fishing_rod/cast" => c.fishing_cast,
            "has_component" => component(
                s,
                v["component"]
                    .as_str()
                    .ok_or("component condition missing ID")?,
            )
            .is_some(),
            "broken" => {
                component(s, "minecraft:damage")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    >= component(s, "minecraft:max_damage")
                        .and_then(Value::as_i64)
                        .unwrap_or(i64::MAX)
                        - 1
            }
            "bundle/has_selected_item" => c.selected_bundle.is_some(),
            other => return Err(format!("unsupported item condition {other}")),
        },
    )
}
fn selection(v: &Value, s: &Stack, c: &ItemContext) -> Result<String> {
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item selection property missing")?,
        ) {
            "display_context" => c.display.clone(),
            "context_dimension" => c.dimension.clone(),
            "local_time" => {
                if v["pattern"].as_str() != Some("MM-dd") {
                    return Err("unsupported item local-time pattern".into());
                }
                c.local_date.clone()
            }
            "block_state" => component(s, "minecraft:block_state")
                .and_then(|x| x.get(v["block_state_property"].as_str().unwrap_or("")))
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            "trim_material" => component(s, "minecraft:trim")
                .and_then(|x| x["material"].as_str())
                .unwrap_or("")
                .into(),
            "charge_type" => {
                let projectiles =
                    component(s, "minecraft:charged_projectiles").and_then(Value::as_array);
                if projectiles.is_some_and(|x| !x.is_empty()) {
                    if projectiles
                        .unwrap()
                        .iter()
                        .any(|x| x["id"].as_str() == Some("minecraft:firework_rocket"))
                    {
                        "rocket"
                    } else {
                        "arrow"
                    }
                } else {
                    "none"
                }
                .into()
            }
            other => return Err(format!("unsupported item select property {other}")),
        },
    )
}
fn number(v: &Value, s: &Stack, c: &ItemContext) -> Result<f32> {
    if let Some(value) = c
        .world
        .as_ref()
        .and_then(|world| world.number(v, s, c.seed))
    {
        return Ok(value);
    }
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item range property missing")?,
        ) {
            "compass" => match v["target"].as_str() {
                Some("spawn") => c.spawn_compass,
                Some("lodestone") => c.lodestone_compass,
                Some("recovery") => c.recovery_compass,
                _ => return Err("compass target missing".into()),
            },
            "time" => match v["source"].as_str() {
                Some("daytime") => c.daytime,
                Some("random") => c.random_time,
                _ => return Err("item time source missing".into()),
            },
            "use_duration" => {
                if c.using_item {
                    c.use_ticks
                } else {
                    0.
                }
            }
            "use_cycle" => {
                if c.using_item {
                    c.use_ticks
                        .rem_euclid(v["period"].as_f64().unwrap_or(1.) as f32)
                } else {
                    0.
                }
            }
            "crossbow/pull" => {
                if c.using_item {
                    c.crossbow_pull
                } else {
                    0.
                }
            }
            other => return Err(format!("unsupported item range property {other}")),
        },
    )
}
fn tint_color(c: &Content, v: &Value, s: &Stack) -> Result<[f32; 3]> {
    let default = v["default"].as_i64().unwrap_or(0xffffff) as u32;
    let color = match short(v["type"].as_str().ok_or("item tint type missing")?) {
        "constant" => v["value"].as_i64().ok_or("constant item tint missing")? as u32,
        "dye" => component(s, "minecraft:dyed_color")
            .and_then(Value::as_i64)
            .map(|x| x as u32)
            .unwrap_or(default),
        "map_color" => component(s, "minecraft:map_color")
            .and_then(Value::as_i64)
            .map(|x| x as u32)
            .unwrap_or(default),
        "potion" => super::tint::potion(component(s, "minecraft:potion_contents"), default)?,
        "firework" => {
            let colors =
                component(s, "minecraft:firework_explosion").and_then(|x| x["colors"].as_array());
            if let Some(xs) = colors.filter(|xs| !xs.is_empty()) {
                let mut sums = [0u64; 3];
                for x in xs {
                    let n = x.as_u64().ok_or("invalid firework color")?;
                    for (i, shift) in [16, 8, 0].into_iter().enumerate() {
                        sums[i] += (n >> shift) & 255;
                    }
                }
                ((sums[0] / xs.len() as u64) << 16
                    | (sums[1] / xs.len() as u64) << 8
                    | sums[2] / xs.len() as u64) as u32
            } else {
                default
            }
        }
        "grass" => {
            let temperature = v["temperature"]
                .as_f64()
                .ok_or("grass tint temperature missing")? as f32;
            let downfall = v["downfall"]
                .as_f64()
                .ok_or("grass tint downfall missing")? as f32;
            if !temperature.is_finite() || !downfall.is_finite() {
                return Err("grass tint range".into());
            }
            return c.grass_tint(temperature, downfall);
        }
        other => return Err(format!("unsupported item tint {other}")),
    };
    Ok([
        ((color >> 16) & 255) as f32 / 255.,
        ((color >> 8) & 255) as f32 / 255.,
        (color & 255) as f32 / 255.,
    ])
}
fn graph_transform(t: &Value) -> Result<Mat4> {
    if t.is_null() {
        return Ok(Mat4::IDENTITY);
    }
    fn vec3(t: &Value, k: &str, default: [f32; 3]) -> Result<glam::Vec3> {
        if t[k].is_null() {
            return Ok(glam::Vec3::from_array(default));
        }
        let a = t[k].as_array().ok_or("invalid item transform")?;
        if a.len() != 3 {
            return Err("invalid item transform vector".into());
        }
        Ok(glam::Vec3::new(
            a[0].as_f64().ok_or("item transform value")? as f32,
            a[1].as_f64().ok_or("item transform value")? as f32,
            a[2].as_f64().ok_or("item transform value")? as f32,
        ))
    }
    fn quat(t: &Value, k: &str) -> Result<glam::Quat> {
        if t[k].is_null() {
            return Ok(glam::Quat::IDENTITY);
        }
        let a = t[k].as_array().ok_or("invalid item transform quaternion")?;
        if a.len() != 4 {
            return Err("invalid item transform quaternion".into());
        }
        Ok(glam::Quat::from_xyzw(
            a[0].as_f64().ok_or("item quaternion")? as f32,
            a[1].as_f64().ok_or("item quaternion")? as f32,
            a[2].as_f64().ok_or("item quaternion")? as f32,
            a[3].as_f64().ok_or("item quaternion")? as f32,
        ))
    }
    // Vanilla parses item transforms in pixel units (translation * 1/16,
    // clamped) and ItemTransform.apply ends with a -0.5 model-space centering.
    let translation = vec3(t, "translation", [0.; 3])? * (1. / 16.);
    let scale = vec3(t, "scale", [1.; 3])?;
    let left = quat(t, "left_rotation")?;
    let right = quat(t, "right_rotation")?;
    Ok(Mat4::from_translation(translation)
        * Mat4::from_quat(left)
        * Mat4::from_scale(scale)
        * Mat4::from_quat(right)
        * Mat4::from_translation(Vec3::splat(-0.5)))
}

fn apply_graph_transform(m: &mut BakedModel, transformation: Mat4) {
    for q in &mut m.quads {
        for p in &mut q.positions {
            *p = transformation
                .transform_point3(Vec3::from_array(*p))
                .to_array();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::models::ModelTransform;
    use serde_json::json;

    #[test]
    fn foil_override_takes_precedence_over_enchantments() {
        let mut stack = Stack::default();
        assert!(!has_foil(&stack));
        Arc::make_mut(&mut stack.components).insert(
            "minecraft:enchantments".into(),
            json!({"minecraft:sharpness": 1}),
        );
        assert!(has_foil(&stack));
        Arc::make_mut(&mut stack.components)
            .insert("minecraft:enchantment_glint_override".into(), json!(false));
        assert!(!has_foil(&stack));
        Arc::make_mut(&mut stack.components).clear();
        Arc::make_mut(&mut stack.components)
            .insert("minecraft:enchantment_glint_override".into(), json!(true));
        assert!(has_foil(&stack));
    }

    #[test]
    fn charge_type_uses_received_projectile_ids() {
        let mut stack = Stack::default();
        let property = json!({"property": "minecraft:charge_type"});
        let context = ItemContext::default();
        for (projectiles, expected) in [
            (json!([]), "none"),
            (json!([{"id": "minecraft:arrow"}]), "arrow"),
            (json!([{"id": "minecraft:firework_rocket"}]), "rocket"),
        ] {
            Arc::make_mut(&mut stack.components)
                .insert("minecraft:charged_projectiles".into(), projectiles);
            assert_eq!(selection(&property, &stack, &context).unwrap(), expected);
        }
    }

    #[test]
    fn clock_graph_uses_same_received_owner_in_gui_and_first_person() {
        use super::super::item_dynamic::ItemWorldContext;
        let graph = json!({"type":"minecraft:range_dispatch","property":"minecraft:time",
            "source":"daytime","wobble":false,
            "fallback":{"type":"minecraft:model","model":"test:early"},
            "entries":[{"threshold":0.5,"model":{"type":"minecraft:model","model":"test:late"}}]});
        let world = ItemWorldContext {
            dimension: "minecraft:overworld".into(),
            position: [0., 64., 0.],
            yaw: 0.,
            game_tick: 1,
            sun_angle: 90.,
            moon_phase: 0.,
            spawn: None,
            death: None,
            needles: Arc::default(),
        };
        let mut ctx = ItemContext {
            world: Some(world),
            ..Default::default()
        };
        let stack = Stack::default();
        for display in ["gui", "firstperson_righthand"] {
            ctx.display = display.into();
            let mut nodes = Vec::new();
            evaluate(
                &graph,
                &stack,
                &ctx,
                0,
                Mat4::IDENTITY,
                &mut nodes,
                &mut Reads::default(),
            )
            .unwrap();
            assert_eq!(nodes[0].0["model"], "test:early");
        }
        let world = ctx.world.as_mut().unwrap();
        world.game_tick += 1;
        world.sun_angle = 270.;
        for display in ["gui", "firstperson_righthand"] {
            ctx.display = display.into();
            let mut nodes = Vec::new();
            evaluate(
                &graph,
                &stack,
                &ctx,
                0,
                Mat4::IDENTITY,
                &mut nodes,
                &mut Reads::default(),
            )
            .unwrap();
            assert_eq!(nodes[0].0["model"], "test:late");
        }
    }

    #[test]
    fn selected_bundle_rechecks_nested_dynamic_branch_within_tick() {
        use super::super::item_dynamic::ItemWorldContext;
        use crate::inventory::{ComponentPatch, ComponentValue, Tag};

        let content = Content::test_cube();
        let mut models = ItemModels::new();
        let cube = content.models[0].clone();
        let quad_count = cube.quads.len();
        models.baked.insert("test:early".into(), cube);
        models
            .baked
            .insert("test:late".into(), BakedModel::default());
        models.definitions.insert(
            "test:clock".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:range_dispatch", "property":"minecraft:time",
                "source":"daytime", "wobble":false,
                "fallback":{"type":"minecraft:model","model":"test:early"},
                "entries":[{"threshold":0.5,"model":{"type":"minecraft:model","model":"test:late"}}]
            }})),
        );
        models.definitions.insert(
            "test:bundle".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:bundle/selected_item"
            }})),
        );
        let mut nested = Stack::default();
        nested.id = 1;
        nested.count = 1;
        Arc::make_mut(&mut nested.components)
            .insert("minecraft:item_model".into(), json!("test:clock"));
        let mut bundle = Stack::default();
        bundle.id = 2;
        bundle.count = 1;
        Arc::make_mut(&mut bundle.components)
            .insert("minecraft:item_model".into(), json!("test:bundle"));
        bundle.patch = Arc::new(ComponentPatch {
            added: BTreeMap::from([(
                0,
                ComponentValue {
                    name: "minecraft:bundle_contents".into(),
                    value: Tag::Empty,
                    wire_range: 0..0,
                    adventure: None,
                    hash: None,
                    nested_stacks: Arc::from([nested]),
                },
            )]),
            removed: Vec::new(),
        });
        let world = ItemWorldContext {
            dimension: "minecraft:overworld".into(),
            position: [0., 64., 0.],
            yaw: 0.,
            game_tick: 1,
            sun_angle: 90.,
            moon_phase: 0.,
            spawn: None,
            death: None,
            needles: Arc::default(),
        };
        let mut ctx = ItemContext {
            selected_bundle: Some(0),
            world: Some(world),
            ..Default::default()
        };
        assert_eq!(
            models
                .model_with(&content, &bundle, &ctx)
                .unwrap()
                .quads
                .len(),
            quad_count
        );
        ctx.world.as_mut().unwrap().sun_angle = 270.;
        assert!(
            models
                .model_with(&content, &bundle, &ctx)
                .unwrap()
                .quads
                .is_empty()
        );
    }

    #[test]
    fn crossbow_draw_graph_uses_enchanted_charge_duration() {
        let mut content = Content::test_cube();
        content.set_registries(BTreeMap::from([(
            "minecraft:enchantment".into(),
            vec![("minecraft:quick_charge".into(), json!({"effects": {
                "minecraft:crossbow_charge_time": {
                    "type": "minecraft:add",
                    "value": {"type": "minecraft:linear", "base": -0.25, "per_level_above_first": -0.25}
                }
            }}))],
        )]));
        let graph = json!({
            "type": "minecraft:condition", "property": "minecraft:using_item",
            "on_false": {"type": "minecraft:model", "model": "test:idle"},
            "on_true": {
                "type": "minecraft:range_dispatch", "property": "minecraft:crossbow/pull",
                "fallback": {"type": "minecraft:model", "model": "test:pull0"},
                "entries": [
                    {"threshold": 0.58, "model": {"type": "minecraft:model", "model": "test:pull1"}},
                    {"threshold": 1.0, "model": {"type": "minecraft:model", "model": "test:pull2"}}
                ]
            }
        });
        let mut stack = Stack::default();
        for (level, ticks, elapsed, using_item, expected) in [
            (0, 25., 14., true, "test:pull0"),
            (0, 25., 15., true, "test:pull1"),
            (3, 10., 5., true, "test:pull0"),
            (3, 10., 6., true, "test:pull1"),
            (3, 10., 10., true, "test:pull2"),
            (3, 10., 10., false, "test:idle"),
        ] {
            Arc::make_mut(&mut stack.components).insert(
                "minecraft:enchantments".into(),
                if level == 0 {
                    json!({})
                } else {
                    json!({"minecraft:quick_charge": level})
                },
            );
            let charge =
                crate::player::crossbow_charge_ticks(&stack, &content, &mut |_| 0.).unwrap();
            assert_eq!(charge, ticks);
            let ctx = ItemContext {
                using_item,
                crossbow_pull: elapsed / charge,
                ..ItemContext::default()
            };
            let mut nodes = Vec::new();
            evaluate(
                &graph,
                &stack,
                &ctx,
                0,
                Mat4::IDENTITY,
                &mut nodes,
                &mut Reads::default(),
            )
            .unwrap();
            assert_eq!(nodes[0].0["model"], expected);
        }
    }

    fn display_fixture(context: &str, transform: ModelTransform) -> BakedModel {
        let mut model = BakedModel::default();
        model.display.insert(context.into(), transform);
        model
    }

    /// Source `ItemTransform.apply` rotates by `rotationXYZ`, i.e. a vertex is turned about
    /// Z first and X last. The handheld first-person pose therefore leaves the sprite's
    /// right edge pointing up and out of the screen plane, not along the view axis.
    #[test]
    fn display_rotation_turns_about_z_first_then_y_then_x() {
        let handheld = ModelTransform {
            rotation: [0., -90., 25.],
            translation: [0.; 3],
            scale: [0.68; 3],
        };
        let model = display_fixture("firstperson_righthand", handheld);
        let sprite_x = display_matrix(&model, "firstperson_righthand").transform_vector3(Vec3::X);
        let angle = 25f32.to_radians();
        let expected = Vec3::new(0., angle.sin(), angle.cos()) * 0.68;
        assert!(
            (sprite_x - expected).length() < 1e-5,
            "sprite x axis {sprite_x:?} != {expected:?}"
        );
        // Both axes at once make the order observable on a second basis vector.
        let both = display_fixture(
            "head",
            ModelTransform {
                rotation: [30., 90., 0.],
                translation: [0.; 3],
                scale: [1.; 3],
            },
        );
        let z_axis = display_matrix(&both, "head").transform_vector3(Vec3::Z);
        // Ry(90) sends +Z to +X, then Rx(30) leaves +X alone.
        assert!((z_axis - Vec3::X).length() < 1e-5, "{z_axis:?}");
    }

    #[test]
    fn parent_graph_transforms_apply_to_selected_leaves_in_order() {
        let content = Content::test_cube();
        let source = content.models[0].clone();
        let mut models = ItemModels::new();
        models.baked.insert("test:cube".into(), source.clone());
        models.definitions.insert(
            "test:transforms".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:condition", "property":"minecraft:using_item",
                "transformation":{"scale":[1.,-1.,-1.]},
                "on_false":{"type":"minecraft:composite","models":[
                    {"type":"minecraft:model","model":"test:cube",
                     "transformation":{"translation":[0.,2.,3.]}},
                    {"type":"minecraft:model","model":"test:cube"}
                ]},
                "on_true":{"type":"minecraft:model","model":"test:cube"}
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:transforms"),
        )]));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_eq!(model.quads.len(), source.quads.len() * 2);
        for (index, quad) in source.quads.iter().enumerate() {
            for (vertex, position) in quad.positions.iter().enumerate() {
                let [x, y, z] = *position;
                assert_eq!(model.quads[index].positions[vertex], [x, -y - 2., -z - 3.]);
                assert_eq!(
                    model.quads[index + source.quads.len()].positions[vertex],
                    [x, -y, -z]
                );
            }
        }
        let ctx = ItemContext {
            using_item: true,
            ..ItemContext::default()
        };
        let model = models.model_with(&content, &stack, &ctx).unwrap();
        assert_eq!(model.quads.len(), source.quads.len());
        let [x, y, z] = source.quads[0].positions[0];
        assert_eq!(model.quads[0].positions[0], [x - 0.5, 0.5 - y, 0.5 - z]);
    }

    #[test]
    fn gui_special_head_faces_are_stably_ordered_by_display_depth() {
        let mut model = Content::test_cube().models[0].clone();
        model.quads.truncate(3);
        for (quad, depth) in model.quads.iter_mut().zip([2., -1., -1.]) {
            for position in &mut quad.positions {
                position[2] = depth;
            }
        }
        model.quads[0].texture_layer = 20;
        model.quads[1].texture_layer = 10;
        model.quads[2].texture_layer = 11;
        let materials = model
            .quads
            .iter()
            .map(|q| (q.layer, q.entity_material))
            .collect::<Vec<_>>();
        sort_gui_special_head(&mut model);
        assert_eq!(
            model
                .quads
                .iter()
                .map(|q| q.texture_layer)
                .collect::<Vec<_>>(),
            [10, 11, 20]
        );
        assert_eq!(
            (model.quads[0].layer, model.quads[0].entity_material),
            materials[1]
        );
        assert_eq!(
            (model.quads[1].layer, model.quads[1].entity_material),
            materials[2]
        );
        assert_eq!(
            (model.quads[2].layer, model.quads[2].entity_material),
            materials[0]
        );
    }

    #[test]
    fn graph_scale_preserves_uv_and_produces_transformed_winding_normals() {
        let mut model = Content::test_cube().models[0].clone();
        let original = model.quads.clone();
        let scale = Vec3::new(2., -3., -4.);
        apply_graph_transform(&mut model, Mat4::from_scale(scale));
        for (before, after) in original.iter().zip(&model.quads) {
            let normal = |positions: [[f32; 3]; 4]| {
                let p = positions.map(Vec3::from_array);
                (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero()
            };
            let expected = (normal(before.positions) / scale).normalize_or_zero();
            assert!((normal(after.positions) - expected).length() < 1e-6);
            assert_eq!(after.uv, before.uv);
            assert_eq!(after.face, before.face);
        }
    }

    fn potion_tint_fixture(contents: Value) -> (Content, ItemModels, Stack) {
        let content = Content::test_cube();
        let mut baked = content.models[0].clone();
        for quad in &mut baked.quads {
            quad.tint = TintKind::Constant([0.1, 0.2, 0.3]);
        }
        baked.quads[0].tint_index = Some(0);
        baked.quads[1].tint_index = Some(1);
        baked.quads[2].tint_index = Some(u32::MAX);
        let mut models = ItemModels::new();
        models.baked.insert("minecraft:item/potion".into(), baked);
        models.definitions.insert(
            "test:potion".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:model",
                "model":"minecraft:item/potion",
                "tints":[{"type":"minecraft:potion","default":-13083194}]
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([
            ("minecraft:item_model".into(), json!("test:potion")),
            ("minecraft:potion_contents".into(), contents),
        ]));
        (content, models, stack)
    }

    fn assert_potion_layers(model: &BakedModel, color: [f32; 3]) {
        assert_eq!(model.quads.len(), 6);
        assert_eq!(model.quads[0].tint, TintKind::Constant(color));
        for quad in &model.quads[1..] {
            assert_eq!(quad.tint, TintKind::None);
        }
    }

    #[test]
    fn potion_tint_water_uses_graph_default_and_missing_layers_are_untinted() {
        let (content, mut models, stack) = potion_tint_fixture(json!({"potion":"minecraft:water"}));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_potion_layers(model, [56. / 255., 93. / 255., 198. / 255.]);
    }

    #[test]
    fn potion_tint_custom_color_overrides_water_default() {
        let (content, mut models, stack) =
            potion_tint_fixture(json!({"potion":"minecraft:water","custom_color":0x123456}));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_potion_layers(model, [18. / 255., 52. / 255., 86. / 255.]);
    }

    #[test]
    fn potion_tint_missing_graph_sources_leave_all_layers_untinted() {
        let (content, mut models, stack) = potion_tint_fixture(json!({"potion":"minecraft:water"}));
        models.definitions.insert(
            "test:potion".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:model","model":"minecraft:item/potion"
            }})),
        );
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_eq!(model.quads.len(), 6);
        assert!(model.quads.iter().all(|quad| quad.tint == TintKind::None));
    }
    fn empty_entity_models() -> entities::Models {
        entities::Models {
            rigs: BTreeMap::new(),
            schemas: Default::default(),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            armor_models: BTreeMap::new(),
            motion: Default::default(),
        }
    }
    #[test]
    fn banner_special_tint_survives_gui_model_selection() {
        let mut content = Content::test_cube();
        let region = |layer| crate::protocol::content::TextureRegion {
            layer,
            uv: [0., 0., 1., 1.],
            width: 64,
            height: 64,
        };
        content.insert_test_texture("minecraft:entity/banner/banner_base", region(5));
        content.insert_test_texture("minecraft:entity/banner/base", region(6));
        content.insert_test_resource("assets/test/models/item/banner_base.json", br#"{}"#);
        let mut models = ItemModels::new();
        models.set_entity_models(Arc::new(empty_entity_models()));
        models.definitions.insert(
            "test:banner".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:special", "base":"test:item/banner_base",
                "model":{"type":"minecraft:banner","color":"red"}
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:banner"),
        )]));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert!(
            model
                .quads
                .iter()
                .any(|q| q.texture_layer == 6 && matches!(q.tint, TintKind::Constant(_)))
        );
    }

    /// `left` equals the reflection (x -> -x) of the right-hand transform applied to the
    /// mirrored unit-cube point, with the same source values in both contexts.
    fn assert_left_mirrored(model: &BakedModel, left_context: &str, right_context: &str) {
        let mut same_transform = model.clone();
        let left = *same_transform
            .display
            .get("thirdperson_lefthand")
            .expect("left hand display resource");
        same_transform
            .display
            .insert("thirdperson_righthand".into(), left);
        let left = display_matrix(model, "thirdperson_lefthand");
        let right = display_matrix(&same_transform, "thirdperson_righthand");
        let reflect = Vec3::new(-1., 1., 1.);
        for point in [
            Vec3::splat(0.5),
            Vec3::new(0.2, 0.3, 0.8),
            Vec3::new(0.9, 0.7, 0.1),
        ] {
            let mirrored_input = (point - Vec3::splat(0.5)) * reflect + Vec3::splat(0.5);
            let expected = right.transform_point3(mirrored_input) * reflect;
            let actual = left.transform_point3(point);
            assert!(
                (actual - expected).length() < 1e-5,
                "{actual:?} != {expected:?}"
            );
        }
    }

    fn assert_shield_hand_frame(
        content: &Content,
        models: &mut ItemModels,
        stack: &Stack,
        using_item: bool,
    ) {
        let ctx = |display: &str| ItemContext {
            display: display.into(),
            using_item,
            ..ItemContext::default()
        };
        let baseline = models
            .model_with(content, stack, &ctx("gui"))
            .unwrap()
            .clone();
        assert!(!baseline.quads.is_empty());
        for hand in [
            "thirdperson_righthand",
            "thirdperson_lefthand",
            "firstperson_lefthand",
        ] {
            let model = models.model_with(content, stack, &ctx(hand)).unwrap();
            assert_eq!(model.quads.len(), baseline.quads.len());
            for (actual, original) in model.quads.iter().zip(&baseline.quads) {
                assert_eq!(
                    actual.positions, original.positions,
                    "geometry changed for {hand}"
                );
            }
            if let Some(right) = hand.strip_suffix("lefthand") {
                assert_left_mirrored(model, hand, &format!("{right}righthand"));
            }
        }
    }

    #[test]
    fn battle_item_models_bake_from_received_item_model_id() {
        macro_rules! pack {
            ($($name:literal),* $(,)?) => {[$((
                $name,
                &include_bytes!(concat!("../../assets/battle_video/assets/krasnokamen_battle/items/", $name, ".json"))[..],
                &include_bytes!(concat!("../../assets/battle_video/assets/krasnokamen_battle/models/item/", $name, ".json"))[..],
            )),*]};
        }
        macro_rules! blocking {
            ($($name:literal),* $(,)?) => {[$((
                $name,
                &include_bytes!(concat!("../../assets/battle_video/assets/krasnokamen_battle/models/item/", $name, "_blocking.json"))[..],
            )),*]};
        }
        let weapons = pack![
            "iron_longsword",
            "iron_spear",
            "iron_lance",
            "iron_halberd",
            "iron_flanged_mace",
            "iron_longbow_standby",
            "iron_heavy_crossbow_standby"
        ];
        let shields = pack![
            "red_heater_shield",
            "blue_heater_shield",
            "iron_kite_shield",
            "iron_tower_shield"
        ];
        let blocking_models = blocking![
            "red_heater_shield",
            "blue_heater_shield",
            "iron_kite_shield",
            "iron_tower_shield"
        ];
        let mut content = Content::test_cube();
        content.insert_test_texture("krasnokamen_battle:item/iron_tower_shield",
            crate::protocol::content::TextureRegion { layer: 10, uv: [0., 0., 1., 1.], width: 64, height: 64 });
        for (name, graph, geometry) in [
            ("iron_kite_shield",
                &include_bytes!("../../assets/battle_video/assets/krasnokamen_battle/items/iron_kite_shield.json")[..],
                &include_bytes!("../../assets/battle_video/assets/krasnokamen_battle/models/item/iron_kite_shield.json")[..]),
            ("iron_tower_shield",
                &include_bytes!("../../assets/battle_video/assets/krasnokamen_battle/items/iron_tower_shield.json")[..],
                &include_bytes!("../../assets/battle_video/assets/krasnokamen_battle/models/item/iron_tower_shield.json")[..]),
        ] {
            content.insert_test_resource(&format!("assets/krasnokamen_battle/items/{name}.json"), graph);
            content.insert_test_resource(&format!("assets/krasnokamen_battle/models/item/{name}.json"), geometry);
            let mut stack = Stack::default();
            stack.id = 1;
            stack.count = 1;
            stack.components = Arc::new(BTreeMap::from([(
                "minecraft:item_model".into(), json!(format!("krasnokamen_battle:{name}")),
            )]));
            stack
        };
        for (name, ..) in &weapons {
            let mut models = ItemModels::new();
            let baked = models
                .model(&content, &stack, "thirdperson_righthand")
                .unwrap();
            assert!(!baked.quads.is_empty(), "{name}");
            assert!(
                baked.quads.iter().all(|quad| quad.texture_layer == 10),
                "{name}"
            );
        }
    }

    #[test]
    fn left_hand_displays_mirror_shield_and_sword_in_first_and_third_person() {
        let mut content = Content::test_cube();
        content.insert_test_texture(
            "minecraft:entity/shield/shield_base_nopattern",
            crate::protocol::content::TextureRegion {
                layer: 5,
                uv: [0., 0., 1., 1.],
                width: 64,
                height: 64,
            },
        );
        let display = br#"{"display":{"thirdperson_righthand":{"rotation":[17,31,43],"translation":[2,3,4]},"thirdperson_lefthand":{"rotation":[17,31,43],"translation":[2,3,4]},"firstperson_lefthand":{"rotation":[17,31,43],"translation":[2,3,4]}}}"#;
        content.insert_test_resource("assets/test/models/item/shield_idle.json", display);
        content.insert_test_resource("assets/test/models/item/shield_blocking.json", display);
        let mut models = ItemModels::new();
        models.set_entity_models(Arc::new(empty_entity_models()));
        let shield = |base| json!({"type":"minecraft:special","base":base,"model":{"type":"minecraft:shield"}});
        models.definitions.insert(
            "test:shield_alias".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:condition","property":"minecraft:using_item",
                "transformation":{"scale":[1.0,-1.0,-1.0]},
                "on_false":shield("test:item/shield_idle"),
                "on_true":shield("test:item/shield_blocking")
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:shield_alias"),
        )]));
        for using_item in [false, true] {
            assert_shield_hand_frame(&content, &mut models, &stack, using_item);
        }

        let asymmetric = ModelTransform {
            rotation: [17., 31., 43.],
            translation: [0.2, 0.3, 0.4],
            scale: [0.7, 0.8, 0.9],
        };
        let mut sword_model = content.models[0].clone();
        sword_model
            .display
            .insert("thirdperson_lefthand".into(), asymmetric);
        sword_model
            .display
            .insert("thirdperson_righthand".into(), asymmetric);
        models.baked.insert("test:sword".into(), sword_model);
        models.definitions.insert(
            "test:sword".into(),
            Arc::new(json!({"model":{"type":"minecraft:model","model":"test:sword"}})),
        );
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:sword"),
        )]));
        let baseline = models.model(&content, &stack, "gui").unwrap().clone();
        for hand in ["thirdperson_righthand", "thirdperson_lefthand"] {
            let sword = models.model(&content, &stack, hand).unwrap();
            assert_eq!(sword.quads.len(), baseline.quads.len());
            for (actual, original) in sword.quads.iter().zip(&baseline.quads) {
                assert_eq!(
                    actual.positions, original.positions,
                    "ordinary model must not shift"
                );
            }
            if hand == "thirdperson_lefthand" {
                assert_left_mirrored(sword, hand, "thirdperson_righthand");
            }
        }
        let mut fixture = BakedModel::default();
        fixture
            .display
            .insert("thirdperson_lefthand".into(), asymmetric);
        fixture
            .display
            .insert("thirdperson_righthand".into(), asymmetric);
        fixture
            .display
            .insert("firstperson_lefthand".into(), asymmetric);
        assert_thirdperson_left_mirrored(&fixture);
        assert_eq!(
            display_matrix(&fixture, "firstperson_lefthand"),
            display_matrix(&fixture, "thirdperson_righthand")
        );
    }

    #[test]
    #[ignore = "requires KRASNOKAMEN_CLIENT_RESOURCE_ROOT with runtime game resources"]
    fn resolved_shield_hand_frame_with_runtime_display_resources() {
        let root = std::path::PathBuf::from(std::env::var_os("KRASNOKAMEN_CLIENT_RESOURCE_ROOT")
            .expect("set KRASNOKAMEN_CLIENT_RESOURCE_ROOT to the activated resource root"));
        let read = |path: &str| std::fs::read(root.join(path)).expect("runtime shield resource");
        let mut content = Content::test_cube();
        content.insert_test_texture(
            "minecraft:entity/shield/shield_base_nopattern",
            crate::protocol::content::TextureRegion {
                layer: 5,
                uv: [0., 0., 1., 1.],
                width: 64,
                height: 64,
            },
        );
        for path in [
            "assets/minecraft/models/item/shield.json",
            "assets/minecraft/models/item/shield_blocking.json",
        ] {
            content.insert_test_resource(path, &read(path));
        }
        let mut models = ItemModels::new();
        models.set_entity_models(Arc::new(empty_entity_models()));
        let graph = serde_json::from_slice(&read("assets/minecraft/items/shield.json")).unwrap();
        models
            .definitions
            .insert("test:shield_alias".into(), Arc::new(graph));
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:shield_alias"),
        )]));
        for using_item in [false, true] {
            assert_shield_hand_frame(&content, &mut models, &stack, using_item);
        }
    }

    /// An item whose graph cannot bake (unknown special family) is a hard error for
    /// `model_with`, but the GUI entry point draws an empty icon instead of ending the session.
    #[test]
    fn icon_model_survives_unbakeable_item_graph() {
        let content = Content::test_cube();
        let mut models = ItemModels::new();
        models.set_entity_models(Arc::new(empty_entity_models()));
        models.definitions.insert(
            "test:bad".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:special",
                "model":{"type":"minecraft:no_such_family"},
                "base":"minecraft:item/potion"
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            json!("test:bad"),
        )]));
        let ctx = ItemContext::default();
        let error = models.model_with(&content, &stack, &ctx).unwrap_err();
        assert!(
            error.contains("unknown special item render family"),
            "{error}"
        );
        let icon = models.icon_model(&content, &stack, &ctx);
        assert!(icon.quads.is_empty());
        // Repeated frames stay non-fatal.
        assert!(models.icon_model(&content, &stack, &ctx).quads.is_empty());
        // A healthy item next to it still bakes normally.
        let (content, mut models, good) = potion_tint_fixture(json!({"potion":"minecraft:water"}));
        assert_eq!(models.icon_model(&content, &good, &ctx).quads.len(), 6);
    }

    /// Received stacks are separate allocations: equal stacks share one retained selection
    /// (a pointer key thrashed the bounded cache once a scene held thousands of them), and
    /// every selection names the context inputs its graph path read.
    #[test]
    fn equal_stacks_share_a_selection_that_names_its_context_inputs() {
        let content = Content::test_cube();
        let mut models = ItemModels::new();
        models.insert_test_model("test:lowered", content.models[0].clone());
        let mut raised = content.models[0].clone();
        for quad in &mut raised.quads {
            for position in &mut quad.positions {
                position[1] += 0.25;
            }
        }
        models.insert_test_model("test:raised", raised);
        models.insert_test_graph(
            "test:shield",
            json!({"type":"minecraft:condition",
            "property":"minecraft:using_item",
            "on_false":{"type":"minecraft:model","model":"test:lowered"},
            "on_true":{"type":"minecraft:range_dispatch","property":"minecraft:use_duration",
                "fallback":{"type":"minecraft:model","model":"test:raised"},"entries":[]}}),
        );
        models.insert_test_graph(
            "test:clock",
            json!({"type":"minecraft:range_dispatch",
            "property":"minecraft:time","source":"daytime",
            "fallback":{"type":"minecraft:model","model":"test:lowered"},"entries":[]}),
        );
        let stack = |model: &str| {
            let mut stack = Stack::default();
            stack.id = 1;
            stack.count = 1;
            stack.components = Arc::new(BTreeMap::from([(
                "minecraft:item_model".to_string(),
                json!(model),
            )]));
            stack
        };
        let (first, second) = (stack("test:shield"), stack("test:shield"));
        let mut ctx = ItemContext {
            display: "thirdperson_lefthand".into(),
            ..ItemContext::default()
        };
        let (_, lowered, reads) = models.model_shape(&content, &first, &ctx).unwrap();
        assert_eq!(reads, Reads::USING_ITEM);
        let (_, shape, _) = models.model_shape(&content, &second, &ctx).unwrap();
        assert_eq!(shape, lowered);
        assert_eq!(
            models.selected.len(),
            1,
            "equal stacks in two allocations share one selection"
        );
        ctx.using_item = true;
        let (_, up, reads) = models.model_shape(&content, &second, &ctx).unwrap();
        assert_ne!(up, lowered);
        assert!(reads.contains(Reads::USING_ITEM) && reads.contains(Reads::USE_TICKS));
        assert!(!reads.contains(Reads::CROSSBOW_PULL) && !reads.contains(Reads::VOLATILE));
        let (_, _, reads) = models
            .model_shape(&content, &stack("test:clock"), &ctx)
            .unwrap();
        assert!(reads.contains(Reads::VOLATILE));
        let generation = models.generation();
        models.clear();
        assert_ne!(models.generation(), generation);
    }
}
