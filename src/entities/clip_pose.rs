//! Animation-pose input for keyframe clips: the interface the clip runtime fills.
//!
//! The clip runtime (battle-video slice C2: clip files, `swing_animation` selection)
//! bakes its clips into a [`ClipLibrary`], [`install`]s it once, and keeps each entity's
//! [`ClipState`] (`Entity::clip`) current. Both renderer paths read that data with the
//! one rule below, so a soldier looks the same whether the CPU entity path
//! (`presentation::evaluate_posed_rig`, `presentation::root_matrix`) or the GPU crowd
//! path (`render/entity_instancing.wgsl`, which mirrors this file) draws it.
//!
//! * A clip is a fixed-rate table: [`BAKE_HZ`] frames per second, each frame holding
//!   one [`ChannelSample`] per [`Channel`]. A rig bone takes the channel of its role
//!   (head, arms, legs) or, for the bone named `body`, [`Channel::Body`] ([`channel_of`]);
//!   a held item takes the item channel of its anatomical hand; [`Channel::Root`] moves
//!   the whole model.
//! * [`ClipState`] holds two layers, action then flinch. A [`ClipLayer`] names a clip
//!   (1-based id; 0 = none), its start in world ticks, a playback rate and a weight;
//!   time is `(tick - start_tick + partial) / 20 * rate` seconds, so a recorded battle
//!   replays identically on a virtual clock (no wall-clock read).
//! * Sampling interpolates neighbouring frames linearly. [`Playback::Loop`] wraps,
//!   [`Playback::Hold`] keeps the last frame, [`Playback::Once`] is inactive past it.
//!   A mirrored layer swaps right and left channels and negates rotation y/z and
//!   offset x.
//! * Bones: after every vanilla pose rule, each active layer applies its sample `s` with
//!   layer weight `w`: `rotation = mix(rotation, rest, s.weight * w) + s.rotation * w`,
//!   `pivot += s.offset * w` (radians, Euler Z then Y then X as in
//!   `resources::transform`; `Bone::pivot` units). Sample weight 0 layers the clip on
//!   the vanilla pose; 1 replaces that channel's vanilla pose by rest + clip. Leg
//!   channels scale `w` by [`leg_factor`], so legs keep walking under an attack.
//! * Torso carry: the `body` bone takes only the replace part of its samples. Their
//!   summed rotation `r` and offset `o` instead re-parent every root-level Head, Body,
//!   RightArm and LeftArm bone through `T(hip + o) R(r) T(-hip)`, `hip` being the
//!   entity's main rig [`hip`], so the whole upper body leans about the waist.
//! * Item grip: a held item is attached through `T(g + o) R(r) T(-g)` of its item
//!   channel between the arm bone and the vanilla attachment, `g` being the vanilla
//!   grip point ([`grip_point`]).
//! * Root: `root_matrix` scales the vanilla death tip-over by `1 - replace` (summed
//!   `s.weight * w`, clamped) and then applies `T(f + o) R(r) T(-f)` about the feet
//!   `f` ([`FEET`]).
use super::resources::{Bone, BoneRole, Rig};
use crate::protocol::Result;
use glam::{Mat4, Vec3};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, LazyLock, OnceLock,
};

/// Baked samples per second of clip time.
pub(crate) const BAKE_HZ: f32 = 60.;
/// Number of [`Channel`]s.
pub(crate) const CHANNELS: usize = 9;
/// Layers per entity: action, then flinch.
pub(crate) const LAYERS: usize = 2;
/// The feet in model space (24 pixels below the model origin, +Y down).
pub(crate) const FEET: Vec3 = Vec3::new(0., 24. / 16., 0.);
/// Bound of one library: 1-based ids must fit 16 bits.
const MAX_CLIPS: usize = u16::MAX as usize;
/// Bound of one clip: ten minutes at [`BAKE_HZ`].
const MAX_FRAMES: usize = 36_000;

/// Part a clip animates. The discriminant is the GPU channel index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Channel {
    Head = 0,
    Body = 1,
    RightArm = 2,
    LeftArm = 3,
    RightLeg = 4,
    LeftLeg = 5,
    RightItem = 6,
    LeftItem = 7,
    Root = 8,
}

