//! Vanilla 26.3 entity draw distance: the gate `EntityRenderer.shouldRender` applies before
//! frustum culling (`Entity.shouldRender(camX, camY, camZ)` → `shouldRenderAtSqrDistance`
//! and its overrides). An entity it rejects is not drawn: no body, shadow or nameplate. The
//! vanilla client still ticks it; here, while frames draw entities, a mob it rejects waits like
//! one outside the frame's radius and catches up when drawn again (`tick_presentation`,
//! docs/ENTITY_PRESENTATION.md). Bytecode: receipts/BvItemCache/javap-26.3-entity-distance.txt.

use super::{Entity, metadata::Metadata, resources::Models};

/// `Options.entityDistanceScaling`: slider stops 2..=20 mapped by `i / 4.0`, codec
/// `doubleRange(0.5, 5.0)`, default 1.0, options.txt key `entityDistanceScaling`.
pub(crate) const MIN_SCALING: f64 = 0.5;
pub(crate) const MAX_SCALING: f64 = 5.0;
pub(crate) const SCALING_STEP: f64 = 0.25;
pub(crate) const DEFAULT_SCALING: f64 = 1.0;

/// `LevelExtractor.extractVisibleEntities`: `Entity.setViewScale(clamp(effective render
/// distance / 8, 1, 2.5) * entityDistanceScaling)`.
pub(crate) fn view_scale(effective_render_distance: u32, scaling: f64) -> f64 {
    (f64::from(effective_render_distance) / 8.).clamp(1., 2.5) * scaling
}

/// Which `shouldRenderAtSqrDistance` (or renderer `shouldRender` override that can draw an
/// entity the distance rejects) a kind uses, resolved once per kind (`Ids::view_rule`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `Entity`: bbox size (NaN: 1) * 64 * viewScale.
    Entity,
    /// `RemotePlayer`, `AbstractArrow`: bbox size * 10 (NaN: 1) * 64 * viewScale.
    TenSizes,
    /// `Display`: view range * 64 * viewScale.
    Display,
    /// `LightningBolt`: 64 * viewScale.
    Lightning,
    /// `EndCrystal` (and `EndCrystalRenderer`): `Entity`, or any crystal with a beam target.
    EndCrystal,
    /// `ArmorStand`: bbox size * 4 (NaN or 0: 4) * 64, without viewScale.
    ArmorStand,
    /// `ItemFrame`: 16 * 64 * viewScale.
    ItemFrame,
    /// `LeashFenceKnotEntity`: within 32 blocks.
    LeashKnot,
    /// `Phantom` always; `FishingHook` drawn by `FishingHookRenderer` whenever it has a player
    /// owner, which a client hook always has.
    Always,
    /// `ThrowableProjectile`, `EyeOfEnder`: not within 3.5 blocks in its first 2 ticks, then
    /// bbox size * 4 (NaN: 4) * 64, without viewScale.
    Throwable,
    /// `AbstractHurtingProjectile`: bbox size * 4 (NaN: 4) * 64, without viewScale.
    Hurting,
    /// `Fireball`, `WindCharge`: not within 3.5 blocks in its first 2 ticks, then `Hurting`.
    NearStartHurting,
    /// `FireworkRocketEntity`: within 64 blocks unless attached to an entity.
    Firework,
    /// `ShulkerBullet`: within 128 blocks.
    ShulkerBullet,
    /// `GuardianRenderer`: `Entity`, or any guardian with an active attack target (its beam).
    Guardian,
}

impl Rule {
    pub(crate) fn of(hierarchy: &[String], player: bool) -> Self {
        let is = |class: &str| hierarchy.iter().any(|entry| entry == class);
        const P: &str = "net.minecraft.world.entity.";
        let class = |name: &str| format!("{P}{name}");
        if player {
            Self::TenSizes
        } else if is(&class("Display")) {
            Self::Display
        } else if is(&class("LightningBolt")) {
            Self::Lightning
        } else if is(&class("boss.enderdragon.EndCrystal")) {
            Self::EndCrystal
        } else if is(&class("decoration.ArmorStand")) {
            Self::ArmorStand
        } else if is(&class("decoration.ItemFrame")) {
            Self::ItemFrame
        } else if is(&class("decoration.LeashFenceKnotEntity")) {
            Self::LeashKnot
        } else if is(&class("monster.Phantom")) || is(&class("projectile.FishingHook")) {
            Self::Always
        } else if is(&class("projectile.EyeOfEnder"))
            || is(&class("projectile.ThrowableProjectile"))
        {
            Self::Throwable
        } else if is(&class("projectile.FireworkRocketEntity")) {
            Self::Firework
        } else if is(&class("projectile.ShulkerBullet")) {
            Self::ShulkerBullet
        } else if is(&class("projectile.arrow.AbstractArrow")) {
            Self::TenSizes
        } else if is(&class("projectile.hurtingprojectile.Fireball"))
            || is(&class("projectile.hurtingprojectile.windcharge.WindCharge"))
        {
            Self::NearStartHurting
        } else if is(&class(
            "projectile.hurtingprojectile.AbstractHurtingProjectile",
        )) {
            Self::Hurting
        } else if is(&class("monster.Guardian")) {
            Self::Guardian
        } else {
            Self::Entity
        }
    }
}

