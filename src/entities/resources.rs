use super::render_living::MotionFacts;
use super::solaris_models;
use crate::protocol::{content::Content, Result};
use glam::{Mat4, Vec3};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

/// Map from the short, trusted names entity facts are looked up by (kinds, metadata
/// accessors), many times per entity each tick and frame.
pub type NameMap<V> = HashMap<String, V, BuildHasherDefault<NameHasher>>;

/// Multiply-rotate hash over 8-byte words (the FxHash step): a fraction of SipHash's cost
/// for the bundled fact names, which are not attacker-chosen.
#[derive(Clone, Copy, Default)]
pub struct NameHasher(u64);
impl NameHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}
impl Hasher for NameHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut words = bytes.chunks_exact(8);
        for word in &mut words {
            self.add(u64::from_le_bytes(word.try_into().unwrap()));
        }
        let rest = words.remainder();
        if !rest.is_empty() {
            let mut word = [0; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// Solaris-owned entity metadata: protocol ids, dimensions, metadata accessors, texture/variant
/// selection, head/body turn limits and armor shell keys. Pure data, no geometry.
const ENTITY_FACTS: &str = include_str!("../../assets/solaris/entities/entity-facts.json");
const FACTS_SCHEMA: u32 = 1;

#[derive(Deserialize)]
struct Document {
    schema: u32,
    game_version: String,
    entities: NameMap<EntitySchema>,
    families: BTreeMap<String, Family>,
    armor_models: BTreeMap<String, ArmorModels>,
    simple_particles: Vec<String>,
    default_skins: Vec<DefaultSkin>,
    poses: BTreeMap<u8, String>,
    attributes: BTreeMap<String, AttributeBounds>,
    builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
    motion: BTreeMap<String, MotionFacts>,
}
#[derive(Deserialize, Debug)]
pub struct DefaultSkin {
    pub texture: String,
    pub model: super::PlayerModel,
}
#[derive(Clone, Deserialize, Debug)]
pub struct Dimensions {
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub fixed: bool,
    pub attachments: BTreeMap<String, Vec<[f64; 3]>>,
}
#[derive(Debug)]
pub enum DimensionConstant {
    Dimensions(Dimensions),
    Poses(BTreeMap<String, Dimensions>),
}
impl<'de> Deserialize<'de> for DimensionConstant {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        // serde's untagged buffer loses serde_json's arbitrary-precision number
        // representation. Dispatch the two object shapes before typed decoding.
        let value = Value::deserialize(deserializer)?;
        if value.get("width").is_some() {
            serde_json::from_value(value)
                .map(Self::Dimensions)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Poses)
                .map_err(serde::de::Error::custom)
        }
    }
}
#[derive(Deserialize, Debug)]
pub struct DefaultAttribute {
    pub id: u32,
    pub value: f64,
}
#[derive(Deserialize, Debug)]
pub struct AttributeBounds {
    pub id: u32,
    pub min: Option<f64>,
    pub max: Option<f64>,
}
#[derive(Deserialize, Debug)]
pub struct EntitySchema {
    pub protocol_id: u32,
    pub width: f32,
    pub fire_immune: bool,
    pub class: String,
    pub eye_height: f32,
    pub dimension_constants: BTreeMap<String, DimensionConstant>,
    pub enum_ids: BTreeMap<String, BTreeMap<i32, String>>,
    pub enum_numbers: BTreeMap<String, BTreeMap<i32, BTreeMap<String, f32>>>,
    pub hierarchy: Vec<String>,
    pub method_owners: BTreeMap<String, String>,
    pub dimensions: Dimensions,
    pub default_attributes: BTreeMap<String, DefaultAttribute>,
    pub height: f32,
    /// Hashed, not ordered: looked up by name per entity many times a tick and frame.
    pub accessors: NameMap<[u32; 2]>,
    /// What `Models::dimensions` reads of this schema, resolved from its names on first use
    /// (a schema does not change once its models are activated).
    #[serde(skip)]
    pub sizing: std::sync::OnceLock<Sizing>,
    /// Metadata indices of the accessors read per entity each frame and tick, resolved from
    /// their names on first use.
    #[serde(skip)]
    pub ids: std::sync::OnceLock<Ids>,
}

/// Metadata indices of the accessors the frame and tick read for every entity, and the kind
/// facts the tick reads, resolved once per kind (`Models::ids`); `None` where the kind has no
/// such accessor.
#[derive(Debug)]
pub struct Ids {
    pub baby: Option<u8>,
    pub pose: Option<u8>,
    pub shared_flags: Option<u8>,
    pub mob_flags: Option<u8>,
    pub client_flags: Option<u8>,
    pub living_flags: Option<u8>,
    pub spell: Option<u8>,
    pub main_hand: Option<u8>,
    pub sleeping_pos: Option<u8>,
    pub size: Option<u8>,
    pub item: Option<u8>,
    pub item_stack: Option<u8>,
    pub peek: Option<u8>,
    pub standing: Option<u8>,
    pub id_flags: Option<u8>,
    pub health: Option<u8>,
    /// `IS_CHARGING_CROSSBOW` (pillager) and `DATA_IS_CHARGING_CROSSBOW` (piglin).
    pub charging: [Option<u8>; 2],
    pub last_pose_change: Option<u8>,
    /// An `AbstractBoat` (reads its pre-tick bounds, `source_state::tick`).
    pub boat: bool,
    /// Vanilla draw-distance rule of the kind (`view_range::within`).
    pub view_rule: super::view_range::Rule,
    /// Owner of `getPassengerAttachmentPoint` (`player::vehicle::passenger_position`).
    pub seat: Seat,
}

/// Owner of a kind's `getPassengerAttachmentPoint`, which places its passengers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// `AbstractBoat`; chest boats and rafts seat differently.
    Boat { chest: bool, raft: bool },
    /// `AbstractHorse` (`SeatMotion`).
    Horse,
    Camel,
    /// `Strider` (`SeatMotion`).
    Strider,
    CubeMob,
    /// `Entity`, `Llama`, `AbstractMinecart`: the received `PASSENGER` attachment.
    Attachment,
    /// No source seat rule.
    Unknown,
}

impl Seat {
    /// The kind tracks `SeatMotion` (horse stand, strider walk) for its passengers.
    pub fn moves(self) -> bool {
        matches!(self, Self::Horse | Self::Strider)
    }
}

impl Ids {
    const NONE: Self = Self {
        baby: None,
        pose: None,
        shared_flags: None,
        mob_flags: None,
        client_flags: None,
        living_flags: None,
        spell: None,
        main_hand: None,
        sleeping_pos: None,
        size: None,
        item: None,
        item_stack: None,
        peek: None,
        standing: None,
        id_flags: None,
        health: None,
        charging: [None; 2],
        last_pose_change: None,
        boat: false,
        seat: Seat::Unknown,
        view_rule: super::view_range::Rule::Entity,
    };
    fn of(schema: &EntitySchema) -> Self {
        let id = |name: &str| schema.accessors.get(name).and_then(|v| u8::try_from(v[0]).ok());
        Self {
            baby: id("DATA_BABY_ID"),
            pose: id("DATA_POSE"),
            shared_flags: id("DATA_SHARED_FLAGS_ID"),
            mob_flags: id("DATA_MOB_FLAGS_ID"),
            client_flags: id("DATA_CLIENT_FLAGS"),
            living_flags: id("DATA_LIVING_ENTITY_FLAGS"),
            spell: id("DATA_SPELL_CASTING_ID"),
            main_hand: id("DATA_PLAYER_MAIN_HAND"),
            sleeping_pos: id("SLEEPING_POS_ID"),
            size: id("ID_SIZE"),
            item: id("DATA_ITEM"),
            item_stack: id("DATA_ITEM_STACK_ID"),
            peek: id("DATA_PEEK_ID"),
            standing: id("DATA_STANDING_ID"),
            id_flags: id("DATA_ID_FLAGS"),
            health: id("DATA_HEALTH_ID"),
            charging: [id("IS_CHARGING_CROSSBOW"), id("DATA_IS_CHARGING_CROSSBOW")],
            last_pose_change: id("LAST_POSE_CHANGE_TICK"),
            boat: schema.hierarchy.iter().any(|class| class == "net.minecraft.world.entity.vehicle.boat.AbstractBoat"),
            seat: match schema.method_owners.get("getPassengerAttachmentPoint").map(String::as_str) {
                Some("net.minecraft.world.entity.vehicle.boat.AbstractBoat") => Seat::Boat {
                    chest: schema.hierarchy.iter().any(|name| {
                        name == "net.minecraft.world.entity.vehicle.boat.AbstractChestBoat"
                    }),
                    raft: matches!(schema.class.as_str(),
                        "net.minecraft.world.entity.vehicle.boat.Raft"
                            | "net.minecraft.world.entity.vehicle.boat.ChestRaft"),
                },
                Some("net.minecraft.world.entity.animal.equine.AbstractHorse") => Seat::Horse,
                Some("net.minecraft.world.entity.animal.camel.Camel") => Seat::Camel,
                Some("net.minecraft.world.entity.monster.Strider") => Seat::Strider,
                Some("net.minecraft.world.entity.monster.cubemob.AbstractCubeMob") => Seat::CubeMob,
                Some(
                    "net.minecraft.world.entity.Entity"
                    | "net.minecraft.world.entity.animal.equine.Llama"
                    | "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart",
                ) => Seat::Attachment,
                _ => Seat::Unknown,
            },
            view_rule: super::view_range::Rule::of(&schema.hierarchy, schema.hierarchy.iter().any(|class| class == "net.minecraft.world.entity.player.Player")),
        }
    }
}

