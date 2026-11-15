//! GPU crowd instancing for humanoid mobs and horses.
//!
//! The CPU entity path emits every vertex of every entity every frame, which caps a
//! scene at `MAX_DYNAMIC_VERTICES`. For the kinds a battle uses, this module instead
//! turns each distinct appearance (main rig, worn armour layers, held and head items)
//! into a static *composite*: one mesh whose vertices stay in their bone's local space,
//! plus a bone table. Per frame each eligible entity contributes only a
//! [`CrowdInstance`]: its root matrix, role pose, clip layers and light. The renderer
//! (`render/entity_instancing.rs`) culls instances on the GPU, evaluates every bone with
//! the rules of `presentation::evaluate_posed_rig` and `clip_pose`, and draws each
//! composite instanced.
//!
//! Invariant: an entity is routed here only when the CPU path would draw exactly the
//! composite's parts, with the same materials, so both paths produce the same triangles
//! ([`try_instance`] checks every input that could differ and otherwise leaves the
//! entity to the CPU path). Fire stays a CPU layer for both.
use super::{
    Entity, Metadata, Models,
    clip_pose::{self, Channel, ClipLibrary},
    equipment::{self, Worn},
    presentation::{self as p, Binding as Bound, EntityDrawFrame, RootStyle},
    resources::{BoneRole, Rig},
};
use crate::{
    api::{EntityMaterial, RenderLayer},
    inventory::Stack,
    protocol::{
        content::{Content, TextureRegion},
        models::{BakedModel, BakedQuad, TintKind},
    },
    ui::items::{ItemContext, ItemModels, Reads},
};
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Entity kinds the crowd renderer draws: their rigs use only role-driven bones.
const KINDS: [&str; 9] = [
    "minecraft:zombie",
    "minecraft:husk",
    "minecraft:skeleton",
    "minecraft:wither_skeleton",
    "minecraft:pillager",
    "minecraft:vindicator",
    "minecraft:horse",
    "minecraft:skeleton_horse",
    "minecraft:zombie_horse",
];

/// The crowd kind named `type_name`, if the crowd renderer draws that kind.
pub(super) fn kind(type_name: &str) -> Option<&'static str> {
    KINDS.into_iter().find(|kind| *kind == type_name)
}
/// Materials a composite may contain, in the CPU path's draw order (`sort_key`).
pub const MATERIALS: [EntityMaterial; 5] = [
    EntityMaterial::ArmorCutoutNoCull,
    EntityMaterial::ArmorDecalCutoutNoCull,
    EntityMaterial::EntitySolid,
    EntityMaterial::EntityCutout,
    EntityMaterial::ItemCutout,
];
/// Detail levels: 0 full, 1 without thin item quads, 2 also without armour trims.
pub const LODS: usize = 3;
/// Bounds of one frame: instances and evaluated bone matrices (48 bytes each).
pub(crate) const MAX_INSTANCES: usize = 100_000;
pub(crate) const MAX_BONES: u32 = 2_000_000;

/// `CrowdVertex::bone_flags`: bits 0-15 bone, then these flags, emission in bits 24-31.
pub const VERTEX_BODY: u32 = 1 << 16;
pub const VERTEX_ITEM_NORMAL: u32 = 1 << 17;
pub const VERTEX_EMISSION_SHIFT: u32 = 24;
/// `CrowdBone::role` codes.
pub const ROLE_FIXED: u32 = 0;
pub const ROLE_HEAD: u32 = 1;
pub const ROLE_RIGHT_ARM: u32 = 2;
pub const ROLE_LEFT_ARM: u32 = 3;
pub const ROLE_RIGHT_LEG: u32 = 4;
pub const ROLE_LEFT_LEG: u32 = 5;
pub const ROLE_ITEM: u32 = 6;
/// `CrowdBone::channel` of a bone no clip animates.
pub const NO_CHANNEL: u32 = u32::MAX;
/// `CrowdBone::flags`: re-parented by the torso carry.
pub const BONE_CARRIED: u32 = 1;
/// `CrowdInstance::words[2]` flags.
pub const INSTANCE_PIVOT_Z_ZERO: u32 = 1 << 12;
pub const INSTANCE_RIDING: u32 = 1 << 13;
pub const INSTANCE_MIRROR: [u32; 2] = [1 << 14, 1 << 15];

/// One static composite vertex, in its bone's local space (56 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CrowdVertex {
    pub position: [f32; 3],
    /// Rig parts: the quad's local normal. Items: the raw face normal (`VERTEX_ITEM_NORMAL`).
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub layer: u32,
    pub bone_flags: u32,
}

/// One composite bone (64 bytes). Parents precede children.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CrowdBone {
    pub pivot: [f32; 3],
    /// Composite-local parent; -1 is the instance root.
    pub parent: i32,
    /// Rest rotation (radians, Euler Z then Y then X).
    pub rotation: [f32; 3],
    pub role: u32,
    pub scale: [f32; 3],
    pub channel: u32,
    pub flags: u32,
    pub _pad: [u32; 3],
}

