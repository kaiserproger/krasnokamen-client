//! Local presentation only: never feeds camera offsets back into physics or raycasts.
use super::*;
use crate::{
    api::EntityMaterial,
    inventory::Stack,
    motion::{self, HandPose, ViewInput, ViewMotion},
    ui::items::{display_matrix, ItemContext, ItemModels},
};
use glam::{Mat4, Vec3};
use std::cell::Cell;

pub(crate) struct FirstPersonFrame<'a> {
    pub partial: f32,
    pub light: [f32; 2],
    pub item_context: &'a ItemContext,
    pub ui: Option<&'a crate::ui::UiState>,
}

fn held(inventory: &InventoryStore, hand: u8) -> Option<&Stack> {
    interaction::held(inventory, hand).filter(|stack| !stack.empty())
}

/// Camera-relative held-map placement; the two arms do not follow map tilt.
struct MapPose {
    arms: Option<Mat4>,
    map: Mat4,
}

fn map_pose(two_handed: bool, side: f32, pitch: f32, equip: f32, swing: f32) -> MapPose {
    let swing = swing.clamp(0., 1.);
    let root = swing.sqrt();
    let wave = (root * std::f32::consts::PI).sin();
    if two_handed {
        let tilt = 0.5 - 0.5 * ((1.1 - pitch / 45.).clamp(0., 1.) * std::f32::consts::PI).cos();
        let lift = 0.1 * (swing * std::f32::consts::PI).sin();
        let arms = Mat4::from_translation(Vec3::new(0., lift, -0.4 * wave))
            * Mat4::from_translation(Vec3::new(0., 0.04 - (1. - equip) * 1.2 - tilt * 0.5, -0.72))
            * Mat4::from_rotation_x((-85. * tilt).to_radians());
        MapPose {
            arms: Some(arms * Mat4::from_rotation_y(90f32.to_radians())),
            map: arms * Mat4::from_rotation_x((20. * wave).to_radians()) * Mat4::from_scale(Vec3::splat(2.)),
        }
    } else {
        MapPose {
            arms: None,
            map: Mat4::from_translation(Vec3::new(side * 0.125, -0.125, 0.))
                * Mat4::from_translation(Vec3::new(side * 0.51, -0.08 - (1. - equip) * 1.2, -0.75))
                * Mat4::from_translation(Vec3::new(
                    side * -0.5 * wave,
                    0.4 * (root * std::f32::consts::TAU).sin() - 0.3 * wave,
                    -0.3 * (swing * std::f32::consts::PI).sin(),
                ))
                * Mat4::from_rotation_x((-45. * wave).to_radians())
                * Mat4::from_rotation_y((-30. * side * wave).to_radians()),
        }
    }
}

/// Map-pixel space (128x128, +y down, layers toward -z) placed on a held pose.
fn map_surface_transform(pose: Mat4) -> Mat4 {
    pose * Mat4::from_rotation_y(std::f32::consts::PI)
        * Mat4::from_rotation_z(std::f32::consts::PI)
        * Mat4::from_scale(Vec3::splat(0.38))
        * Mat4::from_translation(Vec3::new(-0.5, -0.5, 0.))
        * Mat4::from_scale(Vec3::splat(1. / 128.))
}

pub(super) struct FirstPerson {
    initialized: bool,
    previous_position: [f64; 3],
    old_eye_height: f64,
    eye_height: f64,
    old_phase: f32,
    phase: f32,
    old_amplitude: f32,
    amplitude: f32,
    old_fov: f32,
    fov: f32,
    swing_age: Cell<u8>,
    swing_hand: Cell<u8>,
    old_equip: f32,
    equip: f32,
    shown: Option<Stack>,
    items: ItemModels,
    motion: ViewMotion,
    last_time: Option<f64>,
    was_grounded: bool,
    fall_peak: f32,
    last_vy: f32,
    consuming: bool,
    consume_hand: u8,
    consume_drink: bool,
    swing_use: Cell<bool>,
    bump_pending: Cell<bool>,
}
impl Default for FirstPerson {
    fn default() -> Self {
        Self {
            initialized: false,
            previous_position: [0.; 3],
            old_eye_height: 1.62,
            eye_height: 1.62,
            old_phase: 0.,
            phase: 0.,
            old_amplitude: 0.,
            amplitude: 0.,
            old_fov: 1.,
            fov: 1.,
            swing_age: Cell::new(6),
            swing_hand: Cell::new(0),
            old_equip: 1.,
            equip: 1.,
            shown: None,
            items: ItemModels::new(),
            motion: ViewMotion::default(),
            last_time: None,
            was_grounded: false,
            fall_peak: 0.,
            last_vy: 0.,
            consuming: false,
            consume_hand: 0,
            consume_drink: false,
            swing_use: Cell::new(false),
            bump_pending: Cell::new(false),
        }
    }
}

