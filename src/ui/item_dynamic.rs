//! Stateful numeric item properties for a local-player presentation owner.
use crate::inventory::Stack;
use serde_json::Value;
use std::{collections::BTreeMap, sync::{Arc, Mutex}};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GlobalPos {
    pub dimension: String,
    pub pos: [i32; 3],
}

#[derive(Clone, Debug)]
pub(crate) struct ItemWorldContext {
    pub dimension: String,
    pub position: [f64; 3],
    pub yaw: f32,
    pub game_tick: i64,
    pub sun_angle: f32,
    pub moon_phase: f32,
    pub spawn: Option<GlobalPos>,
    pub death: Option<GlobalPos>,
    pub needles: Arc<Mutex<Needles>>,
}

#[derive(Debug, Default)]
pub(crate) struct Needles {
    states: BTreeMap<(&'static str, Option<GlobalPos>, bool), Needle>,
    random: u64,
}

#[derive(Debug, Default)]
struct Needle { tick: Option<i64>, rotation: f32, velocity: f32 }

impl Needle {
    fn sample(&mut self, tick: i64, target: f32, factor: f32, wobble: bool) -> f32 {
        if !wobble {
            self.rotation = target;
            return target;
        }
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            let difference = (target - self.rotation + 0.5).rem_euclid(1.) - 0.5;
            self.velocity = (self.velocity + difference * 0.1) * factor;
            self.rotation = (self.rotation + self.velocity).rem_euclid(1.);
        }
        self.rotation
    }
}

fn lodestone(stack: &Stack) -> Option<GlobalPos> {
    let target = stack.component("minecraft:lodestone_tracker")?.get("target")?;
    let p = target.get("pos")?.as_array()?;
    if p.len() != 3 { return None; }
    Some(GlobalPos {
        dimension: target.get("dimension")?.as_str()?.to_owned(),
        pos: std::array::from_fn(|i| p[i].as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(i32::MIN)),
    }).filter(|p| !p.pos.contains(&i32::MIN))
}