/// One entity this frame (144 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CrowdInstance {
    /// Rows of the affine model-root matrix, relative to `Scene::origin`.
    pub root: [[f32; 4]; 3],
    /// Head pitch, head yaw, right and left leg gait (`RolePose`).
    pub pose: [f32; 4],
    /// Right and left arm `AxisMap` offsets; `arms[0][3]` is the clip leg factor.
    /// The `AxisMap` scales are 2-bit codes in `words[2]` (0: 0, 1: 1, 2: 0.5).
    pub arms: [[f32; 4]; 2],
    /// Seconds and weight of the action layer, then of the flinch layer.
    pub layers: [f32; 4],
    /// Sky and block light, hurt overlay (0 or 1), unused.
    pub light: [f32; 4],
    /// Composite, first bone matrix, flags, action clip | flinch clip << 16.
    pub words: [u32; 4],
}

/// Draw data of one composite.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrowdGroup {
    pub base_vertex: i32,
    /// First index of each [`MATERIALS`] entry.
    pub first_index: [u32; MATERIALS.len()],
    /// Index count of each material at each LOD (a prefix of its range).
    pub counts: [[u32; LODS]; MATERIALS.len()],
    pub bone_first: u32,
    pub bone_count: u32,
    /// Torso carry pivot (`clip_pose::hip` of the main rig).
    pub hip: [f32; 3],
    /// Bound of every vertex around the model origin under any pose (model units).
    pub radius: f32,
}

/// One composite's mesh: its vertices, its indices (vertex numbers from its group's
/// `base_vertex`) and its bones, at the pool offsets `first` (vertices, indices, bones).
#[derive(Debug, Default)]
pub struct CrowdChunk {
    pub first: [u32; 3],
    pub vertices: Vec<CrowdVertex>,
    pub indices: Vec<u32>,
    pub bones: Vec<CrowdBone>,
}

/// Every composite, shared across frames; `revision` changes with the contents. A pool only
/// grows until `Registry::clear_composites` starts another (`pool`), and each composite's
/// mesh is its own shared chunk (`chunks[i]` belongs to `groups[i]`): appending to a pool
/// that a frame or the renderer still holds copies the composite table, never every mesh.
#[derive(Clone, Debug, Default)]
pub struct CrowdMeshes {
    pub revision: u64,
    pub pool: u64,
    pub chunks: Vec<Arc<CrowdChunk>>,
    pub groups: Vec<CrowdGroup>,
    /// Vertices, indices and bones of every chunk.
    pub lens: [u32; 3],
}

/// This frame's crowd: meshes, instances and the clip table they sample.
#[derive(Clone, Debug, Default)]
pub struct CrowdScene {
    pub meshes: Arc<CrowdMeshes>,
    pub instances: Vec<CrowdInstance>,
    /// Instances per composite.
    pub counts: Vec<u32>,
    /// Bone matrices the instances need in total.
    pub bones: u32,
    pub clips: Arc<ClipLibrary>,
}

static REVISIONS: AtomicU64 = AtomicU64::new(1);

/// Appearance a composite is built from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    rig: usize,
    texture: [u32; 5],
    /// Worn layers: rig, texture, colour, material.
    worn: Vec<(usize, [u32; 5], [u32; 3], u8)>,
    /// Held/head items: slot, anatomical hand, attachment bone, shape, skull pose.
    items: Vec<(u8, u8, u16, u64, bool)>,
}

fn texture_key(texture: TextureRegion) -> [u32; 5] {
    [
        texture.layer,
        texture.uv[0].to_bits(),
        texture.uv[1].to_bits(),
        texture.uv[2].to_bits(),
        texture.uv[3].to_bits(),
    ]
}

/// An entity's last composite, valid while nothing it was resolved from changed: the crowd
/// and item generations, the texture, every slot's stack, the metadata it read ([`Facts`]),
/// and per held/head slot the frame-varying item state its selection read. An unchanged
/// appearance revision proves the facts unchanged, so a frame costs no lookup by name and no
/// allocation; a changed revision (any metadata, health included) re-reads only the facts.
#[derive(Debug)]
pub(crate) struct Binding {
    generation: u64,
    items: u64,
    appearance: u64,
    texture: [u32; 5],
    rig: usize,
    facts: Facts,
    /// Metadata index of the kind's living flags (item use), resolved once.
    flags: Option<u8>,
    /// Every slot's stack: its component handle (retained, so the address cannot be reused)
    /// and item.
    stacks: [Option<(Arc<BTreeMap<String, Value>>, u32)>; 8],
    /// Slots 0, 1 and 5 ([`HELD_SLOTS`]): the held item's shape, the context inputs its
    /// selection read and their values; `None` when the slot holds nothing drawn as an item.
    held: [Option<(u64, Reads, ItemUse)>; 3],
    composite: u32,
}

/// Held (0, 1) and head (5) slots: their item model is selected per frame.
const HELD_SLOTS: [usize; 3] = [0, 1, 5];

/// The metadata a composite and its crowd routing read: main arm (held item sides), baby
/// (armour rigs), invisible/glowing flags and pose (routing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Facts {
    left_main: bool,
    baby: bool,
    hidden: bool,
    pose: i32,
}

impl Facts {
    fn of(models: &Models, entity: &Entity) -> Self {
        Self {
            left_main: p::left_main_arm(models, entity),
            baby: p::boolean(models, entity, "DATA_BABY_ID"),
            // Invisible bodies still draw equipment; glowing adds an outline.
            hidden: p::number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & (32 | 64) != 0,
            pose: p::number(models, entity, "DATA_POSE").unwrap_or(0),
        }
    }
}

