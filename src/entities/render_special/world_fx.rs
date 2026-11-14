//! Independently authored world effects. Public behavior references and Krasnokamen
//! approximation constants are recorded in the WorldFx provenance receipt.
use super::{Entity, Handled, Metadata, Models, Store, p, translation};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content, models::TintKind},
};
use glam::Vec3;
use std::collections::BTreeMap;

// 26.3 TntRenderer uses the interpolated remaining fuse for both effects.
fn tnt_appearance(fuse: f32, partial: f32) -> (f32, bool) {
    let remaining = fuse + 1. - partial.clamp(0., 1.);
    let urgency = (1. - remaining / 10.).clamp(0., 1.);
    (
        1. + 0.3 * urgency.powi(4),
        remaining >= 0. && (remaining / 5.) as i32 % 2 == 0,
    )
}

fn block_tint(
    tint: TintKind,
    position: [i32; 3],
    cache: &std::cell::Cell<[Option<[f32; 3]>; 4]>,
    sample: &dyn Fn(TintKind, [i32; 3]) -> Result<[f32; 3]>,
) -> Result<[f32; 3]> {
    let index = match tint {
        TintKind::None => return Ok([1.; 3]),
        TintKind::Constant(color) => return Ok(color),
        TintKind::Grass => 0,
        TintKind::Foliage => 1,
        TintKind::DryFoliage => 2,
        TintKind::Water => 3,
    };
    let mut cached = cache.get();
    match cached[index] {
        Some(color) => Ok(color),
        None => {
            let color = sample(tint, position)?;
            cached[index] = Some(color);
            cache.set(cached);
            Ok(color)
        }
    }
}

pub(super) fn append_block(
    store: &mut Store,
    slot: usize,
    models: &Models,
    content: &Content,
    frame: p::EntityDrawFrame,
    tint_source: &dyn Fn(TintKind, [i32; 3]) -> Result<[f32; 3]>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let entity = &store.slots[slot];
    let is_tnt = entity.type_name == "minecraft:tnt";
    let state = if is_tnt {
        match p::value(models, entity, "DATA_BLOCK_STATE_ID") {
            Some(Metadata::BlockState(state)) => *state,
            Some(_) => return Err("invalid TNT block state metadata".into()),
            None => match store.special_render_cache.tnt_state {
                Some(state) => state,
                None => {
                    let state = content.resolve_block_state("minecraft:tnt", &BTreeMap::new())?;
                    store.special_render_cache.tnt_state = Some(state);
                    state
                }
            },
        }
    } else {
        u32::try_from(entity.spawn_data).map_err(|_| "negative falling block spawn state")?
    };
    if content.block_is_air(state)? {
        return Ok(Handled::NoGeometry);
    }
    let model = content.model_for(state)?.ok_or_else(|| {
        format!(
            "{} block model unavailable for received state {state}",
            entity.type_name
        )
    })?;
    let (swell, flash) = if is_tnt {
        let timeline = &entity
            .source
            .as_ref()
            .ok_or("TNT timeline unavailable")?
            .special;
        if timeline.discarded {
            return Ok(Handled::NoGeometry);
        }
        tnt_appearance(timeline.fuse as f32, frame.partial)
    } else {
        (1., false)
    };
    let transform = frame.transform;
    // Swelling is centered vertically; the block model is one block wide.
    let matrix = translation(transform.position, frame.origin)
        * glam::Mat4::from_translation(Vec3::Y * 0.5)
        * glam::Mat4::from_scale(Vec3::splat(swell))
        * glam::Mat4::from_translation(Vec3::splat(-0.5));
    let seed_position = if is_tnt {
        transform.position.map(|p| p.floor() as i32)
    } else {
        match p::value(models, entity, "DATA_START_POS") {
            Some(Metadata::BlockPosition(position)) => *position,
            Some(_) => return Err("invalid falling block start position metadata".into()),
            None => entity.source.as_ref()
                .ok_or("falling block timeline unavailable")?.special.falling_start,
        }
    };
    let seed = crate::protocol::models::position_seed(seed_position);
    let current = store.presented_transform(entity, frame.tick, 0.)?.position;
    let bounds = super::super::physical::bounds_at(models, entity, current)?;
    let tint_position = [
        current[0].floor() as i32,
        bounds.max[1].floor() as i32,
        current[2].floor() as i32,
    ];
    let tint_cache = std::cell::Cell::new([None; 4]);
    let start = out.vertices.len();
    let index_starts = out.indices.each_ref().map(Vec::len);
    p::append_block_quads(
        model,
        seed,
        matrix,
        frame.light,
        |quad| block_tint(quad.tint, tint_position, &tint_cache, tint_source),
        out,
    )?;
    if flash {
        // Reuse the engine's white overlay, retaining the pack's UVs, alpha and
        // geometry. Move only this draw's indices to overlay-capable materials;
        // no per-frame model copy or duplicate coplanar mesh is necessary.
        for (from, to) in [
            (EntityMaterial::ItemCutout, EntityMaterial::EntityCutout),
            (
                EntityMaterial::ItemTranslucent,
                EntityMaterial::EntityTranslucent,
            ),
        ] {
            let [source, target] = out
                .indices
                .get_disjoint_mut([from as usize, to as usize])
                .expect("distinct block and entity material partitions");
            target.extend(source.drain(index_starts[from as usize]..));
        }
        for vertex in &mut out.vertices[start..] {
            vertex.overlay_uv = [15., 10.];
        }
    }
    Ok(Handled::Rendered)
}

