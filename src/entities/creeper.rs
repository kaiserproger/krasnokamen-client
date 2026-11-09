//! Creeper fuse warning and charged aura. minecraft.wiki: a creeper about to explode
//! "hisses, flashes and expands" for 30 ticks; a charged creeper has a blue aura that
//! stays visible while the creeper is invisible. Every size, rate and colour here is a
//! Solaris-authored approximation, not a game constant.
use super::{Entity, Models, presentation as p};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content},
};
use glam::{Mat4, Vec3};

/// Fuse progress (0..1) of a creeper, interpolated between ticks.
pub(super) fn swell(entity: &Entity, partial: f32) -> f32 {
    if entity.type_name != "minecraft:creeper" {
        return 0.;
    }
    entity
        .source
        .as_ref()
        .and_then(|source| source.living.as_ref())
        .map_or(0., |living| living.swell(partial))
}

/// Grows the model about its feet while the fuse burns: wider than taller.
pub(super) fn swollen(matrix: Mat4, feet: Vec3, swell: f32) -> Mat4 {
    let wide = 1. + 0.3 * swell;
    let tall = 1. + 0.15 * swell;
    Mat4::from_translation(feet)
        * Mat4::from_scale(Vec3::new(wide, tall, wide))
        * Mat4::from_translation(-feet)
        * matrix
}

/// White flash strength (overlay palette column, 0 = none) for a fuse at `swell`.
/// Blinks start slow and speed up: the blink phase grows with the square of progress.
pub(super) fn flash(swell: f32) -> f32 {
    if swell <= 0. || (swell * swell * 8.).fract() >= 0.5 {
        0.
    } else {
        10.
    }
}

/// Charged aura: the creeper's own pose, enlarged, drawn as a scrolling energy swirl
/// sampled from the runtime `entity/creeper/creeper_armor` texture.
pub(super) fn append_charge(
    entity: &Entity,
    models: &Models,
    content: &Content,
    pose: (u64, f32, Mat4),
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<()> {
    if !p::boolean(models, entity, "DATA_IS_POWERED") {
        return Ok(());
    }
    let (tick, partial, matrix) = pose;
    let aura = super::solaris_models::authored("minecraft:creeper_charge#main")
        .ok_or("missing authored creeper charge rig")?;
    let texture = p::texture(content, "minecraft:entity/creeper/creeper_armor")?;
    p::evaluate_rig(aura, entity, models, content, tick, partial, matrix, transforms);
    let time = (tick % 4000) as f32 + partial;
    p::emit_wrapped(
        aura,
        transforms,
        p::WrappedStyle {
            tex: texture,
            scroll: [time * 0.01, time * 0.01],
            color: [0.5, 0.5, 0.5],
            layer: EntityMaterial::EnergySwirl,
            light: [15.; 2],
        },
        out,
    )
}
