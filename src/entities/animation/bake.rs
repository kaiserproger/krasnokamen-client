//! Bakes parsed keyframe clips into the renderer's fixed-rate tables (`clip_pose`).
//!
//! Every [`BAKE_HZ`] frame samples each track's curves once. Per channel the renderer
//! applies `rotation = mix(rotation, rest, weight) + rotation_sample` and adds the offset
//! (`clip_pose` module docs), so the baker encodes the clip's layering here:
//! * arm channels of an `override_previous_animation` clip replace the procedural pose
//!   (weight = envelope) and carry `envelope * clip` rotation, which is
//!   `lerp(procedural, rest + clip, envelope)`;
//! * the root channel of such a clip replaces the vanilla death tip-over the same way;
//! * every other channel adds `envelope * clip` to the procedural pose (the head keeps
//!   looking at its target, legs keep the received gait, the torso turns through the
//!   carry).
//!
//! The envelope fades one-shot and held clips in over [`FADE_IN`] seconds of clip time and
//! one-shot clips back out over [`FADE_OUT`]; looping clips play at full weight. A
//! strike's contact frame must lie on full weight (checked here).
use super::super::clip_pose::{BakedClip, ChannelSample, Playback, BAKE_HZ, CHANNELS};
use super::format::{Clip, Looping, Part};
use crate::protocol::Result;
use glam::Vec3;

/// Clip seconds over which a clip blends in from the procedural pose.
pub(crate) const FADE_IN: f32 = 0.1;
/// Clip seconds over which a one-shot clip blends back out.
pub(crate) const FADE_OUT: f32 = 0.1;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Layer strength of `clip` at clip time `time`.
pub(crate) fn envelope(clip: &Clip, time: f32) -> f32 {
    match clip.looping {
        Looping::Once => {
            smoothstep(0., FADE_IN, time) * (1. - smoothstep(clip.length - FADE_OUT, clip.length, time))
        }
        Looping::Hold => smoothstep(0., FADE_IN, time),
        Looping::Loop => 1.,
    }
}

/// Number of frames covering `[0, length]` at [`BAKE_HZ`], end frame included.
pub(crate) fn frame_count(length: f32) -> usize {
    (length * BAKE_HZ).ceil() as usize + 1
}

pub(crate) fn bake(clip: &Clip) -> Result<BakedClip> {
    if clip.looping == Looping::Once && clip.length < FADE_IN + FADE_OUT {
        return Err(format!("{}: one-shot shorter than its fades", clip.name));
    }
    if let Some(contact) = clip.contact
        && envelope(clip, contact) < 1.
    {
        return Err(format!("{}: contact at {contact}s lies inside a fade", clip.name));
    }
    for track in &clip.tracks {
        if let Some(scale) = &track.scale
            && scale.keys.iter().any(|key| key.pre != Vec3::ONE || key.post != Vec3::ONE)
        {
            return Err(format!("{}: the renderer has no scale channel ({:?})", clip.name, track.part));
        }
    }
    let wrap = (clip.looping == Looping::Loop).then_some(clip.length);
    let count = frame_count(clip.length);
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let time = (index as f32 / BAKE_HZ).min(clip.length);
        let fade = envelope(clip, time);
        let mut frame = [ChannelSample::default(); CHANNELS];
        for track in &clip.tracks {
            let channel = track.part.channel() as usize;
            let rotation = track
                .rotation
                .as_ref()
                .map_or(Vec3::ZERO, |curve| curve.sample(time, wrap) * std::f32::consts::PI / 180.);
            let offset = track.position.as_ref().map_or(Vec3::ZERO, |curve| curve.sample(time, wrap) / 16.);
            let replaces = clip.replace && matches!(track.part, Part::RightArm | Part::LeftArm | Part::Root);
            frame[channel] = ChannelSample {
                rotation: (rotation * fade).to_array(),
                weight: if replaces { fade } else { 0. },
                offset: (offset * fade).to_array(),
                _pad: 0.,
            };
        }
        frames.push(frame);
    }
    Ok(BakedClip {
        name: clip.name.clone(),
        playback: match clip.looping {
            Looping::Once => Playback::Once,
            Looping::Loop => Playback::Loop,
            Looping::Hold => Playback::Hold,
        },
        frames,
    })
}