pub(super) fn append_bolt(
    entity: &Entity,
    origin: [f64; 3],
    transform: super::super::Transform,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let timeline = &entity
        .source
        .as_ref()
        .ok_or("lightning timeline unavailable")?
        .special;
    if !timeline.lightning_visible() {
        return Ok(Handled::NoGeometry);
    }
    let position = translation(transform.position, origin)
        .transform_point3(Vec3::ZERO);
    append_lightning(timeline.lightning_seed, position, out)?;
    Ok(Handled::Rendered)
}

// A local xorshift stream makes the shape stable within a timeline flash without
// consuming the simulation RNG. Geometry, branches, segment count and radii are
// Krasnokamen-authored; the 128-block total height is from the public wiki infobox.
fn bolt_noise(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 40) as f32 / 16_777_215.) * 2. - 1.
}

fn append_lightning(seed: i64, position: Vec3, out: &mut DynamicGeometry) -> Result<()> {
    const STEPS: usize = 13;
    let mut random = (seed as u64).wrapping_add(0x6a09_e667_f3bc_c909);
    if random == 0 {
        random = 1;
    }
    let mut spine = [Vec3::ZERO; STEPS + 1];
    spine[0] = position;
    for step in 1..=STEPS {
        let previous = spine[step - 1];
        spine[step] = Vec3::new(
            previous.x + bolt_noise(&mut random) * 1.8,
            position.y + step as f32 * 128. / STEPS as f32,
            previous.z + bolt_noise(&mut random) * 1.8,
        );
    }
    for pair in spine.windows(2) {
        bolt_segment(pair[0], pair[1], out)?;
    }
    for junction in [5, 9] {
        let mut a = spine[junction];
        let direction = Vec3::new(bolt_noise(&mut random), -0.8, bolt_noise(&mut random));
        for _ in 0..4 {
            let b = a
                + direction * 4.
                + Vec3::new(bolt_noise(&mut random), 0., bolt_noise(&mut random));
            bolt_segment(a, b, out)?;
            a = b;
        }
    }
    Ok(())
}

fn bolt_segment(a: Vec3, b: Vec3, out: &mut DynamicGeometry) -> Result<()> {
    tube_segment(
        a,
        b,
        0.09,
        [0.94, 0.96, 1.],
        1.,
        EntityMaterial::Lightning,
        [[15.; 2]; 2],
        out,
    )?;
    tube_segment(
        a,
        b,
        0.28,
        [0.45, 0.5, 1.],
        0.2,
        EntityMaterial::Lightning,
        [[15.; 2]; 2],
        out,
    )
}

fn rope_point(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let horizontal = Vec3::new(b.x - a.x, 0., b.z - a.z).length();
    let sag = (horizontal * 0.1).clamp(0.08, 0.75);
    a.lerp(b, t) - Vec3::Y * (4. * sag * t * (1. - t))
}

fn leash_anchors(
    mob: &Entity,
    holder: &Entity,
    models: &Models,
    store: &Store,
    tick: u64,
    partial: f32,
) -> Result<([f64; 3], [f64; 3])> {
    fn anchor(
        entity: &Entity,
        models: &Models,
        store: &Store,
        tick: u64,
        partial: f32,
        holder: bool,
    ) -> Result<[f64; 3]> {
        let t = store.presented_transform(entity, tick, partial)?;
        let (dimensions, scale) = models.dimensions(entity)?;
        let height = dimensions.height * scale;
        let width = dimensions.width * scale;
        let yaw = entity
            .source
            .as_ref()
            .and_then(|s| s.living.as_ref())
            .map_or(t.yaw, |living| living.body(partial))
            .to_radians();
        let forward = Vec3::new(-yaw.sin(), 0., yaw.cos());
        let offset = if holder && entity.type_name == "minecraft:leash_knot" {
            // Center of the existing knot rig, whose main loop is above its tail.
            Vec3::Y * 0.75
        } else if holder && entity.type_name == "minecraft:player" {
            let side = if p::number(models, entity, "DATA_PLAYER_MAIN_HAND") == Some(0) {
                -1.
            } else {
                1.
            };
            Vec3::Y * height * 0.65
                + forward * width * 0.45
                + Vec3::new(yaw.cos(), 0., yaw.sin()) * width * 0.55 * side
        } else {
            Vec3::Y * height * 0.7 + forward * width * 0.5
        };
        Ok(std::array::from_fn(|i| {
            t.position[i] + f64::from(offset[i])
        }))
    }
    Ok((
        anchor(mob, models, store, tick, partial, false)?,
        anchor(holder, models, store, tick, partial, true)?,
    ))
}

