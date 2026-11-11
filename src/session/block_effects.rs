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
        // Source Particle(x, y, z, xa, ya, za): the requested motion plus +-0.4 noise,
        // renormalised to a random speed, with a 0.1 upward bias.
        fn spread(next: &mut impl FnMut() -> f32, requested: [f64; 3]) -> [f64; 3] {
            let raw: [f64; 3] = std::array::from_fn(|i| requested[i] + f64::from(next() * 2. - 1.) * 0.4);
            let speed = f64::from(next() + next() + 1.) * 0.15;
            let length = raw.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9);
            let mut velocity = raw.map(|v| v / length * speed * 0.4);
            velocity[1] += 0.1;
            velocity
        }
        fn gaussian(next: &mut impl FnMut() -> f32) -> f64 {
            let (a, b) = (1. - next(), next());
            f64::from((-2. * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos())
        }
        // Bottom centre of the impact block (Vec3.atBottomCenterOf).
        let center = [f64::from(position[0]) + 0.5, f64::from(position[1]), f64::from(position[2]) + 0.5];
        let color = [16, 8, 0].map(|shift| ((data >> shift) & 255) as f32 / 255.);
        // Source LevelEventHandler.potionSplashParticles: eight ITEM particles of the
        // splash potion (BreakingItemParticle: a quarter of the item sprite, half the
        // default quad, gravity 1, a tenth of the base motion plus a gaussian kick).
        for _ in 0..8 {
            let kick = [gaussian(&mut next) * 0.15, f64::from(next()) * 0.2, gaussian(&mut next) * 0.15];
            let base = spread(&mut next, [0.; 3]);
            let size = 0.1 * (next() * 0.5 + 0.5);
            let lifetime = (4. / (next() * 0.9 + 0.1)) as u8;
            let (uo, vo) = (next() * 3., next() * 3.);
            let [su0, sv0, su1, sv1] = bottle.uv;
            let (u0, u1) = (su0 + (su1 - su0) * (uo + 1.) / 4., su0 + (su1 - su0) * uo / 4.);
            let (v0, v1) = (sv0 + (sv1 - sv0) * vo / 4., sv0 + (sv1 - sv0) * (vo + 1.) / 4.);
            self.fragments.push(Fragment {
                position: center, previous: center,
                velocity: std::array::from_fn(|i| base[i] * 0.1 + kick[i]),
                uv: [[u0, v1], [u1, v1], [u1, v0], [u0, v0]], layer: bottle.layer, color: [1.; 3],
                size, remaining: lifetime.max(1), spell: None,
            });
        }
        // Then 100 SpellParticles (EFFECT or INSTANT_EFFECT) tinted by the event RGB.
        for _ in 0..100 {
            let power = f64::from(next()) * 4.;
            let angle = f64::from(next()) * std::f64::consts::TAU;
            let requested = [angle.cos() * power, 0.01 + f64::from(next()) * 0.5, angle.sin() * power];
            let brightness = 0.75 + next() * 0.25;
            let at = [center[0] + requested[0] * 0.1, center[1] + 0.3, center[2] + requested[2] * 0.1];
            // SpellParticle ignores the requested horizontal motion: it seeds the base
            // particle with (0.5 - r, ya, 0.5 - r), damps the vertical part to a fifth
            // and a motionless request to a tenth; the level event then scales the
            // motion by its power (Particle.setPower).
            let base_request = [0.5 - f64::from(next()), requested[1], 0.5 - f64::from(next())];
            let mut velocity = spread(&mut next, base_request);
            velocity[1] *= 0.2;
            if requested[0] == 0. && requested[2] == 0. {
                velocity[0] *= 0.1;
                velocity[2] *= 0.1;
            }
            velocity[0] *= power;
            velocity[1] = (velocity[1] - 0.1) * power + 0.1;
            velocity[2] *= power;
            // SingleQuadParticle half size 0.1..0.2, three quarters for a spell.
            let size = 0.2 * (next() * 0.5 + 0.5) * 0.75;
            let lifetime = (8. / (next() * 0.8 + 0.2)) as u8;
            self.fragments.push(Fragment {
                position: at, previous: at, velocity,
                uv: uv(sprite), layer: sprite.layer, color: color.map(|v| v * brightness),
                size, remaining: lifetime, spell: Some((family, lifetime)),
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
            // Sample between the last two ticks so fragments glide. Terrain fragments
            // shrink out over their final eight ticks instead of vanishing; a spell
            // particle keeps its size and alpha until it expires (SpellParticle).
            let at = std::array::from_fn::<_, 3, _>(|i| {
                fragment.previous[i] + (fragment.position[i] - fragment.previous[i]) * f64::from(partial)
            });
            let fade = if fragment.spell.is_some() {
                1.
            } else {
                crate::motion::smoothstep((f32::from(fragment.remaining) - partial) / 8.)
            };
            if fade <= 0. { continue; }
            let [sky, block] = world.light_levels(at.map(|v| v.floor() as i32));
            let Some((sky, block)) = sky.zip(block) else { continue };
            let center = std::array::from_fn::<_, 3, _>(|i| (at[i] - origin[i]) as f32);
            let positions = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, y]| {
                std::array::from_fn(|i| center[i] + fragment.size * fade * (right[i] * x + up[i] * y))
            });
            let (uv, layer, material) = if let Some((family, lifetime)) = fragment.spell {
                // Source SpriteSet.get(age, lifetime): age * (frames - 1) / lifetime.
                let age = lifetime - fragment.remaining;
                let index = (usize::from(age) * 7 / usize::from(lifetime)).min(7);
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
    #[test]
    fn potion_impact_spell_particles_spread_size_and_expire_like_the_source() {
        // Source LevelEventHandler.potionSplashParticles + SpellParticle + Particle:
        // power 0..4 scales a 0.06..0.18 base motion, half size 0.075..0.15, 8..40 ticks.
        let mut content = Content::test_cube();
        let region = TextureRegion { layer: 3, uv: [0.25, 0.5, 0.5, 0.75], width: 16, height: 16 };
        for prefix in ["effect", "spell"] {
            for frame in 0..8 {
                content.insert_test_texture(&format!("minecraft:particle/{prefix}_{frame}"), region);
            }
        }
        content.insert_test_texture("minecraft:item/splash_potion", region);
        let world = World::new(0, 16);
        let mut effects = BlockEffects::default();
        assert!(effects.potion(&content, [0, 3, 0], 0x385dc6, false, 7).unwrap());
        let spells: Vec<_> = effects.fragments.iter().filter(|f| f.spell.is_some()).copied().collect();
        let shards: Vec<_> = effects.fragments.iter().filter(|f| f.spell.is_none()).copied().collect();
        assert_eq!((spells.len(), shards.len()), (100, 8));
        assert!(spells.iter().all(|f| (0.075..=0.15).contains(&f.size)));
        assert!(spells.iter().any(|f| f.size > 0.13) && spells.iter().any(|f| f.size < 0.09));
        assert!(spells.iter().all(|f| (8..=40).contains(&f.remaining)));
        assert!(spells.iter().all(|f| (f.position[1] - 3.3).abs() < 1e-9));
        let horizontal = |f: &Fragment| f.velocity[0].hypot(f.velocity[2]);
        let fastest = spells.iter().map(horizontal).fold(0., f64::max);
        assert!(fastest > 0.3 && fastest < 0.75, "{fastest}");
        // Bottle shards: a quarter of the item sprite, half a default quad.
        assert!(shards.iter().all(|f| (0.05..=0.1).contains(&f.size)));
        for shard in &shards {
            let us = shard.uv.map(|p| p[0]);
            assert!(((us[1] - us[0]).abs() - 0.0625).abs() < 1e-6 && us.iter().all(|u| (0.25..=0.5).contains(u)));
        }
        for _ in 0..12 {
            effects.tick(&world, &content);
        }
        let reach = effects.fragments.iter().filter(|f| f.spell.is_some())
            .map(|f| (f.position[0] - 0.5).hypot(f.position[2] - 0.5)).fold(0., f64::max);
        assert!(reach > 2., "the cloud spreads blocks wide within twelve ticks: {reach}");
    }
}