/// Frame-varying item state of a held slot (`presentation::held_item_use`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ItemUse {
    using: bool,
    ticks: f32,
    pull: f32,
}

impl ItemUse {
    /// Whether every input in `reads` is equal (bitwise for the floats).
    fn same(self, other: Self, reads: Reads) -> bool {
        (!reads.contains(Reads::USING_ITEM) || self.using == other.using)
            && (!reads.contains(Reads::USE_TICKS) || self.ticks.to_bits() == other.ticks.to_bits())
            && (!reads.contains(Reads::CROSSBOW_PULL)
                || self.pull.to_bits() == other.pull.to_bits())
    }
}

fn same_stack(kept: &Option<(Arc<BTreeMap<String, Value>>, u32)>, stack: Option<&Stack>) -> bool {
    match (kept, stack) {
        (None, None) => true,
        (Some((components, id)), Some(stack)) => {
            *id == stack.id && Arc::ptr_eq(components, &stack.components)
        }
        _ => false,
    }
}

/// Composites and this frame's instances.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    /// Set by a renderer that draws `Scene::crowd`.
    pub enabled: bool,
    generation: u64,
    clip_revision: u64,
    meshes: Arc<CrowdMeshes>,
    keys: HashMap<Key, u32>,
    instances: Vec<CrowdInstance>,
    counts: Vec<u32>,
    bones: u32,
}

impl Registry {
    /// Forgets every composite (resources changed); bindings re-resolve.
    pub(crate) fn clear_composites(&mut self) {
        self.generation += 1;
        self.keys.clear();
        let revision = REVISIONS.fetch_add(1, Ordering::Relaxed);
        self.meshes = Arc::new(CrowdMeshes {
            revision,
            pool: revision,
            ..Default::default()
        });
        self.instances.clear();
        self.counts.clear();
        self.bones = 0;
    }
    /// Starts a frame: no instances yet.
    pub(crate) fn begin_frame(&mut self) {
        self.instances.clear();
        self.counts.clear();
        self.bones = 0;
        let library = clip_pose::library();
        if library.revision() != self.clip_revision {
            // Composite bounds include the library's largest clip offset.
            self.clear_composites();
            self.clip_revision = library.revision();
        }
    }
    /// Moves this frame's instances into `scene`.
    pub(crate) fn take(&mut self, scene: &mut CrowdScene) {
        scene.meshes = Arc::clone(&self.meshes);
        std::mem::swap(&mut scene.instances, &mut self.instances);
        std::mem::swap(&mut scene.counts, &mut self.counts);
        scene.counts.resize(scene.meshes.groups.len(), 0);
        scene.bones = self.bones;
        scene.clips = clip_pose::library();
        self.instances.clear();
        self.counts.clear();
        self.bones = 0;
    }
    #[cfg(test)]
    pub(crate) fn meshes(&self) -> &CrowdMeshes {
        &self.meshes
    }
    #[cfg(test)]
    pub(crate) fn instances(&self) -> &[CrowdInstance] {
        &self.instances
    }
}

/// Per-entity inputs of [`try_instance`] that live outside the entity.
pub(super) struct Inputs<'a> {
    pub models: &'a Models,
    pub content: &'a Content,
    pub frame: EntityDrawFrame,
    pub item_context: Option<&'a ItemContext>,
    pub bed_direction: &'a dyn Fn([i32; 3]) -> Option<[i32; 2]>,
}

/// Why an entity of a crowd kind stays on the CPU path (`PresentationStats::crowd_fallback`).
pub(super) type Fallback = &'static str;

/// Routes `entity` (of a crowd [`kind`]) to the crowd renderer and pushes its instance,
/// or names why the CPU path must draw it (nothing pushed). `Ok(true)`: its appearance was
/// resolved again this frame (`PresentationStats::crowd_resolved`).
pub(super) fn try_instance(
    registry: &mut Registry,
    items: &mut ItemModels,
    equipment: &mut equipment::Cache,
    entity: &mut Entity,
    inputs: Inputs<'_>,
) -> Result<bool, Fallback> {
    let Inputs {
        models,
        content,
        frame,
        ..
    } = inputs;
    if registry.instances.len() >= MAX_INSTANCES {
        return Err("instance budget");
    }
    let Some(Ok(bound)) = entity.binding.as_ref() else {
        return Err("unbound");
    };
    if !matches!(bound.style, RootStyle::Living) || bound.layers.is_some() || bound.anger.is_some()
    {
        return Err("special layers");
    }
    // A kept binding passed the checks below for the same rig and facts.
    let (composite, resolved) = match kept(registry, items, entity, &inputs)? {
        Some(composite) => (composite, false),
        None => {
            let Some(Ok(bound)) = entity.binding.as_ref() else {
                return Err("unbound");
            };
            let rig = models.rig(&bound.rig);
            let facts = Facts::of(models, entity);
            if rig.is_placeholder() {
                return Err("placeholder rig");
            }
            if facts.hidden {
                return Err("invisible or glowing");
            }
            if facts.pose != 0 {
                return Err("pose");
            }
            (
                composite(registry, items, equipment, entity, rig, facts, &inputs)?,
                true,
            )
        }
    };
    let bone_count = registry.meshes.groups[composite as usize].bone_count;
    if registry.bones + bone_count > MAX_BONES {
        return Err("bone budget");
    }
    let Some(Ok(bound)) = entity.binding.as_ref() else {
        return Err("unbound");
    };
    let mut instance =
        instance(entity, models, content, bound, frame, inputs.bed_direction).ok_or("arm pose")?;
    instance.words[0] = composite;
    instance.words[1] = registry.bones;
    registry.bones += bone_count;
    if registry.counts.len() <= composite as usize {
        registry.counts.resize(composite as usize + 1, 0);
    }
    registry.counts[composite as usize] += 1;
    registry.instances.push(instance);
    Ok(resolved)
}

