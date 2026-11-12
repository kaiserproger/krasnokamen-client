//! Keyframe combat clips: files, baking and selection from received packets.
//!
//! * `assets/krasnokamen/animations/*.animation.json` hold Krasnokamen-authored clips in a
//!   Bedrock-style keyframe format ([`format`]); they are embedded at build time, baked to
//!   the renderer's fixed-rate tables ([`bake`]) and installed once into
//!   [`clip_pose`](super::clip_pose), which both entity render paths sample with one rule.
//! * A received 26.3 `swing_animation` starts the clip chosen by the item in the swinging
//!   hand, the swing type, the mount state and the entity's swing counter ([`select`]),
//!   stretched so the clip spans the received duration: its authored contact frame then
//!   lands at the same fraction of the swing (PLAN.md: windup/duration). A swing no clip
//!   matches falls back to the vanilla arm swing over the received duration.
//! * Once an entity has started a clip it is "armed": hurt packets add the flinch layer
//!   and its death plays the death fall instead of the vanilla tip-over (its mount too).
//!   An entity that never received such a swing keeps the vanilla look.
mod bake;
mod format;
mod select;
#[cfg(test)]
mod tests;

pub(crate) use select::SwingKind;

use super::{
    Entity, Handle, Models,
    clip_pose::{self, ClipLayer},
};
use crate::protocol::content::Content;
use std::{collections::BTreeMap, sync::LazyLock};

/// `ClipState` layer slots (renderer contract): the action, then the additive flinch.
const ACTION: usize = 0;
const FLINCH: usize = 1;

/// Shipped clip files, embedded so the client never reads them at run time.
const SOURCES: [(&str, &str); 2] = [
    (
        "humanoid.animation.json",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/krasnokamen/animations/humanoid.animation.json")),
    ),
    (
        "horse.animation.json",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/krasnokamen/animations/horse.animation.json")),
    ),
];

#[derive(Debug)]
struct Entry {
    /// 1-based id in the installed `ClipLibrary`.
    id: u16,
    /// Authored clip seconds.
    length: f32,
}

/// The installed clips by name.
#[derive(Debug)]
pub(crate) struct Catalog {
    clips: BTreeMap<String, Entry>,
}

impl Catalog {
    /// A malformed file or clip is reported once and skipped: swings that would pick it
    /// fall back to the vanilla swing instead of taking the client down.
    fn load() -> Self {
        let mut baked = Vec::new();
        let mut lengths = BTreeMap::new();
        for (file, source) in SOURCES {
            let clips = match format::parse(source) {
                Ok(clips) => clips,
                Err(error) => {
                    eprintln!("krasnokamen animation {file} rejected: {error}");
                    continue;
                }
            };
            for clip in clips {
                if lengths.contains_key(&clip.name) {
                    eprintln!("krasnokamen animation {file}: duplicate clip {}", clip.name);
                    continue;
                }
                match bake::bake(&clip) {
                    Ok(table) => {
                        lengths.insert(clip.name, clip.length);
                        baked.push(table);
                    }
                    Err(error) => eprintln!("krasnokamen animation {file} clip rejected: {error}"),
                }
            }
        }
        if let Err(error) = clip_pose::ClipLibrary::new(baked).and_then(clip_pose::install) {
            eprintln!("krasnokamen animation library not installed: {error}");
        }
        let library = clip_pose::library();
        let clips = lengths
            .into_iter()
            .filter_map(|(name, length)| {
                let id = library.id(&name)?;
                Some((name, Entry { id, length }))
            })
            .collect();
        Self { clips }
    }
    fn get(&self, name: &str) -> Option<&Entry> {
        self.clips.get(name)
    }
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::load);

/// Loads and installs the clips on first use.
pub(crate) fn catalog() -> &'static Catalog {
    &CATALOG
}

/// Per-entity selection state, beside the renderer's `ClipState`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Selection {
    /// Swing packets received: the combo counter.
    swings: u32,
    /// Started a clip once: hurt and death play clips from now on.
    armed: bool,
    /// Duration of the latest vanilla-fallback swing, in ticks.
    vanilla_ticks: u32,
    /// World tick at which the playing swing clip ends (0: none plays).
    swing_end: u64,
}