pub(crate) struct View {
    pub eye: [f64; 3],
    pub bob: [f32; 2],
    pub pitch_bob: f32,
    pub fov: f32,
    /// Landing dip in blocks (positive = camera lower), render only.
    pub dip: f32,
    /// Subtle strafe roll in degrees, render only.
    pub roll_deg: f32,
    swing: f32,
    equip: f32,
    consume: f32,
    hand: HandPose,
}
impl FirstPerson {
    pub(super) fn swing(&self, hand: u8) {
        // Mining calls swing every tick. Let each six-tick stroke reach its
        // midpoint before accepting a restart, instead of freezing the fist.
        if self.swing_age.get() >= 3 {
            self.swing_age.set(0);
            self.swing_hand.set(hand);
            self.swing_use.set(false);
        }
    }
    /// Placing or using an item: a smaller poke plus a springy hand bump.
    pub(super) fn use_bump(&self) {
        self.swing_use.set(true);
        self.bump_pending.set(true);
    }
    pub(super) fn reset(&mut self) {
        *self = Self {
            items: std::mem::take(&mut self.items),
            ..Self::default()
        };
    }
    fn sample(&self, player: &Player, partial: f32) -> View {
        let partial = partial.clamp(0., 1.);
        let mix = |a: f32, b: f32| a + (b - a) * partial;
        let phase = mix(self.old_phase, self.phase) * std::f32::consts::PI;
        let amplitude = mix(self.old_amplitude, self.amplitude);
        let eye = player.eye_position();
        let height =
            self.old_eye_height + (self.eye_height - self.old_eye_height) * f64::from(partial);
        View {
            eye: if self.initialized {
                std::array::from_fn(|i| {
                    self.previous_position[i]
                        + (player.position[i] - self.previous_position[i]) * f64::from(partial)
                        + if i == 1 { height } else { 0. }
                })
            } else {
                eye
            },
            bob: [
                phase.sin() * amplitude * 0.035,
                -phase.cos().abs() * amplitude * 0.04,
            ],
            pitch_bob: (phase * 2.).cos() * amplitude * 0.5,
            fov: mix(self.old_fov, self.fov),
            swing: if self.swing_age.get() < 6 {
                ((f32::from(self.swing_age.get()) + partial) / 6.).min(1.)
            } else {
                0.
            },
            equip: mix(self.old_equip, self.equip),
            dip: self.motion.dip.x,
            roll_deg: self.motion.roll.x,
            consume: self.motion.consume,
            hand: self.motion.hand(amplitude, player.yaw, player.pitch),
        }
    }
}
impl Player {
    pub(super) fn visual_tick(&mut self, previous: [f64; 3], inventory: &InventoryStore) {
        let target_height = self.eye_position()[1] - self.position[1];
        // Eating/drinking pose follows the held consumable's own animation kind.
        let consuming = self.using.as_ref().and_then(|using| {
            let animation = held(inventory, using.hand)?
                .component("minecraft:consumable")?
                .get("animation")
                .and_then(Value::as_str)
                .unwrap_or("eat");
            matches!(animation, "eat" | "drink").then_some((using.hand, animation == "drink"))
        });
        let grounded_motion =
            self.vehicle.is_none() && !self.abilities.flying && !self.swimming && !self.gliding;
        let v = &mut self.visual;
        if let Some((hand, drink)) = consuming {
            v.consume_hand = hand;
            v.consume_drink = drink;
        }
        v.consuming = consuming.is_some();
        // Landing dip scales with the fall; the airborne peak is kept because
        // the player's own counter has already been cleared on touchdown.
        if !self.grounded {
            v.fall_peak = v.fall_peak.max(self.fall_distance as f32);
        } else if !v.was_grounded && grounded_motion {
            v.motion.land(v.fall_peak + (-v.last_vy).max(0.));
        }
        if self.grounded {
            v.fall_peak = 0.;
        }
        v.was_grounded = self.grounded;
        v.last_vy = self.velocity[1] as f32;
        if !v.initialized {
            v.eye_height = target_height;
            v.old_eye_height = target_height;
            v.initialized = true;
        }
        v.previous_position = previous;
        v.old_eye_height = v.eye_height;
        v.eye_height += (target_height - v.eye_height) * 0.32;
        v.old_phase = v.phase;
        v.old_amplitude = v.amplitude;
        let distance = ((self.position[0] - previous[0]).powi(2)
            + (self.position[2] - previous[2]).powi(2))
        .sqrt() as f32;
        let walking = self.grounded && self.vehicle.is_none() && !self.abilities.flying;
        v.amplitude += ((if walking { distance * 4. } else { 0. }).min(1.) - v.amplitude) * 0.4;
        if walking {
            v.phase += distance.min(1.) * 1.6;
        }
        v.old_fov = v.fov;
        v.fov += ((if self.sprinting { 1.075 } else { 1. }) - v.fov) * 0.35;
        v.swing_age.set(v.swing_age.get().saturating_add(1).min(6));
        v.old_equip = v.equip;
        let held = held(inventory, 0);
        let same = match (&v.shown, held) {
            (Some(a), Some(b)) => a.id == b.id && a.components == b.components,
            (None, None) => true,
            _ => false,
        };
        v.equip += ((if same { 1. } else { 0. }) - v.equip).clamp(-0.4, 0.4);
        if v.equip < 0.1 {
            v.shown = held.cloned();
        }
    }