/// The per-frame instance of a bound entity; `None` when its pose has no instance form.
pub(super) fn instance(
    entity: &Entity,
    models: &Models,
    content: &Content,
    bound: &Bound,
    frame: EntityDrawFrame,
    bed_direction: &dyn Fn([i32; 3]) -> Option<[i32; 2]>,
) -> Option<CrowdInstance> {
    let EntityDrawFrame {
        tick,
        partial,
        light,
        ..
    } = frame;
    let root = p::root_matrix(entity, models, bound, frame, bed_direction);
    let pose = p::role_pose(entity, models, content, tick, partial);
    let code = |scale: f32| -> Option<u32> {
        if scale == 0. {
            Some(0)
        } else if scale == 1. {
            Some(1)
        } else if scale == 0.5 {
            Some(2)
        } else {
            None
        }
    };
    let mut flags = 0_u32;
    for (arm, map) in pose.arms.iter().enumerate() {
        for axis in 0..3 {
            flags |= code(map.scale[axis])? << (arm * 6 + axis * 2);
        }
    }
    if pose.arm_pivot_z_zero {
        flags |= INSTANCE_PIVOT_Z_ZERO;
    }
    if pose.riding {
        flags |= INSTANCE_RIDING;
    }
    let layers = entity.clip.active(tick, partial);
    let mut clips = 0;
    let mut timing = [0.; 4];
    for (index, layer) in layers.iter().enumerate() {
        if let Some(layer) = layer {
            clips |= u32::from(layer.clip) << (16 * index);
            timing[index * 2] = layer.seconds;
            timing[index * 2 + 1] = layer.weight;
            if layer.mirror {
                flags |= INSTANCE_MIRROR[index];
            }
        }
    }
    let hurt = entity
        .damage_tick
        .is_some_and(|damage| tick.saturating_sub(damage) < 10)
        || entity.death_tick.is_some();
    let row = |i: usize| {
        [
            root.x_axis[i],
            root.y_axis[i],
            root.z_axis[i],
            root.w_axis[i],
        ]
    };
    let leg = clip_pose::leg_factor(pose.amplitude, pose.riding);
    Some(CrowdInstance {
        root: [row(0), row(1), row(2)],
        pose: [pose.head[0], pose.head[1], pose.legs[0], pose.legs[1]],
        arms: [
            pose.arms[0].offset.extend(leg).to_array(),
            pose.arms[1].offset.extend(0.).to_array(),
        ],
        layers: timing,
        light: [light[0], light[1], f32::from(u8::from(hurt)), 0.],
        words: [0, 0, flags, clips],
    })
}

/// A held (slots 0, 1) or head (slot 5) item and where it attaches.
struct Held {
    slot: usize,
    hand: usize,
    bone: usize,
    context: &'static str,
    state: ItemContext,
    shape: u64,
    reads: Reads,
    skull: bool,
}

impl Held {
    fn item_use(&self) -> ItemUse {
        ItemUse {
            using: self.state.using_item,
            ticks: self.state.use_ticks,
            pull: self.state.crossbow_pull,
        }
    }
}

/// The composite of `entity`'s kept [`Binding`] when nothing it was resolved from changed and
/// every held item still selects the same shape (re-selecting those whose read inputs
/// changed); `None` when the appearance must be resolved again.
fn kept(
    registry: &Registry,
    items: &mut ItemModels,
    entity: &mut Entity,
    inputs: &Inputs<'_>,
) -> Result<Option<u32>, Fallback> {
    let Inputs {
        models,
        content,
        frame,
        item_context,
        ..
    } = *inputs;
    let Some(mut binding) = entity.crowd.take() else {
        return Ok(None);
    };
    let Some(Ok(bound)) = entity.binding.as_ref() else {
        return Err("unbound");
    };
    let texture = texture_key(bound.texture);
    if binding.generation != registry.generation
        || binding.items != items.generation()
        || binding.texture != texture
        || !binding
            .stacks
            .iter()
            .enumerate()
            .all(|(slot, kept)| same_stack(kept, entity.equipment_value(slot)))
    {
        return Ok(None);
    }
    if binding.appearance != entity.appearance_revision {
        // New metadata (most often health): only the facts it could change are compared.
        if std::ptr::from_ref(models.rig(&bound.rig)) as usize != binding.rig
            || Facts::of(models, entity) != binding.facts
        {
            return Ok(None);
        }
        binding.appearance = entity.appearance_revision;
    }
    let mut flags = None;
    for (memo, slot) in binding.held.iter_mut().zip(HELD_SLOTS) {
        let (Some((shape, reads, used)), Some(stack)) = (*memo, entity.equipment_value(slot))
        else {
            continue;
        };
        let flags = *flags.get_or_insert_with(|| {
            binding
                .flags
                .and_then(|index| entity.metadata_value(index))
                .and_then(Metadata::int)
                .unwrap_or(0)
        });
        let (using, ticks, pull) = p::held_item_use(entity, flags, slot, frame.partial);
        if !reads.contains(Reads::VOLATILE) && used.same(ItemUse { using, ticks, pull }, reads) {
            continue;
        }
        let rig = models.rig(&bound.rig);
        let held = held_item(
            entity,
            models,
            content,
            items,
            rig,
            slot,
            stack,
            binding.facts.left_main,
            frame.partial,
            item_context,
        )?;
        if held.shape != shape {
            return Ok(None);
        }
        *memo = Some((held.shape, held.reads, held.item_use()));
    }
    let composite = binding.composite;
    entity.crowd = Some(binding);
    Ok(Some(composite))
}