/// The parts of a schema `Models::dimensions` and `physical::bounds_at` consult on every
/// call, resolved once from its names (kind, classes, method owners, dimension constants), so
/// a call reads only the entity's metadata and attributes.
#[derive(Debug)]
pub struct Sizing {
    pose: Option<u8>,
    client_flags: Option<u8>,
    baby: Option<u8>,
    state: Option<u8>,
    puff: Option<u8>,
    variant: Option<u8>,
    size: Option<u8>,
    living: bool,
    camel: bool,
    /// `minecraft:scale` attribute id and default value.
    scale: Option<(u32, f64)>,
    sanitize: Sanitize,
    /// `getAgeScale` of a baby.
    baby_scale: f32,
    /// Owner of `getDefaultDimensions`.
    owner: SizeOwner,
    /// `minecraft:armor_stand`: marker, small or type dimensions by its client flags.
    armor_stand: bool,
    /// The kind has a `POSES` table (avatars): dimensions by pose name.
    avatar: bool,
    /// Per `DATA_POSE` id (`Models::poses`, `None` for an id it lacks): what `sized` asks of
    /// the pose's name, resolved once instead of comparing names per entity.
    poses: Vec<Option<PoseRule>>,
    goat: bool,
    /// A sniffer's `State` ids named `DIGGING` (empty for every other kind).
    digging_states: Vec<i32>,
    /// `*_DIMENSIONS` constants of the kind (`None`: the kind has none).
    sleeping: Option<Dimensions>,
    marker: Option<Dimensions>,
    baby_dimensions: Option<Dimensions>,
    sitting: Option<Dimensions>,
    long_jumping: Option<Dimensions>,
    digging: Option<Dimensions>,
    /// The kind's box rule in `physical::bounds_at`.
    shape: Shape,
}

/// One pose of a kind (`Sizing::poses`).
#[derive(Debug)]
struct PoseRule {
    sleeping: bool,
    sitting: bool,
    long_jumping: bool,
    /// An avatar's dimensions in this pose: its `POSES` entry, else `STANDING_DIMENSIONS`.
    avatar: Option<Dimensions>,
}

/// How `physical::bounds_at` derives a kind's box from its dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Width and height around the position.
    Box,
    /// `item_frame`, `glow_item_frame`: a thin plate on the attached face.
    ItemFrame,
    /// `interaction`: received width and height.
    Interaction,
    /// `polar_bear`: taller while standing.
    PolarBear,
    /// `warden`: 1 block high while emerging or digging.
    Warden,
    /// `shulker`: widened toward its attach face by its peek.
    Shulker,
    /// `AbstractWindCharge`: moved down 0.15.
    WindCharge,
}

/// `sanitizeScale` owner of a kind.
#[derive(Debug)]
enum Sanitize {
    Keep,
    AtMost(f32),
    One,
}

#[derive(Debug, PartialEq, Eq)]
enum SizeOwner {
    Other,
    ChestedHorse,
    CubeMob,
    Pufferfish,
    Salmon,
    Phantom,
}

impl Sizing {
    fn of(schema: &EntitySchema, kind: &str, poses: &BTreeMap<u8, String>) -> Self {
        let id = |name: &str| schema.accessors.get(name).and_then(|v| u8::try_from(v[0]).ok());
        let inherits = |class: &str| schema.hierarchy.iter().any(|entry| entry == class);
        let owner = |method: &str| schema.method_owners.get(method).map(String::as_str);
        let constant = |name: &str| match schema.dimension_constants.get(name) {
            Some(DimensionConstant::Dimensions(value)) => Some(value.clone()),
            _ => None,
        };
        Self {
            pose: id("DATA_POSE"),
            client_flags: id("DATA_CLIENT_FLAGS"),
            baby: id("DATA_BABY_ID"),
            state: id("DATA_STATE"),
            puff: id("PUFF_STATE"),
            variant: id("DATA_TYPE"),
            size: id("ID_SIZE"),
            living: inherits("net.minecraft.world.entity.LivingEntity"),
            camel: inherits("net.minecraft.world.entity.animal.camel.Camel"),
            scale: schema.default_attributes.get("minecraft:scale").map(|scale| (scale.id, scale.value)),
            sanitize: match owner("sanitizeScale") {
                Some("net.minecraft.world.entity.animal.happyghast.HappyGhast") => Sanitize::AtMost(1.),
                Some("net.minecraft.world.entity.monster.Shulker") => Sanitize::AtMost(3.),
                Some("net.minecraft.world.entity.boss.enderdragon.EnderDragon") => Sanitize::One,
                _ => Sanitize::Keep,
            },
            baby_scale: match owner("getAgeScale") {
                Some(
                    "net.minecraft.world.entity.animal.armadillo.Armadillo"
                    | "net.minecraft.world.entity.animal.camel.Camel",
                ) => 0.6,
                Some("net.minecraft.world.entity.animal.dolphin.Dolphin") => 0.65,
                Some("net.minecraft.world.entity.animal.goat.Goat") => 0.55,
                Some("net.minecraft.world.entity.animal.happyghast.HappyGhast") => 0.2375,
                Some("net.minecraft.world.entity.animal.turtle.Turtle") => 0.3,
                _ => 0.5,
            },
            owner: match owner("getDefaultDimensions") {
                Some("net.minecraft.world.entity.animal.equine.AbstractChestedHorse") => SizeOwner::ChestedHorse,
                Some("net.minecraft.world.entity.monster.cubemob.AbstractCubeMob") => SizeOwner::CubeMob,
                Some("net.minecraft.world.entity.animal.fish.Pufferfish") => SizeOwner::Pufferfish,
                Some("net.minecraft.world.entity.animal.fish.Salmon") => SizeOwner::Salmon,
                Some("net.minecraft.world.entity.monster.Phantom") => SizeOwner::Phantom,
                _ => SizeOwner::Other,
            },
            armor_stand: kind == "minecraft:armor_stand",
            avatar: matches!(schema.dimension_constants.get("POSES"), Some(DimensionConstant::Poses(_))),
            poses: {
                let table = match schema.dimension_constants.get("POSES") {
                    Some(DimensionConstant::Poses(table)) => Some(table),
                    _ => None,
                };
                let mut rules: Vec<Option<PoseRule>> = Vec::new();
                for (&id, name) in poses {
                    let index = usize::from(id);
                    if rules.len() <= index {
                        rules.resize_with(index + 1, || None);
                    }
                    rules[index] = Some(PoseRule {
                        sleeping: name == "SLEEPING",
                        sitting: name == "SITTING",
                        long_jumping: name == "LONG_JUMPING",
                        avatar: table.and_then(|table| table.get(name)).cloned()
                            .or_else(|| constant("STANDING_DIMENSIONS")),
                    });
                }
                rules
            },
            goat: schema.class == "net.minecraft.world.entity.animal.goat.Goat",
            digging_states: if schema.class == "net.minecraft.world.entity.animal.sniffer.Sniffer" {
                schema.enum_ids.get("State").map_or_else(Vec::new, |ids| {
                    ids.iter().filter(|(_, name)| *name == "DIGGING").map(|(id, _)| *id).collect()
                })
            } else {
                Vec::new()
            },
            sleeping: constant("SLEEPING_DIMENSIONS"),
            marker: constant("MARKER_DIMENSIONS"),
            baby_dimensions: constant("BABY_DIMENSIONS"),
            sitting: constant("SITTING_DIMENSIONS"),
            long_jumping: constant("LONG_JUMPING_DIMENSIONS"),
            digging: constant("DIGGING_DIMENSIONS"),
            shape: match kind {
                "minecraft:item_frame" | "minecraft:glow_item_frame" => Shape::ItemFrame,
                "minecraft:interaction" => Shape::Interaction,
                "minecraft:polar_bear" => Shape::PolarBear,
                "minecraft:warden" => Shape::Warden,
                "minecraft:shulker" => Shape::Shulker,
                _ if inherits("net.minecraft.world.entity.projectile.hurtingprojectile.windcharge.AbstractWindCharge") => Shape::WindCharge,
                _ => Shape::Box,
            },
        }
    }
}

impl EntitySchema {
    /// Forgets the facts resolved from this schema's names (it may be about to change).
    fn forget_resolved(&mut self) {
        self.sizing = std::sync::OnceLock::new();
        self.ids = std::sync::OnceLock::new();
    }
}

static SCHEMA_TABLES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Entity schemas by kind name. A kind keeps its slot for the table's life, and an entity
/// remembers its kind's slot (`Models::schema_of`), so per-entity lookups index instead of
/// hashing the kind name. Changing a schema (`get_mut`, `insert`) forgets the facts resolved
/// from its names.
#[derive(Debug)]
pub struct Schemas {
    /// Process-unique: a slot an entity remembers belongs to one table.
    table: u64,
    slots: NameMap<u32>,
    schemas: Vec<Option<(String, EntitySchema)>>,
}

impl Default for Schemas {
    fn default() -> Self {
        Self {
            table: SCHEMA_TABLES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            slots: NameMap::default(),
            schemas: Vec::new(),
        }
    }
}

impl Schemas {
    pub fn get(&self, kind: &str) -> Option<&EntitySchema> {
        self.at(*self.slots.get(kind)?)
    }
    pub fn get_mut(&mut self, kind: &str) -> Option<&mut EntitySchema> {
        let slot = *self.slots.get(kind)?;
        let (_, schema) = self.schemas[slot as usize].as_mut()?;
        schema.forget_resolved();
        Some(schema)
    }
    pub fn contains_key(&self, kind: &str) -> bool {
        self.slots.contains_key(kind)
    }
    pub fn insert(&mut self, kind: String, mut schema: EntitySchema) -> Option<EntitySchema> {
        schema.forget_resolved();
        if let Some(&slot) = self.slots.get(&kind) {
            return self.schemas[slot as usize].replace((kind, schema)).map(|(_, old)| old);
        }
        let slot = u32::try_from(self.schemas.len()).expect("entity kinds fit u32");
        self.slots.insert(kind.clone(), slot);
        self.schemas.push(Some((kind, schema)));
        None
    }
    /// Removes `kind`; its slot stays empty, so entities that remembered it look it up again.
    pub fn remove(&mut self, kind: &str) -> Option<EntitySchema> {
        let slot = self.slots.remove(kind)?;
        self.schemas[slot as usize].take().map(|(_, schema)| schema)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &EntitySchema)> {
        self.schemas.iter().flatten().map(|(kind, schema)| (kind, schema))
    }
    fn at(&self, slot: u32) -> Option<&EntitySchema> {
        self.schemas.get(slot as usize)?.as_ref().map(|(_, schema)| schema)
    }
}

