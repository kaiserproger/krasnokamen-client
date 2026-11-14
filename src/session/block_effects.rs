//! Bounded received level-event effects: terrain fragments and potion impacts.
//! Simulation advances on game ticks, never render frames.
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::{Content, TextureRegion}, models::TintKind, world::World},
};

impl super::Session {
    pub(super) fn present_block_break(&mut self, event: i32, position: [i32; 3], data: i32) -> Result<bool> {
        if !self.presentation { return Ok(false); }
        if matches!(event, 2002 | 2007) {
            let content = self.content.as_ref().ok_or("content unavailable")?;
            return self.block_effects.potion(content, position, data, event == 2007,
                self.client_tick ^ self.packet_sequence);
        }
        if event != 2001 || data < 0 { return Ok(false); }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        if content.info(data as u32).is_err() {
            return Ok(false);
        }
        self.block_effects.burst(content, &self.world, position, data as u32, self.client_tick ^ self.packet_sequence)
    }
}

const MAX_FRAGMENTS: usize = 2048;
#[derive(Clone, Copy)]
struct Fragment {
    position: [f64; 3],
    previous: [f64; 3],
    velocity: [f64; 3],
    uv: [[f32; 2]; 4],
    layer: u32,
    color: [f32; 3],
    size: f32,
    remaining: u8,
    // (sprite family, original lifetime); None is a falling terrain/bottle fragment.
    spell: Option<(usize, u8)>,
}
#[derive(Default)]
pub(super) struct BlockEffects {
    fragments: Vec<Fragment>,
    spell_sprites: [Option<[TextureRegion; 8]>; 2],
}
impl BlockEffects {
    pub fn clear(&mut self) {
        self.fragments.clear();
        self.spell_sprites = [None; 2];
    }
    pub fn burst(&mut self, content: &Content, world: &World, position: [i32; 3], state: u32, seed: u64) -> Result<bool> {
        if content.block_is_air(state)? {
            return Ok(false);
        }
        let Some(model) = content.model_for(state)? else { return Ok(false) };
        // Prefer an untinted body face over a grass overlay. Every selected UV
        // remains a real fragment of this state, including resource-pack models.
        let mut sprite = None;
        model.for_each_quad(position, |quad| {
            if sprite.is_none() || sprite.is_some_and(|(_, _, tint)| tint != TintKind::None) && quad.tint == TintKind::None {
                sprite = Some((quad.uv, quad.texture_layer, quad.tint));
            }
        });
        let Some((source_uv, layer, tint)) = sprite else { return Ok(false) };
        let color = match tint {
            TintKind::None => [1.; 3],
            TintKind::Constant(color) => color,
            tint => {
                let Some(biome) = world.biome(position) else { return Ok(false) };
                let biome = content.registry_entry("minecraft:worldgen/biome", biome as i32)?;
                let colors = content.biome_colors(biome, f64::from(position[0]), f64::from(position[2]))?;
                match tint {
                    TintKind::Grass => colors.grass,
                    TintKind::Foliage => colors.foliage,
                    TintKind::DryFoliage => colors.dry_foliage,
                    TintKind::Water => colors.water,
                    TintKind::None | TintKind::Constant(_) => unreachable!(),
                }
            }
        }.map(|channel| channel * 0.6);
        let min = source_uv.into_iter().fold([f32::INFINITY; 2], |a, b| [a[0].min(b[0]), a[1].min(b[1])]);
        let max = source_uv.into_iter().fold([f32::NEG_INFINITY; 2], |a, b| [a[0].max(b[0]), a[1].max(b[1])]);
        if self.fragments.len() + 64 > MAX_FRAGMENTS {
            let remove = (self.fragments.len() + 64 - MAX_FRAGMENTS).min(self.fragments.len());
            self.fragments.drain(..remove);
        }
        let mut random = seed ^ 0xa0761d6478bd642f;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            (random >> 40) as f32 / (1u32 << 24) as f32
        };
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let offset = [x, y, z].map(|n| (f64::from(n) + 0.5) / 4.);
                    let u = next() * 0.75;
                    let v = next() * 0.75;
                    let uv = [[u, v + 0.25], [u + 0.25, v + 0.25], [u + 0.25, v], [u, v]]
                        .map(|p| [min[0] + p[0] * (max[0] - min[0]), min[1] + p[1] * (max[1] - min[1])]);
                    self.fragments.push(Fragment {
                        position: std::array::from_fn(|i| f64::from(position[i]) + offset[i]),
                        previous: std::array::from_fn(|i| f64::from(position[i]) + offset[i]),
                        velocity: [(offset[0] - 0.5) * 0.22, 0.08 + next() as f64 * 0.12, (offset[2] - 0.5) * 0.22],
                        uv,
                        layer,
                        color,
                        size: 0.05 + next() * 0.05,
                        remaining: 20 + (next() * 20.) as u8,
                        spell: None,
                    });
                }
            }
        }
        Ok(true)
    }
    /// 26.3 LevelEventHandler: 2002 is EFFECT, 2007 is INSTANT_EFFECT
    /// (not a splash/lingering discriminator). RGB is the received event data.
    pub fn potion(&mut self, content: &Content, position: [i32; 3], data: i32, instant: bool, seed: u64) -> Result<bool> {
        let family = usize::from(instant);
        if self.spell_sprites[family].is_none() {
            let prefix = if instant { "spell" } else { "effect" };
            let lookup = |frame| {
                let name = format!("minecraft:particle/{prefix}_{frame}");
                content.texture(&name).ok_or_else(|| format!("potion impact sprite missing: {name}"))
            };
            let mut sprites = [lookup(7)?; 8];
            for (index, sprite) in sprites.iter_mut().enumerate().skip(1) {
                *sprite = lookup(7 - index)?;
            }
            self.spell_sprites[family] = Some(sprites);
        }
        let bottle = content.texture("minecraft:item/splash_potion")
            .ok_or("potion impact bottle sprite missing")?;
        let sprite = self.spell_sprites[family].as_ref().unwrap()[0];
        let uv = |sprite: TextureRegion| {
            let [u0, v0, u1, v1] = sprite.uv;
            [[u0, v1], [u1, v1], [u1, v0], [u0, v0]]
        };
        if self.fragments.len() + 108 > MAX_FRAGMENTS {
            self.fragments.drain(..self.fragments.len() + 108 - MAX_FRAGMENTS);
        }
        let mut random = seed ^ 0xa0761d6478bd642f;
        let mut next = || {
            random ^= random << 13; random ^= random >> 7; random ^= random << 17;
            (random >> 40) as f32 / (1u32 << 24) as f32
        };
        let center = [f64::from(position[0]) + 0.5, f64::from(position[1]), f64::from(position[2]) + 0.5];
        let color = [16, 8, 0].map(|shift| ((data >> shift) & 255) as f32 / 255.);
        // Bottle shards use the real item sprite, not a block-state substitute.
        for _ in 0..8 {
            self.fragments.push(Fragment {
                position: center, previous: center,
                velocity: [(f64::from(next()) - 0.5) * 0.3, f64::from(next()) * 0.2, (f64::from(next()) - 0.5) * 0.3],
                uv: uv(bottle), layer: bottle.layer, color: [1.; 3],
                size: 0.04, remaining: 20 + (next() * 20.) as u8, spell: None,
            });
        }
        // Authored deterministic motion with the observed radial spawn, tint
        // brightness, rising spell gravity and finite vanilla lifetime range.
        for _ in 0..100 {
            let radius = next() * 4.;
            let angle = next() * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            let at = [center[0] + f64::from(cos * radius * 0.1), center[1] + 0.3,
                center[2] + f64::from(sin * radius * 0.1)];
            let brightness = 0.75 + next() * 0.25;
            let lifetime = (8. / (next() * 0.8 + 0.2)) as u8;
            self.fragments.push(Fragment {
                position: at, previous: at,
                velocity: [(f64::from(next()) - 0.5) * 0.1, 0.002 + f64::from(next()) * 0.1,
                    (f64::from(next()) - 0.5) * 0.1],
                uv: uv(sprite), layer: sprite.layer, color: color.map(|v| v * brightness),
                size: 0.075, remaining: lifetime, spell: Some((family, lifetime)),
            });
        }
        Ok(true)
    }
    pub fn tick(&mut self, world: &World, content: &Content) {
        self.fragments.retain_mut(|fragment| {
            fragment.remaining = fragment.remaining.saturating_sub(1);
            if fragment.remaining == 0 { return false; }
            fragment.velocity[1] += if fragment.spell.is_some() { 0.004 } else { -0.04 };
            let old = fragment.position;
            let mut next = std::array::from_fn(|i| old[i] + fragment.velocity[i]);
            let cell = next.map(|v| v.floor() as i32);
            if fragment.spell.is_none() && fragment.velocity[1] < 0.
                && let Some(state) = world.block(cell)
                && let Some(boxes) = content.collision_boxes(state)
            {
                for bounds in boxes {
                    let top = f64::from(cell[1]) + bounds[4];
                    if old[1] >= top && next[1] <= top
                        && next[0] >= f64::from(cell[0]) + bounds[0]
                        && next[0] <= f64::from(cell[0]) + bounds[3]
                        && next[2] >= f64::from(cell[2]) + bounds[2]
                        && next[2] <= f64::from(cell[2]) + bounds[5]
                    {
                        next[1] = top + 0.01;
                        fragment.velocity[1] = 0.;
                        fragment.velocity[0] *= 0.7;
                        fragment.velocity[2] *= 0.7;
                    }
                }
            }
            fragment.previous = old;
            fragment.position = next;
            let friction = if fragment.spell.is_some() { 0.96 } else { 0.98 };
            fragment.velocity = fragment.velocity.map(|v| v * friction);
            true
        });
    }
    pub fn append(&self, world: &World, origin: [f64; 3], yaw: f32, pitch: f32, partial: f32, out: &mut DynamicGeometry) -> Result<()> {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let right = [cy, 0., sy];
        let up = [-sy * sp, cp, cy * sp];
        for fragment in &self.fragments {
            // Sample between the last two ticks so fragments glide, and shrink
            // them out over the final eight ticks instead of vanishing.
            let at = std::array::from_fn::<_, 3, _>(|i| {
                fragment.previous[i] + (fragment.position[i] - fragment.previous[i]) * f64::from(partial)
            });
            let fade = crate::motion::smoothstep((f32::from(fragment.remaining) - partial) / 8.);
            if fade <= 0. { continue; }
            let [sky, block] = world.light_levels(at.map(|v| v.floor() as i32));
            let Some((sky, block)) = sky.zip(block) else { continue };
            let center = std::array::from_fn::<_, 3, _>(|i| (at[i] - origin[i]) as f32);
            let positions = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, y]| {
                std::array::from_fn(|i| center[i] + fragment.size * fade * (right[i] * x + up[i] * y))
            });
            let (uv, layer, material) = if let Some((family, lifetime)) = fragment.spell {
                let age = lifetime - fragment.remaining;
                let index = (usize::from(age) * 8 / usize::from(lifetime)).min(7);
                let sprite = self.spell_sprites[family].as_ref().ok_or("potion sprites unavailable")?[index];
                let [u0, v0, u1, v1] = sprite.uv;
                ([[u0, v1], [u1, v1], [u1, v0], [u0, v0]], sprite.layer, EntityMaterial::EntityTranslucent)
            } else {
                (fragment.uv, fragment.layer, EntityMaterial::EntityCutout)
            };
            crate::entities::presentation_emit(positions, uv, fragment.color, layer,
                material, 1., [f32::from(sky), f32::from(block)], out)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragment_burst_uses_block_uvs_gravity_and_bounded_lifetime() {
        let content = Content::test_cube();
        let mut effects = BlockEffects::default();
        assert!(effects.burst(&content, &World::new(0, 16), [0, 3, 0], 1, 9).unwrap());
        assert_eq!(effects.fragments.len(), 64);
        let first = effects.fragments[0];
        assert!(first.uv.iter().all(|p| p.iter().all(|v| (0. ..=1.).contains(v))));
        effects.tick(&World::new(0, 16), &content);
        assert!(effects.fragments[0].velocity[1] < first.velocity[1]);
        assert_ne!(effects.fragments[0].position, first.position);
        for _ in 0..40 { effects.tick(&World::new(0, 16), &content); }
        assert!(effects.fragments.is_empty());
    }
    #[test]
    fn burst_budget_and_reset_are_bounded() {
        let content = Content::test_cube();
        let mut effects = BlockEffects::default();
        for seed in 0..40 { effects.burst(&content, &World::new(0, 16), [0, 3, 0], 1, seed).unwrap(); }
        assert_eq!(effects.fragments.len(), MAX_FRAGMENTS);
        assert!(!effects.burst(&content, &World::new(0, 16), [0, 3, 0], 0, 1).unwrap());
        effects.clear();
        assert!(effects.fragments.is_empty());
    }
}