/// The composite of `entity`'s appearance (`rig` is its bound rig, `facts` its metadata),
/// resolving and building it when needed, and its new [`Binding`].
fn composite(
    registry: &mut Registry,
    items: &mut ItemModels,
    equipment: &mut equipment::Cache,
    entity: &mut Entity,
    rig: &Rig,
    facts: Facts,
    inputs: &Inputs<'_>,
) -> Result<u32, Fallback> {
    let Inputs {
        models,
        content,
        frame,
        item_context,
        ..
    } = *inputs;
    let Some(Ok(bound)) = entity.binding.as_ref() else {
        return Err("unbound");
    };
    let rig_key = std::ptr::from_ref(rig) as usize;
    let texture = texture_key(bound.texture);
    let bound_texture = bound.texture;
    let left_main = facts.left_main;
    let mut held = Vec::new();
    for slot in HELD_SLOTS {
        let Some(stack) = entity.equipment_value(slot) else {
            continue;
        };
        if !matches!(
            equipment.resolve(entity, slot, stack, models, content),
            Ok(Worn::NotWorn)
        ) {
            continue;
        }
        held.push(held_item(
            entity,
            models,
            content,
            items,
            rig,
            slot,
            stack,
            left_main,
            frame.partial,
            item_context,
        )?);
    }
    let mut worn = Vec::new();
    for slot in [4, 3, 2, 5, 0, 1, 6, 7] {
        let Some(stack) = entity.equipment_value(slot) else {
            continue;
        };
        match equipment
            .resolve(entity, slot, stack, models, content)
            .map_err(|_| "unresolved equipment")?
        {
            Worn::Omitted => {}
            // Other slots report a missing attachment on the CPU path.
            Worn::NotWorn if matches!(slot, 0 | 1 | 5) => {}
            Worn::NotWorn => return Err("equipment without attachment"),
            Worn::Layers(layers) => {
                if layers.foil {
                    return Err("enchantment glint");
                }
                for part in equipment.parts(&layers, None) {
                    if !matches!(
                        part.material,
                        EntityMaterial::ArmorCutoutNoCull | EntityMaterial::ArmorDecalCutoutNoCull
                    ) {
                        return Err("armour material");
                    }
                    worn.push((layers.rig, part));
                }
            }
        }
    }
    let key = Key {
        rig: rig_key,
        texture,
        worn: worn
            .iter()
            .map(|(rig, part)| {
                (
                    std::ptr::from_ref(*rig) as usize,
                    texture_key(part.texture),
                    part.color.map(f32::to_bits),
                    part.material as u8,
                )
            })
            .collect(),
        items: held
            .iter()
            .map(|h| (h.slot as u8, h.hand as u8, h.bone as u16, h.shape, h.skull))
            .collect(),
    };
    let composite = match registry.keys.get(&key) {
        Some(&composite) => composite,
        None => {
            let mut parts = vec![Part::Rig {
                rig,
                texture: bound_texture,
                color: [1.; 3],
                material: EntityMaterial::EntityCutout,
                body: true,
            }];
            parts.extend(worn.iter().map(|(rig, part)| Part::Rig {
                rig,
                texture: part.texture,
                color: part.color,
                material: part.material,
                body: false,
            }));
            let mut quads = Vec::new();
            for h in &held {
                let stack = entity.equipment_value(h.slot).ok_or("held item")?;
                let (model, _, _) = items
                    .model_shape(content, stack, &h.state)
                    .map_err(|_| "held item model")?;
                quads.push((h, model.clone()));
            }
            parts.extend(quads.iter().map(|(h, model)| Part::Item { held: h, model }));
            let built =
                build(&parts, rig, clip_pose::library().max_offset()).ok_or("composite build")?;
            let meshes = Arc::make_mut(&mut registry.meshes);
            let composite = u32::try_from(meshes.groups.len()).map_err(|_| "composite budget")?;
            meshes.append(built);
            meshes.revision = REVISIONS.fetch_add(1, Ordering::Relaxed);
            registry.keys.insert(key, composite);
            composite
        }
    };
    let held = HELD_SLOTS.map(|slot| {
        held.iter()
            .find(|h| h.slot == slot)
            .map(|h| (h.shape, h.reads, h.item_use()))
    });
    entity.crowd = Some(Binding {
        generation: registry.generation,
        items: items.generation(),
        appearance: entity.appearance_revision,
        texture,
        rig: rig_key,
        facts,
        flags: models.accessor(&entity.type_name, "DATA_LIVING_ENTITY_FLAGS"),
        stacks: std::array::from_fn(|slot| {
            entity
                .equipment_value(slot)
                .map(|stack| (Arc::clone(&stack.components), stack.id))
        }),
        held,
        composite,
    });
    Ok(composite)
}

