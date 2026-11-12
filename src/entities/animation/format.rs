//! Keyframe clip files (`assets/krasnokamen/animations/*.animation.json`).
//!
//! The file layout is the Bedrock entity-animation subset documented at
//! <https://bedrock.dev/docs/stable/Animations> and the schema
//! <https://github.com/bedrock-studio/bedrock-json-schemas/blob/master/animation.json>:
//! `{"format_version", "animations": {name: {"loop", "animation_length",
//! "override_previous_animation", "timeline", "bones": {bone: {"rotation" | "position" |
//! "scale": channel}}}}}`. A channel is a constant `[x, y, z]` (or one number for all
//! axes) or a map from keyframe time in seconds to a value or to
//! `{"pre", "post", "lerp_mode": "linear" | "catmullrom" | "step"}`.
//!
//! Interpolation follows Blockbench's keyframe evaluator
//! (<https://github.com/JannisX11/blockbench/blob/master/js/animations/timeline_animators.js>):
//! a segment runs from the earlier key's `post` to the later key's `pre`; it is
//! Catmull-Rom when either end is `catmullrom` (neighbour keys feed the tangents unless
//! that end carries separate `pre`/`post`, then the end value is repeated), a `step` key
//! holds its value, otherwise linear. Before the first key the first `pre` applies, after
//! the last key the last `post`.
//!
//! Krasnokamen rules on top of the subset, all fail-closed at load:
//! * values are numbers in the Krasnokamen rig convention (`krasnokamen_models.rs`): rotation in
//!   degrees applied Z, then Y, then X; position in pixels with +Y down and -Z the front.
//!   This is not Bedrock's axis convention (Bedrock negates X/Y rotation and X position).
//! * bones are the canonical parts in [`Part`]; MoLang strings and unknown bones are
//!   rejected (our own assets, so a typo must not silently do nothing).
//! * the only `timeline` instruction is `"contact"`: the frame where a strike lands.
use super::super::clip_pose::Channel;
use crate::protocol::Result;
use glam::Vec3;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Canonical clip bones. `Root` moves the whole model about its feet; `Body` is the torso
/// (it carries the head and arms); items turn the held item about its grip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Part {
    Root,
    Body,
    Head,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
    RightItem,
    LeftItem,
}

