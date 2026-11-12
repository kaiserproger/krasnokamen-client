//! Krasnokamen-authored GUI item bars, from public minecraft.wiki Durability/Bundle descriptions.
//! Geometry/palette/rounding are our approximations, not extracted renderer constants.
use crate::{
    api::{MeshGeometry, RenderLayer},
    inventory::Stack,
    protocol::{Result, content::Content},
    ui::render::Painter,
};
use glam::Mat4;
use serde_json::Value;

// In a 16-unit icon: a 12-unit track at (2, 13), two units high, with a
// one-unit coloured stripe. Scale with the icon, not the item's model transform.
const TRACK: [f32; 4] = [2. / 16., 13. / 16., 12. / 16., 2. / 16.];
const EMPTY: [f32; 3] = [0.18; 3];
const BUNDLE_BLUE: [f32; 3] = [0.22, 0.48, 0.92];
const BUNDLE_FULL: [f32; 3] = [0.90, 0.18, 0.18];

/// Called once by the GUI stack painter after the icon/foil, before count text.
/// Uses the effective components, so received removals and overrides are respected.
pub(super) fn append(
    content: &Content,
    stack: &Stack,
    [x, y, w, h]: [f32; 4],
    transform: Mat4,
    out: &mut MeshGeometry,
) -> Result<()> {
    if stack.empty() {
        return Ok(());
    }
    let (fraction, color, is_bundle) = if stack.component("minecraft:bundle_contents").is_some() {
        let fullness = bundle_fullness(stack, 0)?;
        if fullness <= 0. {
            return Ok(());
        }
        // Display-only tolerance for floating-point sums of custom stack-size
        // reciprocals. Inventory admission continues to own exact rational weights.
        let full = fullness >= 1. - 16. * f64::EPSILON;
        (
            fullness.min(1.) as f32,
            if full { BUNDLE_FULL } else { BUNDLE_BLUE },
            true,
        )
    } else {
        if stack.component("minecraft:unbreakable").is_some() {
            return Ok(());
        }
        let damage = stack
            .component("minecraft:damage")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let maximum = stack
            .component("minecraft:max_damage")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if damage <= 0 || maximum <= 0 {
            return Ok(());
        }
        let remaining = 1. - damage.min(maximum) as f64 / maximum as f64;
        let remaining = remaining as f32;
        // Our two linear ramps interpolate green -> yellow -> orange -> red.
        let color = [
            (2. * (1. - remaining)).min(1.),
            (2. * remaining).min(1.),
            0.,
        ];
        (remaining, color, false)
    };
    let [left, top, width, height] = TRACK;
    let track = [x + left * w, y + top * h, width * w, height * h];
    let mut painter = Painter {
        content,
        out,
        transform,
        light: [-1.; 2],
        render_layer: RenderLayer::Translucent,
    };
    painter.solid(track, EMPTY, 1.);
    // Durability can round down to an empty track at the last few uses. A
    // nonempty bundle always has at least one visible logical pixel of blue.
    let filled = track[2] * fraction;
    let filled = if is_bundle {
        filled.round().max(1.).min(track[2])
    } else {
        filled.floor()
    };
    if filled > 0. {
        painter.solid([track[0], track[1], filled, track[3] / 2.], color, 1.);
    }
    Ok(())
}

/// Approximate visual occupancy, borrowing the decoder's retained nested Stacks;
/// no template re-encoding, cloning or per-frame allocation. Capacity semantics
/// (reciprocal max stack size, nested overhead and bees) are described by the wiki.
fn bundle_fullness(bundle: &Stack, depth: u8) -> Result<f64> {
    if depth > 32 {
        return Err("GUI bundle fullness exceeds retained stack depth bound".into());
    }
    let entries = bundle
        .component("minecraft:bundle_contents")
        .and_then(Value::as_array)
        .ok_or("GUI bundle contents is not a list")?;
    if entries.is_empty() {
        return Ok(0.);
    }
    let stacks = bundle
        .patch
        .added
        .values()
        .find(|value| value.name == "minecraft:bundle_contents")
        .map(|value| value.nested_stacks.as_ref())
        .filter(|stacks| stacks.len() == entries.len())
        .ok_or("GUI bundle contents lacks retained nested stacks")?;
    let mut fullness = 0.;
    for stack in stacks {
        if stack.empty() {
            continue;
        }
        let weight = if stack.component("minecraft:bundle_contents").is_some() {
            1. / 16. + bundle_fullness(stack, depth + 1)?
        } else if stack
            .component("minecraft:bees")
            .and_then(Value::as_array)
            .is_some_and(|bees| !bees.is_empty())
        {
            1.
        } else {
            1. / f64::from(stack.max_count()?)
        };
        fullness += weight * f64::from(stack.count);
        if fullness >= 1. {
            return Ok(1.);
        }
    }
    Ok(fullness)
}