/// Resolves the held/head item of `slot`, or names why the crowd cannot draw it as is.
#[expect(
    clippy::too_many_arguments,
    reason = "the equipment pose inputs of one slot"
)]
fn held_item(
    entity: &Entity,
    models: &Models,
    content: &Content,
    items: &mut ItemModels,
    rig: &Rig,
    slot: usize,
    stack: &Stack,
    left_main: bool,
    partial: f32,
    item_context: Option<&ItemContext>,
) -> Result<Held, Fallback> {
    if rig
        .bones
        .iter()
        .any(|bone| bone.role == BoneRole::CrossedArms)
    {
        return Err("held item with crossed arms");
    }
    let hand = slot ^ usize::from(left_main);
    let target = match slot {
        0 | 1 if hand == 0 => BoneRole::RightArm,
        0 | 1 => BoneRole::LeftArm,
        _ => BoneRole::Head,
    };
    let bone = rig
        .bones
        .iter()
        .position(|bone| bone.role == target)
        .ok_or("held item without arm")?;
    let (context, state) = p::held_item_context(entity, models, slot, hand, partial, item_context);
    if crate::ui::items::foil_texture(content, stack)
        .map_err(|_| "held item model")?
        .is_some()
    {
        return Err("held item glint");
    }
    let (model, shape, reads) = items
        .model_shape(content, stack, &state)
        .map_err(|_| "held item model")?;
    // Plain models are item cutout; shields (special models) are entity solid.
    if model.quads.iter().any(|quad| {
        !matches!(
            item_material(quad),
            EntityMaterial::ItemCutout | EntityMaterial::EntitySolid
        )
    }) {
        return Err("held item material");
    }
    Ok(Held {
        slot,
        hand,
        bone,
        context,
        state,
        shape,
        reads,
        skull: p::worn_skull(content, stack, slot),
    })
}

/// The material the CPU path draws an item quad with (`presentation::append_baked`).
fn item_material(quad: &BakedQuad) -> EntityMaterial {
    quad.entity_material.unwrap_or(match quad.layer {
        RenderLayer::Opaque | RenderLayer::Cutout => EntityMaterial::ItemCutout,
        RenderLayer::Translucent => EntityMaterial::ItemTranslucent,
    })
}

enum Part<'a> {
    Rig {
        rig: &'a Rig,
        texture: TextureRegion,
        color: [f32; 3],
        material: EntityMaterial,
        body: bool,
    },
    Item {
        held: &'a Held,
        model: &'a BakedModel,
    },
}

/// One composite before it is appended to the shared buffers.
#[derive(Debug, Default)]
pub(crate) struct Built {
    vertices: Vec<CrowdVertex>,
    /// Per material: (max LOD, quad indices) in emission order.
    quads: [Vec<(usize, [u32; 6])>; MATERIALS.len()],
    bones: Vec<CrowdBone>,
    hip: Vec3,
    radius: f32,
}

impl CrowdMeshes {
    fn append(&mut self, built: Built) {
        let [vertex, index, bone] = self.lens;
        let mut group = CrowdGroup {
            base_vertex: vertex as i32,
            bone_first: bone,
            bone_count: built.bones.len() as u32,
            hip: built.hip.to_array(),
            radius: built.radius,
            ..Default::default()
        };
        let mut indices = Vec::new();
        for (material, mut quads) in built.quads.into_iter().enumerate() {
            group.first_index[material] = index + indices.len() as u32;
            // Each LOD draws a prefix: parts kept at coarser levels come first.
            quads.sort_by_key(|(lod, _)| std::cmp::Reverse(*lod));
            for lod in 0..LODS {
                group.counts[material][lod] =
                    6 * quads.iter().filter(|(max, _)| *max >= lod).count() as u32;
            }
            indices.extend(quads.iter().flat_map(|(_, indices)| *indices));
        }
        let chunk = CrowdChunk {
            first: self.lens,
            vertices: built.vertices,
            indices,
            bones: built.bones,
        };
        self.lens = [
            vertex + chunk.vertices.len() as u32,
            index + chunk.indices.len() as u32,
            bone + chunk.bones.len() as u32,
        ];
        self.chunks.push(Arc::new(chunk));
        self.groups.push(group);
    }
}

fn role_code(role: BoneRole) -> Option<u32> {
    Some(match role {
        BoneRole::Fixed => ROLE_FIXED,
        BoneRole::Head => ROLE_HEAD,
        BoneRole::RightArm => ROLE_RIGHT_ARM,
        BoneRole::LeftArm => ROLE_LEFT_ARM,
        BoneRole::RightLeg => ROLE_RIGHT_LEG,
        BoneRole::LeftLeg => ROLE_LEFT_LEG,
        BoneRole::WingNegativeX
        | BoneRole::WingPositiveX
        | BoneRole::CrossedArms
        | BoneRole::Nose => return None,
    })
}

