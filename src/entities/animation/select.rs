//! Clip choice for a received `swing_animation`: the item in the swinging hand, the
//! swing type, whether the entity rides, and the entity's swing counter.
//!
//! The table matches the battle emission agreed with the core combat slice
//! (receipts/BattleVideo/PLAN.md line 16; bow/crossbow/shield/stagger timings from
//! BvCombatActions, 2026-11-09): melee clips put contact at windup/duration, the bow
//! releases at 3/4, the crossbow fires at 2/5, a shield covers from 1/5 to 4/5.
use crate::{inventory::Stack, protocol::content::Content};

/// 26.3 `SwingAnimationType` wire ids (javap `net.minecraft.world.item.SwingAnimationType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwingKind {
    None,
    Whack,
    Stab,
}

impl SwingKind {
    pub(crate) fn from_wire(id: i32) -> crate::protocol::Result<Self> {
        Ok(match id {
            0 => Self::None,
            1 => Self::Whack,
            2 => Self::Stab,
            _ => return Err(format!("swing_animation type {id}")),
        })
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Whack => "whack",
            Self::Stab => "stab",
        }
    }
}

/// How a held item fights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Sword,
    /// Mace and axes: overhead smash.
    Heavy,
    /// Spears and the trident.
    Spear,
    Bow,
    Crossbow,
    Shield,
}

pub(crate) fn classify(content: &Content, stack: &Stack) -> Option<Class> {
    let name = content.item_names.get(&stack.id).map(String::as_str)?;
    Some(match name {
        "minecraft:bow" => Class::Bow,
        "minecraft:crossbow" => Class::Crossbow,
        "minecraft:trident" => Class::Spear,
        "minecraft:mace" => Class::Heavy,
        "minecraft:shield" => Class::Shield,
        _ if name.ends_with("_sword") => Class::Sword,
        _ if name.ends_with("_spear") => Class::Spear,
        _ if name.ends_with("_axe") => Class::Heavy,
        _ if stack.component("minecraft:blocks_attacks").is_some() => Class::Shield,
        _ if stack.component("minecraft:kinetic_weapon").is_some() => Class::Spear,
        _ => return None,
    })
}

/// Clips a swing may play, cycled by the entity's swing counter, and the anatomical arm
/// (0 right, 1 left) they were authored for; the other arm plays them mirrored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Family {
    pub clips: &'static [&'static str],
    pub arm: usize,
}

pub(crate) const SWORD_COMBO: [&str; 3] = [
    "animation.solaris.humanoid.sword_slash",
    "animation.solaris.humanoid.sword_backhand",
    "animation.solaris.humanoid.sword_overhead",
];
pub(crate) const SPEAR: &str = "animation.solaris.humanoid.spear_thrust";
pub(crate) const MACE: &str = "animation.solaris.humanoid.mace_overhead";
pub(crate) const SHIELD: &str = "animation.solaris.humanoid.shield_block";
pub(crate) const STAGGER: &str = "animation.solaris.humanoid.stagger";
pub(crate) const BOW: &str = "animation.solaris.humanoid.bow_shot";
pub(crate) const CROSSBOW: &str = "animation.solaris.humanoid.crossbow_shot";
pub(crate) const LANCE: &str = "animation.solaris.humanoid.lance_strike";
pub(crate) const MOUNTED_SWORD: &str = "animation.solaris.humanoid.mounted_sword";
pub(crate) const HURT: &str = "animation.solaris.humanoid.hurt";
pub(crate) const DEATH: &str = "animation.solaris.humanoid.death";
pub(crate) const HORSE_HURT: &str = "animation.solaris.horse.hurt";
pub(crate) const HORSE_DEATH: &str = "animation.solaris.horse.death";

/// Every clip name the selection can start; the shipped files must provide each.
#[cfg(test)]
pub(crate) const ALL: [&str; 15] = [
    SWORD_COMBO[0], SWORD_COMBO[1], SWORD_COMBO[2], SPEAR, MACE, SHIELD, STAGGER, BOW, CROSSBOW,
    LANCE, MOUNTED_SWORD, HURT, DEATH, HORSE_HURT, HORSE_DEATH,
];

const fn right(clips: &'static [&'static str]) -> Family {
    Family { clips, arm: 0 }
}

pub(crate) fn family(class: Class, kind: SwingKind, mounted: bool) -> Option<Family> {
    Some(match (class, kind) {
        (Class::Sword | Class::Heavy | Class::Spear, SwingKind::None) => right(&[STAGGER]),
        (_, SwingKind::None) => return None,
        (Class::Sword, SwingKind::Whack) if mounted => right(&[MOUNTED_SWORD]),
        (Class::Sword, SwingKind::Whack) => right(&SWORD_COMBO),
        (Class::Sword, SwingKind::Stab) => return None,
        (Class::Spear, _) if mounted => right(&[LANCE]),
        (Class::Spear, _) => right(&[SPEAR]),
        (Class::Heavy, _) => right(&[MACE]),
        (Class::Bow, _) => right(&[BOW]),
        (Class::Crossbow, _) => right(&[CROSSBOW]),
        (Class::Shield, _) => Family { clips: &[SHIELD], arm: 1 },
    })
}

/// Entity types whose rig takes the horse clips.
pub(crate) fn horse_like(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:horse" | "minecraft:donkey" | "minecraft:mule" | "minecraft:zombie_horse"
            | "minecraft:skeleton_horse"
    )
}