/// Whether vanilla draws `e` at `distance_squared` from the camera, `position` being where
/// it is drawn (vanilla measures from its tick position; the drawn one is at most a tick of
/// motion away).
pub(super) fn within(
    models: &Models,
    e: &Entity,
    position: [f64; 3],
    distance_squared: f64,
    view_scale: f64,
) -> bool {
    let rule = models.ids(e).view_rule;
    // AABB.getSize: the mean of the box's three sides (NaN when the box cannot be resolved).
    let size = || {
        super::physical::bounds_at(models, e, position).map_or(f64::NAN, |b| {
            ((b.max[0] - b.min[0]) + (b.max[1] - b.min[1]) + (b.max[2] - b.min[2])) / 3.
        })
    };
    let or = |value: f64, fallback: f64| if value.is_nan() { fallback } else { value };
    let range = |reach: f64| distance_squared < reach * reach;
    let young =
        || e.source.as_ref().is_some_and(|source| source.age < 2) && distance_squared < 12.25;
    let value = |name| super::presentation::value(models, e, name);
    match rule {
        Rule::Entity => range(or(size(), 1.) * 64. * view_scale),
        Rule::TenSizes => range(or(size() * 10., 1.) * 64. * view_scale),
        Rule::Display => {
            let view_range = match value("DATA_VIEW_RANGE_ID") {
                Some(Metadata::Float(range)) => f64::from(*range),
                _ => 1.,
            };
            range(view_range * 64. * view_scale)
        }
        Rule::Lightning => range(64. * view_scale),
        Rule::EndCrystal => {
            range(or(size(), 1.) * 64. * view_scale)
                || matches!(
                    value("DATA_BEAM_TARGET"),
                    Some(Metadata::OptionalBlockPosition(Some(_)))
                )
        }
        Rule::ArmorStand => {
            let reach = size() * 4.;
            range(
                if reach.is_nan() || reach == 0. {
                    4.
                } else {
                    reach
                } * 64.,
            )
        }
        Rule::ItemFrame => range(16. * 64. * view_scale),
        Rule::LeashKnot => distance_squared < 1024.,
        Rule::Always => true,
        Rule::Throwable => !young() && range(or(size() * 4., 4.) * 64.),
        Rule::Hurting => range(or(size() * 4., 4.) * 64.),
        Rule::NearStartHurting => !young() && range(or(size() * 4., 4.) * 64.),
        Rule::Firework => {
            distance_squared < 4096.
                && !matches!(
                    value("DATA_ATTACHED_TO_TARGET"),
                    Some(Metadata::OptionalUnsignedInt(Some(_)))
                )
        }
        Rule::ShulkerBullet => distance_squared < 16384.,
        Rule::Guardian => {
            range(or(size(), 1.) * 64. * view_scale)
                || super::presentation::number(models, e, "DATA_ID_ATTACK_TARGET")
                    .is_some_and(|id| id != 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::content::Content;
    use std::sync::Arc;

    /// The frame draws no body for an entity past its vanilla distance although the frame's
    /// radius reaches it, and draws it again once Entity Distance brings it within.
    #[test]
    fn frame_skips_entities_past_their_vanilla_distance() {
        let (models, entity) = super::super::animation_tests::fixture();
        let size = {
            let b = super::super::physical::bounds_at(&models, &entity, entity.transform.position)
                .unwrap();
            ((b.max[0] - b.min[0]) + (b.max[1] - b.min[1]) + (b.max[2] - b.min[2])) / 3.
        };
        let id = entity.handle.network_id;
        let mut store = super::super::Store::new();
        store.models = Some(Arc::new(models));
        store.ids.insert(id, 0);
        store.spatial.entry([0, 0]).or_default().insert(id);
        let position = entity.transform.position;
        store.slots.push(entity);
        let content = Content::test_cube();
        // Past the reach at view scale 0.2, within the frame radius (32) and the reach at 1.
        let distance = (size * 64. * 0.2 + 32.) / 2.;
        assert!(size * 64. * 0.2 < distance && distance < 32. && distance < size * 64.);
        for (scale, drawn) in [(0.2, false), (1., true)] {
            store.set_view_scale(scale);
            let mut geometry = crate::api::DynamicGeometry::default();
            let stats = super::super::presentation::append(
                &mut store,
                crate::entities::EntityRenderFrame {
                    maps: crate::ui::MapRenderFixture::empty().view(),
                    tint_source: &|_, _| Ok([1.; 3]),
                    content: &content,
                    origin: [0.; 3],
                    camera: [position[0] + distance, position[1], position[2]],
                    camera_rotation: [0.; 2],
                    radius: 32.,
                    tick: 0,
                    partial: 0.,
                    game_time: 0,
                    clock_environment: None,
                    light: &|_| Ok(Some([15., 0.])),
                    bed_direction: &|_| None,
                },
                &mut geometry,
            )
            .unwrap();
            assert_eq!(
                stats.eligible == 1,
                drawn,
                "view scale {scale}: {} eligible",
                stats.eligible
            );
        }
    }
}