/// Builds a composite from `parts`; `main` is the entity's rig (hip, item attachments).
fn build(parts: &[Part<'_>], main: &Rig, max_offset: f32) -> Option<Built> {
    let mut built = Built {
        hip: clip_pose::hip(main),
        ..Default::default()
    };
    // Composite bone offset of each distinct rig; the main rig is always first.
    let mut rigs: Vec<(usize, u32)> = Vec::new();
    let mut reach: Vec<(f32, f32)> = Vec::new();
    let mut add_rig = |built: &mut Built, reach: &mut Vec<(f32, f32)>, rig: &Rig| -> Option<u32> {
        let key = std::ptr::from_ref(rig) as usize;
        if let Some(&(_, offset)) = rigs.iter().find(|(rig, _)| *rig == key) {
            return Some(offset);
        }
        let offset = built.bones.len() as u32;
        for bone in &rig.bones {
            let parent = bone
                .parent
                .map_or(-1, |parent| (offset as usize + parent) as i32);
            let carried = clip_pose::carried(rig, bone);
            built.bones.push(CrowdBone {
                pivot: bone.pivot.to_array(),
                parent,
                rotation: bone.rotation.to_array(),
                role: role_code(bone.role)?,
                scale: bone.scale.to_array(),
                channel: clip_pose::channel_of(bone).map_or(NO_CHANNEL, |channel| channel as u32),
                flags: if carried { BONE_CARRIED } else { 0 },
                _pad: [0; 3],
            });
            // Distance bound from the model origin under any rotation: every pivot of the
            // chain (with two clip layers' offsets), the carry about the hip, and scales.
            let (base, scale) = if parent < 0 {
                (0., 1.)
            } else {
                reach[parent as usize]
            };
            let mut extent = bone.pivot.length() + 2. * max_offset;
            if carried {
                extent += 2. * built.hip.length() + 2. * max_offset;
            }
            reach.push((
                base + scale * extent,
                scale * bone.scale.abs().max_element(),
            ));
        }
        rigs.push((key, offset));
        Some(offset)
    };
    add_rig(&mut built, &mut reach, main)?;
    let mut radius = 0_f32;
    for part in parts {
        match *part {
            Part::Rig {
                rig,
                texture,
                color,
                material,
                body,
            } => {
                let offset = add_rig(&mut built, &mut reach, rig)?;
                let slot = MATERIALS.iter().position(|m| *m == material)?;
                let max_lod = if material == EntityMaterial::ArmorDecalCutoutNoCull {
                    1
                } else {
                    2
                };
                for (index, bone) in rig.bones.iter().enumerate() {
                    let id = offset + index as u32;
                    let (base, scale) = reach[id as usize];
                    for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
                        let first = built.vertices.len() as u32;
                        for v in quad {
                            let position = [v[0], v[1], v[2]];
                            radius = radius.max(base + scale * Vec3::from_array(position).length());
                            built.vertices.push(CrowdVertex {
                                position,
                                normal: *normal,
                                uv: [
                                    texture.uv[0] + v[3] * (texture.uv[2] - texture.uv[0]),
                                    texture.uv[1] + v[4] * (texture.uv[3] - texture.uv[1]),
                                ],
                                color: [color[0], color[1], color[2], 1.],
                                layer: texture.layer,
                                bone_flags: id | if body { VERTEX_BODY } else { 0 },
                            });
                        }
                        built.quads[slot].push((
                            max_lod,
                            [first, first + 1, first + 2, first + 2, first + 3, first],
                        ));
                    }
                }
            }
            Part::Item { held, model } => {
                let parent = held.bone as u32;
                let id = built.bones.len() as u32;
                let channel = if held.slot < 2 {
                    Channel::item(held.hand) as u32
                } else {
                    NO_CHANNEL
                };
                built.bones.push(CrowdBone {
                    parent: parent as i32,
                    role: ROLE_ITEM,
                    scale: [1.; 3],
                    channel,
                    ..Default::default()
                });
                let (base, scale) = reach[parent as usize];
                let grip = if held.slot < 2 {
                    2. * clip_pose::grip_point(held.hand).length() + 2. * max_offset
                } else {
                    0.
                };
                reach.push((base + scale * grip, scale));
                let attachment = p::equipment_item_matrix(
                    Mat4::IDENTITY,
                    model,
                    if held.slot < 2 { held.hand } else { held.slot },
                    held.context,
                    held.skull,
                );
                let area = |q: &[[f32; 3]; 4]| {
                    (Vec3::from_array(q[1]) - Vec3::from_array(q[0]))
                        .cross(Vec3::from_array(q[3]) - Vec3::from_array(q[0]))
                        .length()
                };
                let largest = model
                    .quads
                    .iter()
                    .map(|q| area(&q.positions))
                    .fold(0., f32::max);
                for quad in &model.quads {
                    let positions = quad
                        .positions
                        .map(|p| attachment.transform_point3(Vec3::from_array(p)));
                    let normal = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
                    let color = match quad.tint {
                        TintKind::Constant(color) => color,
                        _ => [1.; 3],
                    };
                    let first = built.vertices.len() as u32;
                    for (position, uv) in positions.iter().zip(quad.uv) {
                        radius = radius.max(base + scale * (grip + position.length()));
                        built.vertices.push(CrowdVertex {
                            position: position.to_array(),
                            normal: normal.to_array(),
                            uv,
                            color: [color[0], color[1], color[2], quad.alpha],
                            layer: quad.texture_layer,
                            bone_flags: id
                                | VERTEX_ITEM_NORMAL
                                | u32::from(quad.light_emission) << VERTEX_EMISSION_SHIFT,
                        });
                    }
                    // Extrusion edges of a generated item vanish below the full detail level.
                    let max_lod = if area(&quad.positions) * 8. < largest {
                        0
                    } else {
                        2
                    };
                    let slot = MATERIALS.iter().position(|m| *m == item_material(quad))?;
                    built.quads[slot].push((
                        max_lod,
                        [first, first + 1, first + 2, first + 2, first + 3, first],
                    ));
                }
            }
        }
    }
    if built.bones.len() > usize::from(u16::MAX) {
        return None;
    }
    built.radius = radius;
    Some(built)
}