    /// Render-only camera/hand motion state for evidence; never fed back.
    pub(crate) fn visual_motion_observation(&self) -> serde_json::Value {
        let m = &self.visual.motion;
        let view = self.visual.sample(self, 0.);
        serde_json::json!({
            "landing_dip": m.dip.x, "strafe_roll_deg": m.roll.x, "use_bump": m.bump.x,
            "eat_blend": m.consume, "hand_offset": view.hand.offset, "hand_roll_deg": view.hand.roll_deg,
            "swing_is_use": self.visual.swing_use.get(),
        })
    }
    pub(crate) fn visual_view(&self, partial: f32) -> View {
        self.visual.sample(self, partial)
    }

    /// Advance the frame-time springs to `ticks` (client tick plus partial).
    /// Called once per rendered frame; time never runs backwards or double-counts.
    pub(crate) fn visual_animate(&mut self, ticks: f64) {
        let grounded = self.grounded && self.vehicle.is_none();
        // Steering a mount is not a body lean.
        let strafe = if self.vehicle.is_some() {
            0.
        } else {
            self.last_input.strafe()
        };
        let (yaw, pitch) = (self.yaw, self.pitch);
        let v = &mut self.visual;
        let dt = match v.last_time {
            Some(last) => ((ticks - last).max(0.) * 0.05) as f32,
            None => 0.,
        };
        v.last_time = Some(v.last_time.map_or(ticks, |last| last.max(ticks)));
        if v.bump_pending.take() {
            v.motion.bump_hand();
        }
        v.motion.step(
            dt,
            &ViewInput {
                yaw,
                pitch,
                strafe,
                grounded,
                consume: if v.consuming { 1. } else { 0. },
            },
        );
    }

    pub(crate) fn visual_swing(&self, tick: u64) -> Option<(u8, u64)> {
        let age = self.visual.swing_age.get();
        (age < 6).then(|| {
            (
                if self.visual.swing_hand.get() == 0 {
                    0
                } else {
                    3
                },
                tick.saturating_sub(u64::from(age)),
            )
        })
    }

    pub(crate) fn clear_visual_resources(&mut self) {
        self.visual.items.clear();
    }

    pub(crate) fn set_visual_models(&mut self, models: Arc<crate::entities::Models>) {
        self.visual.items.set_entity_models(models);
    }