impl FromIterator<(String, EntitySchema)> for Schemas {
    fn from_iter<I: IntoIterator<Item = (String, EntitySchema)>>(entries: I) -> Self {
        let mut schemas = Self::default();
        for (kind, schema) in entries {
            schemas.insert(kind, schema);
        }
        schemas
    }
}

impl<K: AsRef<str> + ?Sized> std::ops::Index<&K> for Schemas {
    type Output = EntitySchema;
    fn index(&self, kind: &K) -> &EntitySchema {
        self.get(kind.as_ref()).expect("entity schema of a known kind")
    }
}

/// The schema slot an entity's kind had in a table (`Models::schema_of`), for the kind name
/// string it was resolved from (address and length: an entity's kind name never changes).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KindSlot {
    table: u64,
    name: (usize, usize),
    slot: u32,
}
#[derive(Deserialize, Debug)]
pub struct Family {
    pub rig: String,
    pub texture: Option<String>,
    pub baby_rig: Option<String>,
    pub textures: Vec<String>,
    pub renderer_class: Option<String>,
    pub materials: BTreeMap<String, Value>,
}
#[derive(Clone, Copy, PartialEq)]
pub enum BoneRole {
    Fixed,
    Head,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
    WingNegativeX,
    WingPositiveX,
    /// Villager-family `arms` bone: the main-hand item rests across it (`CrossedArmsItemLayer`).
    CrossedArms,
    /// Witch nose: a held potion hangs from it.
    Nose,
}
pub struct Bone {
    pub parent: Option<usize>,
    pub pivot: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub role: BoneRole,
    pub name: String,
    pub quads: Vec<[[f32; 5]; 4]>,
    pub normals: Vec<[f32; 3]>,
}
pub struct Rig {
    pub bones: Vec<Bone>,
    pub part_by_name: BTreeMap<String, usize>,
}
pub struct Models {
    pub rigs: BTreeMap<String, Rig>,
    /// Looked up by kind per entity many times a tick and frame (`Models::schema_of`).
    pub schemas: Schemas,
    pub families: BTreeMap<String, Family>,
    pub builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
    pub simple_particles: Vec<String>,
    pub default_skins: Vec<DefaultSkin>,
    pub poses: BTreeMap<u8, String>,
    pub attributes: BTreeMap<String, AttributeBounds>,
    pub attribute_ids: BTreeMap<u32, String>,
    pub digest: String,
    pub(crate) armor_models: BTreeMap<String, ArmorModels>,
    /// Hashed by kind name: looked up per ticked living entity.
    pub(crate) motion: NameMap<MotionFacts>,
}
/// Renderer rig keys of the per-slot armor shells of a humanoid family.
#[derive(Deserialize, Debug)]
pub(crate) struct ArmorModels {
    pub adult: BTreeMap<String, String>,
    pub baby: BTreeMap<String, String>,
}
impl Models {
    fn parse() -> Result<Document> {
        let doc: Document = serde_json::from_str(ENTITY_FACTS)
            .map_err(|e| format!("solaris entity facts schema: {e}"))?;
        if doc.schema != FACTS_SCHEMA || doc.game_version != "26.3" {
            return Err("solaris entity facts source/target mismatch".into());
        }
        if doc.entities.len() > 1024 || doc.poses.len() != 18 {
            return Err("solaris entity facts catalog mismatch".into());
        }
        for (name, schema) in &doc.entities {
            if !schema.width.is_finite()
                || schema.width < 0.
                || !schema.height.is_finite()
                || schema.height < 0.
                || !schema.eye_height.is_finite()
            {
                return Err(format!("entity dimensions invalid for {name}"));
            }
        }
        if doc.motion.len() != doc.entities.len()
            || doc.motion.iter().any(|(kind, facts)| {
                doc.entities
                    .get(kind)
                    .is_none_or(|schema| schema.class != facts.class)
            })
        {
            return Err("entity motion holder coverage mismatch".into());
        }
        Ok(doc)
    }
    fn check_registry(doc: &Document, content: &Content) -> Result<()> {
        for (name, schema) in &doc.entities {
            if content.entity_names.get(&schema.protocol_id) != Some(name) {
                return Err(format!("entity registry/facts mismatch for {name}"));
            }
        }
        Ok(())
    }
    fn digest() -> String {
        let mut digest = Sha256::new();
        digest.update(ENTITY_FACTS.as_bytes());
        digest.update(solaris_models::embedded_digest());
        format!("{:x}", digest.finalize())
    }
    fn attribute_ids(attributes: &BTreeMap<String, AttributeBounds>) -> BTreeMap<u32, String> {
        attributes
            .iter()
            .map(|(name, value)| (value.id, name.clone()))
            .collect()
    }
    /// Protocol/dimension/motion metadata only: no families or render resources.
    pub(crate) fn load_state(content: &Content) -> Result<Self> {
        let doc = Self::parse()?;
        Self::check_registry(&doc, content)?;
        Ok(Self {
            rigs: BTreeMap::new(),
            families: BTreeMap::new(),
            simple_particles: doc.simple_particles,
            default_skins: Vec::new(),
            schemas: doc.entities.into_iter().collect(),
            poses: doc.poses,
            builtin_variants: doc.builtin_variants,
            attribute_ids: Self::attribute_ids(&doc.attributes),
            attributes: doc.attributes,
            digest: Self::digest(),
            armor_models: BTreeMap::new(),
            motion: doc.motion.into_iter().collect(),
        })
    }
    pub fn load(content: &Content) -> Result<Self> {
        let doc = Self::parse()?;
        if doc.default_skins.len() != 18 {
            return Err("source skin catalog mismatch".into());
        }
        Self::check_registry(&doc, content)?;
        // Geometry is Solaris-authored (`solaris_models`); the facts carry no meshes. A family
        // whose rig has no authored model gets a visible entity-sized placeholder box.
        let mut rigs = BTreeMap::new();
        for (kind, family) in &doc.families {
            let (width, height) = doc
                .entities
                .get(kind)
                .map_or((0.6, 1.8), |schema| (schema.width, schema.height));
            for key in std::iter::once(&family.rig).chain(family.baby_rig.as_ref()) {
                if solaris_models::authored(key).is_none() {
                    rigs.entry(key.clone())
                        .or_insert_with(|| solaris_models::placeholder(width, height));
                }
            }
        }
        Ok(Self {
            rigs,
            schemas: doc.entities.into_iter().collect(),
            families: doc.families,
            builtin_variants: doc.builtin_variants,
            simple_particles: doc.simple_particles,
            default_skins: doc.default_skins,
            poses: doc.poses,
            attribute_ids: Self::attribute_ids(&doc.attributes),
            attributes: doc.attributes,
            digest: Self::digest(),
            armor_models: doc.armor_models,
            motion: doc.motion.into_iter().collect(),
        })
    }
    /// Rig key of one armor slot shell (`head`/`chest`/`legs`/`feet`) of a humanoid family.
    pub(crate) fn armor_rig(&self, kind: &str, baby: bool, part: &str) -> Option<&str> {
        let armor = self.armor_models.get(kind)?;
        let set = if baby { &armor.baby } else { &armor.adult };
        set.get(part).map(String::as_str)
    }
    /// Geometry for a renderer rig key: explicit entry, else Solaris-authored model, else a
    /// placeholder box. Never fails; a missing model must not stop the frame.
    pub(crate) fn rig(&self, key: &str) -> &Rig {
        static FALLBACK: std::sync::LazyLock<Rig> =
            std::sync::LazyLock::new(|| solaris_models::placeholder(0.75, 0.75));
        self.rigs
            .get(key)
            .or_else(|| solaris_models::authored(key))
            .unwrap_or(&FALLBACK)
    }
    /// Whether a real (non-placeholder) model exists for the key.
    pub(crate) fn has_authored_rig(&self, key: &str) -> bool {
        self.rigs.get(key).is_some_and(|rig| !rig.is_placeholder())
            || (!self.rigs.contains_key(key) && solaris_models::authored(key).is_some())
    }
    pub fn accessor(&self, kind: &str, name: &str) -> Option<u8> {
        self.schemas
            .get(kind)?
            .accessors
            .get(name)
            .and_then(|v| u8::try_from(v[0]).ok())
    }
    /// `e`'s schema: by the slot `e` remembered from an earlier lookup in this table, else by
    /// its kind name (and remembered).
    pub(crate) fn schema_of(&self, e: &super::Entity) -> Option<&EntitySchema> {
        let name = (e.type_name.as_ptr() as usize, e.type_name.len());
        let known = e.kind_slot.get();
        if known.table == self.schemas.table
            && known.name == name
            && let Some(schema) = self.schemas.at(known.slot)
        {
            return Some(schema);
        }
        let slot = *self.schemas.slots.get(e.type_name.as_str())?;
        e.kind_slot.set(KindSlot { table: self.schemas.table, name, slot });
        self.schemas.at(slot)
    }
    /// Metadata index of `e`'s accessor `name`.
    pub(crate) fn entity_accessor(&self, e: &super::Entity, name: &str) -> Option<u8> {
        self.schema_of(e)?.accessors.get(name).and_then(|v| u8::try_from(v[0]).ok())
    }
    /// `e`'s resolved per-frame accessor indices (all `None` for an unknown kind).
    pub(crate) fn ids(&self, e: &super::Entity) -> &Ids {
        self.schema_of(e).map_or(&Ids::NONE, |schema| schema.ids.get_or_init(|| Ids::of(schema)))
    }
    pub fn default_skin(&self, uuid: Option<&str>) -> Result<&DefaultSkin> {
        let index = if let Some(uuid) = uuid {
            let value = uuid::Uuid::parse_str(uuid)
                .map_err(|_| "invalid profile UUID")?
                .as_u128();
            let h = (value as u64) ^ ((value >> 64) as u64);
            (((h >> 32) as u32 ^ h as u32) as i32).rem_euclid(18) as usize
        } else {
            6
        };
        Ok(&self.default_skins[index])
    }
    pub(crate) fn dimensions<'a>(
        &'a self,
        entity: &super::Entity,
    ) -> Result<(&'a Dimensions, f32)> {
        self.sized(entity).map(|(dimensions, scale, _)| (dimensions, scale))
    }
    /// `dimensions` and the kind's box rule (`physical::bounds_at`). Inside a `SizedScope`
    /// (one frame's or tick's presentation, where metadata and attributes do not change) an
    /// entity's repeated calls are answered from its memo of that scope.
    pub(crate) fn sized<'a>(
        &'a self,
        entity: &super::Entity,
    ) -> Result<(&'a Dimensions, f32, Shape)> {
        let schema = self.schema_of(entity).ok_or("entity dimensions source not activated")?;
        let sizing = schema.sizing.get_or_init(|| Sizing::of(schema, &entity.type_name, &self.poses));
        let scope = SIZED_SCOPE.with(std::cell::Cell::get);
        let memo = entity.sized_memo.get();
        let (choice, scale) = if scope != 0 && memo.scope == scope {
            (memo.choice, memo.scale)
        } else {
            let (choice, scale) = Self::size_choice(entity, schema, sizing)?;
            if scope != 0 {
                entity.sized_memo.set(SizedMemo { scope, choice, scale });
            }
            (choice, scale)
        };
        Ok((choice.of(schema, sizing), scale, sizing.shape))
    }
    fn size_choice(
        entity: &super::Entity,
        schema: &EntitySchema,
        sizing: &Sizing,
    ) -> Result<(SizedChoice, f32)> {
        let number = |id: Option<u8>| {
            id.and_then(|id| entity.metadata_value(id)).and_then(super::Metadata::int)
        };
        let boolean = |id: Option<u8>| {
            id.and_then(|id| entity.metadata_value(id))
                .and_then(super::Metadata::boolean)
                .unwrap_or(false)
        };
        let pose_id = number(sizing.pose).unwrap_or(0) as u8;
        let pose = sizing
            .poses
            .get(usize::from(pose_id))
            .and_then(Option::as_ref)
            .ok_or("unknown entity pose")?;
        let living = sizing.living;
        let mut attribute_scale = if living {
            let (id, default) = sizing.scale.ok_or("source scale attribute missing")?;
            entity.attributes.get(&id).copied().unwrap_or(default) as f32
        } else {
            1.
        };
        if !attribute_scale.is_finite() || attribute_scale <= 0. {
            return Err("invalid entity scale".into());
        }
        attribute_scale = match sizing.sanitize {
            Sanitize::Keep => attribute_scale,
            Sanitize::AtMost(most) => attribute_scale.min(most),
            Sanitize::One => 1.,
        };
        let present = |dimensions: &Option<Dimensions>, missing: &str| {
            dimensions.as_ref().map(|_| ()).ok_or_else(|| missing.to_string())
        };
        if living && pose.sleeping {
            present(&sizing.sleeping, "source sleeping dimensions missing")?;
            return Ok((SizedChoice::Sleeping, 1.));
        }
        if sizing.armor_stand {
            let flags = number(sizing.client_flags).unwrap_or(0);
            let choice = if flags & 16 != 0 {
                present(&sizing.marker, "source marker dimensions missing")?;
                SizedChoice::Marker
            } else if flags & 1 != 0 {
                present(&sizing.baby_dimensions, "source small armor stand dimensions missing")?;
                SizedChoice::Baby
            } else {
                SizedChoice::Schema
            };
            return Ok((choice, attribute_scale));
        }
        if sizing.avatar {
            present(&pose.avatar, "source avatar dimensions missing")?;
            return Ok((SizedChoice::Avatar(pose_id), attribute_scale));
        }
        let baby = boolean(sizing.baby);
        let age_scale = if baby { sizing.baby_scale } else { 1. };
        if sizing.camel && pose.sitting {
            present(&sizing.sitting, "source camel sitting dimensions missing")?;
            return Ok((SizedChoice::Sitting, age_scale * attribute_scale));
        }
        let special = if sizing.goat && pose.long_jumping {
            present(&sizing.long_jumping, "source goat jump dimensions missing")?;
            Some(SizedChoice::LongJumping)
        } else if !sizing.digging_states.is_empty()
            && sizing.digging_states.contains(&number(sizing.state).unwrap_or(0))
        {
            present(&sizing.digging, "source sniffer digging dimensions missing")?;
            Some(SizedChoice::Digging)
        } else {
            None
        };
        if let Some(choice) = special {
            let fixed = choice.of(schema, sizing).fixed;
            return Ok((choice, if fixed { 1. } else { age_scale * attribute_scale }));
        }
        if sizing.baby_dimensions.is_some() {
            return Ok((if baby { SizedChoice::Baby } else { SizedChoice::Schema }, attribute_scale));
        }
        let owner = &sizing.owner;
        if *owner == SizeOwner::ChestedHorse && !baby {
            return Ok((SizedChoice::Schema, attribute_scale));
        }
        // AbstractCubeMob.getDefaultDimensions scales the type dimensions by size alone,
        // replacing LivingEntity's age scale: a baby sulfur cube is as large as its size.
        let age_scale = if *owner == SizeOwner::CubeMob { 1. } else { age_scale };
        let family_scale = match owner {
            SizeOwner::Pufferfish => match number(sizing.puff).unwrap_or(0) {
                0 => 0.5,
                1 => 0.7,
                _ => 1.,
            },
            SizeOwner::Salmon => *schema
                .enum_numbers
                .get("Variant")
                .and_then(|variants| variants.get(&number(sizing.variant).unwrap_or(1)))
                .and_then(|fields| fields.get("boundingBoxScale"))
                .ok_or("source salmon numeric variant missing")?,
            SizeOwner::CubeMob => number(sizing.size).unwrap_or(1) as f32,
            SizeOwner::Phantom => 1. + 0.15 * number(sizing.size).unwrap_or(0) as f32,
            SizeOwner::Other | SizeOwner::ChestedHorse => 1.,
        };
        Ok((
            SizedChoice::Schema,
            if schema.dimensions.fixed {
                1.
            } else {
                age_scale * attribute_scale * family_scale
            },
        ))
    }
}