impl Channel {
    /// The other side's channel, for mirrored layers.
    fn mirrored(self) -> Self {
        match self {
            Self::RightArm => Self::LeftArm,
            Self::LeftArm => Self::RightArm,
            Self::RightLeg => Self::LeftLeg,
            Self::LeftLeg => Self::RightLeg,
            Self::RightItem => Self::LeftItem,
            Self::LeftItem => Self::RightItem,
            other => other,
        }
    }
    /// Item channel of an anatomical hand (0 right, 1 left).
    pub(crate) fn item(arm: usize) -> Self {
        if arm == 0 { Self::RightItem } else { Self::LeftItem }
    }
}

/// Clip channel of a rig bone: its role, or [`Channel::Body`] for the bone named `body`.
pub(crate) fn channel_of(bone: &Bone) -> Option<Channel> {
    match bone.role {
        BoneRole::Head => Some(Channel::Head),
        BoneRole::RightArm => Some(Channel::RightArm),
        BoneRole::LeftArm => Some(Channel::LeftArm),
        BoneRole::RightLeg => Some(Channel::RightLeg),
        BoneRole::LeftLeg => Some(Channel::LeftLeg),
        _ if bone.name == "body" => Some(Channel::Body),
        _ => None,
    }
}
/// Whether the torso carry re-parents this bone: an upper-body bone directly below the
/// rig root (authored rigs bake a synthetic `root` bone at index 0).
pub(crate) fn carried(rig: &Rig, bone: &Bone) -> bool {
    bone.parent.is_none_or(|parent| rig.bones[parent].parent.is_none())
        && matches!(channel_of(bone), Some(Channel::Head | Channel::Body | Channel::RightArm | Channel::LeftArm))
}

/// Waist pivot of a main rig: centre of its first leg bone's pivot, else the model origin.
pub(crate) fn hip(rig: &Rig) -> Vec3 {
    rig.bones
        .iter()
        .find(|bone| matches!(bone.role, BoneRole::RightLeg | BoneRole::LeftLeg))
        .map_or(Vec3::ZERO, |bone| Vec3::new(0., bone.pivot.y, bone.pivot.z))
}

/// Vanilla grip point of a held item below its arm bone (`held_item_attachment`).
pub(crate) fn grip_point(arm: usize) -> Vec3 {
    Vec3::new(if arm == 0 { -1. } else { 1. } / 16., 0.625, -0.125)
}

/// Scale of leg-channel layer weights: legs keep their gait while walking and stay
/// seated while riding.
pub(crate) fn leg_factor(amplitude: f32, riding: bool) -> f32 {
    if riding { 0. } else { 1. - (2. * amplitude).clamp(0., 1.) }
}

/// One channel at one sample time. Layout is shared with the GPU table (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChannelSample {
    /// Radians added to the bone's Euler rotation.
    pub rotation: [f32; 3],
    /// 0 adds to the vanilla pose; 1 replaces it by the rest pose first; blends linearly.
    pub weight: f32,
    /// Added to the bone pivot, in `Bone::pivot` units.
    pub offset: [f32; 3],
    pub _pad: f32,
}

impl ChannelSample {
    fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            rotation: std::array::from_fn(|i| mix(self.rotation[i], other.rotation[i])),
            weight: mix(self.weight, other.weight),
            offset: std::array::from_fn(|i| mix(self.offset[i], other.offset[i])),
            _pad: 0.,
        }
    }
    fn mirror(self) -> Self {
        let [x, y, z] = self.rotation;
        let [ox, oy, oz] = self.offset;
        Self { rotation: [x, -y, -z], offset: [-ox, oy, oz], ..self }
    }
}

/// What a clip shows past its last frame. The discriminant is the GPU header word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Playback {
    /// Inactive (one-shot attack).
    Once = 0,
    /// Wraps to the first frame (cycles).
    Loop = 1,
    /// Keeps the last frame (death).
    Hold = 2,
}

/// A clip as the renderer consumes it.
#[derive(Clone, Debug)]
pub struct BakedClip {
    pub name: String,
    pub playback: Playback,
    /// One entry per frame at [`BAKE_HZ`], indexed by [`Channel`].
    pub frames: Vec<[ChannelSample; CHANNELS]>,
}

