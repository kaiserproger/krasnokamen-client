use super::super::{presentation as p, Entity, Metadata, Models};
use super::modern::{Axolotl, Fox};
use super::motion::{BodyRule, MotionFacts, WalkRule};
use crate::{
    player::math,
    protocol::{models::JavaRandom, Result},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timer(i32);
impl Default for Timer {
    fn default() -> Self {
        Self(i32::MIN)
    }
}
impl Timer {
    pub fn start(&mut self, age: i32) {
        self.0 = age;
    }
    pub fn stop(&mut self) {
        self.0 = i32::MIN;
    }
    pub fn started(self) -> bool {
        self.0 != i32::MIN
    }
    pub fn start_if_stopped(&mut self, age: i32) {
        if !self.started() {
            self.start(age);
        }
    }
    pub fn animate_when(&mut self, condition: bool, age: i32) {
        if condition {
            self.start_if_stopped(age);
        } else {
            self.stop();
        }
    }
    pub fn fast_forward(&mut self, ticks: i32, scale: f32) {
        if self.started() {
            self.0 = self.0.wrapping_sub((ticks as f32 * scale) as i32);
        }
    }
    pub fn millis(self, age: f32) -> Option<i64> {
        self.started().then(|| ((age - self.0 as f32) * 50.) as i64)
    }
}
#[derive(Debug)]
pub(crate) struct Walk {
    speed_old: f32,
    speed: f32,
    position: f32,
    position_scale: f32,
}
impl Default for Walk {
    fn default() -> Self {
        Self {
            speed_old: 0.,
            speed: 0.,
            position: 0.,
            position_scale: 1.,
        }
    }
}
impl Walk {
    fn update(&mut self, target: f32, factor: f32, scale: f32) {
        self.speed_old = self.speed;
        self.speed += (target - self.speed) * factor;
        self.position += self.speed;
        self.position_scale = scale;
    }
    fn stop(&mut self) {
        self.speed_old = 0.;
        self.speed = 0.;
        self.position = 0.;
    }
    pub fn speed(&self, partial: f32) -> f32 {
        (self.speed_old + partial * (self.speed - self.speed_old)).min(1.)
    }
    pub fn position(&self, partial: f32) -> f32 {
        (self.position - self.speed * (1. - partial)) * self.position_scale
    }
    pub fn moving(&self) -> bool {
        self.speed > 1e-5
    }
}
#[derive(Debug)]
pub(crate) enum Modern {
    None,
    Bat {
        fly: Timer,
        rest: Timer,
    },
    CopperGolem {
        idle_start_tick: i32,
        idle: Timer,
        interactions: [Timer; 4],
    },
    Rabbit {
        idle_timeout: i32,
        jump_ticks: i32,
        jump_duration: i32,
        hop: Timer,
        head_tilt: Timer,
    },
    Axolotl(Axolotl),
    Fox(Fox),
    Frog {
        jump: Timer,
        croak: Timer,
        tongue: Timer,
        swim_idle: Timer,
    },
    Sniffer {
        happy: Timer,
        scent: Timer,
        sniff: Timer,
        dig: Timer,
        rise: Timer,
    },
    Armadillo {
        in_state_ticks: i64,
        peek_received: bool,
        roll_up: Timer,
        roll_out: Timer,
        peek: Timer,
    },
    Camel {
        idle_timeout: i32,
        sit: Timer,
        sit_pose: Timer,
        sit_up: Timer,
        idle: Timer,
        dash: Timer,
    },
    Warden {
        roar: Timer,
        sniff: Timer,
        emerge: Timer,
        dig: Timer,
        attack: Timer,
        sonic: Timer,
    },
    Breeze {
        idle: Timer,
        slide: Timer,
        slide_back: Timer,
        long_jump: Timer,
        shoot: Timer,
        inhale: Timer,
    },
    Creaking {
        attack: Timer,
        invulnerable: Timer,
        death: Timer,
        attack_remaining: i32,
        invulnerable_remaining: i32,
        eyes_glowing: bool,
        next_flicker: i32,
    },
}
impl Modern {
    fn new(kind: &str, random: &mut JavaRandom) -> Self {
        let timer = Timer::default();
        match kind {
            "minecraft:bat" => Self::Bat {
                fly: timer,
                rest: timer,
            },
            "minecraft:copper_golem" => Self::CopperGolem {
                idle_start_tick: 0,
                idle: timer,
                interactions: [timer; 4],
            },
            "minecraft:rabbit" => Self::Rabbit {
                idle_timeout: random.next_int(40) as i32 + 180,
                jump_ticks: 0,
                jump_duration: 0,
                hop: timer,
                head_tilt: timer,
            },
            "minecraft:axolotl" => Self::Axolotl(Axolotl::default()),
            "minecraft:fox" => Self::Fox(Fox::default()),
            "minecraft:frog" => Self::Frog {
                jump: timer,
                croak: timer,
                tongue: timer,
                swim_idle: timer,
            },
            "minecraft:sniffer" => Self::Sniffer {
                happy: timer,
                scent: timer,
                sniff: timer,
                dig: timer,
                rise: timer,
            },
            "minecraft:armadillo" => Self::Armadillo {
                in_state_ticks: 0,
                peek_received: false,
                roll_up: timer,
                roll_out: timer,
                peek: timer,
            },
            "minecraft:camel" | "minecraft:camel_husk" => Self::Camel {
                idle_timeout: 0,
                sit: timer,
                sit_pose: timer,
                sit_up: timer,
                idle: timer,
                dash: timer,
            },
            "minecraft:warden" => Self::Warden {
                roar: timer,
                sniff: timer,
                emerge: timer,
                dig: timer,
                attack: timer,
                sonic: timer,
            },
            "minecraft:breeze" => Self::Breeze {
                idle: timer,
                slide: timer,
                slide_back: timer,
                long_jump: timer,
                shoot: timer,
                inhale: timer,
            },
            "minecraft:creaking" => Self::Creaking {
                attack: timer,
                invulnerable: timer,
                death: timer,
                attack_remaining: 0,
                invulnerable_remaining: 0,
                eyes_glowing: false,
                next_flicker: 0,
            },
            _ => Self::None,
        }
    }
}
/// Facts computed by the shared Source tick owner. No clock or world query is duplicated here.
pub(crate) struct TickInput {
    pub age: i32,
    pub position: [f64; 3],
    pub yaw: f32,
    pub alive: bool,
    pub passenger: bool,
    pub baby: bool,
    pub visually_swimming: bool,
    pub in_water: bool,
    pub uses_y_for_walk: bool,
    pub rotation_changed_before_base_tick: bool,
    pub is_mob: bool,
    pub first_passenger_is_mob: bool,
    pub max_head_yaw: f32,
    pub max_body_head_yaw: f32,
    pub attack_active: bool,
    pub camel_sitting: bool,
    pub camel_visually_sitting: bool,
    pub camel_sitting_down: bool,
    pub camel_pose_transition: bool,
    pub camel_pose_time: i64,
}
#[derive(Debug)]
pub(crate) struct State {
    position_old: [f64; 3],
    body_rule: BodyRule,
    walk_rule: WalkRule,
    clamps_received_head: bool,
    pub body_yaw: f32,
    pub body_yaw_old: f32,
    pub head_yaw: f32,
    pub head_yaw_old: f32,
    pub hurt_time: i32,
    pub death_time: i32,
    stable_head_yaw: f32,
    stable_head_ticks: i32,
    pub walk: Walk,
    swim: f32,
    swim_old: f32,
    wings: f32,
    wings_old: f32,
    swell: i32,
    swell_old: i32,
    use_item: Option<(usize, crate::inventory::Stack)>,
    use_start: i32,
    use_age: i32,
    charge_duration: Option<Result<f32>>,
    effect_gaussian: Option<f64>,
    local_item_use: Option<(f32, f32)>,
    pub in_water: bool,
    pub modern: Modern,
    pub avatar: Avatar,
    random: Rc<RefCell<JavaRandom>>,
}
impl State {
    pub fn new(
        kind: &str,
        position: [f64; 3],
        body_yaw: f32,
        head_yaw: f32,
        random: Rc<RefCell<JavaRandom>>,
        facts: &MotionFacts,
    ) -> Result<Self> {
        let modern = Modern::new(kind, &mut random.borrow_mut());
        Ok(Self {
            body_rule: facts.body_rule.ok_or("living Source body rule missing")?,
            walk_rule: facts.walk_rule.ok_or("living Source walk rule missing")?,
            clamps_received_head: kind == "minecraft:goat",
            position_old: position,
            body_yaw,
            body_yaw_old: body_yaw,
            head_yaw,
            head_yaw_old: head_yaw,
            stable_head_yaw: 0.,
            stable_head_ticks: 0,
            hurt_time: 0,
            death_time: 0,
            walk: Walk::default(),
            swim: 0.,
            swim_old: 0.,
            wings: 0.,
            wings_old: 0.,
            swell: 0,
            swell_old: 0,
            use_item: None,
            use_start: 0,
            use_age: 0,
            charge_duration: None,
            effect_gaussian: None,
            local_item_use: None,
            in_water: false,
            modern,
            avatar: Avatar::default(),
            random,
        })
    }
    pub fn swim(&self, partial: f32) -> f32 {
        self.swim_old + partial * (self.swim - self.swim_old)
    }
    /// Updates the wing beat of mobs that fold their wings while standing.
    /// minecraft.wiki: chickens flap their wings while in midair. The beat eases in
    /// and out by a quarter per tick (a Krasnokamen choice), so landing settles in four ticks.
    pub fn tick_wings(&mut self, supported: bool) {
        const EASE: f32 = 0.25;
        self.wings_old = self.wings;
        self.wings = if supported {
            (self.wings - EASE).max(0.)
        } else {
            (self.wings + EASE).min(1.)
        };
    }
    /// Wing beat strength: 0 folded at rest, 1 fully beating.
    pub fn wings(&self, partial: f32) -> f32 {
        self.wings_old + partial * (self.wings - self.wings_old)
    }
    /// Creeper fuse progress, 0 idle .. 1 about to explode. The client counts the
    /// received swell direction (or ignition) itself, one step per tick, over the
    /// 30-tick (1.5 s) fuse minecraft.wiki gives for a creeper.
    pub fn swell(&self, partial: f32) -> f32 {
        (self.swell_old as f32 + partial * (self.swell - self.swell_old) as f32) / CREEPER_FUSE as f32
    }
    fn tick_swell(&mut self, models: &Models, entity: &Entity) {
        self.swell_old = self.swell;
        let direction = if p::boolean(models, entity, "DATA_IS_IGNITED") {
            1
        } else {
            p::number(models, entity, "DATA_SWELL_DIR").unwrap_or(-1).signum()
        };
        self.swell = (self.swell + direction).clamp(0, CREEPER_FUSE);
    }
    /// Independent pose curves driven solely by the existing Source action timers.
    pub fn apply_part(
        &self, models: &Models, entity: &Entity, bone: &super::super::resources::Bone,
        age: f32, partial: f32, pivot: &mut glam::Vec3, rotation: &mut glam::Vec3,
        scale: &mut glam::Vec3,
    ) {
        if matches!(&self.modern, Modern::None) { return; }
        let name = bone.name.as_str();
        let top = bone.parent == Some(0);
        let baby = p::boolean(models, entity, "DATA_BABY_ID");
        let unit = (if baby { 0.5 } else { 1. }) / 16.;
        let seconds = |timer: Timer| timer.millis(age).map(|t| t.max(0) as f32 / 1000.);
        let pulse = |timer: Timer, duration: f32| seconds(timer).map_or(0., |s| {
            if s < duration { (s / duration * std::f32::consts::PI).sin() } else { 0. }
        });
        let cycle = |timer: Timer, duration: f32| seconds(timer).map_or(0., |s| {
            (s.rem_euclid(duration) / duration * std::f32::consts::PI).sin()
        });
        use super::super::resources::BoneRole;
        let leg = matches!(bone.role, BoneRole::LeftLeg | BoneRole::RightLeg);
        let arm = matches!(bone.role, BoneRole::LeftArm | BoneRole::RightArm);
        match &self.modern {
            Modern::Bat { rest, .. } => {
                if rest.started() {
                    if name == "root" { rotation.z += std::f32::consts::PI; pivot.y += 2.; }
                    if name.contains("wing") {
                        let side = if name.starts_with("right") { 1. } else { -1. };
                        rotation.y = side * 1.3;
                        rotation.z = side * 0.15;
                    }
                } else if name.contains("wing") {
                    let side = if name.starts_with("right") { 1. } else { -1. };
                    rotation.y = side * (age * std::f32::consts::TAU / 10.).cos() * 0.9;
                    rotation.z = bone.rotation.z;
                }
            }
            Modern::Armadillo { roll_up, roll_out, peek, .. } => {
                let tuck = match p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) {
                    1 => seconds(*roll_up).map_or(0., |s| (s / 0.5).clamp(0., 1.)),
                    2 => 1.,
                    3 => seconds(*roll_out).map_or(1., |s| 1. - (s / 1.5).clamp(0., 1.)),
                    _ => 0.,
                };
                if name == "head" { rotation.x -= tuck * 1.2 - pulse(*peek, 2.5) * 0.5; pivot.z += tuck * 2. * unit; }
                else if leg { rotation.x = bone.rotation.x + tuck * 1.4; }
                else if name == "tail" { rotation.x -= tuck * 1.2; }
            }
            Modern::Camel { sit, sit_pose, sit_up, dash, idle, .. } => {
                let down = if sit_pose.started() { 1. } else { seconds(*sit).map_or(0., |s| (s / 2.).clamp(0., 1.)) };
                let down = if sit_up.started() { seconds(*sit_up).map_or(0., |s| 1. - (s / 2.6).clamp(0., 1.)) } else { down };
                // Resting depth of the authored rigs: the adult back drops onto the
                // received sitting seat and its folded front legs tuck back under the chest.
                let (drop, tuck) = if baby { (4.5, 0.) } else { (19., 12.) };
                if top { pivot.y += down * drop / 16.; }
                if leg {
                    rotation.x = bone.rotation.x - down * 1.5;
                    if name.contains("front") { pivot.z += down * tuck / 16.; }
                }
                if name == "body" && dash.started() { rotation.x += 5_f32.to_radians(); }
                if name == "head" { rotation.x -= pulse(*dash, 0.5) * 0.4; }
                if name == "tail" { rotation.y += pulse(*idle, 4.) * (age * 0.4).sin() * 0.15; }
            }
            Modern::Frog { jump, croak, tongue, swim_idle } => {
                if name == "body" { rotation.x -= pulse(*jump, 0.5) * 0.4; }
                if leg { rotation.x -= pulse(*jump, 0.5); }
                if name == "head" { rotation.x -= pulse(*tongue, 0.5); }
                if name == "tongue" { scale.z *= 0.05 + pulse(*tongue, 0.5) * 2.; }
                if name == "croaking_body" { let p = pulse(*croak, 3.); scale.y *= 1. + p; scale.z *= 1. + p * 0.3; }
                if name == "body" && swim_idle.started() { rotation.x += (age * 0.1).sin() * 0.08; }
            }
            Modern::Warden { roar, sniff, emerge, dig, attack, sonic } => {
                let pose = p::number(models, entity, "DATA_POSE").unwrap_or(0);
                if name == "root" {
                    if pose == 13 { pivot.y += seconds(*emerge).map_or(0., |s| (1. - s / 6.7).clamp(0., 1.)) * 32. * unit; }
                    if pose == 14 { pivot.y += seconds(*dig).map_or(0., |s| (s / 5.).clamp(0., 1.)) * 32. * unit; }
                }
                if name == "head" { rotation.x -= pulse(*roar, 4.2) * 0.6 + pulse(*sonic, 3.) * 0.6; rotation.x += pulse(*sniff, 4.16) * 0.4; }
                if arm {
                    rotation.x -= pulse(*attack, 1. / 3.) * 2.4;
                    let side = if name.starts_with("right") { 1. } else { -1. };
                    rotation.z += side * (pulse(*roar, 4.2) + pulse(*sonic, 3.)) * 0.5;
                }
            }
            Modern::Rabbit { hop, head_tilt, jump_ticks, jump_duration, .. } => {
                let hop = if *jump_duration > 0 {
                    (((*jump_ticks as f32 + partial) / *jump_duration as f32).clamp(0., 1.) * std::f32::consts::PI).sin()
                } else { pulse(*hop, 0.75) };
                if leg { rotation.x = bone.rotation.x + if name.contains("hind") { hop * 0.9 } else { -hop * 0.7 }; }
                if name == "head" { rotation.z += pulse(*head_tilt, 4.) * 0.3; }
                if name == "body" { rotation.x -= hop * 0.2; }
            }
            Modern::Axolotl(axolotl) => {
                use super::modern::AxolotlAnimation;
                let dead = if baby { if axolotl.animation(AxolotlAnimation::PlayDead).started() { 1. } else { 0. } }
                    else { axolotl.playing_dead.factor(partial) };
                if name == "root" { rotation.z -= dead * 0.3; }
                if name == "body" { rotation.x -= dead * 0.25; }
                if name == "tail" { rotation.y += (age * 0.2).sin() * (1. - dead) * 0.4; }
                if name.ends_with("gills") { rotation.x += (age * 0.15).sin() * 0.15 * (1. - dead); }
                if leg { rotation.x = bone.rotation.x + (1. - dead) * (age * 0.3).sin() * self.walk.speed(partial); }
            }
            Modern::Fox(fox) => {
                if name == "head" { rotation.z += fox.head_roll(partial); }
                if top { pivot.y += fox.crouch(partial) * unit; }
            }
            Modern::Sniffer { happy, scent, sniff, dig, rise } => {
                if name == "head" || name == "snout" {
                    rotation.x += pulse(*dig, 8.) * 0.8 + cycle(*sniff, 8.) * 0.3 - pulse(*scent, 1.) * 0.5 - pulse(*rise, 3.) * 0.3;
                    rotation.y += cycle(*happy, 2.) * (age * 0.4).sin() * 0.3;
                }
                if name == "body" { rotation.x += pulse(*dig, 8.) * 0.15; }
            }
            Modern::Breeze { shoot, inhale, long_jump, slide, slide_back, .. } => {
                if name == "root" {
                    pivot.y += pulse(*inhale, 2.) * 10. * unit - pulse(*long_jump, 0.5) * 11. * unit;
                    let sliding = seconds(*slide).map_or(0., |s| (s / 0.2).clamp(0., 1.));
                    let returning = seconds(*slide_back).map_or(0., |s| 1. - (s / 0.1).clamp(0., 1.));
                    pivot.z -= (sliding + returning) * 6. * unit;
                }
                if name == "head" { rotation.x -= pulse(*shoot, 1.125) * 12.5_f32.to_radians(); }
            }
            Modern::CopperGolem { idle, interactions, .. } => {
                if name == "head" { rotation.y += pulse(*idle, 3.5) * 0.6; }
                if let Some((index, timer)) = interactions.iter().enumerate().find(|(_, timer)| timer.started()) {
                    let progress = seconds(*timer).unwrap_or(0.).rem_euclid(3.) / 3.;
                    let reach = (progress * std::f32::consts::PI).sin();
                    if arm { rotation.x -= reach * if index < 2 { 1.3 } else { 0.9 }; }
                    if name == "head" && index % 2 == 1 { rotation.y += (progress * std::f32::consts::TAU).sin() * 0.25; }
                }
            }
            Modern::Creaking { attack, invulnerable, death, .. } => {
                if arm { rotation.x -= pulse(*attack, 0.7083) * 1.7; }
                if name == "root" {
                    rotation.z += pulse(*invulnerable, 0.2917) * (age * 2.).sin() * 0.1;
                    if death.started() { pivot.y += seconds(*death).unwrap_or(0.).min(2.25) * unit; }
                }
            }
            Modern::None => {}
        }
    }

    pub fn part_visible(&self, models: &Models, entity: &Entity, name: &str, age: f32) -> bool {
        match &self.modern {
            Modern::Frog { tongue, croak, .. } => match name {
                "tongue" => tongue.millis(age).is_some_and(|t| (1..500).contains(&t)),
                "croaking_body" => croak.millis(age).is_some_and(|t| (0..3000).contains(&t)),
                _ => true,
            },
            Modern::Armadillo { peek, roll_up, .. } => {
                let state = p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0);
                let closed = state == 2 || state == 1 && roll_up.millis(age).is_some_and(|t| t >= 400);
                match name {
                    "cube" => closed,
                    "body" | "tail" | "left_hind_leg" | "right_hind_leg" | "left_front_leg" | "right_front_leg" => !closed,
                    "head" | "left_ear" | "right_ear" => !closed || peek.millis(age).is_some_and(|t| (400..1400).contains(&t)),
                    _ => true,
                }
            }
            _ => true,
        }
    }

    pub fn set_local_item_use(&mut self, action: Option<(f32, f32)>) {
        self.local_item_use = action;
    }
    /// Resolve stochastic enchantment effects once per received use/stack activation.
    /// The local avatar borrows the duration its actual interaction owner already chose.
    pub fn prepare_item_use(
        &mut self, models: &Models, entity: &Entity,
        content: &crate::protocol::content::Content, age: i32,
    ) -> Result<()> {
        self.update_item_use(models, entity, age);
        if let Some((_, duration)) = self.local_item_use {
            self.charge_duration = (duration > 0.).then_some(Ok(duration));
            return Ok(());
        }
        let Some((_, stack)) = &self.use_item else { return Ok(()) };
        if content.item_names.get(&stack.id).is_none_or(|name| name != "minecraft:crossbow") {
            return Ok(());
        }
        if self.charge_duration.is_none() {
            let mut random = self.random.borrow_mut();
            let gaussian = &mut self.effect_gaussian;
            self.charge_duration = Some(crate::player::crossbow_charge_ticks(stack, content, &mut |kind| {
                match kind {
                    crate::player::EffectSample::Uniform => f64::from(random.next_float()),
                    crate::player::EffectSample::Gaussian =>
                        crate::player::sample_gaussian(gaussian, &mut || random.next_double()),
                }
            }));
        }
        self.charge_duration.as_ref().unwrap().as_ref().map(|_| ()).map_err(Clone::clone)
    }
    pub fn crossbow_pull(&self, partial: f32) -> f32 {
        self.charge_duration.as_ref().and_then(|r| r.as_ref().ok())
            .map_or(0., |duration| (self.item_use_ticks(partial) / duration).clamp(0., 1.))
    }

    /// Active-use time belongs to the action, not the last movement packet.
    pub fn item_use_ticks(&self, partial: f32) -> f32 {
        if let Some((ticks, _)) = self.local_item_use { return ticks + partial; }
        self.use_item.as_ref().map_or(0., |_| {
            (self.use_age - self.use_start).max(0) as f32 + partial
        })
    }
    fn update_item_use(&mut self, models: &Models, entity: &Entity, age: i32) {
        let flags = p::number(models, entity, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
        let charging = p::boolean(models, entity, "IS_CHARGING_CROSSBOW")
            || p::boolean(models, entity, "DATA_IS_CHARGING_CROSSBOW");
        let hand = usize::from(flags & 2 != 0);
        let active = (flags & 1 != 0 || charging)
            .then(|| entity.equipment_value(hand))
            .flatten()
            .filter(|stack| !stack.empty());
        match active {
            Some(stack) => {
                if self.use_item.as_ref().is_none_or(|(old_hand, old)| *old_hand != hand || old.id != stack.id) {
                    self.use_start = age;
                }
                if self.use_item.as_ref().is_none_or(|(old_hand, old)| {
                    *old_hand != hand || !old.same_item(stack)
                }) {
                    self.use_item = Some((hand, stack.clone()));
                    self.charge_duration = None;
                }
            }
            None => { self.use_item = None; self.charge_duration = None; }
        }
        self.use_age = age;
    }
    pub fn body(&self, partial: f32) -> f32 {
        self.body_yaw_old + partial * wrap(self.body_yaw - self.body_yaw_old)
    }
    pub fn head(&self, partial: f32) -> f32 {
        self.head_yaw_old + partial * wrap(self.head_yaw - self.head_yaw_old)
    }
    pub fn on_head_rotation(&mut self, yaw: f32) {
        self.head_yaw = if self.clamps_received_head {
            self.body_yaw + wrap(yaw - self.body_yaw).clamp(-15., 15.)
        } else {
            yaw
        };
    }
    pub fn on_metadata(
        &mut self,
        models: &Models,
        entity: &Entity,
        changed: &[bool; 256],
        age: i32,
    ) {
        self.update_item_use(models, entity, age);
        let received = |name| {
            models
                .accessor(&entity.type_name, name)
                .is_some_and(|id| changed[usize::from(id)])
        };
        let pose = p::number(models, entity, "DATA_POSE").unwrap_or(0);
        match &mut self.modern {
            Modern::Frog {
                jump,
                croak,
                tongue,
                ..
            } if received("DATA_POSE") => {
                for (timer, expected) in [(jump, 6), (croak, 8), (tongue, 9)] {
                    if pose == expected {
                        timer.start(age);
                    } else {
                        timer.stop();
                    }
                }
            }
            Modern::Sniffer {
                happy,
                scent,
                sniff,
                dig,
                rise,
            } if received("DATA_STATE") => {
                let state = p::number(models, entity, "DATA_STATE").unwrap_or(0);
                for (timer, expected) in [(happy, 1), (scent, 2), (sniff, 3), (dig, 5), (rise, 6)] {
                    timer.stop();
                    if state == expected {
                        timer.start_if_stopped(age);
                    }
                }
            }
            Modern::Armadillo { in_state_ticks, .. } if received("ARMADILLO_STATE") => {
                *in_state_ticks = 0
            }
            Modern::Warden {
                roar,
                sniff,
                emerge,
                dig,
                ..
            } if received("DATA_POSE") => {
                // The Source handler starts the selected state; it does not stop prior timers here.
                match pose {
                    11 => roar.start(age),
                    12 => sniff.start(age),
                    13 => emerge.start(age),
                    14 => dig.start(age),
                    _ => {}
                }
            }
            Modern::Breeze {
                idle,
                slide,
                long_jump,
                shoot,
                inhale,
                ..
            } if received("DATA_POSE") => {
                shoot.stop();
                idle.stop();
                inhale.stop();
                long_jump.stop();
                match pose {
                    16 => shoot.start_if_stopped(age),
                    17 => inhale.start_if_stopped(age),
                    15 => slide.start_if_stopped(age),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    pub fn on_status(&mut self, status: u8, age: i32) {
        match &mut self.modern {
            Modern::Rabbit {
                jump_ticks,
                jump_duration,
                ..
            } if status == 1 => {
                *jump_ticks = 0;
                *jump_duration = 15;
            }
            Modern::Armadillo { peek_received, .. } if status == 64 => *peek_received = true,
            Modern::Warden {
                roar,
                attack,
                sonic,
                ..
            } => match status {
                4 => {
                    roar.stop();
                    attack.start(age);
                }
                62 => sonic.start(age),
                _ => {}
            },
            Modern::Creaking {
                attack_remaining,
                invulnerable_remaining,
                ..
            } => match status {
                4 => *attack_remaining = 15,
                66 => *invulnerable_remaining = 8,
                _ => {}
            },
            _ => {}
        }
    }
    pub fn on_leash_change(&mut self) {
        if let Modern::Rabbit { head_tilt, .. } = &mut self.modern {
            head_tilt.stop();
        }
    }
    pub fn on_damage(&mut self) {
        self.hurt_time = 10;
    }
    pub fn tick(&mut self, models: &Models, entity: &Entity, input: &TickInput) {
        self.update_item_use(models, entity, input.age);
        self.in_water = input.in_water;
        let delta = std::array::from_fn::<_, 3, _>(|i| input.position[i] - self.position_old[i]);
        self.body_yaw_old = self.body_yaw;
        self.head_yaw_old = self.head_yaw;
        self.on_head_rotation(entity.presented_transform(0, 0.).head_yaw);
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
        if entity.type_name == "minecraft:creeper" {
            self.tick_swell(models, entity);
        }
        if !input.alive {
            self.death_time = self.death_time.wrapping_add(1);
        }
        if let Modern::Breeze {
            idle,
            slide,
            slide_back,
            long_jump,
            ..
        } = &mut self.modern
        {
            let age_before_super = input.age.wrapping_sub(1);
            let pose = p::number(models, entity, "DATA_POSE").unwrap_or(0);
            if pose == 6 {
                long_jump.start_if_stopped(age_before_super);
            }
            idle.start_if_stopped(age_before_super);
            if pose != 15 && slide.started() {
                slide_back.start(age_before_super);
                slide.stop();
            }
        }
        self.swim_old = self.swim;
        self.swim = if input.visually_swimming {
            (self.swim + 0.09).min(1.)
        } else {
            (self.swim - 0.09).max(0.)
        };
        // Frog.tick samples last tick's walk state before LivingEntity updates it.
        if let Modern::Frog { swim_idle, .. } = &mut self.modern {
            swim_idle.animate_when(
                input.in_water && !self.walk.moving(),
                input.age.wrapping_sub(1),
            );
        }
        // These baseTick overrides sample the previous WalkAnimationState, before aiStep.
        match &mut self.modern {
            Modern::Axolotl(axolotl) => axolotl.tick(
                input.age,
                input.baby,
                p::boolean(models, entity, "DATA_PLAYING_DEAD"),
                input.in_water,
                entity.on_ground,
                self.walk.moving() || input.rotation_changed_before_base_tick,
            ),
            Modern::Rabbit {
                idle_timeout,
                jump_ticks,
                hop,
                head_tilt,
                ..
            } => {
                let no_ai = p::number(models, entity, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 1 != 0;
                if *idle_timeout <= 0 && entity.leash_value().is_none() && !no_ai {
                    *idle_timeout = self.random.borrow_mut().next_int(40) as i32 + 180;
                    head_tilt.start(input.age);
                } else if *jump_ticks > 0 {
                    hop.start_if_stopped(input.age);
                    head_tilt.stop();
                } else {
                    *idle_timeout = idle_timeout.wrapping_sub(1);
                    hop.stop();
                }
            }
            _ => {}
        }
        let distance = (delta[0] * delta[0]
            + delta[2] * delta[2]
            + if input.uses_y_for_walk {
                delta[1] * delta[1]
            } else {
                0.
            })
        .sqrt() as f32;
        if input.passenger || !input.alive {
            self.walk.stop();
        } else {
            let (target, factor, scale) = match self.walk_rule {
                WalkRule::Frog => (
                    if matches!(&self.modern, Modern::Frog { jump, .. } if jump.started()) {
                        0.
                    } else {
                        (distance * 25.).min(1.)
                    },
                    0.4,
                    if input.baby { 3. } else { 1. },
                ),
                WalkRule::Camel => (
                    if p::number(models, entity, "DATA_POSE").unwrap_or(0) == 0
                        && matches!(&self.modern, Modern::Camel { dash, .. } if !dash.started())
                    {
                        (distance * 6.).min(1.)
                    } else {
                        0.
                    },
                    0.2,
                    if input.baby { 3. } else { 1. },
                ),
                WalkRule::Creaking => ((distance * 25.).min(3.), 0.4, 1.),
                WalkRule::Standard => (
                    (distance * 4.).min(1.),
                    0.4,
                    if input.baby { 3. } else { 1. },
                ),
            };
            self.walk.update(target, factor, scale);
        }
        let body_enabled = match self.body_rule {
            BodyRule::Shulker => false,
            BodyRule::Armadillo => p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) == 0,
            BodyRule::Camel => !input.camel_sitting && !input.camel_pose_transition,
            BodyRule::Creaking => p::value(models, entity, "CAN_MOVE")
                .and_then(Metadata::boolean)
                .unwrap_or(true),
            _ => true,
        };
        if matches!(self.body_rule, BodyRule::ArmorStand) {
            self.body_yaw = input.yaw;
        } else if matches!(self.body_rule, BodyRule::Phantom) {
            self.head_yaw = self.body_yaw;
            self.body_yaw = input.yaw;
        } else if input.is_mob && body_enabled {
            if matches!(self.body_rule, BodyRule::HappyGhast) && !entity.passenger_ids.is_empty() {
                self.body_yaw = input.yaw;
                self.head_yaw = input.yaw;
            }
            if delta[0] * delta[0] + delta[2] * delta[2] > 2.500000277905201e-7 {
                self.body_yaw = input.yaw;
                self.head_yaw =
                    rotate_if_necessary(self.head_yaw, self.body_yaw, input.max_head_yaw);
                self.stable_head_yaw = self.head_yaw;
                self.stable_head_ticks = 0;
            } else if !input.first_passenger_is_mob {
                if (self.head_yaw - self.stable_head_yaw).abs() > 15. {
                    self.stable_head_ticks = 0;
                    self.stable_head_yaw = self.head_yaw;
                    self.body_yaw =
                        rotate_if_necessary(self.body_yaw, self.head_yaw, input.max_head_yaw);
                } else {
                    self.stable_head_ticks = self.stable_head_ticks.wrapping_add(1);
                    if self.stable_head_ticks > 10 {
                        let fraction = ((self.stable_head_ticks - 10) as f32 / 10.).clamp(0., 1.);
                        self.body_yaw = rotate_if_necessary(
                            self.body_yaw,
                            self.head_yaw,
                            input.max_head_yaw * (1. - fraction),
                        );
                    }
                }
            }
        } else if !input.is_mob {
            let mut target = self.body_yaw;
            if (delta[0] * delta[0] + delta[2] * delta[2]) as f32 > 0.0025000002 {
                let direction = math::atan2(delta[2], delta[0]) as f32 * 57.295776 - 90.;
                let difference = (wrap(input.yaw) - direction).abs();
                target = if difference > 95. && difference < 265. {
                    direction - 180.
                } else {
                    direction
                };
            }
            if input.attack_active {
                target = input.yaw;
            }
            self.body_yaw += wrap(target - self.body_yaw) * 0.3;
            let difference = wrap(input.yaw - self.body_yaw);
            if difference.abs() > input.max_body_head_yaw {
                self.body_yaw += difference
                    - if difference > 0. {
                        input.max_body_head_yaw
                    } else {
                        -input.max_body_head_yaw
                    };
            }
        }
        match &mut self.modern {
            Modern::Bat { fly, rest } => {
                let resting = p::number(models, entity, "DATA_ID_FLAGS").unwrap_or(0) & 1 != 0;
                fly.animate_when(!resting, input.age);
                rest.animate_when(resting, input.age);
            }
            Modern::Fox(fox) => fox.tick(p::number(models, entity, "DATA_FLAGS_ID").unwrap_or(0)),
            Modern::CopperGolem {
                idle_start_tick,
                idle,
                interactions,
            } => {
                if p::number(models, entity, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 1 == 0 {
                    match p::number(models, entity, "COPPER_GOLEM_STATE").unwrap_or(0) {
                        0 => {
                            for animation in interactions {
                                animation.stop();
                            }
                            if *idle_start_tick == input.age {
                                idle.start(input.age);
                            } else if *idle_start_tick == 0 {
                                *idle_start_tick = input.age.wrapping_add(
                                    self.random.borrow_mut().next_int(40) as i32 + 200,
                                );
                            }
                            if input.age as f32 == *idle_start_tick as f32 + 10. {
                                *idle_start_tick = 0;
                            }
                        }
                        state @ 1..=4 => {
                            idle.stop();
                            *idle_start_tick = 0;
                            for (index, animation) in interactions.iter_mut().enumerate() {
                                animation.animate_when(index == (state - 1) as usize, input.age);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Modern::Rabbit {
                jump_ticks,
                jump_duration,
                ..
            } => {
                if jump_ticks != jump_duration {
                    *jump_ticks = jump_ticks.wrapping_add(1);
                } else if *jump_duration != 0 {
                    *jump_ticks = 0;
                    *jump_duration = 0;
                }
            }
            Modern::Armadillo {
                in_state_ticks,
                peek_received,
                roll_up,
                roll_out,
                peek,
            } => {
                match p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) {
                    0 => {
                        roll_up.stop();
                        roll_out.stop();
                        peek.stop();
                    }
                    1 => {
                        roll_out.stop();
                        roll_up.start_if_stopped(input.age);
                        peek.stop();
                    }
                    3 => {
                        roll_out.start_if_stopped(input.age);
                        roll_up.stop();
                        peek.stop();
                    }
                    2 => {
                        roll_out.stop();
                        roll_up.stop();
                        if *peek_received {
                            peek.stop();
                            *peek_received = false;
                        }
                        if *in_state_ticks == 0 {
                            peek.start(input.age);
                            peek.fast_forward(50, 1.);
                        } else {
                            peek.start_if_stopped(input.age);
                        }
                    }
                    _ => {}
                }
                *in_state_ticks = in_state_ticks.wrapping_add(1);
            }
            Modern::Camel {
                idle_timeout,
                sit,
                sit_pose,
                sit_up,
                idle,
                dash,
            } => {
                if *idle_timeout <= 0 {
                    *idle_timeout = self.random.borrow_mut().next_int(40) as i32 + 80;
                    idle.start(input.age);
                } else {
                    *idle_timeout -= 1;
                }
                if input.camel_visually_sitting {
                    sit_up.stop();
                    dash.stop();
                    if input.camel_sitting_down {
                        sit.start_if_stopped(input.age);
                        sit_pose.stop();
                    } else {
                        sit.stop();
                        sit_pose.start_if_stopped(input.age);
                    }
                } else {
                    sit.stop();
                    sit_pose.stop();
                    dash.animate_when(p::boolean(models, entity, "DASH"), input.age);
                    sit_up.animate_when(
                        input.camel_pose_transition && input.camel_pose_time >= 0,
                        input.age,
                    );
                }
            }
            Modern::Creaking {
                attack,
                invulnerable,
                death,
                attack_remaining,
                invulnerable_remaining,
                eyes_glowing,
                next_flicker,
            } => {
                if *attack_remaining > 0 {
                    *attack_remaining -= 1;
                }
                if *invulnerable_remaining > 0 {
                    *invulnerable_remaining -= 1;
                }
                attack.animate_when(*attack_remaining > 0, input.age);
                invulnerable.animate_when(*invulnerable_remaining > 0, input.age);
                death.animate_when(p::boolean(models, entity, "IS_TEARING_DOWN"), input.age);
                if self.death_time > *next_flicker {
                    let low = if *eyes_glowing {
                        2
                    } else {
                        self.death_time / 4
                    };
                    let high = if *eyes_glowing {
                        8
                    } else {
                        self.death_time / 2
                    };
                    *next_flicker = self.death_time.wrapping_add(
                        low + self.random.borrow_mut().next_int((high - low + 1) as u32) as i32,
                    );
                    *eyes_glowing = !*eyes_glowing;
                }
            }
            _ => {}
        }
        if matches!(self.body_rule, BodyRule::Armadillo) && !body_enabled {
            self.head_yaw = self.body_yaw;
        }
        if matches!(self.body_rule, BodyRule::Camel) && !body_enabled {
            self.head_yaw = rotate_if_necessary(self.head_yaw, self.body_yaw, input.max_head_yaw);
        }
        self.position_old = input.position;
    }
}
/// minecraft.wiki: a creeper explodes 30 ticks after it starts to swell.
const CREEPER_FUSE: i32 = 30;
/// Mobs that walk with folded wings and flap only in midair (minecraft.wiki: chickens
/// "flap their wings when they are in midair"; parrots flap to fly and slow their falls).
/// BeeModel also folds its wings on the ground; bat hanging is owned by Modern::Bat.
pub(crate) fn folds_wings(kind: &str) -> bool {
    matches!(kind, "minecraft:chicken" | "minecraft:parrot" | "minecraft:bee")
}
pub(crate) fn wrap(angle: f32) -> f32 {
    let angle = angle % 360.;
    if angle >= 180. {
        angle - 360.
    } else if angle < -180. {
        angle + 360.
    } else {
        angle
    }
}
fn rotate_if_necessary(current: f32, target: f32, limit: f32) -> f32 {
    target - wrap(target - current).clamp(-limit, limit)
}
#[derive(Debug, Default)]
pub(crate) struct Avatar {
    pub velocity_old: [f64; 3],
    cloak: [f64; 3],
    cloak_old: [f64; 3],
    walk: f32,
    walk_old: f32,
    bob: f32,
    bob_old: f32,
}
impl Avatar {
    pub fn tick(&mut self, position: [f64; 3], velocity: [f64; 3]) {
        self.walk_old = self.walk;
        self.velocity_old = velocity;
        self.cloak_old = self.cloak;
        for i in 0..3 {
            let delta = position[i] - self.cloak[i];
            if delta > 10. || delta < -10. {
                self.cloak[i] = position[i];
                self.cloak_old[i] = position[i];
            } else {
                self.cloak[i] += delta * 0.25;
            }
        }
    }
    pub fn add_walk_distance(&mut self, added: f32) {
        self.walk += added;
    }
    pub fn update_bob(&mut self, target: f32) {
        self.bob_old = self.bob;
        self.bob += (target - self.bob) * 0.4;
    }
    pub fn reset_bob(&mut self) {
        self.bob_old = self.bob;
        self.bob = 0.;
    }
    pub fn cloak(&self, partial: f32) -> [f64; 3] {
        std::array::from_fn(|i| {
            self.cloak_old[i] + f64::from(partial) * (self.cloak[i] - self.cloak_old[i])
        })
    }
    pub fn bob(&self, partial: f32) -> f32 {
        self.bob_old + partial * (self.bob - self.bob_old)
    }
    pub fn walk(&self, partial: f32) -> f32 {
        self.walk_old + partial * (self.walk - self.walk_old)
    }
    pub fn backwards_walk(&self, partial: f32) -> f32 {
        -(self.walk + (self.walk - self.walk_old) * partial)
    }
}