    pub(crate) fn append_first_person(
        &mut self,
        content: &Content,
        inventory: &InventoryStore,
        entities: &mut EntityStore,
        scene: &mut crate::api::Scene,
        frame: FirstPersonFrame<'_>,
    ) -> Result<()> {
        let FirstPersonFrame { partial, light, item_context, ui } = frame;
        if self.game_mode == 3 || !self.pose_known || self.health.is_some_and(|h| h <= 0.) {
            return Ok(());
        }
        let view = self.visual.sample(self, partial);
        let eye = Vec3::from_array(std::array::from_fn(|i| {
            (scene.camera_position[i] - scene.origin[i]) as f32
        }));
        let camera_to_world = crate::render::camera_view(eye, scene.yaw, scene.pitch).inverse();
        for hand in 0..=1 {
            let stack = if hand == 0 {
                self.visual.shown.as_ref()
            } else {
                held(inventory, 1)
            };
            if hand == 1 && stack.is_none() {
                continue;
            }
            let right = self.main_arm_right != (hand == 1);
            let side = if right { 1. } else { -1. };
            let stroke = if self.visual.swing_hand.get() == hand {
                // Quick strike, slower recovery; placing/using is a smaller poke.
                motion::swing_stroke(view.swing)
                    * if self.visual.swing_use.get() {
                        0.55
                    } else {
                        1.
                    }
            } else {
                0.
            };
            let using = self.using.as_ref().filter(|u| u.hand == hand);
            let use_ticks = using.map_or(0., |u| (u.duration - u.remaining) as f32 + partial);
            let consume = if self.visual.consume_hand == hand {
                motion::smoothstep(view.consume)
            } else {
                0.
            };
            let use_bob = if using.is_some() && consume < 0.01 {
                (use_ticks * 1.8).sin() * 0.025
            } else {
                (use_ticks * 1.9).cos().abs() * 0.03 * consume
            };
            let equip = if hand == 0 {
                motion::smoothstep(view.equip)
            } else {
                1.
            };
            let [hx, hy, hz] = view.hand.offset;
            // `base` is the rest placement of the hand space in front of the camera
            // (right, up, forward) and `drop` the lowered-hand travel while equipping.
            // The swing, consume and view-motion deltas are shared by arm and item.
            let hand_root = |base: Vec3, drop: f32| {
                camera_to_world
                    * Mat4::from_translation(Vec3::new(
                        side * (base.x - stroke * 0.16) * (1. - 0.65 * consume) + hx,
                        base.y - (1. - equip) * drop
                            + stroke * 0.1
                            + view.bob[1]
                            + use_bob
                            + hy
                            + consume * 0.1,
                        base.z + stroke * 0.12 + hz + consume * 0.12,
                    ))
                    * Mat4::from_rotation_z(view.hand.roll_deg.to_radians())
                    * Mat4::from_rotation_y(side * (-stroke * 0.8))
                    * Mat4::from_rotation_x(
                        -stroke * 0.9
                            - (1. - equip) * 0.45
                            - consume * if self.visual.consume_drink { 0.7 } else { 0.35 },
                    )
            };
            // A received filled map is a surface, not its inventory sprite.
            if let Some(value) = stack.and_then(|stack| stack.component("minecraft:map_id")) {
                let id = value
                    .as_i64()
                    .and_then(|id| i32::try_from(id).ok())
                    .filter(|id| *id >= 0)
                    .ok_or("invalid held map id")?;
                let ui = ui.ok_or("held map UI state unavailable")?;
                let swing = if self.visual.swing_hand.get() == hand { view.swing } else { 0. };
                let pose = map_pose(hand == 0 && held(inventory, 1).is_none(), side, scene.pitch, equip, swing);
                let sway = camera_to_world
                    * Mat4::from_translation(Vec3::new(hx, view.bob[1] + hy, hz))
                    * Mat4::from_rotation_z(view.hand.roll_deg.to_radians());
                if let Some(arms) = pose.arms {
                    for arm_right in [true, false] {
                        let f: f32 = if arm_right { 1. } else { -1. };
                        let shoulder = sway * arms
                            * Mat4::from_rotation_y(92f32.to_radians())
                            * Mat4::from_rotation_x(45f32.to_radians())
                            * Mat4::from_rotation_z((-41. * f).to_radians())
                            * Mat4::from_translation(Vec3::new(0.3 * f, -1.1, 0.45))
                            * Mat4::from_translation(Vec3::new(-5. / 16. * f, 2. / 16., 0.));
                        entities.append_local_arm(content, arm_right, shoulder, light, &mut scene.first_person)?;
                    }
                } else {
                    let shoulder = hand_root(Vec3::new(0.38, -0.32, -0.65), 0.55)
                        * Mat4::from_translation(Vec3::new(side * 0.14, -0.58, 0.))
                        * Mat4::from_rotation_z(side * 0.25)
                        * Mat4::from_rotation_x(-0.15);
                    entities.append_local_arm(content, right, shoulder, light, &mut scene.first_person)?;
                }
                ui.append_held_map(id, content, map_surface_transform(sway * pose.map), light, &mut scene.first_person)?;
                continue;
            }
            // Ordinary item graphs and resource-pack first-person transforms;
            // the equipped stack is switched only while the hand is lowered.
            let held_model = match stack {
                Some(stack) => {
                    let display = if right {
                        "firstperson_righthand"
                    } else {
                        "firstperson_lefthand"
                    };
                    let ctx = ItemContext {
                        display: display.into(),
                        using_item: using.is_some(),
                        use_ticks,
                        crossbow_pull: using.map_or(0., |u| u.crossbow_pull(partial)),
                        fishing_cast: self.fishing_hook.is_some(),
                        ..item_context.clone()
                    };
                    Some((display, self.visual.items.model_with(content, stack, &ctx)))
                }
                None => None,
            };
            let (item, failure) = match held_model {
                None => (None, None),
                Some((display, Ok(model))) => (Some((display, model)), None),
                Some((_, Err(reason))) => (None, Some(reason)),
            };
            let Some((display, model)) = item else {
                // Empty hand (or an item without geometry): the bare arm; a
                // model failure is still reported.
                // Camera-space rest pose fitted to the Java oracle's arm
                // silhouette at FOV 70 (villagers-side, 1280x720). These are
                // our rounded fit parameters, not source-model transforms.
                // Fit the runtime skin's visible faces as well as the outline.
                let shoulder = hand_root(Vec3::new(0.515, -0.812, -0.676), 0.55)
                    * Mat4::from_rotation_z(side * -2.015)
                    * Mat4::from_rotation_y(side * 0.697)
                    * Mat4::from_rotation_x(-1.453);
                entities.append_local_arm(content, right, shoulder, light, &mut scene.first_person)?;
                match failure {
                    Some(reason) => return Err(reason),
                    None => continue,
                }
            };
            // A held item replaces the arm (the source draws no arm for a non-empty
            // hand): the item's own display transform is applied once, from the
            // source's first-person hand anchor, with no extra rotation or scale.
            let matrix =
                hand_root(Vec3::new(0.56, -0.52, -0.72), 0.6) * display_matrix(model, display);
            let foil = match stack {
                Some(stack) => crate::ui::items::foil_texture(content, stack)?,
                None => None,
            };
            for quad in &model.quads {
                let color = match quad.tint {
                    crate::protocol::models::TintKind::Constant(color) => color,
                    _ => [1.; 3],
                };
                let first = scene.first_person.vertices.len();
                crate::entities::presentation_emit(
                    quad.positions
                        .map(|p| matrix.transform_point3(Vec3::from_array(p)).to_array()),
                    quad.uv,
                    color,
                    quad.texture_layer,
                    quad.entity_material.unwrap_or(if quad.layer == crate::api::RenderLayer::Translucent {
                        EntityMaterial::ItemTranslucent
                    } else {
                        EntityMaterial::ItemCutout
                    }),
                    quad.alpha,
                    light,
                    &mut scene.first_person,
                )?;
                if let Some(texture) = foil
                    && (quad.entity_material.is_none() || matches!(quad.entity_material,
                        Some(EntityMaterial::ItemCutout | EntityMaterial::ItemTranslucent)))
                {
                    crate::ui::items::mark_foil(&mut scene.first_person.vertices[first..], texture);
                }
            }
        }
        Ok(())
    }
}