/// One playing clip of an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipLayer {
    /// 1-based library id; 0 = none.
    pub clip: u16,
    /// World tick at which clip time 0 is shown.
    pub start_tick: u64,
    /// Clip seconds per real second (stretches a clip to a received swing duration).
    pub rate: f32,
    /// Layer strength, 0..=1.
    pub weight: f32,
    /// Play the other side's channels (left-handed swing from a right-handed clip).
    pub mirror: bool,
}

impl Default for ClipLayer {
    fn default() -> Self {
        Self { clip: 0, start_tick: 0, rate: 1., weight: 1., mirror: false }
    }
}

/// What an entity plays: action layer, then flinch layer. The default plays nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipState {
    pub layers: [ClipLayer; LAYERS],
}

/// A layer resolved at one frame time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActiveLayer {
    pub clip: u16,
    pub seconds: f32,
    pub weight: f32,
    pub mirror: bool,
}

/// The layers playing at one frame time.
pub(crate) type Active = [Option<ActiveLayer>; LAYERS];

impl ClipState {
    /// Layers playing at `tick + partial` (before the start or without a clip: `None`).
    pub(crate) fn active(&self, tick: u64, partial: f32) -> Active {
        self.layers.map(|layer| {
            (layer.clip != 0 && tick >= layer.start_tick && layer.weight > 0.).then(|| ActiveLayer {
                clip: layer.clip,
                seconds: ((tick - layer.start_tick) as f32 + partial) / 20. * layer.rate,
                weight: layer.weight.min(1.),
                mirror: layer.mirror,
            })
        })
    }
}

/// Summed samples of one channel over the active layers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Delta {
    pub rotation: Vec3,
    pub offset: Vec3,
    /// Summed `s.weight * w`, clamped to 1.
    pub replace: f32,
}

impl Delta {
    /// `T(pivot + offset) R(rotation) T(-pivot)`.
    pub(crate) fn about(&self, pivot: Vec3) -> Mat4 {
        Mat4::from_translation(pivot + self.offset)
            * super::resources::transform(Vec3::ZERO, self.rotation, Vec3::ONE)
            * Mat4::from_translation(-pivot)
    }
}

/// Validated clip table with a process-unique revision (GPU upload identity).
#[derive(Debug)]
pub struct ClipLibrary {
    clips: Vec<BakedClip>,
    revision: u64,
    max_offset: f32,
}

static REVISIONS: AtomicU64 = AtomicU64::new(1);

impl Default for ClipLibrary {
    fn default() -> Self {
        Self { clips: Vec::new(), revision: REVISIONS.fetch_add(1, Ordering::Relaxed), max_offset: 0. }
    }
}