fn leash_holder<'a>(entity: &Entity, store: &'a Store) -> Option<&'a Entity> {
    if entity.predicted_leash == Some(None) {
        return None;
    }
    if let Some(handle) = entity.leash_value() {
        return store
            .resolve(handle)
            .ok()
            .or_else(|| {
                store
                    .local_avatar
                    .as_ref()
                    .filter(|holder| holder.handle == handle)
            })
            .or_else(|| {
                store
                    .local_render_slot
                    .map(|slot| &store.slots[slot])
                    .filter(|holder| holder.handle == handle)
            });
    }
    // SET_ENTITY_LINK retains the network ID when the holder is the local
    // player (not a received entity row), or has not spawned yet.
    let id = entity.leash_holder_id?;
    store
        .get(id)
        .or_else(|| {
            store
                .local_avatar
                .as_ref()
                .filter(|holder| holder.handle.network_id == id)
        })
        .or_else(|| {
            store
                .local_render_slot
                .map(|slot| &store.slots[slot])
                .filter(|holder| holder.handle.network_id == id)
        })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Shares the single scene extraction frame"
)]
pub(super) fn append_leash(
    entity: &Entity,
    store: &Store,
    models: &Models,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light_source: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let Some(holder) = leash_holder(entity, store) else {
        return Ok(Handled::Unhandled);
    };
    let (mob_point, holder_point) = leash_anchors(entity, holder, models, store, tick, partial)?;
    let (Some(mob_light), Some(holder_light)) = (
        p::sampled(light_source, mob_point)?,
        p::sampled(light_source, holder_point)?,
    ) else {
        // Missing light at either rope endpoint must not suppress the mob's rig.
        return Ok(Handled::Unhandled);
    };
    let a = Vec3::from_array(std::array::from_fn(|i| (mob_point[i] - origin[i]) as f32));
    let b = Vec3::from_array(std::array::from_fn(|i| {
        (holder_point[i] - origin[i]) as f32
    }));
    const SEGMENTS: usize = 24;
    let light_at =
        |t: f32| std::array::from_fn(|i| mob_light[i] + (holder_light[i] - mob_light[i]) * t);
    for step in 0..SEGMENTS {
        let t0 = step as f32 / SEGMENTS as f32;
        let t1 = (step + 1) as f32 / SEGMENTS as f32;
        let color = if step % 2 == 0 {
            [0.38, 0.25, 0.13]
        } else {
            [0.3, 0.19, 0.09]
        };
        tube_segment(
            rope_point(a, b, t0),
            rope_point(a, b, t1),
            0.018,
            color,
            1.,
            EntityMaterial::Leash,
            [light_at(t0), light_at(t1)],
            out,
        )?;
    }
    // The mob still needs its ordinary rig after the rope is emitted.
    Ok(Handled::Unhandled)
}

#[expect(
    clippy::too_many_arguments,
    reason = "One bounded quad emitter for the two authored tube effects"
)]
fn tube_segment(
    a: Vec3,
    b: Vec3,
    radius: f32,
    color: [f32; 3],
    alpha: f32,
    material: EntityMaterial,
    lights: [[f32; 2]; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let axis = (b - a).normalize_or_zero();
    let reference = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = axis.cross(reference).normalize_or_zero() * radius;
    let v = axis.cross(u);
    let ring = [u + v, -u + v, -u - v, u - v];
    for face in 0..4 {
        let next = (face + 1) % 4;
        let first = out.vertices.len();
        p::emit(
            [
                a + ring[face],
                a + ring[next],
                b + ring[next],
                b + ring[face],
            ]
            .map(|point| point.to_array()),
            [[-1.; 2]; 4],
            color,
            0,
            material,
            alpha,
            lights[0],
            out,
        )?;
        out.vertices[first + 2].light = lights[1];
        out.vertices[first + 3].light = lights[1];
    }
    Ok(())
}

#[cfg(test)]
#[path = "world_fx_tests.rs"]
mod tests;