impl Default for Selection {
    fn default() -> Self {
        // `SwingAnimation.DEFAULT`: whack for 6 ticks.
        Self { swings: 0, armed: false, vanilla_ticks: 6, swing_end: 0 }
    }
}

impl Selection {
    /// Ticks of the vanilla arm swing `Entity::animation` plays.
    pub(crate) fn vanilla_ticks(&self) -> u32 {
        self.vanilla_ticks
    }
    /// A swing clip plays at `tick` (vanilla `LivingEntity.swinging` for the received span).
    pub(crate) fn swinging(&self, tick: u64) -> bool {
        tick < self.swing_end
    }
    /// Mounts of armed riders take the hurt and death clips too.
    pub(crate) fn arm(&mut self) {
        self.armed = true;
    }
}

/// What a received swing did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Swing {
    /// Started the named clip; the entity's mount, if any, should be armed.
    Clip(&'static str, Option<Handle>),
    /// No clip matched: the vanilla arm swing plays.
    Vanilla,
    /// `none` swing without a clip, zero duration or a dead entity: nothing plays.
    Ignored,
}

/// Applies one received `swing_animation` to `e`.
pub(crate) fn receive_swing(
    e: &mut Entity,
    models: Option<&Models>,
    content: &Content,
    hand: u8,
    kind: SwingKind,
    duration: u32,
    tick: u64,
) -> Swing {
    let combo = e.physical.combat.swings;
    e.physical.combat.swings = combo.wrapping_add(1);
    if duration == 0 || e.death_tick.is_some() {
        return Swing::Ignored;
    }
    let chosen = e
        .equipment_value(usize::from(hand))
        .filter(|stack| !stack.empty())
        .and_then(|stack| select::classify(content, stack))
        .and_then(|class| select::family(class, kind, e.vehicle.is_some()))
        .and_then(|family| {
            let name = family.clips[combo as usize % family.clips.len()];
            catalog().get(name).map(|entry| (family, name, entry))
        });
    match chosen {
        Some((family, name, entry)) => {
            let left_handed = models.is_some_and(|models| super::presentation::left_main_arm(models, e));
            let arm = usize::from(hand) ^ usize::from(left_handed);
            e.clip.layers[ACTION] = ClipLayer {
                clip: entry.id,
                start_tick: tick,
                rate: entry.length / (duration as f32 / 20.),
                weight: 1.,
                mirror: arm != family.arm,
            };
            e.physical.combat.armed = true;
            e.animation = None;
            e.physical.combat.swing_end = tick + u64::from(duration);
            Swing::Clip(name, e.vehicle)
        }
        None => {
            e.clip.layers[ACTION] = ClipLayer::default();
            e.physical.combat.swing_end = 0;
            if kind == SwingKind::None {
                return Swing::Ignored;
            }
            e.physical.combat.vanilla_ticks = duration;
            e.animation = Some((if hand == 0 { 0 } else { 3 }, tick));
            Swing::Vanilla
        }
    }
}

fn start(e: &mut Entity, layer: usize, name: &str, tick: u64) {
    if let Some(entry) = catalog().get(name) {
        e.clip.layers[layer] =
            ClipLayer { clip: entry.id, start_tick: tick, rate: 1., weight: 1., mirror: false };
    }
}

/// A received hurt (`hurt_animation` / `damage_event`) adds the flinch to armed entities.
pub(crate) fn on_hurt(e: &mut Entity, tick: u64) {
    if e.physical.combat.armed && e.death_tick.is_none() {
        let clip = if select::horse_like(&e.type_name) { select::HORSE_HURT } else { select::HURT };
        start(e, FLINCH, clip, tick);
    }
}

/// A received death (entity event 3) plays the death fall on armed entities.
pub(crate) fn on_death(e: &mut Entity, tick: u64) {
    if e.physical.combat.armed {
        let clip = if select::horse_like(&e.type_name) { select::HORSE_DEATH } else { select::DEATH };
        e.clip.layers[FLINCH] = ClipLayer::default();
        start(e, ACTION, clip, tick);
    }
}