impl ClipLibrary {
    pub fn new(clips: Vec<BakedClip>) -> Result<Self> {
        if clips.len() > MAX_CLIPS {
            return Err(format!("clip library holds {} clips, bound {MAX_CLIPS}", clips.len()));
        }
        let mut max_offset = 0_f32;
        for clip in &clips {
            if clip.frames.is_empty() || clip.frames.len() > MAX_FRAMES {
                return Err(format!("clip {} has {} frames", clip.name, clip.frames.len()));
            }
            for sample in clip.frames.iter().flatten() {
                let finite = sample.rotation.iter().chain(&sample.offset).chain([&sample.weight]).all(|v| v.is_finite());
                if !finite || !(0. ..=1.).contains(&sample.weight) {
                    return Err(format!("clip {} has a non-finite sample or weight outside 0..=1", clip.name));
                }
                max_offset = max_offset.max(Vec3::from_array(sample.offset).length());
            }
        }
        Ok(Self { clips, revision: REVISIONS.fetch_add(1, Ordering::Relaxed), max_offset })
    }
    /// 1-based id of the clip named `name`.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.clips.iter().position(|clip| clip.name == name).map(|index| index as u16 + 1)
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    /// Largest pivot offset one sample applies.
    pub(crate) fn max_offset(&self) -> f32 {
        self.max_offset
    }
    /// The sample of `channel` in `layer`; `None` when the clip is unknown or finished.
    pub(crate) fn sample(&self, layer: ActiveLayer, channel: Channel) -> Option<ChannelSample> {
        let clip = self.clips.get(usize::from(layer.clip).checked_sub(1)?)?;
        let last = clip.frames.len() - 1;
        let mut frame = layer.seconds * BAKE_HZ;
        if !(frame >= 0.) {
            return None;
        }
        match clip.playback {
            Playback::Loop => frame = frame.rem_euclid((last + 1) as f32),
            Playback::Hold => frame = frame.min(last as f32),
            Playback::Once if frame > last as f32 => return None,
            Playback::Once => {}
        }
        let first = (frame.floor() as usize).min(last);
        let second = match clip.playback {
            Playback::Loop => (first + 1) % (last + 1),
            Playback::Once | Playback::Hold => (first + 1).min(last),
        };
        let source = if layer.mirror { channel.mirrored() } else { channel } as usize;
        let sample = clip.frames[first][source].lerp(clip.frames[second][source], frame - first as f32);
        Some(if layer.mirror { sample.mirror() } else { sample })
    }
    /// Applies every active layer to a bone of `channel` already posed by the vanilla
    /// rules (`legs`: [`leg_factor`] of the entity).
    pub(crate) fn apply(
        &self,
        layers: &Active,
        channel: Channel,
        legs: f32,
        rest: Vec3,
        rotation: &mut Vec3,
        pivot: &mut Vec3,
    ) {
        let scale = if matches!(channel, Channel::RightLeg | Channel::LeftLeg) { legs } else { 1. };
        for layer in layers.iter().flatten() {
            if let Some(s) = self.sample(*layer, channel) {
                let w = layer.weight * scale;
                *rotation = rotation.lerp(rest, s.weight * w);
                if channel != Channel::Body {
                    *rotation += Vec3::from_array(s.rotation) * w;
                    *pivot += Vec3::from_array(s.offset) * w;
                }
            }
        }
    }
    /// Summed samples of `channel`; `None` when no layer samples it.
    pub(crate) fn delta(&self, layers: &Active, channel: Channel) -> Option<Delta> {
        let mut found: Option<Delta> = None;
        for layer in layers.iter().flatten() {
            if let Some(s) = self.sample(*layer, channel) {
                let delta = found.get_or_insert(Delta { rotation: Vec3::ZERO, offset: Vec3::ZERO, replace: 0. });
                delta.rotation += Vec3::from_array(s.rotation) * layer.weight;
                delta.offset += Vec3::from_array(s.offset) * layer.weight;
                delta.replace = (delta.replace + s.weight * layer.weight).min(1.);
            }
        }
        found
    }
    /// Torso carry parent of the root-level upper-body bones, about the main rig's hip.
    pub(crate) fn carry(&self, layers: &Active, hip: Vec3) -> Option<Mat4> {
        Some(self.delta(layers, Channel::Body)?.about(hip))
    }
    /// Grip of the item held by anatomical hand `arm`, between the arm bone and the item.
    pub(crate) fn grip(&self, layers: &Active, arm: usize) -> Option<Mat4> {
        Some(self.delta(layers, Channel::item(arm))?.about(grip_point(arm)))
    }
    /// GPU tables: per clip `[first frame, frame count, playback, 0]`, then all frames
    /// flattened as `(first + frame) * CHANNELS + channel`.
    pub(crate) fn gpu_tables(&self) -> (Vec<[u32; 4]>, Vec<ChannelSample>) {
        let mut headers = Vec::with_capacity(self.clips.len());
        let mut frames = Vec::new();
        for clip in &self.clips {
            headers.push([
                (frames.len() / CHANNELS) as u32,
                clip.frames.len() as u32,
                clip.playback as u32,
                0,
            ]);
            frames.extend(clip.frames.iter().flatten());
        }
        (headers, frames)
    }
}

static INSTALLED: OnceLock<Arc<ClipLibrary>> = OnceLock::new();
static EMPTY: LazyLock<Arc<ClipLibrary>> = LazyLock::new(|| Arc::new(ClipLibrary::default()));