/// CPU mirror of the GPU bone pass (`entity_instancing_prepare.wgsl`): the composite
/// bone matrices of `instance`, model root included. Tests compare both renderers with it.
#[cfg(test)]
pub(crate) fn evaluate(
    meshes: &CrowdMeshes,
    instance: &CrowdInstance,
    clips: &ClipLibrary,
) -> Vec<Mat4> {
    let group = &meshes.groups[instance.words[0] as usize];
    let bones = &meshes.chunks[instance.words[0] as usize].bones;
    let r = instance.root;
    let root = Mat4::from_cols(
        glam::Vec4::new(r[0][0], r[1][0], r[2][0], 0.),
        glam::Vec4::new(r[0][1], r[1][1], r[2][1], 0.),
        glam::Vec4::new(r[0][2], r[1][2], r[2][2], 0.),
        glam::Vec4::new(r[0][3], r[1][3], r[2][3], 1.),
    );
    let flags = instance.words[2];
    let layers: clip_pose::Active = std::array::from_fn(|index| {
        let clip = (instance.words[3] >> (16 * index)) as u16;
        (clip != 0).then(|| clip_pose::ActiveLayer {
            clip,
            seconds: instance.layers[index * 2],
            weight: instance.layers[index * 2 + 1],
            mirror: flags & INSTANCE_MIRROR[index] != 0,
        })
    });
    let decode = |code: u32| [0., 1., 0.5][code as usize & 3];
    let arms: [p::AxisMap; 2] = std::array::from_fn(|arm| p::AxisMap {
        scale: Vec3::new(
            decode(flags >> (arm * 6)),
            decode(flags >> (arm * 6 + 2)),
            decode(flags >> (arm * 6 + 4)),
        ),
        offset: Vec3::new(
            instance.arms[arm][0],
            instance.arms[arm][1],
            instance.arms[arm][2],
        ),
    });
    let carry = clips.carry(&layers, Vec3::from_array(group.hip));
    let mut out: Vec<Mat4> = Vec::with_capacity(bones.len());
    for bone in bones {
        let rest = Vec3::from_array(bone.rotation);
        let parent = if bone.parent < 0 {
            root
        } else {
            out[bone.parent as usize]
        };
        if bone.role == ROLE_ITEM {
            let grip = if bone.channel == NO_CHANNEL {
                None
            } else {
                clips.delta(
                    &layers,
                    if bone.channel == Channel::RightItem as u32 {
                        Channel::RightItem
                    } else {
                        Channel::LeftItem
                    },
                )
            };
            let hand = usize::from(bone.channel == Channel::LeftItem as u32);
            out.push(grip.map_or(parent, |grip| {
                parent * grip.about(clip_pose::grip_point(hand))
            }));
            continue;
        }
        let mut rotation = rest;
        let mut pivot = Vec3::from_array(bone.pivot);
        match bone.role {
            ROLE_HEAD => {
                rotation.x += instance.pose[0];
                rotation.y += instance.pose[1];
            }
            ROLE_RIGHT_LEG => rotation.x += instance.pose[2],
            ROLE_LEFT_LEG => rotation.x += instance.pose[3],
            ROLE_RIGHT_ARM | ROLE_LEFT_ARM => {
                rotation = arms[usize::from(bone.role == ROLE_LEFT_ARM)].apply(rotation);
                if flags & INSTANCE_PIVOT_Z_ZERO != 0 {
                    pivot.z = 0.;
                }
            }
            _ => {}
        }
        if flags & INSTANCE_RIDING != 0 && matches!(bone.role, ROLE_RIGHT_LEG | ROLE_LEFT_LEG) {
            let side = if bone.role == ROLE_RIGHT_LEG { 1. } else { -1. };
            rotation = Vec3::new(
                p::RIDING_LEG_PITCH,
                side * p::RIDING_LEG_YAW,
                side * p::RIDING_LEG_ROLL,
            );
        }
        if bone.channel != NO_CHANNEL {
            let channel = [
                Channel::Head,
                Channel::Body,
                Channel::RightArm,
                Channel::LeftArm,
                Channel::RightLeg,
                Channel::LeftLeg,
            ][bone.channel as usize];
            clips.apply(
                &layers,
                channel,
                instance.arms[0][3],
                rest,
                &mut rotation,
                &mut pivot,
            );
        }
        let mut parent = parent;
        if let Some(carry) = carry
            && bone.flags & BONE_CARRIED != 0
        {
            parent *= carry;
        }
        out.push(
            parent * super::resources::transform(pivot, rotation, Vec3::from_array(bone.scale)),
        );
    }
    out
}

#[cfg(test)]
#[path = "crowd_tests.rs"]
pub(crate) mod tests;