thread_local! {
    /// Nonzero while a `SizedScope` is alive on this thread: its id.
    static SIZED_SCOPE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
static NEXT_SIZED_SCOPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// One frame's or tick's presentation, during which no metadata or attribute changes: while it
/// is alive `Models::sized` memoizes each entity's answer (`Entity::sized_memo`).
pub(crate) struct SizedScope(u64);
impl SizedScope {
    pub(crate) fn enter() -> Self {
        let id = NEXT_SIZED_SCOPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(SIZED_SCOPE.with(|scope| scope.replace(id)))
    }
}
impl Drop for SizedScope {
    fn drop(&mut self) {
        SIZED_SCOPE.with(|scope| scope.set(self.0));
    }
}

/// An entity's `Models::sized` answer within one `SizedScope` (`scope` 0: none).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SizedMemo {
    scope: u64,
    choice: SizedChoice,
    scale: f32,
}

/// Which of its kind's dimensions `Models::sized` chose (each present when chosen).
#[derive(Clone, Copy, Debug, Default)]
enum SizedChoice {
    #[default]
    Schema,
    Sleeping,
    Marker,
    Baby,
    Avatar(u8),
    Sitting,
    LongJumping,
    Digging,
}
impl SizedChoice {
    fn of<'a>(self, schema: &'a EntitySchema, sizing: &'a Sizing) -> &'a Dimensions {
        let present = |dimensions: &'a Option<Dimensions>| dimensions.as_ref().expect("chosen dimensions are present");
        match self {
            Self::Schema => &schema.dimensions,
            Self::Sleeping => present(&sizing.sleeping),
            Self::Marker => present(&sizing.marker),
            Self::Baby => present(&sizing.baby_dimensions),
            Self::Avatar(pose) => present(
                &sizing.poses[usize::from(pose)].as_ref().expect("chosen pose is known").avatar,
            ),
            Self::Sitting => present(&sizing.sitting),
            Self::LongJumping => present(&sizing.long_jumping),
            Self::Digging => present(&sizing.digging),
        }
    }
}
pub(super) fn part_lookup(bones: &[Bone]) -> BTreeMap<String, usize> {
    if bones.is_empty() {
        return BTreeMap::new();
    }
    let mut links = vec![(usize::MAX, usize::MAX); bones.len()];
    for child in (1..bones.len()).rev() {
        let parent = bones[child]
            .parent
            .expect("flattened non-root bone has parent");
        links[child].1 = links[parent].0;
        links[parent].0 = child;
    }
    fn insert_children(
        parent: usize,
        bones: &[Bone],
        links: &[(usize, usize)],
        names: &mut BTreeMap<String, usize>,
    ) {
        let mut child = links[parent].0;
        while child != usize::MAX {
            names.entry(bones[child].name.clone()).or_insert(child);
            child = links[child].1;
        }
        child = links[parent].0;
        while child != usize::MAX {
            insert_children(child, bones, links, names);
            child = links[child].1;
        }
    }
    let mut names = BTreeMap::new();
    names.insert(bones[0].name.clone(), 0);
    insert_children(0, bones, &links, &mut names);
    names
}
pub fn transform(pivot: Vec3, rotation: Vec3, scale: Vec3) -> Mat4 {
    Mat4::from_translation(pivot)
        * Mat4::from_rotation_z(rotation.z)
        * Mat4::from_rotation_y(rotation.y)
        * Mat4::from_rotation_x(rotation.x)
        * Mat4::from_scale(scale)
}