impl Part {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "root" => Self::Root,
            "body" => Self::Body,
            "head" => Self::Head,
            "right_arm" => Self::RightArm,
            "left_arm" => Self::LeftArm,
            "right_leg" => Self::RightLeg,
            "left_leg" => Self::LeftLeg,
            "right_item" => Self::RightItem,
            "left_item" => Self::LeftItem,
            _ => return None,
        })
    }
    /// The renderer channel this part bakes into.
    pub(crate) fn channel(self) -> Channel {
        match self {
            Self::Root => Channel::Root,
            Self::Body => Channel::Body,
            Self::Head => Channel::Head,
            Self::RightArm => Channel::RightArm,
            Self::LeftArm => Channel::LeftArm,
            Self::RightLeg => Channel::RightLeg,
            Self::LeftLeg => Channel::LeftLeg,
            Self::RightItem => Channel::RightItem,
            Self::LeftItem => Channel::LeftItem,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Looping {
    Once,
    Loop,
    /// One-shot that keeps its last frame (`"loop": "hold_on_last_frame"`).
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lerp {
    Linear,
    CatmullRom,
    Step,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Key {
    pub time: f32,
    pub pre: Vec3,
    pub post: Vec3,
    pub lerp: Lerp,
    /// Separate `pre` and `post`: a deliberate discontinuity; neighbours do not shape it.
    pub split: bool,
}

/// Keys strictly increasing in time; never empty.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Curve {
    pub keys: Vec<Key>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub part: Part,
    /// Degrees.
    pub rotation: Option<Curve>,
    /// Pixels.
    pub position: Option<Curve>,
    pub scale: Option<Curve>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Clip {
    pub name: String,
    pub length: f32,
    pub looping: Looping,
    /// `override_previous_animation`: arm and item tracks replace the procedural pose.
    pub replace: bool,
    /// Seconds into the clip where the strike lands.
    pub contact: Option<f32>,
    pub tracks: Vec<Track>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSpec {
    format_version: String,
    animations: BTreeMap<String, ClipSpec>,
}

// Untagged serde enums buffer the client's arbitrary-precision JSON numbers as private
// maps (see `render_living::motion`), so the alternatives below are decoded from
// `serde_json::Value` by hand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipSpec {
    #[serde(rename = "loop", default)]
    looping: Option<Value>,
    animation_length: Option<f32>,
    #[serde(default)]
    override_previous_animation: bool,
    #[serde(default)]
    timeline: BTreeMap<String, Value>,
    #[serde(default)]
    bones: BTreeMap<String, BoneSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoneSpec {
    rotation: Option<Value>,
    position: Option<Value>,
    scale: Option<Value>,
}

/// `[x, y, z]` or one number for all axes.
fn vec3(value: &Value, what: &str) -> Result<Vec3> {
    let number = |value: &Value| {
        value.as_f64().map(|v| v as f32).ok_or_else(|| format!("{what}: {value} is not a number"))
    };
    match value {
        Value::Array(items) if items.len() == 3 => {
            Ok(Vec3::new(number(&items[0])?, number(&items[1])?, number(&items[2])?))
        }
        Value::Number(_) => Ok(Vec3::splat(number(value)?)),
        _ => Err(format!("{what}: {value} is not a vector")),
    }
}

fn seconds(text: &str, what: &str) -> Result<f32> {
    let time: f32 = text.parse().map_err(|_| format!("{what}: time {text:?} is not seconds"))?;
    if !time.is_finite() || time < 0. {
        return Err(format!("{what}: time {text:?} out of range"));
    }
    Ok(time)
}

fn key(time: f32, value: &Value, what: &str) -> Result<Key> {
    let Value::Object(fields) = value else {
        let value = vec3(value, what)?;
        return Ok(Key { time, pre: value, post: value, lerp: Lerp::Linear, split: false });
    };
    if let Some(unknown) = fields.keys().find(|name| !matches!(name.as_str(), "pre" | "post" | "lerp_mode")) {
        return Err(format!("{what}: unknown keyframe field {unknown:?}"));
    }
    let lerp = match fields.get("lerp_mode").map(|mode| mode.as_str().ok_or_else(|| format!("{what}: lerp_mode {mode}"))) {
        None => Lerp::Linear,
        Some(mode) => match mode? {
            "linear" => Lerp::Linear,
            "catmullrom" => Lerp::CatmullRom,
            "step" => Lerp::Step,
            other => return Err(format!("{what}: lerp_mode {other:?}")),
        },
    };
    let pre = fields.get("pre").map(|v| vec3(v, what)).transpose()?;
    let post = fields.get("post").map(|v| vec3(v, what)).transpose()?;
    let (pre, post) = match (pre, post) {
        (Some(pre), Some(post)) => (pre, post),
        (Some(value), None) | (None, Some(value)) => (value, value),
        (None, None) => return Err(format!("{what}: key at {time} has no value")),
    };
    Ok(Key { time, pre, post, lerp, split: fields.contains_key("pre") && fields.contains_key("post") })
}

fn curve(spec: &Value, what: &str) -> Result<Curve> {
    let mut keys = match spec {
        Value::Object(map) => {
            map.iter().map(|(time, value)| key(seconds(time, what)?, value, what)).collect::<Result<Vec<_>>>()?
        }
        constant => vec![key(0., constant, what)?],
    };
    keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    if keys.is_empty() {
        return Err(format!("{what}: no keyframes"));
    }
    if keys.windows(2).any(|pair| pair[0].time == pair[1].time) {
        return Err(format!("{what}: duplicate keyframe time"));
    }
    if keys.iter().any(|key| !key.pre.is_finite() || !key.post.is_finite()) {
        return Err(format!("{what}: non-finite value"));
    }
    Ok(Curve { keys })
}

/// Parses one animation file into its clips, in file (name) order.
pub(crate) fn parse(source: &str) -> Result<Vec<Clip>> {
    let file: FileSpec = serde_json::from_str(source).map_err(|e| e.to_string())?;
    if file.format_version != "1.8.0" {
        return Err(format!("animation format_version {}", file.format_version));
    }
    let mut clips = Vec::with_capacity(file.animations.len());
    for (name, spec) in file.animations {
        let looping = match &spec.looping {
            None | Some(Value::Bool(false)) => Looping::Once,
            Some(Value::Bool(true)) => Looping::Loop,
            Some(Value::String(mode)) if mode == "hold_on_last_frame" => Looping::Hold,
            Some(other) => return Err(format!("{name}: loop {other}")),
        };
        let mut tracks = Vec::with_capacity(spec.bones.len());
        let mut last_key = 0_f32;
        for (bone, channels) in spec.bones {
            let part = Part::parse(&bone).ok_or_else(|| format!("{name}: unknown bone {bone:?}"))?;
            let what = |channel: &str| format!("{name}.{bone}.{channel}");
            let mut channel = |spec: Option<Value>, label: &str| -> Result<Option<Curve>> {
                spec.map(|spec| curve(&spec, &what(label))).transpose().inspect(|curve| {
                    if let Some(curve) = curve {
                        last_key = last_key.max(curve.keys.last().unwrap().time);
                    }
                })
            };
            let track = Track {
                part,
                rotation: channel(channels.rotation, "rotation")?,
                position: channel(channels.position, "position")?,
                scale: channel(channels.scale, "scale")?,
            };
            if track.rotation.is_none() && track.position.is_none() && track.scale.is_none() {
                return Err(format!("{name}: bone {bone:?} animates nothing"));
            }
            tracks.push(track);
        }
        let length = spec.animation_length.unwrap_or(last_key);
        if !length.is_finite() || length <= 0. || length > 600. {
            return Err(format!("{name}: animation_length {length}"));
        }
        let mut contact = None;
        for (time, instructions) in spec.timeline {
            let time = seconds(&time, &name)?;
            let list = match instructions {
                Value::String(one) => vec![one],
                Value::Array(many) => many
                    .into_iter()
                    .map(|item| match item {
                        Value::String(text) => Ok(text),
                        other => Err(format!("{name}: timeline instruction {other}")),
                    })
                    .collect::<Result<Vec<_>>>()?,
                other => return Err(format!("{name}: timeline instruction {other}")),
            };
            for instruction in list {
                if instruction != "contact" {
                    return Err(format!("{name}: timeline instruction {instruction:?}"));
                }
                if contact.replace(time).is_some() || time > length {
                    return Err(format!("{name}: contact must be one time within the clip"));
                }
            }
        }
        clips.push(Clip { name, length, looping, replace: spec.override_previous_animation, contact, tracks });
    }
    Ok(clips)
}

/// Uniform Catmull-Rom through `p1` (t = 0) and `p2` (t = 1), as three.js `CatmullRom`.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let v0 = (p2 - p0) * 0.5;
    let v1 = (p3 - p1) * 0.5;
    let t2 = t * t;
    let t3 = t2 * t;
    (2. * p1 - 2. * p2 + v0 + v1) * t3 + (-3. * p1 + 3. * p2 - 2. * v0 - v1) * t2 + v0 * t + p1
}

impl Curve {
    /// Value at clip time `time` (seconds); `wrap` is the clip length of a looping clip.
    pub(crate) fn sample(&self, time: f32, wrap: Option<f32>) -> Vec3 {
        let keys = &self.keys;
        let first = keys[0];
        let last = keys[keys.len() - 1];
        if time < first.time {
            return first.pre;
        }
        if time >= last.time {
            return last.post;
        }
        let after = keys.partition_point(|key| key.time <= time);
        let (a, b) = (keys[after - 1], keys[after]);
        if a.lerp == Lerp::Step {
            return a.post;
        }
        let alpha = (time - a.time) / (b.time - a.time);
        if a.lerp != Lerp::CatmullRom && b.lerp != Lerp::CatmullRom {
            return a.post.lerp(b.pre, alpha);
        }
        let neighbour = |index: Option<usize>| index.and_then(|i| keys.get(i).copied());
        let looped = wrap.is_some() && keys.len() >= 3;
        let before = neighbour(after.checked_sub(2))
            .or_else(|| looped.then(|| keys[keys.len() - 2]))
            .filter(|_| !a.split)
            .map_or(a.post, |key| key.post);
        let beyond = neighbour(Some(after + 1))
            .or_else(|| looped.then(|| keys[1]))
            .filter(|_| !b.split)
            .map_or(b.pre, |key| key.pre);
        catmull_rom(before, a.post, b.pre, beyond, alpha)
    }
}
