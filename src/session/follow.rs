//! Filming (`--follow-player NAME`): a spectator camera over a named player's shoulder.
//! Every frame the camera samples the target's interpolated received pose, so it moves
//! as smoothly as that player's own model; the local spectator body trails it each
//! client tick so the server keeps streaming the chunks and entities around him.
use super::*;

/// Behind, above and right of the target's eye, in blocks (about a four-block boom).
const BACK: f64 = 3.5;
const UP: f64 = 1.8;
const RIGHT: f64 = 0.7;
/// Downward look past the target toward the front, degrees.
const PITCH: f32 = 14.;
/// Yaw smoothing time constant in client ticks (half a second).
const YAW_TICKS: f64 = 10.;

pub(super) struct Follow {
    name: String,
    /// Client time (ticks plus partial) and the smoothed yaw at that time.
    yaw: Option<(f64, f32)>,
}

impl Follow {
    /// The followed player's entity, resolved by name through the received player
    /// list and UUID on every call: a respawn or a new entity id is picked up at once.
    fn entity<'a>(&self, entities: &'a crate::entities::Store) -> Option<&'a crate::entities::Entity> {
        let profile = entities.profiles.values().find(|profile| profile.name == self.name)?;
        entities.uuid_entities.get(&profile.uuid)?.iter()
            .filter_map(|id| entities.get(*id))
            .find(|entity| entity.type_name == "minecraft:player")
    }

    /// Each client tick the local spectator trails the camera, so the server streams
    /// the area around the target. Spectators only: they fly through blocks anyway.
    pub(super) fn trail(&self, entities: &crate::entities::Store, player: &mut crate::player::Player) {
        if player.game_mode != 3 {
            return;
        }
        let Some(entity) = self.entity(entities) else { return };
        let yaw = self.yaw.map_or(entity.transform.head_yaw, |(_, yaw)| yaw);
        let offset = boom(yaw);
        player.position = std::array::from_fn(|i| entity.position[i] + offset[i]);
        player.velocity = [0.; 3];
        player.yaw = yaw;
        player.pitch = PITCH;
    }
}

impl Session {
    pub(crate) fn set_follow_player(&mut self, name: Option<String>) {
        self.follow = name.map(|name| Follow { name, yaw: None });
    }

    /// Camera over the followed player's shoulder, pulled in before terrain; None
    /// while he is not received (the spectator's own view stays until he is).
    pub(super) fn follow_camera(&mut self) -> Option<([f64; 3], f32, f32)> {
        let entity = self.follow.as_ref()?.entity(&self.entities)?;
        let (eye, head_yaw, _) = self.entities.eye_view(entity, self.world_tick, self.presentation_partial).ok()?;
        let now = self.client_tick as f64 + f64::from(self.local_partial);
        let follow = self.follow.as_mut()?;
        let yaw = match follow.yaw {
            Some((then, previous)) if now >= then => {
                let blend = 1. - (-(now - then) / YAW_TICKS).exp();
                previous + wrap(head_yaw - previous) * blend as f32
            }
            Some((_, previous)) => previous,
            None => head_yaw,
        };
        follow.yaw = Some((now, wrap(yaw)));
        let offset = boom(yaw);
        let length = offset.iter().map(|v| v * v).sum::<f64>().sqrt();
        let direction = offset.map(|v| v / length);
        let distance = self.content.as_ref().map_or(length, |content| {
            crate::player::camera_distance(&self.world, content, eye, direction).min(length)
        });
        Some((std::array::from_fn(|i| eye[i] + direction[i] * distance), wrap(yaw), PITCH))
    }
}

/// World offset from the target's eye: back along his yaw, up, and to his right.
fn boom(yaw: f32) -> [f64; 3] {
    let radians = f64::from(yaw).to_radians();
    let forward = [-radians.sin(), 0., radians.cos()];
    let right = [-radians.cos(), 0., -radians.sin()];
    std::array::from_fn(|i| -forward[i] * BACK + right[i] * RIGHT + if i == 1 { UP } else { 0. })
}

fn wrap(angle: f32) -> f32 {
    (angle + 180.).rem_euclid(360.) - 180.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boom_is_behind_above_and_right_of_the_look() {
        // Yaw 0 faces +Z: behind is -Z, the right hand is -X.
        let offset = boom(0.);
        assert!((offset[2] + BACK).abs() < 1e-9 && (offset[1] - UP).abs() < 1e-9 && (offset[0] + RIGHT).abs() < 1e-9);
        // Yaw -90 faces +X: behind is -X.
        assert!((boom(-90.)[0] + BACK).abs() < 1e-9);
        assert_eq!(wrap(190.), -170.);
    }
}