/// Test content naming every embedded entity kind at its protocol id, and models loaded for it.
#[cfg(test)]
pub(super) fn every_kind() -> (Content, Models) {
    let doc = Models::parse().unwrap();
    let mut content = Content::test_cube();
    content.entity_names = doc.entities.iter()
        .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
    let models = Models::load(&content).unwrap();
    (content, models)
}
/// Test-side selection inventory, not a second runtime catalog. Rows retain both the demanded
/// key and the key actually selected by `material`, so a variant silently using its base rig
/// cannot earn coverage merely because the base family exists.
#[cfg(test)]
pub(super) fn rig_selection_contract() -> Vec<Value> {
    use super::{Metadata, Store, presentation};
    use crate::protocol::{Reader, Writer, content::TextureRegion, packet_ids::play::clientbound};
    use serde_json::json;
    let doc = Models::parse().unwrap();
    let mut content = Content::test_cube();
    content.entity_names = doc.entities.iter()
        .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
    let models = Models::load(&content).unwrap();
    let mut textures = std::collections::BTreeSet::new();
    fn collect(value: &Value, textures: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::String(s) if s.contains("entity/") => {
                let (namespace, path) = s.split_once(':').unwrap_or(("minecraft", s));
                textures.insert(format!("{namespace}:{}",
                    path.trim_start_matches("textures/").trim_end_matches(".png")));
            }
            Value::Object(fields) => fields.values().for_each(|v| collect(v, textures)),
            Value::Array(values) => values.iter().for_each(|v| collect(v, textures)),
            _ => {}
        }
    }
    collect(&serde_json::from_str::<Value>(ENTITY_FACTS).unwrap(), &mut textures);
    // VillagerProfessionLayer computes these constructor-default paths rather than storing
    // them in the facts. Keep fixture resource absence separate from selection coverage.
    for family in ["villager", "zombie_villager"] {
        for layer in ["type", "baby"] {
            textures.insert(format!("minecraft:entity/{family}/{layer}/plains"));
        }
    }
    let textures: Vec<_> = textures.into_iter().collect();
    for (index, name) in textures.iter().enumerate() {
        content.insert_test_texture(name.trim_end_matches(".png"), TextureRegion {
            layer: index as u32 + 1, uv: [0., 0., 1., 1.], width: 64, height: 64,
        });
    }
    let mut rows = Vec::new();
    let mut store = Store::default();
    for (registry, entries) in &models.builtin_variants {
        store.registries.insert(registry.clone(), entries.iter()
            .map(|(name, value)| (name.clone(), value.clone())).collect());
    }
    let mut kinds: Vec<_> = models.schemas.iter().collect();
    kinds.sort_unstable_by_key(|(kind, _)| *kind);
    for (kind, schema) in kinds {
        let entity = || {
            let mut fixture = Store::default();
            let mut spawn = Writer::default();
            spawn.var(1);
            spawn.0.extend_from_slice(&[0; 16]);
            spawn.var(schema.protocol_id as i32);
            spawn.0.extend_from_slice(&[0; 29]);
            fixture.apply(clientbound::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
            fixture.slots.pop().unwrap()
        };
        let Some(family) = models.families.get(kind) else {
            rows.push(json!({"id":format!("entity/{kind}"),"class":"non-family",
                "entity":kind,"required_rig":null,"selected_rig":null,
                "selection_error":"No facts family; special/resource-driven or unsupported route must be verified separately"}));
            continue;
        };
        for (age, key) in std::iter::once(("adult", &family.rig))
            .chain(family.baby_rig.as_ref().map(|key| ("baby", key)))
        {
            rows.push(json!({"id":format!("family/{kind}/{age}"),"class":"family",
                "entity":kind,"age":age,"required_rig":key,"selected_rig":null,
                "texture":family.texture,"materials":family.materials,
                "scope":"declared family demand, including keys not reached by current selectors"}));
        }
        for baby in [false, true] {
            // Inherited age metadata is not sufficient: the local 26.3
            // ZombieNautilus.canBeABaby contract explicitly returns false.
            if baby && (!schema.accessors.contains_key("DATA_BABY_ID")
                || kind == "minecraft:zombie_nautilus") { continue; }
            let base = if baby { family.baby_rig.as_ref().unwrap_or(&family.rig) } else { &family.rig };
            let age = if baby { "baby" } else { "adult" };
            let default_entity = entity();
            let default_key = if kind == "minecraft:player"
                && models.default_skin(Some(&default_entity.uuid)).unwrap().model == super::PlayerModel::Slim
            {
                "minecraft:player_slim#main".to_owned()
            } else if kind == "minecraft:pufferfish" {
                "minecraft:pufferfish_small#main".to_owned()
            } else { base.clone() };
            let mut cases = vec![("default".to_owned(), default_entity, default_key, None)];
            if kind == "minecraft:wither_skull" {
                let mut dangerous = entity();
                dangerous.metadata.insert(models.accessor(kind, "DATA_DANGEROUS").unwrap(), Metadata::Bool(true));
                cases.push(("DATA_DANGEROUS/true".into(), dangerous, base.clone(),
                    family.materials["WITHER_INVULNERABLE_LOCATION"].as_str().map(str::to_owned)));
                cases[0].3 = family.materials["WITHER_LOCATION"].as_str().map(str::to_owned);
            }
            if let Some(registry) = presentation::variant_registry(kind)
                && let Some(entries) = store.registries.get(registry)
            {
                for (index, (name, data)) in entries.iter().enumerate() {
                    let mut e = entity();
                    let accessor = models.accessor(kind, "DATA_VARIANT_ID")
                        .or_else(|| models.accessor(kind, "DATA_VARIANT")).unwrap();
                    e.metadata.insert(accessor, Metadata::Int(index as i32));
                    let expected = match data["model"].as_str() {
                        Some(model) if !matches!(model, "normal" | "temperate") =>
                            format!("minecraft:{model}_{}{}#main",
                                kind.trim_start_matches("minecraft:"), if baby { "_baby" } else { "" }),
                        _ => base.clone(),
                    };
                    let expected_texture = if data["assets"].is_object() {
                        if baby && data["baby_assets"].is_object() {
                            data["baby_assets"]["wild"].as_str()
                        } else {
                            data["assets"]["wild"].as_str()
                        }
                    } else if baby {
                        data["baby_asset_id"].as_str().or(data["asset_id"].as_str())
                    } else {
                        data["asset_id"].as_str()
                    }.map(str::to_owned);
                    cases.push((format!("received/{name}"), e, expected, expected_texture));
                }
            }
            // Only these numeric selectors change the rig in `material`.
            let shape = match kind.as_str() {
                "minecraft:pufferfish" => Some(("PUFF_STATE", vec![
                    (0, "minecraft:pufferfish_small#main"),
                    (1, "minecraft:pufferfish_medium#main"), (2, "minecraft:pufferfish#main")])),
                "minecraft:tropical_fish" => Some(("DATA_ID_TYPE_VARIANT", vec![
                    (0, base.as_str()), (1, "minecraft:tropical_fish_large#main")])),
                _ => None,
            };
            if let Some((accessor, values)) = shape {
                for (value, expected) in values {
                    let mut e = entity();
                    e.metadata.insert(models.accessor(kind, accessor).unwrap(), Metadata::Int(value));
                    cases.push((format!("{accessor}/{value}"), e, expected.to_owned(), None));
                }
            }
            for (case, mut e, expected, expected_texture) in cases {
                if let Some(accessor) = models.accessor(kind, "DATA_BABY_ID") {
                    e.metadata.insert(accessor, Metadata::Bool(baby));
                }
                let selected = presentation::material(&store, &models, &e, &content, 0);
                let (key, selected_texture, error) = match selected {
                    Ok(binding) => (Some(binding.rig),
                        textures.get(binding.texture.layer.saturating_sub(1) as usize).cloned(), None),
                    Err(error) => (None, None, Some(error)),
                };
                rows.push(json!({"id":format!("selection/{kind}/{age}/{case}"),
                    "class":"selection","entity":kind,"age":age,"case":case,
                    "required_rig":expected,"selected_rig":key,"selection_error":error,
                    "required_texture":expected_texture,"selected_texture":selected_texture,
                    "vanilla_reachability":if baby && family.baby_rig.is_none() {
                        "unverified_inherited_baby_accessor"
                    } else { "declared_family" }}));
            }
            // Poses share a rig, but each remains a distinct visual/animation obligation.
            for (id, pose) in &models.poses {
                rows.push(json!({"id":format!("pose/{kind}/{age}/{pose}"),"class":"pose",
                    "entity":kind,"age":age,"pose":pose,"metadata_value":id,
                    "required_rig":base,"selected_rig":null,
                    "selection_error":null,"scope":"rig demand only; animation/pixels unverified",
                    "vanilla_reachability":if baby && family.baby_rig.is_none() {
                        "unverified_inherited_baby_accessor"
                    } else { "declared_family" }}));
            }
        }
        for (enum_name, values) in &schema.enum_ids {
            for (id, value) in values {
                rows.push(json!({"id":format!("enum/{kind}/{enum_name}/{id}"),"class":"enum",
                    "entity":kind,"enum":enum_name,"metadata_value":id,"value":value,
                    "required_rig":family.rig,"selected_rig":null,
                    "scope":"facts inventory; received enum presentation unverified"}));
            }
        }
        for texture in &family.textures {
            rows.push(json!({"id":format!("texture/{kind}/{texture}"),"class":"texture",
                "entity":kind,"texture":texture,"required_rig":family.rig,"selected_rig":null,
                "scope":"declared material inventory, not evidence that a state selects it"}));
        }
    }
    for (kind, armor) in &models.armor_models {
        for (age, slots) in [("adult", &armor.adult), ("baby", &armor.baby)] {
            for (slot, key) in slots {
                rows.push(json!({"id":format!("armor/{kind}/{age}/{slot}"),"class":"armor",
                    "entity":kind,"age":age,"slot":slot,"required_rig":key,"selected_rig":null,
                    "scope":"facts armor key; reachability and equipped pixels unverified"}));
            }
        }
    }
    for (index, skin) in models.default_skins.iter().enumerate() {
        rows.push(json!({"id":format!("skin/{index}"),"class":"skin","texture":skin.texture,
            "required_rig":if skin.model == super::PlayerModel::Slim {
                "minecraft:player_slim#main"
            } else { "minecraft:player#main" }, "selected_rig":null,
            "scope":"default skin layout demand; profile publication and pixels unverified"}));
    }
    for (name, data) in &models.builtin_variants["minecraft:painting_variant"] {
        rows.push(json!({"id":format!("painting/{name}"),"class":"painting-variant",
            "required_rig":null,"selected_rig":null,"variant":data,
            "scope":"resource-driven painting segments, not an authored entity rig"}));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Textures of the received-render fixture; each has its own atlas layer.
    const FIXTURE_TEXTURES: [&str; 8] = [
        "test:painting/canvas", "minecraft:painting/back", "minecraft:test/frame",
        "minecraft:entity/minecart/minecart", "minecraft:entity/cushion/white_cushion",
        "minecraft:entity/cushion/lime_cushion", "minecraft:entity/boat/poplar",
        "minecraft:entity/chest_boat/poplar",
    ];
    fn fixture_layer(texture: &str) -> u32 {
        7 + FIXTURE_TEXTURES.iter().position(|name| *name == texture).unwrap() as u32
    }

    fn render_received_fixture(kind: &str, direction: i32, metadata: &[u8]) -> crate::api::DynamicGeometry {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let variant = serde_json::json!({"width":1,"height":1,"asset_id":"test:canvas"});
        content.set_registries(BTreeMap::from([("minecraft:painting_variant".into(),
            vec![("test:canvas".into(), variant.clone())])]));
        let region = crate::protocol::content::TextureRegion {
            layer: 7, uv: [0., 0., 1., 1.], width: 16, height: 16,
        };
        for texture in FIXTURE_TEXTURES {
            content.insert_test_texture(texture, crate::protocol::content::TextureRegion {
                layer: fixture_layer(texture), ..region
            });
        }
        // Owned box fixture, not copied vanilla geometry.
        let faces: serde_json::Map<String, Value> = ["north", "south", "east", "west", "up", "down"]
            .into_iter().map(|face| (face.into(), serde_json::json!({"texture":"#frame"}))).collect();
        let frame = serde_json::to_vec(&serde_json::json!({
            "textures":{"frame":"test/frame"},
            "elements":[{"from":[2,2,14],"to":[14,14,16],"faces":faces}]
        })).unwrap();
        for name in ["item_frame", "glow_item_frame"] {
            content.insert_test_resource(&format!("assets/minecraft/models/block/{name}.json"), &frame);
        }
        content.item_names.insert(1, "test:token".into());
        content.insert_test_item_components(1, BTreeMap::new());
        content.insert_test_resource("assets/test/models/token.json", &frame);
        content.insert_test_resource("assets/test/items/token.json",
            br#"{"model":{"type":"minecraft:model","model":"test:token"}}"#);
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.registries.insert("minecraft:painting_variant".into(),
            vec![("test:canvas".into(), variant)]);
        store.activate_models(models);
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0;16]);
        spawn.var(doc.entities[kind].protocol_id as i32);
        for coordinate in [4_f64, 4., 3.] {
            spawn.0.extend_from_slice(&coordinate.to_be_bytes());
        }
        spawn.0.extend_from_slice(&[0;4]);
        spawn.var(direction);
        store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        if !metadata.is_empty() {
            store.apply(cb::SET_ENTITY_DATA, &mut Reader::new(metadata), &content, 1).unwrap();
        }
        let mut out = crate::api::DynamicGeometry::default();
        let stats = store.append_geometry(crate::entities::EntityRenderFrame { content: &content, origin: [4.,4.,3.], camera: [4.,5.,0.], camera_rotation: [0.;2], radius: 32., tick: 1, partial: 0., game_time: 0, clock_environment: None, maps: crate::ui::MapRenderFixture::empty().view(), tint_source: &|tint, _| match tint { crate::protocol::models::TintKind::None => Ok([1.; 3]), crate::protocol::models::TintKind::Constant(color) => Ok(color), _ => Err("test biome tint not installed".into()) }, light: &|point| {
            // A painting's segment cells are available, but not its generic eye sample.
            Ok((kind != "minecraft:painting" || point.iter().all(|v| v.fract() == 0.))
                .then_some([15.;2]))
        }, bed_direction: &|_| None }, &mut out).unwrap();
        assert_eq!(stats.submitted, 1, "{kind}: {:?}", stats.unrendered);
        assert!(!out.vertices.is_empty(), "{kind}");
        out
    }

    /// Spatial queries pad by the widest received entity at its actual size, not by every kind
    /// of the registry at the largest scale: with only soldiers present a query near the camera
    /// visits no distant entity, a scale attribute that grows an entity widens the padding so
    /// a query its grown body reaches finds it, and a dragon widens it once it arrives.
    #[test]
    fn spatial_padding_follows_present_kinds() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        assert_eq!(store.query_padding(), 4.);
        let spawn = |store: &mut super::super::Store, id: i32, kind: &str, x: f64| {
            let mut spawn = Writer::default();
            spawn.var(id);
            spawn.0.extend_from_slice(&[0; 16]);
            spawn.var(doc.entities[kind].protocol_id as i32);
            for coordinate in [x, 64., 0.] {
                spawn.0.extend_from_slice(&coordinate.to_be_bytes());
            }
            spawn.0.extend_from_slice(&[0; 4]);
            spawn.var(0);
            store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        };
        spawn(&mut store, 1, "minecraft:zombie", 0.);
        spawn(&mut store, 2, "minecraft:zombie", 1000.);
        assert_eq!(store.query_padding(), 4.);
        let near = crate::player::Bounds { min: [-1., 60., -1.], max: [1., 70., 1.] };
        let ids: Vec<_> = store.candidates(near).map(|e| e.handle.network_id).collect();
        assert_eq!(ids, [1]);
        // A horse in the next spatial cell, scaled 16 times: its body reaches `beside`.
        spawn(&mut store, 3, "minecraft:horse", 16.5);
        let beside = crate::player::Bounds { min: [8., 60., -1.], max: [10., 70., 1.] };
        let found = |store: &super::super::Store| store.candidates(beside).any(|e| e.handle.network_id == 3);
        assert!(!found(&store));
        let scale = *models.attribute_ids.iter().find(|(_, name)| name.as_str() == "minecraft:scale").unwrap().0;
        let mut attributes = Writer::default();
        attributes.var(3);
        attributes.var(1);
        attributes.var(scale as i32);
        attributes.0.extend_from_slice(&16f64.to_be_bytes());
        attributes.var(0);
        store.apply(cb::UPDATE_ATTRIBUTES, &mut Reader::new(&attributes.0), &content, 1).unwrap();
        let horse = store.get(3).unwrap().handle;
        assert!(store.bounds(horse, 1, 0.).unwrap().intersects(beside), "the grown horse reaches it");
        assert!(found(&store), "padding {}", store.query_padding());
        store.activate_models(std::sync::Arc::clone(&models));
        assert!(found(&store), "activation keeps the grown horse's padding");
        spawn(&mut store, 4, "minecraft:ender_dragon", 2000.);
        let dragon = f64::from(doc.entities["minecraft:ender_dragon"].width) * 16.;
        assert!(dragon > 32. && store.query_padding() == dragon);
        // Models activated again keep the padding of the entities present.
        store.activate_models(models);
        assert_eq!(store.query_padding(), dragon);
    }

    /// A received husk and horse at render distance 8 and Entity Distance 100% are drawn just
    /// inside vanilla's `AABB.getSize * 64` blocks (67.2 and 93.7) and not just outside; 200%
    /// doubles it (`view_range`).
    #[test]
    fn husk_and_horse_draw_within_their_vanilla_distance() {
        use super::super::view_range::{DEFAULT_SCALING, view_scale, within};
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        assert_eq!(view_scale(8, DEFAULT_SCALING), 1.);
        assert_eq!(view_scale(20, DEFAULT_SCALING), 2.5);
        assert_eq!(view_scale(4, 2.), 2.);
        for (id, kind, reach) in [(1, "minecraft:husk", 67.2), (2, "minecraft:horse", 93.7167)] {
            let mut spawn = Writer::default();
            spawn.var(id);
            spawn.0.extend_from_slice(&[0; 16]);
            spawn.var(doc.entities[kind].protocol_id as i32);
            for coordinate in [0f64, 64., 0.] {
                spawn.0.extend_from_slice(&coordinate.to_be_bytes());
            }
            spawn.0.extend_from_slice(&[0; 4]);
            spawn.var(0);
            store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
            let e = store.get(id).unwrap();
            let position = e.transform.position;
            let at = |distance: f64, scale| within(&models, e, position, distance * distance, scale);
            for (scaling, factor) in [(DEFAULT_SCALING, 1.), (2., 2.)] {
                let scale = view_scale(8, scaling);
                assert!(at(reach * factor - 0.01, scale), "{kind} at {scaling} inside");
                assert!(!at(reach * factor + 0.01, scale), "{kind} at {scaling} outside");
            }
        }
    }

    #[test]
    fn entity_render_rc1_empty_item_frame_has_body_at_packet_anchor() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[]);
        assert!(out.vertices.iter().all(|v| (0. ..=1.).contains(&v.position[2])));
        assert!(out.vertices.iter().any(|v| v.position[2] >= 0.9));
    }

    #[test]
    fn entity_render_rc1_empty_glow_item_frame_has_body() {
        render_received_fixture("minecraft:glow_item_frame", 5, &[]);
    }

    #[test]
    fn entity_render_rc1_painting_emits_mesh_from_received_spawn() {
        let out = render_received_fixture("minecraft:painting", 2, &[]);
        assert!(out.vertices.iter().all(|v| (0.9374..=1.0001).contains(&v.position[2])));
    }

    #[test]
    fn entity_render_rc1_minecart_mesh_is_above_rail_not_buried() {
        let out = render_received_fixture("minecraft:minecart", 0, &[]);
        let bottom = out.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
        let top = out.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
        assert!(bottom >= -0.001, "minecart bottom below received rail position: {bottom}");
        assert!(top > 0.5, "minecart top buried: {top}");
    }


    #[test]
    fn entity_render_263_poplar_boats_use_boat_hulls_and_poplar_textures() {
        let boat = render_received_fixture("minecraft:poplar_boat", 0, &[]);
        let chest_boat = render_received_fixture("minecraft:poplar_chest_boat", 0, &[]);
        let count = |out: &crate::api::DynamicGeometry, texture: &str| {
            out.vertices.iter().filter(|v| v.texture_layer == fixture_layer(texture)).count()
        };
        let hull = count(&boat, "minecraft:entity/boat/poplar");
        assert!(hull > 0);
        assert_eq!(count(&boat, "minecraft:entity/chest_boat/poplar"), 0);
        // The chest boat model is the boat hull plus its chest parts, on its own texture.
        assert!(count(&chest_boat, "minecraft:entity/chest_boat/poplar") > hull);
        assert_eq!(count(&chest_boat, "minecraft:entity/boat/poplar"), 0);
    }

    #[test]
    fn entity_render_rc1_frame_metadata_direction_overrides_spawn() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[1,8,12,5,255]);
        assert!(out.vertices.iter().all(|v| v.position[0] >= -0.0001 && v.position[0] <= 0.1251));
    }

    #[test]
    fn entity_render_rc1_frame_emits_received_held_item_too() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[1,9,7,1,1,0,0,10,1,2,255]);
        assert_eq!(out.vertices.len(), 48, "owned frame box plus owned held-item box");
    }

    #[test]
    fn wire_263_raid_omen_particle_list_metadata_packet() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.particle_names.insert(123, "minecraft:raid_omen".into());
        let models = std::sync::Arc::new(Models::load_state(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(models);
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]);
        spawn.var(120); // 26.3 slime
        spawn.0.extend_from_slice(&[0; 29]);
        assert!(store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap());
        // Entity 1, particle-list accessor 10, serializer 17, one particle 123,
        // followed by a byte metadata value to verify the particle consumes no options.
        let packet = [1, 10, 17, 1, 123, 8, 0, 42, 255];
        let mut reader = Reader::new(&packet);
        assert!(store.apply(cb::SET_ENTITY_DATA, &mut reader, &content, 2).unwrap());
        reader.done().unwrap();
        match &store.get(1).unwrap().metadata[&10] {
            super::super::metadata::Metadata::Particles(particles) => {
                assert_eq!(particles, &[serde_json::json!({
                    "type_id": 123, "type": "minecraft:raid_omen", "data": null
                })]);
            }
            other => panic!("unexpected metadata: {other:?}"),
        }
        assert_eq!(store.get(1).unwrap().metadata[&8].int(), Some(42));
    }

    #[test]
    fn wire_263_all_reported_simple_particles_decode_without_options() {
        use crate::protocol::{Reader, Writer};
        // The embedded facts are the sorted 26.3 SimpleParticleType registry report.
        let doc = Models::parse().unwrap();
        assert_eq!(doc.simple_particles.len(), 106);
        assert!(doc.simple_particles.windows(2).all(|pair| pair[0] < pair[1]));
        for name in [
            "minecraft:raid_omen", "minecraft:sulfur_bubbles", "minecraft:noxious_gas",
            "minecraft:noxious_gas_cloud", "minecraft:red_poplar_leaves",
            "minecraft:orange_poplar_leaves", "minecraft:yellow_poplar_leaves",
            "minecraft:sulfur_cube_goo",
        ] {
            assert!(doc.simple_particles.iter().any(|particle| particle == name), "{name}");
        }
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = Models::load_state(&content).unwrap();
        for (id, name) in doc.simple_particles.iter().enumerate() {
            content.particle_names.insert(id as u32, name.clone());
            let mut packet = Writer::default();
            packet.0.extend_from_slice(&[8, 17, 1]);
            packet.var(id as i32);
            packet.0.extend_from_slice(&[9, 0, 42, 255]);
            let mut reader = Reader::new(&packet.0);
            let values = super::super::metadata::decode(&mut reader, &content, &models)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            reader.done().unwrap();
            match &values[&8] {
                super::super::metadata::Metadata::Particles(particles) => {
                    assert_eq!(particles, &[serde_json::json!({
                        "type_id": id, "type": name, "data": null
                    })], "{name}");
                }
                other => panic!("{name}: unexpected metadata: {other:?}"),
            }
            assert_eq!(values[&9].int(), Some(42), "{name}");
        }
    }

    #[test]
    fn embedded_entity_facts_parse_and_carry_no_geometry() {
        let doc = Models::parse().unwrap();
        assert_eq!(doc.entities.len(), 161);
        assert_eq!(doc.motion.len(), doc.entities.len());
        assert_eq!(doc.families.len(), 131);
        assert!(!doc.families.contains_key("minecraft:cushion"));
        assert!(!doc.families.contains_key("minecraft:sulfur_cube"));
        assert_eq!(doc.default_skins.len(), 18);
        let zombie = &doc.entities["minecraft:zombie"];
        assert_eq!(zombie.protocol_id, 154);
        assert!(zombie.dimension_constants.contains_key("BABY_DIMENSIONS"));
        assert!(
            ENTITY_FACTS.len() < 1 << 20,
            "entity facts must stay a data table"
        );
        for geometry in ["\"roots\"", "\"quads\"", "\"polygons\"", "\"animations\""] {
            assert!(
                !ENTITY_FACTS.contains(geometry),
                "{geometry} in entity facts"
            );
        }
    }

    #[test]
    fn wire_263_slime_metadata_uses_regenerated_accessor_schema() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        assert_eq!(doc.game_version, "26.3");
        assert_eq!(doc.entities["minecraft:slime"].accessors["ID_SIZE"], [18, 1]);
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.item_names.insert(0, "test:body".into());
        content.insert_test_item_components(0, BTreeMap::new());
        let models = std::sync::Arc::new(Models::load_state(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]); // UUID
        spawn.var(120); // reports/registries.json: minecraft:slime
        spawn.0.extend_from_slice(&[0; 29]); // xyz, zero LP Vec3, rotations, spawn data
        assert!(store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap());
        // entity=1, accessor=18, INT serializer=1, size=4, end=255.
        assert!(store.apply(cb::SET_ENTITY_DATA, &mut Reader::new(&[1, 18, 1, 4, 255]), &content, 2).unwrap());
        assert_eq!(store.get(1).unwrap().metadata[&18].int(), Some(4));
        assert_eq!(models.dimensions(store.get(1).unwrap()).unwrap().1, 4.);
        // 26.3 SulfurCube's vertical walk animation is hasBodyItem(), not the
        // removed FlyingAnimal interface or a fabricated constant.
        let sulfur = &models.motion["minecraft:sulfur_cube"];
        assert!(!sulfur.walk_uses_y(store.get(1).unwrap()));
        let body = crate::inventory::read_stack(&mut Reader::new(&[1, 0, 0, 0]), &content).unwrap();
        store.get_mut(1).unwrap().equipment[6] = Some(body);
        assert!(sulfur.walk_uses_y(store.get(1).unwrap()));
        store.get_mut(1).unwrap().equipment[6] = None;
        assert!(!sulfur.walk_uses_y(store.get(1).unwrap()));
    }

    #[test]
    fn regression_263_baby_cube_mob_dimensions_scale_by_size_not_age() {
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = Models::load_state(&content).unwrap();
        let (_, mut cube) = super::super::animation_tests::fixture();
        cube.type_name = "minecraft:sulfur_cube".into();
        assert_eq!(models.accessor(&cube.type_name, "DATA_BABY_ID"), Some(16));
        assert_eq!(models.accessor(&cube.type_name, "ID_SIZE"), Some(18));
        for baby in [false, true] {
            cube.metadata.insert(16, super::super::metadata::Metadata::Bool(baby));
            cube.metadata.insert(18, super::super::metadata::Metadata::Int(2));
            let (dimensions, scale) = models.dimensions(&cube).unwrap();
            assert_eq!((dimensions.width, dimensions.height), (0.49, 0.49));
            assert_eq!(scale, 2., "baby={baby}");
        }
    }

    #[test]
    fn sizing_constants_and_box_rules_resolve_per_kind_and_follow_schema_changes() {
        use super::super::metadata::Metadata;
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let mut models = Models::load_state(&content).unwrap();
        let entity = |kind: &str| {
            let (_, mut e) = super::super::animation_tests::fixture();
            e.type_name = kind.into();
            e.metadata.clear();
            e
        };
        let size = |models: &Models, e: &super::super::Entity| {
            let (d, scale) = models.dimensions(e).unwrap();
            (d.width, d.height, scale)
        };
        let mut husk = entity("minecraft:husk");
        husk.metadata.insert(16, Metadata::Bool(true));
        assert_eq!(size(&models, &husk), (0.49, 0.98, 1.), "a baby husk takes BABY_DIMENSIONS");
        husk.metadata.insert(16, Metadata::Bool(false));
        assert_eq!(size(&models, &husk), (0.6, 1.95, 1.));
        husk.metadata.insert(6, Metadata::Pose(2));
        assert_eq!(size(&models, &husk), (0.2, 0.2, 1.), "a sleeping husk takes SLEEPING_DIMENSIONS");
        // A changed schema forgets what was resolved from it: the next call reads the new
        // constant, not the one resolved on the first call.
        let DimensionConstant::Dimensions(sleeping) = models.schemas.get_mut("minecraft:husk").unwrap()
            .dimension_constants.get_mut("SLEEPING_DIMENSIONS").unwrap() else { panic!("dimensions") };
        sleeping.width = 0.3;
        assert_eq!(size(&models, &husk), (0.3, 0.2, 1.));
        husk.metadata.insert(6, Metadata::Pose(0));
        let mut stand = entity("minecraft:armor_stand");
        stand.metadata.insert(15, Metadata::Byte(1));
        assert_eq!(size(&models, &stand), (0.25, 0.9875, 1.), "a small armor stand");
        stand.metadata.insert(15, Metadata::Byte(16));
        assert_eq!(size(&models, &stand), (0., 0., 1.), "a marker armor stand");
        let mut player = entity("minecraft:player");
        player.metadata.insert(6, Metadata::Pose(5));
        assert_eq!(size(&models, &player), (0.6, 1.5, 1.), "a crouching avatar takes its POSES entry");
        player.metadata.insert(6, Metadata::Pose(8));
        assert_eq!(size(&models, &player), (0.6, 1.8, 1.), "an avatar pose without an entry stands");
        // Box rules: a wind charge's box is 0.15 below its position, a husk's stands on it.
        let charge = entity("minecraft:wind_charge");
        let b = super::super::physical::bounds_at(&models, &charge, [0., 10., 0.]).unwrap();
        assert_eq!(b.min[1], 10. - f64::from(0.15f32));
        let b = super::super::physical::bounds_at(&models, &husk, [0., 10., 0.]).unwrap();
        assert_eq!((b.min[1], b.max[1]), (10., 10. + f64::from(1.95f32)));
    }

    /// Each mount kind's `getPassengerAttachmentPoint` owner selects its seat rule once.
    #[test]
    fn mount_seat_rules_resolve_per_kind() {
        let (_, models) = super::every_kind();
        let seat = |kind: &str| {
            let (_, mut e) = super::super::animation_tests::fixture();
            e.type_name = kind.into();
            models.ids(&e).seat
        };
        assert_eq!(seat("minecraft:oak_boat"), Seat::Boat { chest: false, raft: false });
        assert_eq!(seat("minecraft:oak_chest_boat"), Seat::Boat { chest: true, raft: false });
        assert_eq!(seat("minecraft:bamboo_raft"), Seat::Boat { chest: false, raft: true });
        assert_eq!(seat("minecraft:bamboo_chest_raft"), Seat::Boat { chest: true, raft: true });
        assert_eq!(seat("minecraft:horse"), Seat::Horse);
        assert_eq!(seat("minecraft:skeleton_horse"), Seat::Horse);
        assert_eq!(seat("minecraft:camel"), Seat::Camel);
        assert_eq!(seat("minecraft:strider"), Seat::Strider);
        assert_eq!(seat("minecraft:slime"), Seat::CubeMob);
        for kind in ["minecraft:minecart", "minecraft:llama", "minecraft:pig"] {
            assert_eq!(seat(kind), Seat::Attachment, "{kind}");
        }
        assert!(seat("minecraft:horse").moves() && seat("minecraft:strider").moves());
        assert!(!seat("minecraft:camel").moves() && !seat("minecraft:oak_boat").moves());
    }

    #[test]
    fn regression_263_cushion_and_painting_metadata() {
        use crate::protocol::{Reader, Writer};
        let doc = Models::parse().unwrap();
        assert_eq!(doc.entities["minecraft:cushion"].accessors["DATA_COLOR"], [8, 43]);
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.set_registries(BTreeMap::from([("minecraft:painting_variant".into(),
            (0..3).map(|id| (format!("test:painting_{id}"), serde_json::json!({}))).collect())]));
        let models = Models::load_state(&content).unwrap();
        let mut reader = Reader::new(&[8, 43, 14, 255]);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&8].serializer_id(), 43);
        assert_eq!(values[&8].int(), Some(14));
        let mut reader = Reader::new(&[9, 34, 1, 255]);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].serializer_id(), 34);
        assert_eq!(values[&9].int(), Some(0), "holder reference 1 names registry entry 0");
        let last = content.registry_entries("minecraft:painting_variant").unwrap().len() as i32;
        let mut packet = Writer::default();
        packet.0.extend_from_slice(&[9, 34]);
        packet.var(last);
        packet.0.push(255);
        let mut reader = Reader::new(&packet.0);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].int(), Some(last - 1));
        let mut packet = Writer::default();
        packet.0.extend_from_slice(&[9, 34, 0, 2, 3]); // inline holder, width, height
        packet.string("test:painting");
        packet.0.extend_from_slice(&[1, 8, 0, 1, b'T', 1, 8, 0, 1, b'A', 10, 0, 5, 255]);
        let mut reader = Reader::new(&packet.0);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].serializer_id(), 34);
        assert_eq!(values[&9].int(), None);
        assert_eq!(values[&10].int(), Some(5), "inline holder consumes exactly its fields");
        let value = serde_json::to_value(&values[&9]).unwrap();
        assert_eq!(value["value"]["width"], 2);
        assert_eq!(value["value"]["height"], 3);
        assert_eq!(value["value"]["asset_id"], "test:painting");
        assert_eq!(value["value"]["title"], "T");
        assert_eq!(value["value"]["author"], "A");
    }

    #[test]
    fn entity_schema_rejects_same_names_at_stale_protocol_ids() {
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id + 1, name.clone())).collect();
        assert!(Models::check_registry(&doc, &content).is_err());
    }

    #[test]
    fn armor_models_resolve_per_slot_for_adult_and_baby() {
        let doc = Models::parse().unwrap();
        let zombie = &doc.armor_models["minecraft:zombie"];
        for part in ["head", "chest", "legs", "feet"] {
            assert!(zombie.adult.contains_key(part), "adult {part}");
            assert!(zombie.baby.contains_key(part), "baby {part}");
        }
    }

    #[test]
    fn dimension_constants_preserve_decimal_dimensions_and_attachments() {
        let dimensions = r#"{"width":0.2,"height":0.2,"eye_height":0.2,"fixed":true,"attachments":{"NAME_TAG":[[0.0,0.20000000298023224,0.0]]}}"#;
        let value: DimensionConstant = serde_json::from_str(dimensions).unwrap();
        let DimensionConstant::Dimensions(value) = value else {
            panic!("dimension variant")
        };
        assert_eq!(value.width, 0.2);
        assert_eq!(
            value.attachments["NAME_TAG"][0],
            [0., 0.20000000298023224, 0.]
        );
        let poses: DimensionConstant =
            serde_json::from_str(&format!(r#"{{"SLEEPING":{dimensions}}}"#)).unwrap();
        let DimensionConstant::Poses(poses) = poses else {
            panic!("pose variant")
        };
        assert_eq!(poses["SLEEPING"].eye_height, 0.2);
        assert!(poses["SLEEPING"].fixed);
    }

    /// `sized` answers each `DATA_POSE` id by the name `Models::poses` gives it: an avatar's
    /// `POSES` entry (its standing dimensions for a pose it lacks), a sleeping living body's
    /// sleeping box, a sitting camel's sitting rule.
    #[test]
    fn sized_follows_each_pose_name() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        for (id, kind) in [(1, "minecraft:mannequin"), (2, "minecraft:camel")] {
            let mut spawn = Writer::default();
            spawn.var(id);
            spawn.0.extend_from_slice(&[0; 16]);
            spawn.var(doc.entities[kind].protocol_id as i32);
            for coordinate in [0f64, 64., 0.] {
                spawn.0.extend_from_slice(&coordinate.to_be_bytes());
            }
            spawn.0.extend_from_slice(&[0; 4]);
            spawn.var(0);
            store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        }
        let size = |d: &Dimensions| (d.width, d.height, d.eye_height);
        let constant = |kind: &str, name: &str| match doc.entities[kind].dimension_constants.get(name) {
            Some(DimensionConstant::Dimensions(d)) => Some(size(d)),
            _ => None,
        };
        let Some(DimensionConstant::Poses(table)) =
            doc.entities["minecraft:mannequin"].dimension_constants.get("POSES")
        else {
            panic!("mannequin POSES table");
        };
        let sized_in = |store: &mut super::super::Store, id: i32, pose: u8| {
            let index = models.entity_accessor(store.get(id).unwrap(), "DATA_POSE").unwrap();
            let entity = store.get_mut(id).unwrap();
            entity.metadata.insert(index, super::super::Metadata::Pose(pose));
            models.dimensions(store.get(id).unwrap()).map(|(d, _)| size(d)).ok()
        };
        let mut checked = 0;
        for (&pose, name) in &models.poses {
            let avatar = if name == "SLEEPING" {
                constant("minecraft:mannequin", "SLEEPING_DIMENSIONS")
            } else {
                table.get(name).map(size).or_else(|| constant("minecraft:mannequin", "STANDING_DIMENSIONS"))
            };
            assert_eq!(sized_in(&mut store, 1, pose), avatar, "mannequin {name}");
            let camel = match name.as_str() {
                "SITTING" => constant("minecraft:camel", "SITTING_DIMENSIONS"),
                "SLEEPING" => constant("minecraft:camel", "SLEEPING_DIMENSIONS"),
                _ => Some(size(&doc.entities["minecraft:camel"].dimensions)),
            };
            assert_eq!(sized_in(&mut store, 2, pose), camel, "camel {name}");
            checked += 1;
        }
        assert_eq!(checked, 18);
        // Each rule changes the answer: sleeping and crouching boxes differ from standing ones,
        // and a sitting camel has no `SITTING_DIMENSIONS` in these facts (an error, not its box).
        assert!(constant("minecraft:camel", "SLEEPING_DIMENSIONS").is_some());
        assert!(constant("minecraft:camel", "SITTING_DIMENSIONS").is_none());
        assert_ne!(table["CROUCHING"].height, table["STANDING"].height);
    }

    /// `sized` memoizes an entity's answer within one `SizedScope` only: a pose received
    /// between two frames resizes the entity in the next one, and outside any scope every call
    /// reads the metadata afresh.
    #[test]
    fn sized_memo_lasts_one_scope() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]);
        spawn.var(doc.entities["minecraft:mannequin"].protocol_id as i32);
        for coordinate in [0f64, 64., 0.] {
            spawn.0.extend_from_slice(&coordinate.to_be_bytes());
        }
        spawn.0.extend_from_slice(&[0; 4]);
        spawn.var(0);
        store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        let pose_of = |name: &str| *models.poses.iter().find(|(_, pose)| pose.as_str() == name).unwrap().0;
        let index = models.entity_accessor(store.get(1).unwrap(), "DATA_POSE").unwrap();
        let set_pose = |store: &mut super::super::Store, name: &str| {
            store.get_mut(1).unwrap().metadata.insert(index, super::super::Metadata::Pose(pose_of(name)));
        };
        let height = |store: &super::super::Store| models.dimensions(store.get(1).unwrap()).unwrap().0.height;
        set_pose(&mut store, "STANDING");
        let standing = {
            let _frame = SizedScope::enter();
            let first = height(&store);
            assert_eq!(height(&store), first, "repeated within the scope");
            first
        };
        set_pose(&mut store, "CROUCHING");
        let crouching = {
            let _frame = SizedScope::enter();
            height(&store)
        };
        assert!(crouching < standing, "the next scope sees the received pose: {crouching} vs {standing}");
        set_pose(&mut store, "STANDING");
        assert_eq!(height(&store), standing, "outside a scope");
    }
}