/// Pull the four-block third-person boom in against real received collision
/// shapes. A small swept camera box also keeps the near plane outside corners.
pub(crate) fn camera_distance(
    world: &World,
    content: &Content,
    eye: [f64; 3],
    direction: [f64; 3],
) -> f64 {
    let mut distance = 4.;
    let end: [f64; 3] = std::array::from_fn(|i| eye[i] + direction[i] * distance);
    let min: [i32; 3] = std::array::from_fn(|i| (eye[i].min(end[i]) - 0.15).floor() as i32);
    let max: [i32; 3] = std::array::from_fn(|i| (eye[i].max(end[i]) + 0.15).floor() as i32);
    for y in min[1]..=max[1] {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                let cell = [x, y, z];
                let Some(state) = world.block(cell) else {
                    continue;
                };
                let Some(shapes) = content.collision_boxes(state) else {
                    continue;
                };
                let offset = content.block_offset(state, cell);
                for shape in shapes {
                    let mut bounds = Bounds::new(*shape)
                        .moved(std::array::from_fn(|i| f64::from(cell[i]) + offset[i]));
                    bounds.min = bounds.min.map(|v| v - 0.15);
                    bounds.max = bounds.max.map(|v| v + 0.15);
                    if let Some((hit, _, _)) = collision::ray_box(eye, direction, bounds, distance)
                    {
                        distance = distance.min((hit - 0.05).max(0.));
                    }
                }
            }
        }
    }
    distance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_map_pose_distinguishes_two_hands_one_hand_pitch_and_swing() {
        let center = |pose: &MapPose| {
            map_surface_transform(pose.map).transform_point3(Vec3::new(64., 64., 0.))
        };
        let two = map_pose(true, 1., 0., 1., 0.);
        let arms = two.arms.expect("an empty off hand holds the map with both arms");
        assert!(center(&two).x.abs() < 1e-5 && center(&two).z < 0.);
        assert!((arms.transform_vector3(Vec3::X).length() - 1.).abs() < 1e-5);
        assert!((two.map.transform_vector3(Vec3::X).length() - 2.).abs() < 1e-5);
        let left = map_pose(false, -1., 0., 1., 0.);
        let right = map_pose(false, 1., 0., 1., 0.);
        assert!(left.arms.is_none() && right.arms.is_none());
        assert!((center(&left).x + center(&right).x).abs() < 1e-5);
        assert!(center(&left).x < 0. && center(&right).x > 0.);
        let pitched = map_pose(true, 1., 45., 1., 0.);
        assert!((center(&pitched) - center(&two)).length() > 0.01);
        let lowered = map_pose(true, 1., 0., 0., 0.);
        assert!(center(&lowered).y < center(&two).y - 1.);
        let swinging = map_pose(true, 1., 0., 1., 0.5);
        assert!((swinging.arms.unwrap().z_axis - arms.z_axis).length() < 1e-5);
        assert!((swinging.map.z_axis - two.map.z_axis).length() > 0.1);
    }

    #[test]
    fn first_person_air_stacks_are_empty_hands_not_equipped_models() {
        let mut inventory = InventoryStore::default();
        inventory.player.slots[36] = Some(Stack::default());
        inventory.player.slots[45] = Some(Stack::default());
        assert!(held(&inventory, 0).is_none());
        assert!(held(&inventory, 1).is_none());
        let mut player = Player::new();
        player.visual_tick(player.position, &inventory);
        assert!(player.visual.shown.is_none());
        assert_eq!(player.visual.equip, 1.);
    }

    #[test]
    fn first_person_third_person_boom_pulls_in_before_received_wall() {
        use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};
        let palette = |size| Palette {
            bits: 0,
            values: vec![0],
            words: vec![],
            size,
        };
        let mut world = World::new(0, 16);
        world
            .insert(
                [0, 0],
                Chunk {
                    sections: vec![Arc::new(Section {
                        blocks: palette(4096),
                        biomes: palette(64),
                        non_empty_block_count: 0,
                    })],
                    sky: BTreeMap::new(),
                    block: BTreeMap::new(),
                    block_entities: Arc::new(vec![]),
                    surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        let content = Content::test_cube();
        let eye = [2.5, 2.5, 2.5];
        let direction = [0., 0., 1.];
        assert_eq!(camera_distance(&world, &content, eye, direction), 4.);
        world.set([2, 2, 4], 1, |s| Ok(s == 0)).unwrap();
        assert!((camera_distance(&world, &content, eye, direction) - 1.3).abs() < 1e-6);
        // A near-plane corner also must not pass through the neighboring wall.
        assert!(camera_distance(&world, &content, [3.1, 2.5, 2.5], direction) < 1.5);
    }

    #[test]
    fn first_person_walk_bobs_only_with_grounded_horizontal_motion() {
        let mut player = Player::new();
        player.pose_known = true;
        player.grounded = true;
        player.visual_tick([0.; 3], &InventoryStore::default());
        let idle = player.visual.sample(&player, 1.);
        player.position[0] = 0.2;
        player.visual_tick([0.; 3], &InventoryStore::default());
        let walking = player.visual.sample(&player, 1.);
        assert_ne!(idle.bob, walking.bob);
        assert!(walking.bob[1].abs() > 0.);
        player.grounded = false;
        for _ in 0..30 {
            player.visual_tick(player.position, &InventoryStore::default());
        }
        assert!(player.visual.sample(&player, 1.).bob[1].abs() < 0.0001);
    }

    #[test]
    fn first_person_sneak_and_sprint_are_smoothed_and_render_only() {
        let mut player = Player::new();
        player.pose_known = true;
        player.visual_tick(player.position, &InventoryStore::default());
        player.pose = Pose::Crouching;
        player.sprinting = true;
        let physical_eye = player.eye_position();
        player.visual_tick(player.position, &InventoryStore::default());
        let sample = player.visual.sample(&player, 1.);
        assert!(sample.eye[1] > physical_eye[1]);
        assert!(sample.eye[1] < 1.62);
        assert!(sample.fov > 1. && sample.fov < 1.075);
        assert_eq!(player.eye_position(), physical_eye);
    }

    #[test]
    fn first_person_swing_uses_ordinary_packet_path_and_finishes() {
        let mut player = Player::new();
        let inventory = InventoryStore::default();
        let mut packets = Vec::new();
        player.swing(0, &mut packets);
        assert_eq!(packets[0].id, 46);
        player.visual_tick(player.position, &inventory);
        assert!(player.visual.sample(&player, 0.5).swing > 0.);
        // Repeated block-hit swings must not pin the animation at its first frame.
        player.swing(0, &mut packets);
        player.visual_tick(player.position, &inventory);
        assert!(player.visual.sample(&player, 0.5).swing > 0.2);
        for _ in 0..8 {
            player.visual_tick(player.position, &inventory);
        }
        assert_eq!(player.visual.sample(&player, 1.).swing, 0.);
    }

    #[test]
    fn first_person_landing_dips_camera_in_proportion_to_the_fall() {
        let landing_peak = |fall: f64| {
            let mut player = Player::new();
            player.pose_known = true;
            let inventory = InventoryStore::default();
            player.visual_animate(0.);
            player.grounded = false;
            player.fall_distance = fall;
            player.visual_tick(player.position, &inventory);
            player.grounded = true;
            player.fall_distance = 0.;
            player.visual_tick(player.position, &inventory);
            let mut peak = 0f32;
            for frame in 1..=60 {
                player.visual_animate(f64::from(frame) / 6.);
                peak = peak.max(player.visual.sample(&player, 0.).dip);
            }
            peak
        };
        let hop = landing_peak(1.25);
        let drop = landing_peak(6.);
        assert!(hop > 0.04, "{hop}");
        assert!(drop > hop * 1.5 && drop <= 0.28, "{drop} vs {hop}");
        assert_eq!(landing_peak(0.1), 0.);
    }

    #[test]
    fn first_person_use_bump_is_a_small_poke_and_springs_back() {
        let mut player = Player::new();
        player.pose_known = true;
        let mut packets = Vec::new();
        player.visual_animate(0.);
        player.swing(0, &mut packets);
        player.visual.use_bump();
        assert!(player.visual.swing_use.get());
        player.visual_animate(0.2);
        assert!(player.visual.sample(&player, 0.).hand.offset[2] < 0.);
        for frame in 1..=40 {
            player.visual_animate(0.2 + f64::from(frame));
        }
        assert!(player.visual.sample(&player, 0.).hand.offset[2].abs() < 0.001);
        // The next ordinary attack is a full swing again.
        for _ in 0..8 {
            player.visual_tick(player.position, &InventoryStore::default());
        }
        player.swing(0, &mut packets);
        assert!(!player.visual.swing_use.get());
    }

    #[test]
    fn first_person_strafe_tilt_is_render_only_and_time_based() {
        let mut player = Player::new();
        player.pose_known = true;
        player.grounded = true;
        player.last_input = HeldInput {
            flags: 1 << 2,
            ..Default::default()
        };
        let eye = player.eye_position();
        player.visual_animate(0.);
        for frame in 1..=20 {
            player.visual_animate(f64::from(frame));
        }
        let view = player.visual.sample(&player, 0.);
        assert!((view.roll_deg + 1.2).abs() < 0.05, "{}", view.roll_deg);
        assert_eq!(player.eye_position(), eye);
        // No time passing means no animation: the same instant is idempotent.
        let before = player.visual.motion.roll.x;
        player.visual_animate(5.);
        assert_eq!(player.visual.motion.roll.x, before);
    }

    #[test]
    fn first_person_tools_in_both_hands_stay_out_of_world_geometry_during_swing_and_use() {
        let mut content = Content::test_cube();
        content.item_names.insert(7, "test:held".into());
        let mut stack = Stack::default();
        stack.id = 7;
        stack.count = 1;
        let mut inventory = InventoryStore::default();
        inventory.player.slots[36] = Some(stack.clone());
        inventory.player.slots[45] = Some(stack.clone());
        let mut player = Player::new();
        player.pose_known = true;
        player.visual.shown = Some(stack);
        player.visual.items.insert_test_model("test:held", content.models[0].clone());
        for (yaw, pitch, using) in [(0., 0., false), (90., 60., false), (-90., -60., true)] {
            player.visual.swing_age.set(2);
            player.using = using.then_some(super::super::interaction::UsingItem {
                hand: 0, item_id: 7, slot_identity: 0, duration: 32, remaining: 16,
                crossbow_charge_ticks: 0., crossbow_components: None,
                movement_factor: 1., can_sprint: false,
            });
            let mut scene = crate::api::Scene {
                yaw, pitch, origin: [4096., 0., -4096.],
                camera_position: [4096.5, 65.62, -4095.5],
                ..Default::default()
            };
            player.append_first_person(&content, &inventory, &mut EntityStore::new(), &mut scene, crate::player::FirstPersonFrame { partial: 0.5, light: [15., 0.], item_context: &ItemContext::default(), ui: None }).unwrap();
            assert_eq!(scene.first_person.vertices.len(), 48);
            assert_eq!(scene.first_person.indices.iter().map(Vec::len).sum::<usize>(), 72);
            assert!(scene.first_person.vertices.iter().flat_map(|v| v.position).all(f32::is_finite));
            assert!(scene.dynamic.vertices.is_empty(), "a camera-held item became a world occluder/caster");
            assert!(scene.dynamic.indices.iter().all(Vec::is_empty));
        }
    }

    #[test]
    fn spyglass_scoping_tracks_active_item_and_release() {
        let mut content = Content::test_cube();
        content.item_names.insert(7, "minecraft:spyglass".into());
        content.item_names.insert(8, "minecraft:bow".into());
        let mut player = Player::new();
        assert!(!player.is_scoping(&content));
        player.using = Some(super::super::interaction::UsingItem {
            hand: 0, item_id: 7, slot_identity: 0, duration: 1200,
            remaining: 1100, crossbow_charge_ticks: 0., crossbow_components: None, movement_factor: 1., can_sprint: false,
        });
        assert!(player.is_scoping(&content));
        player.using.as_mut().unwrap().item_id = 8;
        assert!(!player.is_scoping(&content));
        player.using = None;
        assert!(!player.is_scoping(&content));
    }

    #[test]
    fn crossbow_pull_tracks_active_use_ticks() {
        let mut use_item = super::super::interaction::UsingItem {
            hand: 0, item_id: 1, slot_identity: 0, duration: 72000, remaining: 71985,
            crossbow_charge_ticks: 25., crossbow_components: None, movement_factor: 1., can_sprint: false,
        };
        assert!((use_item.crossbow_pull(0.) - 0.6).abs() < 0.0001);
        use_item.crossbow_charge_ticks = 10.;
        assert!((use_item.crossbow_pull(0.) - 1.5).abs() < 0.0001);
        use_item.crossbow_charge_ticks = 0.;
        assert_eq!(use_item.crossbow_pull(0.), 0.);
    }

    #[test]
    fn crossbow_active_use_adopts_same_id_enchantment_updates() {
        use serde_json::json;
        let mut content = Content::test_cube();
        content.item_names.insert(7, "minecraft:crossbow".into());
        content.insert_test_item_interaction(7, "CrossbowItem", "CrossbowItem", "Item");
        content.set_registries(BTreeMap::from([(
            "minecraft:enchantment".into(),
            vec![("minecraft:quick_charge".into(), json!({"effects": {
                "minecraft:crossbow_charge_time": {"type":"minecraft:add",
                    "value":{"type":"minecraft:linear","base":-0.25,"per_level_above_first":-0.25}}
            }}))],
        )]));
        let mut stack = Stack::default();
        stack.id = 7;
        stack.count = 1;
        let mut inventory = InventoryStore::default();
        inventory.player.slots[36] = Some(stack.clone());
        let mut player = Player::new();
        player.start_using(Some(&stack), 0, &content, &inventory, 0).unwrap();
        player.using.as_mut().unwrap().remaining = 71985;
        assert!((player.using.as_ref().unwrap().crossbow_pull(0.) - 0.6).abs() < 0.0001);
        Arc::make_mut(&mut stack.components).insert(
            "minecraft:enchantments".into(), json!({"minecraft:quick_charge": 3}),
        );
        inventory.predict_use_slot(36, Some(stack), false).unwrap();
        player.tick_using_item(&World::new(0, 16), &content, &inventory,
            &EntityStore::new(), 1, &mut Vec::new()).unwrap();
        let using = player.using.as_ref().unwrap();
        assert_eq!(using.crossbow_charge_ticks, 10.);
        assert!((using.crossbow_pull(0.) - 1.6).abs() < 0.0001);
        player.stop_using(&mut Vec::new()).unwrap();
        assert!(player.using.is_none());
    }
}