/// Makes `library` the table every renderer path samples, once per process.
pub fn install(library: ClipLibrary) -> Result<()> {
    INSTALLED.set(Arc::new(library)).map_err(|_| "clip library already installed".into())
}

/// The installed clip table; empty before [`install`].
pub(crate) fn library() -> Arc<ClipLibrary> {
    INSTALLED.get().unwrap_or(&EMPTY).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(x: f32) -> ChannelSample {
        ChannelSample { rotation: [x, x, 0.], weight: 0., offset: [x, 0., 0.], _pad: 0. }
    }
    fn clip(playback: Playback) -> BakedClip {
        BakedClip {
            name: format!("{playback:?}"),
            playback,
            frames: (0..4).map(|i| [sample(i as f32); CHANNELS]).collect(),
        }
    }
    fn layer(clip: u16, seconds: f32) -> ActiveLayer {
        ActiveLayer { clip, seconds, weight: 1., mirror: false }
    }
    fn library_of(frames: [ChannelSample; CHANNELS]) -> ClipLibrary {
        ClipLibrary::new(vec![BakedClip { name: "a".into(), playback: Playback::Once, frames: vec![frames; 2] }]).unwrap()
    }

    #[test]
    fn samples_interpolate_wrap_hold_and_expire() {
        let library = ClipLibrary::new(vec![clip(Playback::Once), clip(Playback::Loop), clip(Playback::Hold)]).unwrap();
        assert_eq!(library.id("Loop"), Some(2));
        let at = |id, seconds| library.sample(layer(id, seconds), Channel::RightArm).map(|s| s.rotation[0]);
        assert_eq!(at(1, 0.), Some(0.));
        assert!((at(1, 1.5 / BAKE_HZ).unwrap() - 1.5).abs() < 1e-5);
        assert_eq!(at(1, 3. / BAKE_HZ), Some(3.), "the last frame is still shown");
        assert_eq!(at(1, 3.5 / BAKE_HZ), None, "a finished one-shot clip is inactive");
        assert!((at(2, 3.5 / BAKE_HZ).unwrap() - 1.5).abs() < 1e-5, "looped clips blend last into first");
        assert!((at(2, 5. / BAKE_HZ).unwrap() - 1.).abs() < 1e-5);
        assert_eq!(at(3, 100.), Some(3.), "a held clip keeps its last frame");
        assert_eq!(at(0, 0.), None);
        assert_eq!(at(4, 0.), None);
        assert_eq!(library.sample(layer(1, -0.1), Channel::Head), None);
        assert_eq!(library.max_offset(), 3.);
    }

    #[test]
    fn mirrored_layers_swap_sides_and_reflect() {
        let mut frames = [ChannelSample::default(); CHANNELS];
        frames[Channel::LeftArm as usize] = ChannelSample { rotation: [1., 2., 3.], weight: 0.5, offset: [4., 5., 6.], _pad: 0. };
        let library = library_of(frames);
        let mirrored = ActiveLayer { mirror: true, ..layer(1, 0.) };
        let right = library.sample(mirrored, Channel::RightArm).unwrap();
        assert_eq!(right.rotation, [1., -2., -3.]);
        assert_eq!(right.offset, [-4., 5., 6.]);
        assert_eq!(right.weight, 0.5);
        assert_eq!(library.sample(mirrored, Channel::LeftArm).unwrap().rotation, [0.; 3]);
    }

    #[test]
    fn clip_time_follows_world_ticks_rate_and_weight() {
        let state = ClipState {
            layers: [
                ClipLayer { clip: 1, start_tick: 100, rate: 2., weight: 1., mirror: false },
                ClipLayer { clip: 2, weight: 0., ..ClipLayer::default() },
            ],
        };
        assert_eq!(state.active(99, 0.9), [None, None]);
        assert_eq!(state.active(110, 0.5)[0].unwrap().seconds, 1.05);
        assert_eq!(state.active(110, 0.)[1], None, "a zero-weight layer is inactive");
        assert_eq!(ClipState::default().active(5, 0.), [None, None]);
    }

    #[test]
    fn bones_blend_towards_rest_and_legs_keep_their_gait() {
        let mut frames = [ChannelSample::default(); CHANNELS];
        let swing = ChannelSample { rotation: [0.5, 0., 0.], weight: 0.5, offset: [0., 0.25, 0.], _pad: 0. };
        frames[Channel::RightArm as usize] = swing;
        frames[Channel::RightLeg as usize] = swing;
        frames[Channel::Body as usize] = swing;
        let library = library_of(frames);
        let layers = [Some(ActiveLayer { weight: 0.5, ..layer(1, 0.) }), None];
        let rest = Vec3::new(0.1, 0., 0.);
        let posed = Vec3::new(1.1, 0.4, 0.);
        let run = |channel, legs| {
            let (mut rotation, mut pivot) = (posed, Vec3::ZERO);
            library.apply(&layers, channel, legs, rest, &mut rotation, &mut pivot);
            (rotation, pivot)
        };
        // mix(posed, rest, 0.25) + 0.5 * 0.5
        let (rotation, pivot) = run(Channel::RightArm, 1.);
        assert!((rotation - Vec3::new(0.85 + 0.25, 0.3, 0.)).length() < 1e-6, "{rotation:?}");
        assert_eq!(pivot, Vec3::new(0., 0.125, 0.));
        assert_eq!(run(Channel::RightLeg, 0.), (posed, Vec3::ZERO), "walking legs ignore the clip");
        let (rotation, pivot) = run(Channel::Body, 1.);
        assert!((rotation - Vec3::new(0.85, 0.3, 0.)).length() < 1e-6, "body bone takes only the replace part");
        assert_eq!(pivot, Vec3::ZERO);
        assert_eq!(leg_factor(0.25, false), 0.5);
        assert_eq!(leg_factor(0., true), 0.);
    }

    #[test]
    fn carry_grip_and_root_turn_about_their_pivots() {
        let mut frames = [ChannelSample::default(); CHANNELS];
        frames[Channel::Body as usize] = ChannelSample { rotation: [0., 0.5, 0.], weight: 0.4, offset: [0., 0., 0.1], _pad: 0. };
        frames[Channel::RightItem as usize] = ChannelSample { rotation: [0.2, 0., 0.], weight: 0., offset: [0., 0.1, 0.], _pad: 0. };
        let library = library_of(frames);
        let layers = [Some(ActiveLayer { weight: 0.5, ..layer(1, 0.) }), None];
        let hip = Vec3::new(0., 0.75, 0.);
        let carry = library.carry(&layers, hip).unwrap();
        assert!((carry.transform_point3(hip) - (hip + Vec3::new(0., 0., 0.05))).length() < 1e-6);
        let turned = carry.transform_vector3(Vec3::X);
        assert!((turned - Vec3::new(0.25_f32.cos(), 0., -0.25_f32.sin())).length() < 1e-6, "{turned:?}");
        let grip = library.grip(&layers, 0).unwrap();
        assert!((grip.transform_point3(grip_point(0)) - (grip_point(0) + Vec3::new(0., 0.05, 0.))).length() < 1e-6);
        assert_eq!(library.delta(&layers, Channel::Body).unwrap().replace, 0.2);
        assert!(library.grip(&[None, None], 0).is_none());
        assert!(library.carry(&[None, None], hip).is_none());
        assert!(library.delta(&layers, Channel::Root).unwrap().about(FEET).abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn invalid_libraries_are_refused_and_tables_flatten_by_channel() {
        assert!(ClipLibrary::new(vec![BakedClip { name: "empty".into(), playback: Playback::Once, frames: Vec::new() }]).is_err());
        let mut bad = clip(Playback::Once);
        bad.frames[1][0].weight = 2.;
        assert!(ClipLibrary::new(vec![bad]).is_err());
        let library = ClipLibrary::new(vec![clip(Playback::Once), clip(Playback::Hold)]).unwrap();
        let (headers, frames) = library.gpu_tables();
        assert_eq!(headers, vec![[0, 4, 0, 0], [4, 4, 2, 0]]);
        assert_eq!(frames.len(), 8 * CHANNELS);
        assert_eq!(frames[(4 + 2) * CHANNELS + Channel::LeftLeg as usize], sample(2.));
        assert_ne!(library.revision(), ClipLibrary::default().revision());
    }
}