impl ItemWorldContext {
    pub(crate) fn number(&self, node: &Value, stack: &Stack, seed: i32) -> Option<f32> {
        let property = node.get("property")?.as_str()?.strip_prefix("minecraft:").unwrap_or(node["property"].as_str()?);
        let source = match property {
            "time" => node.get("source")?.as_str()?,
            "compass" => node.get("target")?.as_str()?,
            _ => return None,
        };
        let key_name = match (property, source) {
            ("time", "daytime") => "time/daytime",
            ("time", "moon_phase") => "time/moon_phase",
            ("time", "random") => "time/random",
            ("compass", "spawn") => "compass/spawn",
            ("compass", "lodestone") => "compass/lodestone",
            ("compass", "recovery") => "compass/recovery",
            ("compass", "none") => "compass/none",
            _ => return None,
        };
        let wobble = node.get("wobble").and_then(Value::as_bool).unwrap_or(true);
        let location = match (property, source) {
            ("compass", "spawn") => self.spawn.clone(),
            ("compass", "recovery") => self.death.clone(),
            ("compass", "lodestone") => lodestone(stack),
            ("compass", "none") | ("time", _) => None,
            _ => return None,
        };
        let valid = location.as_ref().filter(|target| {
            target.dimension == self.dimension && (0..3).map(|i| {
                let d = f64::from(target.pos[i]) + 0.5 - self.position[i]; d * d
            }).sum::<f64>() >= 1e-5
        });
        let mut needles = self.needles.lock().expect("item needle state poisoned");
        let key = (key_name, location.clone(), wobble);
        if needles.states.len() >= 256 && !needles.states.contains_key(&key) { needles.states.clear(); }
        let updating = !wobble || needles.states.get(&key).is_none_or(|state| state.tick != Some(self.game_tick));
        let random = if updating && ((property == "compass" && valid.is_none()) || (property == "time" && source == "random")) {
            needles.random = needles.random.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((needles.random >> 40) as f32) / 16777216.
        } else { 0. };
        let state = needles.states.entry(key).or_default();
        if property == "time" {
            let target = match source { "daytime" => self.sun_angle / 360., "moon_phase" => self.moon_phase / 8., "random" => random, _ => return None };
            return Some(state.sample(self.game_tick, target, 0.9, wobble));
        }
        if let Some(target) = valid {
            let dx = f64::from(target.pos[0]) + 0.5 - self.position[0];
            let dz = f64::from(target.pos[2]) + 0.5 - self.position[2];
            let angle = (dz.atan2(dx) / std::f64::consts::TAU) as f32;
            let facing = 0.75 - (self.yaw / 360.).rem_euclid(1.);
            Some((angle + state.sample(self.game_tick, facing, 0.8, wobble)).rem_euclid(1.))
        } else {
            let hash = seed.wrapping_mul(1327217883) as f32 / 2147483648.;
            Some((state.sample(self.game_tick, random, 0.8, wobble) + hash).rem_euclid(1.))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context() -> ItemWorldContext {
        ItemWorldContext {
            dimension: "minecraft:overworld".into(),
            position: [0.5, 64.5, 0.5],
            yaw: 0.,
            game_tick: 1,
            sun_angle: 90.,
            moon_phase: 2.,
            spawn: Some(GlobalPos { dimension: "minecraft:overworld".into(), pos: [10, 64, 0] }),
            death: None,
            needles: Arc::default(),
        }
    }

    #[test]
    fn local_time_uses_environment_angle_and_shared_tick_state() {
        let mut world = context();
        let clock = json!({"property":"minecraft:time","source":"daytime","wobble":false});
        let stack = Stack::default();
        assert_eq!(world.number(&clock, &stack, 0), Some(0.25));
        world.sun_angle = 270.;
        world.game_tick += 1;
        assert_eq!(world.number(&clock, &stack, 0), Some(0.75));
        let moon = json!({"property":"minecraft:time","source":"moon_phase","wobble":false});
        assert_eq!(world.number(&moon, &stack, 0), Some(0.25));
    }

    #[test]
    fn compass_tracks_yaw_and_per_stack_lodestones() {
        let mut world = context();
        let compass = json!({"property":"minecraft:compass","target":"spawn","wobble":false});
        let stack = Stack::default();
        let east = world.number(&compass, &stack, 0).unwrap();
        world.yaw = 90.;
        world.game_tick += 1;
        let west_view = world.number(&compass, &stack, 0).unwrap();
        assert!((east - 0.75).abs() < 1e-6);
        assert!((west_view - 0.5).abs() < 1e-6);
        let lodestone = json!({"property":"minecraft:compass","target":"lodestone","wobble":false});
        let mut a = Stack::default();
        let mut b = Stack::default();
        Arc::make_mut(&mut a.components).insert("minecraft:lodestone_tracker".into(), json!({"target":{"dimension":"minecraft:overworld","pos":[10,64,0]}}));
        Arc::make_mut(&mut b.components).insert("minecraft:lodestone_tracker".into(), json!({"target":{"dimension":"minecraft:overworld","pos":[0,64,10]}}));
        let a_angle = world.number(&lodestone, &a, 0).unwrap();
        let b_angle = world.number(&lodestone, &b, 0).unwrap();
        assert!((a_angle - b_angle).abs() > 0.2);
    }

    #[test]
    fn absent_or_wrong_dimension_spins_and_wobble_is_tick_fenced() {
        let mut world = context();
        world.spawn.as_mut().unwrap().dimension = "minecraft:the_nether".into();
        let compass = json!({"property":"minecraft:compass","target":"spawn"});
        let stack = Stack::default();
        let first = world.number(&compass, &stack, 0).unwrap();
        assert_eq!(world.number(&compass, &stack, 0), Some(first));
        world.game_tick += 1;
        assert_ne!(world.number(&compass, &stack, 0), Some(first));
    }
}
