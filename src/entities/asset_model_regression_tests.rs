//! Asset regressions for MODEL-OPUS-6/8/9/N8/N9/N10 and the shulker shell finding.
//!
//! These inspect the baked geometry/UVs consumed by rendering, not JSON spellings.
//! They protect silhouette, attachment and texture handedness rather than freezing
//! every authored coordinate. Original observations: model-review-opus-20261102
//! sections 3.2/3.4 and model-review-opus-finish-20261102 N8–N10. Texture islands
//! were inspected in the locally supplied 26.3 atlas; no oracle pixels are embedded.
//! Native/Java frame comparisons and isolated negative controls belong to Main.

use super::{
    krasnokamen_models,
    resources::{self, Rig},
};
use glam::{Mat4, Vec3};

type Quad = [[f32; 5]; 4];

fn rig(id: &str) -> &'static Rig {
    krasnokamen_models::authored(&format!("minecraft:{id}#main"))
        .unwrap_or_else(|| panic!("missing embedded asset {id}"))
}

fn rest(rig: &Rig) -> Vec<Mat4> {
    let mut transforms = Vec::with_capacity(rig.bones.len());
    for bone in &rig.bones {
        let parent = bone
            .parent
            .map_or(Mat4::IDENTITY, |index| transforms[index]);
        transforms.push(parent * resources::transform(bone.pivot, bone.rotation, bone.scale));
    }
    transforms
}

fn bounds(rig: &Rig, transforms: &[Mat4], name: &str) -> (Vec3, Vec3) {
    let index = rig.part_by_name[name];
    let bone = &rig.bones[index];
    assert!(!bone.quads.is_empty(), "{name} must emit geometry");
    bone.quads
        .iter()
        .flatten()
        .fold((Vec3::MAX, Vec3::MIN), |(low, high), vertex| {
            let point =
                transforms[index].transform_point3(Vec3::new(vertex[0], vertex[1], vertex[2]));
            (low.min(point), high.max(point))
        })
}

fn face<'a>(rig: &'a Rig, name: &str, normal: Vec3) -> &'a Quad {
    let bone = &rig.bones[rig.part_by_name[name]];
    bone.quads
        .iter()
        .zip(&bone.normals)
        .find(|(_, n)| Vec3::from_array(**n).dot(normal) > 0.99)
        .map(|(quad, _)| quad)
        .unwrap_or_else(|| panic!("{name}: missing face {normal}"))
}

fn uv_bounds(quad: &Quad) -> [f32; 4] {
    quad.iter()
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |mut span, v| {
            span[0] = span[0].min(v[3]);
            span[1] = span[1].min(v[4]);
            span[2] = span[2].max(v[3]);
            span[3] = span[3].max(v[4]);
            span
        })
}

// Direction of increasing texture U in model space. Unlike a mirror flag check,
// this also catches an accidental face override that reverses the actual texels.
fn texture_right(quad: &Quad) -> Vec3 {
    let low = quad.iter().min_by(|a, b| a[3].total_cmp(&b[3])).unwrap();
    let high = quad.iter().max_by(|a, b| a[3].total_cmp(&b[3])).unwrap();
    assert!(high[3] > low[3]);
    Vec3::new(high[0] - low[0], high[1] - low[1], high[2] - low[2])
}

#[test]
fn cow_family_front_and_hind_hooves_have_equal_body_setback() {
    for id in ["cow", "mooshroom"] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        for side in ["left", "right"] {
            let front = bounds(rig, &pose, &format!("{side}_front_leg"));
            let hind = bounds(rig, &pose, &format!("{side}_hind_leg"));
            let front_inset = front.0.z - body.0.z;
            let hind_inset = body.1.z - hind.1.z;
            assert!(
                front_inset > 0.,
                "{id}: front hoof must not project to the chest edge"
            );
            assert!(
                (front_inset - hind_inset).abs() < 1e-5,
                "{id}: unequal leg alignment"
            );
            assert!(
                (front.1.y - hind.1.y).abs() < 1e-5,
                "{id}: hooves share a floor"
            );
        }
    }
}

#[test]
fn glow_squid_tentacle_front_texels_face_the_ring_center() {
    let rig = rig("glow_squid");
    let pose = rest(rig);
    for index in 0..8 {
        let name = format!("tentacle{index}");
        let bone = rig.part_by_name[&name];
        let front = face(rig, &name, -Vec3::Z);
        let p = |v: [f32; 5]| pose[bone].transform_point3(Vec3::new(v[0], v[1], v[2]));
        let normal = (p(front[1]) - p(front[0]))
            .cross(p(front[2]) - p(front[0]))
            .normalize();
        let pivot = pose[bone].transform_point3(Vec3::ZERO);
        let radial = Vec3::new(pivot.x, 0., pivot.z).normalize();
        assert!(
            normal.dot(radial) < -0.999,
            "{name}: texture panel twists around the limb"
        );
    }
}

#[test]
fn bed_item_uses_resource_model_halves_without_a_legacy_entity_texture() {
    use crate::protocol::content::{Content, TextureRegion};
    let mut content = Content::test_cube();
    content.item_names.insert(1, "minecraft:red_bed".into());
    content.insert_test_resource(
        "assets/minecraft/items/red_bed.json",
        br#"{
        "model":{"type":"minecraft:composite","models":[
            {"type":"minecraft:model","model":"minecraft:block/red_bed_head"},
            {"type":"minecraft:model","model":"minecraft:block/red_bed_foot",
             "transformation":{"translation":[0,0,1]}}
        ]}
    }"#,
    );
    // Independent cube fixtures exercise the supplied 26.3 resource graph's
    // normal-model path, not the deleted 26.1.2 special/entity-texture path.
    for (part, layer) in [("head", 5), ("foot", 6)] {
        let texture = format!("minecraft:block/red_bed_{part}_up");
        content.insert_test_texture(
            &texture,
            TextureRegion {
                layer,
                uv: [0., 0., 1., 1.],
                width: 16,
                height: 16,
            },
        );
        let model = serde_json::json!({"textures":{"bed":texture},"elements":[{
        "from":[0,0,0],"to":[16,9,16],"faces":{
            "up":{"texture":"#bed"},"down":{"texture":"#bed"},
            "north":{"texture":"#bed"},"south":{"texture":"#bed"},
            "east":{"texture":"#bed"},"west":{"texture":"#bed"}
        }}]});
        content.insert_test_resource(
            &format!("assets/minecraft/models/block/red_bed_{part}.json"),
            &serde_json::to_vec(&model).unwrap(),
        );
    }
    let mut items = crate::ui::items::ItemModels::new();
    let mut stack = crate::inventory::Stack::default();
    stack.id = 1;
    stack.count = 1;
    let model = items.model(&content, &stack, "gui").unwrap();
    assert_eq!(model.quads.len(), 12);
    for layer in [5, 6] {
        assert_eq!(
            model
                .quads
                .iter()
                .filter(|q| q.texture_layer == layer)
                .count(),
            6
        );
    }
    let (low, high) = model
        .quads
        .iter()
        .flat_map(|q| q.positions)
        .map(|p| p[2])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
            (lo.min(z), hi.max(z))
        });
    assert_eq!((low, high), (0., 2.));
}

#[test]
fn skeleton_and_creeper_limb_texture_handedness_is_not_interchangeable() {
    for id in ["skeleton", "stray", "wither_skeleton", "bogged"] {
        let rig = rig(id);
        for limb in ["arm", "leg"] {
            let left = face(rig, &format!("left_{limb}"), -Vec3::Z);
            let right = face(rig, &format!("right_{limb}"), -Vec3::Z);
            assert_eq!(uv_bounds(left), uv_bounds(right), "{id}: shared limb atlas");
            assert!(
                texture_right(left).x * texture_right(right).x < 0.,
                "{id}: left {limb} texture must reflect"
            );
        }
    }
    let rig = rig("creeper");
    for end in ["front", "hind"] {
        let left = face(rig, &format!("left_{end}_leg"), -Vec3::Z);
        let right = face(rig, &format!("right_{end}_leg"), -Vec3::Z);
        assert!(
            texture_right(left).x * texture_right(right).x > 0.,
            "creeper legs repeat, not reflect"
        );
    }
}

#[test]
fn shulker_base_uses_lower_shell_and_spiral_floor_not_lid_texels() {
    for id in ["shulker_box", "shulker"] {
        let rig = rig(id);
        for normal in [-Vec3::X, Vec3::X, -Vec3::Z, Vec3::Z] {
            let base = uv_bounds(face(rig, "base", normal));
            let lid = uv_bounds(face(rig, "lid", normal));
            assert!(base[1] > lid[3], "{id}: base side samples lid rows");
            assert!(
                base[1] >= 44. / 64. && base[3] <= 52. / 64.,
                "{id}: base skirt island"
            );
        }
        let floor = uv_bounds(face(rig, "base", Vec3::Y));
        assert!(floor[0] >= 32. / 64. && floor[2] <= 48. / 64.);
        assert!(
            floor[1] >= 28. / 64. && floor[3] <= 44. / 64.,
            "{id}: bottom spiral island"
        );
        assert!(floor[2] > floor[0] && floor[3] > floor[1]);
    }
}

#[test]
fn shulker_box_closed_shell_matches_the_authored_shulker_shell() {
    let entity = rig("shulker");
    let box_rig = rig("shulker_box");
    let entity_pose = rest(entity);
    let box_pose = rest(box_rig);
    for part in ["base", "lid"] {
        assert_eq!(
            bounds(box_rig, &box_pose, part),
            bounds(entity, &entity_pose, part),
            "{part}: the placed box must use the complete shell"
        );
    }
}

#[test]
fn closed_shulker_shell_encloses_its_head_and_has_matching_sides() {
    let rig = rig("shulker");
    let pose = rest(rig);
    let base = bounds(rig, &pose, "base");
    let lid = bounds(rig, &pose, "lid");
    let head = bounds(rig, &pose, "head");
    for axis in [0, 2] {
        assert!((base.0[axis] - lid.0[axis]).abs() < 1e-5);
        assert!((base.1[axis] - lid.1[axis]).abs() < 1e-5);
        assert!(
            head.0[axis] > base.0[axis] && head.1[axis] < base.1[axis],
            "head protrudes through closed shell"
        );
    }
    assert!(head.0.y > lid.0.y && head.1.y < base.1.y);
    assert!(
        lid.1.y > base.0.y,
        "lid skirt must cover the base's upper fingers"
    );
    assert!(
        (base.1.y - lid.0.y - (base.1.x - base.0.x)).abs() < 1e-5,
        "closed shell is cubic"
    );
}

#[test]
fn chest_lid_top_samples_the_outer_plank_net_and_the_underside_the_inner_one() {
    // model-review-opus-20261102 §4: a double chest without lid overrides showed
    // the dark lid underside on top. The top face takes the (d+w .. d+2w, 0 .. d)
    // island and the underside the (d .. d+w, 0 .. d) one for every chest size.
    for (id, width) in [
        ("chest", 14.),
        ("double_chest_left", 15.),
        ("double_chest_right", 15.),
    ] {
        let rig = rig(id);
        let texture = 64.;
        let depth = 14.;
        let top = uv_bounds(face(rig, "lid", -Vec3::Y));
        let bottom = uv_bounds(face(rig, "lid", Vec3::Y));
        for (actual, expected) in top.into_iter().zip([
            (depth + width) / texture,
            0.,
            (depth + 2. * width) / texture,
            depth / texture,
        ]) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "{id} lid top {top:?} != {expected}"
            );
        }
        for (actual, expected) in bottom.into_iter().zip([
            depth / texture,
            0.,
            (depth + width) / texture,
            depth / texture,
        ]) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "{id} lid underside {bottom:?} != {expected}"
            );
        }
    }
}

#[test]
fn banner_flags_keep_the_full_vanilla_height() {
    // model-review-opus-20261102 §13: the flags shipped 20x25 (standing) and
    // 20x19 (wall) against vanilla's 20x40, so patterns were truncated.
    for id in ["standing_banner", "wall_banner"] {
        let rig = rig(id);
        let pose = rest(rig);
        let (low, high) = bounds(rig, &pose, "flag");
        assert!((high.x - low.x - 20. / 16.).abs() < 1e-5, "{id} flag width");
        assert!(
            (high.y - low.y - 40. / 16.).abs() < 1e-5,
            "{id} flag height {} is shorter than 40 px",
            (high.y - low.y) * 16.
        );
    }
}

#[test]
fn tropical_fish_both_shapes_have_two_sided_fins_and_a_trailing_tail() {
    for id in ["tropical_fish", "tropical_fish_large"] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let right = bounds(rig, &pose, "right_fin");
        let left = bounds(rig, &pose, "left_fin");
        assert!(
            right.0.x < body.0.x && left.1.x > body.1.x,
            "{id}: missing side silhouette"
        );
        assert!((right.0.x + left.1.x).abs() < 1e-5, "{id}: unbalanced fins");
        for name in ["left_fin", "right_fin"] {
            let front = uv_bounds(face(rig, name, -Vec3::Z));
            let back = uv_bounds(face(rig, name, Vec3::Z));
            assert_eq!(front, back, "{id}: fin disappears from one side");
            assert!(front[0] >= 2. / 32. && front[2] <= 4. / 32.);
            assert!(
                front[1] >= 12. / 32. && front[3] <= 18. / 32.,
                "{id}: side fin atlas, not body pixels"
            );
        }
        let tail = bounds(rig, &pose, "tail");
        assert!(
            tail.0.z >= body.1.z && tail.1.z > body.1.z,
            "{id}: tail must trail the body"
        );
        assert!(
            tail.1.x - tail.0.x < (body.1.x - body.0.x) * 0.1,
            "{id}: tail is a fin, not a box"
        );
    }
    let small = rig("tropical_fish");
    let large = rig("tropical_fish_large");
    let a = bounds(small, &rest(small), "body");
    let b = bounds(large, &rest(large), "body");
    assert!(
        b.1.y - b.0.y > (a.1.y - a.0.y) * 1.5,
        "large fish must not reuse the small silhouette"
    );
}

#[test]
fn pufferfish_inflation_changes_body_volume_and_exposes_spines() {
    // PUFF_STATE selects three vanilla layers: `pufferfish_small#main` (3x2x3 body),
    // `pufferfish_medium#main` (5x5x5) and `pufferfish_big#main` (8x8x8). The small
    // and medium rigs are quad-exact against the 26.3 dump; the big layer samples
    // outside its 32x32 atlas (`top_back_fin` front face [31,18,39,19]), so the big
    // state keeps its authored rig and its own spine names until that policy is decided.
    for (id, expected) in [
        ("pufferfish_small", [3., 2., 3.]),
        ("pufferfish_medium", [5., 5., 5.]),
        ("pufferfish", [8., 8., 8.]),
    ] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let actual = (body.1 - body.0) * 16.;
        assert!(
            (actual - Vec3::from_array(expected)).abs().max_element() < 0.05,
            "{id}: body {actual}"
        );
        if id == "pufferfish_small" {
            for eye in ["left_eye", "right_eye"] {
                assert!(
                    bounds(rig, &pose, eye).0.y < body.0.y,
                    "deflated eyes rise above the body"
                );
            }
            for fin in ["right_fin", "left_fin", "back_fin"] {
                assert!(rig.part_by_name.contains_key(fin), "deflated {fin}");
            }
            continue;
        }
        let spines: [(&str, usize, bool); 4] = if id == "pufferfish_medium" {
            [
                ("top_front_fin", 1, false),
                ("bottom_front_fin", 1, true),
                ("right_front_fin", 0, false),
                ("left_front_fin", 0, true),
            ]
        } else {
            [
                ("top_front_spines", 1, false),
                ("bottom_front_spines", 1, true),
                ("right_front_spines", 0, false),
                ("left_front_spines", 0, true),
            ]
        };
        let size = body.1 - body.0;
        for (name, axis, positive) in spines {
            let spine = bounds(rig, &pose, name);
            let protrusion = if positive {
                spine.1[axis] - body.1[axis]
            } else {
                body.0[axis] - spine.0[axis]
            } * 16.;
            assert!(
                protrusion > size[axis] * 16. * 0.05,
                "{id}: {name} hidden inside body"
            );
            // Vanilla's inflated fins are thin plates sampled from one strip of the
            // 32x32 atlas, so the two sides are not mirrored texels.
            assert!(
                rig.bones[rig.part_by_name[name]].quads.len() >= 2,
                "{id}: {name} is a plate"
            );
        }
        if id == "pufferfish_medium" {
            for blue in ["left_blue_fin", "right_blue_fin"] {
                assert!(rig.part_by_name.contains_key(blue), "inflated {blue}");
            }
        }
        assert!(
            !rig.part_by_name.contains_key("tail_base"),
            "inflated silhouette must not grow the old rear slab"
        );
    }
}

#[test]
fn ghast_bodies_are_cubic_and_sample_the_complete_face() {
    for (id, min_width, texture_height) in [("ghast", 4.4, 64.), ("happy_ghast", 3.9, 128.)] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let size = body.1 - body.0;
        assert!(
            size.x > min_width && size.x < min_width + 0.2,
            "{id}: world-scale silhouette"
        );
        assert!(
            (size.x - size.y).abs() < 1e-5 && (size.x - size.z).abs() < 1e-5,
            "{id}: slab instead of cube"
        );
        let face = uv_bounds(face(rig, "body", -Vec3::Z));
        assert_eq!(
            face,
            [
                32. / 128.,
                32. / texture_height,
                64. / 128.,
                64. / texture_height
            ],
            "{id}: complete eyes/mouth face island"
        );
        for index in 0..9 {
            let tentacle = bounds(rig, &pose, &format!("tentacle_{index}"));
            assert!(
                tentacle.0.y < body.1.y && tentacle.1.y > body.1.y,
                "{id}: disconnected or buried tentacle"
            );
            assert!(tentacle.0.x >= body.0.x && tentacle.1.x <= body.1.x);
            assert!(tentacle.0.z >= body.0.z && tentacle.1.z <= body.1.z);
        }
    }
}

#[test]
fn corrected_fish_assets_keep_all_emitted_uvs_inside_the_atlas() {
    for id in [
        "pufferfish",
        "pufferfish_small",
        "pufferfish_medium",
        "tropical_fish",
        "tropical_fish_large",
    ] {
        let rig = rig(id);
        let mut visible_faces = 0;
        for bone in &rig.bones {
            for quad in &bone.quads {
                let p = |i: usize| Vec3::new(quad[i][0], quad[i][1], quad[i][2]);
                if (p(1) - p(0)).cross(p(2) - p(0)).length_squared() == 0. {
                    continue;
                }
                visible_faces += 1;
                for vertex in quad {
                    assert!(
                        vertex.iter().all(|v| v.is_finite()),
                        "{id}: nonfinite vertex"
                    );
                    assert!(
                        (0. ..=1.).contains(&vertex[3]) && (0. ..=1.).contains(&vertex[4]),
                        "{id}/{}: out-of-atlas texel",
                        bone.name
                    );
                }
            }
        }
        assert!(visible_faces > 6, "{id}: body-only fallback");
    }
}

#[test]
fn frog_tongue_extends_from_the_mouth_not_from_the_look_pivot() {
    let rig = rig("frog");
    let tongue_index = rig.part_by_name["tongue"];
    let tongue = &rig.bones[tongue_index];
    assert_ne!(tongue.parent, Some(rig.part_by_name["head"]));
    let pose = rest(rig);
    let mouth = pose[tongue_index].transform_point3(Vec3::ZERO);
    let tongue_bounds = bounds(rig, &pose, "tongue");
    assert!(tongue_bounds.0.z < mouth.z && (tongue_bounds.1.z - mouth.z).abs() < 1e-5);
    let up = uv_bounds(face(rig, "tongue", -Vec3::Y));
    let down = uv_bounds(face(rig, "tongue", Vec3::Y));
    assert_eq!(up, down, "tongue must be visible from above and below");
    assert!(up[0] >= 24. / 48. && up[2] <= 28. / 48.);
    assert!(
        up[1] >= 13. / 48. && up[3] <= 20. / 48.,
        "pink tongue island"
    );
    let throat = bounds(rig, &pose, "croaking_body");
    let body = bounds(rig, &pose, "body");
    assert!(
        throat.0.z < body.0.z && throat.1.z <= body.1.z,
        "throat expands below the mouth"
    );
    assert_eq!(
        rig.bones[rig.part_by_name["croaking_body"]].parent,
        Some(rig.part_by_name["body"])
    );
}

#[test]
fn armadillo_roll_shells_are_closed_grounded_and_baby_sized() {
    let mut adult_width = 0.;
    for id in ["armadillo", "armadillo_baby"] {
        let rig = rig(id);
        let shell = &rig.bones[rig.part_by_name["cube"]];
        assert_eq!(shell.parent, rig.bones[rig.part_by_name["body"]].parent);
        let pose = rest(rig);
        let (low, high) = bounds(rig, &pose, "cube");
        let size = high - low;
        assert!((size.x - size.y).abs() < 1e-5 && (size.x - size.z).abs() < 1e-5);
        assert!(
            (high.y - 1.5).abs() < 1e-5,
            "{id}: rolling shell must rest on the feet plane"
        );
        for normal in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let span = uv_bounds(face(rig, "cube", normal));
            assert!(
                span[2] > span[0] && span[3] > span[1],
                "{id}: missing shell face"
            );
        }
        if id == "armadillo" {
            adult_width = size.x;
        } else {
            assert!(
                size.x < adult_width * 0.75,
                "baby must not reuse adult shell scale"
            );
        }
    }
}

#[test]
fn species_layer_scales_preserve_the_feet_and_distinguish_related_mobs() {
    for (id, reference, scale) in [
        ("cave_spider", "spider", 0.7),
        ("cat", "ocelot", 0.8),
        ("wither_skeleton", "skeleton", 1.2),
    ] {
        let rig = rig(id);
        let reference = self::rig(reference);
        let pose = rest(rig);
        let reference_pose = rest(reference);
        let (low, high) = bounds(rig, &pose, "body");
        let (reference_low, reference_high) = bounds(reference, &reference_pose, "body");
        let expected = (reference_high - reference_low) * scale;
        assert!(
            (high - low - expected).abs().max_element() < 1e-5,
            "{id}: lost species scale"
        );
        // The 26.3 layers fold the species scale into the top bones (the dump has no
        // `layer_root`), so the check is that the hooves stay on the 24 px model floor.
        // Spider legs are `leg0`..`leg7` and have no per-side name, so the floor check
        // uses whichever humanoid or quadruped leg bone the rig carries.
        if let Some(leg) = ["right_hind_leg", "right_leg"]
            .into_iter()
            .find(|name| rig.part_by_name.contains_key(*name))
        {
            let (_, foot_high) = bounds(rig, &pose, leg);
            assert!(
                (foot_high.y * 16. - 24.).abs() < 0.05,
                "{id}: scaling moves the floor"
            );
        }
    }
}

#[test]
fn warden_has_long_arms_and_a_head_above_its_full_height_torso() {
    let rig = rig("warden");
    let pose = rest(rig);
    let body = bounds(rig, &pose, "body");
    let head_index = rig.part_by_name["head"];
    let head_front = face(rig, "head", -Vec3::Z);
    let head_bottom = head_front
        .iter()
        .map(|v| {
            pose[head_index]
                .transform_point3(Vec3::new(v[0], v[1], v[2]))
                .y
        })
        .fold(f32::MIN, f32::max);
    assert!(head_bottom < 0. && (head_bottom - body.0.y).abs() < 1e-5);
    for side in ["right", "left"] {
        let arm = bounds(rig, &pose, &format!("{side}_arm"));
        let leg = bounds(rig, &pose, &format!("{side}_leg"));
        assert!(
            arm.1.y - arm.0.y > body.1.y - body.0.y,
            "warden arms exceed torso height"
        );
        assert!(
            arm.1.y > leg.0.y && arm.1.y < leg.1.y,
            "hands reach below the hips, above feet"
        );
        assert!((leg.1.y - 1.5).abs() < 1e-5);
    }
    let face_uv = uv_bounds(head_front);
    assert!(face_uv[0] >= 10. / 128. && face_uv[2] <= 26. / 128.);
    assert!(
        face_uv[1] >= 42. / 128. && face_uv[3] <= 58. / 128.,
        "face must not use chest texels"
    );
}

#[test]
fn polar_bear_has_a_long_two_section_torso_and_grounded_paws() {
    let rig = rig("polar_bear");
    let pose = rest(rig);
    let body = bounds(rig, &pose, "body");
    let head = bounds(rig, &pose, "head");
    assert!(
        body.1.z - body.0.z > 1.8,
        "adult bear must not have the short cub-like torso"
    );
    assert!(
        head.0.z < body.0.z && head.1.z >= body.0.z,
        "head attaches at the front of the torso"
    );
    for side in ["right", "left"] {
        for end in ["front", "hind"] {
            let paw = bounds(rig, &pose, &format!("{side}_{end}_leg"));
            assert!((paw.1.y - 1.5).abs() < 1e-5);
            assert!(
                paw.1.z - paw.0.z > paw.1.x - paw.0.x,
                "long paws, not narrow posts"
            );
        }
    }
}

/// BV-HORSE (owner 2026-11-13: horses "look like small rhinos") + MODEL-OPUS-11: the adult
/// horse rig follows the 26.3 equine layer (`AbstractEquineModel.createBodyMesh`, javap): a
/// 10 x 10 x 22 body (inflated 0.05) whose back is 21 px above the hooves, four 11-px legs,
/// and the head carried forward of the chest on a neck raised 30 degrees. The 26.1 mesh
/// transformer (translate -2.4016, scale 1.1) is folded into the top bones the way the client
/// applies no per-entity scale, so every number below carries the 1.1 factor.
#[test]
fn adult_horse_rig_has_vanilla_equine_proportions() {
    let horse = rig("horse");
    let transforms = rest(horse);
    let (low, high) = bounds(horse, &transforms, "body");
    let size = (high - low) * 16.;
    assert!(
        (size - Vec3::new(11.11, 11.11, 24.31)).abs().max_element() < 1e-2,
        "body {size}"
    );
    assert!(
        (low.y * 16. - 0.84).abs() < 1e-2,
        "back height {}",
        low.y * 16.
    );
    for leg in [
        "left_front_leg",
        "right_front_leg",
        "left_hind_leg",
        "right_hind_leg",
    ] {
        let (leg_low, leg_high) = bounds(horse, &transforms, leg);
        assert!(
            (leg_high.y * 16. - 23.99).abs() < 1e-2,
            "{leg} reaches the ground"
        );
        assert!(
            ((leg_high.y - leg_low.y) * 16. - 12.1).abs() < 1e-2,
            "{leg} length"
        );
    }
    let (head_low, _) = bounds(horse, &transforms, "head");
    assert!(
        head_low.z < low.z - 6. / 16.,
        "head forward of the chest: {} vs {}",
        head_low.z,
        low.z
    );
}

/// Pixel-space rest bounds of one authored bone: (low, high) in 1/16-block units.
fn rest_px(rig: &Rig, transforms: &[Mat4], name: &str) -> ([f32; 3], [f32; 3]) {
    let (low, high) = bounds(rig, transforms, name);
    (
        [low.x * 16., low.y * 16., low.z * 16.],
        [high.x * 16., high.y * 16., high.z * 16.],
    )
}

fn assert_px(actual: ([f32; 3], [f32; 3]), expected: ([f32; 3], [f32; 3]), what: &str) {
    for axis in 0..3 {
        assert!(
            (actual.0[axis] - expected.0[axis]).abs() < 0.05
                && (actual.1[axis] - expected.1[axis]).abs() < 0.05,
            "{what}: {:?}..{:?} != {:?}..{:?}",
            actual.0,
            actual.1,
            expected.0,
            expected.1
        );
    }
}

// MODEL-OPUS-2: vanilla's pig is a 10x16x8 horizontal torso whose head sits on the
// body line with a 4x3x1 snout; the invented ears and three-cube tail are gone.
#[test]
fn pig_is_the_vanilla_horizontal_torso_with_a_grounded_head_and_short_legs() {
    let pig = rig("pig");
    let transforms = rest(pig);
    let parts: Vec<&str> = pig
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| *n != "root")
        .collect();
    assert_eq!(
        parts,
        [
            "head",
            "body",
            "right_hind_leg",
            "left_hind_leg",
            "right_front_leg",
            "left_front_leg"
        ],
        "vanilla pig has head, body and four legs only"
    );
    assert_px(
        rest_px(pig, &transforms, "body"),
        ([-5., 10., -8.], [5., 18., 8.]),
        "pig body",
    );
    assert_px(
        rest_px(pig, &transforms, "head"),
        ([-4., 8., -15.], [4., 16., -6.]),
        "pig head and snout",
    );
    for (leg, z) in [("right_front_leg", -7.), ("right_hind_leg", 5.)] {
        assert_px(
            rest_px(pig, &transforms, leg),
            ([-5., 18., z], [-1., 24., z + 4.]),
            leg,
        );
    }
}

// MODEL-OPUS-5: the face lives on the inner cube and the translucent shell is a
// separate rig, so a plain green box never covers the eyes and mouth.
#[test]
fn slime_face_parts_sit_on_the_inner_cube_under_a_separate_shell() {
    let inner = rig("slime");
    let transforms = rest(inner);
    let names: Vec<&str> = inner
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| *n != "root")
        .collect();
    assert_eq!(names, ["right_eye", "mouth", "left_eye", "cube"]);
    assert_px(
        rest_px(inner, &transforms, "right_eye"),
        ([-3.25, 18., -3.5], [-1.25, 20., -1.5]),
        "slime right eye",
    );
    assert_px(
        rest_px(inner, &transforms, "left_eye"),
        ([1.25, 18., -3.5], [3.25, 20., -1.5]),
        "slime left eye",
    );
    assert_px(
        rest_px(inner, &transforms, "mouth"),
        ([0., 21., -3.5], [1., 22., -2.5]),
        "slime mouth",
    );
    let shell = rig("slime_outer");
    let shell_transforms = rest(shell);
    assert_px(
        rest_px(shell, &shell_transforms, "cube"),
        ([-4., 16., -4.], [4., 24., 4.]),
        "slime shell",
    );
}

// MODEL-OPUS-N2: the magma cube's eyes are the front texels of the eight one-pixel
// layers plus the inside cube; there is no split eye bone and nothing sits low.
#[test]
fn magma_cube_eyes_stay_on_the_vanilla_one_pixel_layers() {
    let cube = rig("magma_cube");
    let transforms = rest(cube);
    let mut layers = 0;
    for bone in &cube.bones {
        assert!(
            !bone.name.contains("eye"),
            "split eye bone {} is not vanilla",
            bone.name
        );
        if bone.name.starts_with("cube") {
            layers += 1;
        }
    }
    assert_eq!(layers, 8, "vanilla magma cube has eight 8x1x8 layers");
    assert_px(
        rest_px(cube, &transforms, "cube0"),
        ([-4., 16., -4.], [4., 17., 4.]),
        "lowest layer",
    );
    assert_px(
        rest_px(cube, &transforms, "cube7"),
        ([-4., 23., -4.], [4., 24., 4.]),
        "highest layer",
    );
    assert_px(
        rest_px(cube, &transforms, "inside_cube"),
        ([-2., 18., -2.], [2., 22., 2.]),
        "inside cube",
    );
}

// MODEL-OPUS-9: axolotl gill fans are flat planes on the head's top and both sides,
// and the tail is a vertical fin rather than a box.
#[test]
fn axolotl_gill_fans_and_tail_are_flat_vanilla_planes() {
    let axolotl = rig("axolotl");
    let transforms = rest(axolotl);
    for (name, expected) in [
        ("top_gills", ([-4., 13.5, -5.], [4., 16.5, -5.])),
        ("right_gills", ([4., 14.5, -5.], [7., 21.5, -5.])),
        ("left_gills", ([-7., 14.5, -5.], [-4., 21.5, -5.])),
        ("tail", ([0., 16.5, 6.], [0., 21.5, 18.])),
    ] {
        assert_px(rest_px(axolotl, &transforms, name), expected, name);
    }
}

// MODEL-OPUS-10: hoglin horns/ears/mane, strider bristles and the bee's legs and
// stinger are the signature parts those rigs used to lose.
#[test]
fn hoglin_strider_and_bee_keep_their_signature_parts() {
    let hoglin = rig("hoglin");
    let transforms = rest(hoglin);
    assert_px(
        rest_px(hoglin, &transforms, "right_horn"),
        ([-8., 4.64, -27.25], [-6., 13.24, -17.54]),
        "hoglin horn",
    );
    assert_px(
        rest_px(hoglin, &transforms, "left_ear"),
        ([6., 0.99, -17.33], [11.24, 7.02, -11.22]),
        "hoglin ear",
    );
    assert_px(
        rest_px(hoglin, &transforms, "mane"),
        ([0., -7., -16.], [0., 3., 3.]),
        "hoglin mane",
    );
    let strider = rig("strider");
    let transforms = rest(strider);
    assert_px(
        rest_px(strider, &transforms, "body"),
        ([-8., -5., -8.], [8., 9., 8.]),
        "strider body depth",
    );
    assert_px(
        rest_px(strider, &transforms, "right_top_bristle"),
        ([-15.71, -4., -8.], [-8., 5.19, 8.]),
        "strider bristle",
    );
    let bee = rig("bee");
    let transforms = rest(bee);
    assert_px(
        rest_px(bee, &transforms, "front_legs"),
        ([-3.5, 22., -2.], [3.5, 24., -2.]),
        "bee legs",
    );
    assert_px(
        rest_px(bee, &transforms, "stinger"),
        ([0., 18., 5.], [0., 19., 7.]),
        "bee stinger",
    );
}

// MODEL-OPUS-11: the rabbit's crouched 8x6x10 torso, the fox's 6x11x6 body, the
// panda's 19x26x13 torso and the vex's plane wings against the old deer/boxy rigs.
#[test]
fn rabbit_fox_panda_and_vex_shapes_match_the_vanilla_proportions() {
    let rabbit = rig("rabbit");
    let transforms = rest(rabbit);
    assert_px(
        rest_px(rabbit, &transforms, "body"),
        ([-4., 14.01, -4.31], [4., 23.38, 7.22]),
        "rabbit body",
    );
    assert_px(
        rest_px(rabbit, &transforms, "right_ear"),
        ([-2.5, 7., -2.48], [-0.5, 12., -1.48]),
        "rabbit ear",
    );
    let fox = rig("fox");
    let transforms = rest(fox);
    assert_px(
        rest_px(fox, &transforms, "body"),
        ([-3., 13.5, -2.], [3., 19.5, 9.]),
        "fox body",
    );
    let panda = rig("panda");
    let transforms = rest(panda);
    assert_px(
        rest_px(panda, &transforms, "body"),
        ([-9.5, 3.5, -13.], [9.5, 16.5, 13.]),
        "panda body",
    );
    assert_px(
        rest_px(panda, &transforms, "head"),
        ([-8.5, 3.5, -23.], [8.5, 16.5, -12.]),
        "panda head",
    );
    let vex = rig("vex");
    let transforms = rest(vex);
    assert_px(
        rest_px(vex, &transforms, "right_wing"),
        ([-0.5, 18.5, 1.], [-0.5, 23.5, 9.]),
        "vex wing plane",
    );
}

// Boat parity (owner round 5): the four hull rigs were invented boxes; the vanilla
// 26.3 layers are `boat/poplar#main`, `chest_boat/poplar#main`,
// `boat/bamboo#main` and `chest_boat/bamboo#main`, and the water patch is the
// `boat#water_patch` mask mesh drawn with the water-mask material.
#[test]
fn boat_and_raft_hulls_match_the_vanilla_26_3_layers() {
    let boat = rig("boat");
    let transforms = rest(boat);
    let names: Vec<&str> = boat
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| *n != "root")
        .collect();
    assert_eq!(
        names,
        [
            "bottom",
            "left_paddle",
            "right_paddle",
            "left",
            "back",
            "front",
            "right"
        ],
        "vanilla boat parts"
    );
    assert_px(
        rest_px(boat, &transforms, "bottom"),
        ([-14., 3., -8.], [14., 6., 8.]),
        "boat floor",
    );
    for (wall, expected) in [
        ("left", ([-14., -3., 8.], [14., 3., 10.])),
        ("right", ([-14., -3., -10.], [14., 3., -8.])),
        ("front", ([14., -3., -8.], [16., 3., 8.])),
        ("back", ([-16., -3., -9.], [-14., 3., 9.])),
    ] {
        assert_px(rest_px(boat, &transforms, wall), expected, wall);
    }
    // Both paddles mount on the same side and mirror by rotation, as in vanilla.
    for (paddle, z) in [("left_paddle", (4., 24.)), ("right_paddle", (-24., -4.))] {
        assert_px(
            rest_px(boat, &transforms, paddle),
            ([1.43, -8.14, z.0], [3.98, -2.06, z.1]),
            paddle,
        );
    }

    let raft = rig("raft");
    let transforms = rest(raft);
    let raft_names: Vec<&str> = raft
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| *n != "root")
        .collect();
    assert_eq!(
        raft_names,
        ["bottom", "left_paddle", "right_paddle"],
        "the bamboo raft is its double-deck hull and two paddles, no walls"
    );
    assert_px(
        rest_px(raft, &transforms, "bottom"),
        ([-14., -2.1, -10.], [14., 5.9, 10.]),
        "raft deck",
    );

    let chest = rig("chest_boat");
    let transforms = rest(chest);
    for (part, expected) in [
        ("chest_bottom", ([-14., -5., -6.], [-2., 3., 6.])),
        ("chest_lid", ([-14., -9., -6.], [-2., -5., 6.])),
        ("chest_lock", ([-2., -6., -1.], [-1., -2., 1.])),
    ] {
        assert_px(rest_px(chest, &transforms, part), expected, part);
    }
    assert!(
        !chest.part_by_name.contains_key("chest"),
        "vanilla splits the chest into bottom, lid and lock"
    );

    let patch = rig("boat_water_patch");
    let transforms = rest(patch);
    let patch_names: Vec<&str> = patch
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| *n != "root")
        .collect();
    assert_eq!(
        patch_names,
        ["water_patch"],
        "the water patch is one mask mesh in the hull frame"
    );
    assert_px(
        rest_px(patch, &transforms, "water_patch"),
        ([-14., -3., -8.], [14., 0., 8.]),
        "water patch",
    );
}

// MODEL-OPUS-N8/N10: the remaining mirror and hat differences against the 26.3 layers.
#[test]
fn cow_horn_cube_and_humanoid_hat_follow_the_26_3_layers() {
    fn find<'a>(
        bones: &'a [super::krasnokamen_models::BoneSpec],
        name: &str,
    ) -> Option<&'a super::krasnokamen_models::BoneSpec> {
        bones.iter().find_map(|bone| {
            (bone.name == name)
                .then_some(bone)
                .or_else(|| find(&bone.bones, name))
        })
    }
    // `cow#main`: four head cubes (skull, snout and two horns) and the outer horn
    // samples the right horn island unflipped; the old rig mirrored it.
    let cow = super::krasnokamen_models::ModelFile::parse(include_str!(
        "../../assets/krasnokamen/models/entity/cow.json"
    ))
    .unwrap();
    let head = find(&cow.bones, "head").expect("cow head bone");
    assert_eq!(
        head.cubes.len(),
        4,
        "vanilla cow head: skull, snout, two horns"
    );
    assert!(
        !head.cubes[3].mirror,
        "the vanilla outer horn cube is not mirrored"
    );
    // The vanilla humanoid layer keeps a `hat` cube; on skeleton and stray skins it
    // samples only transparent texels, which is why the audit called it minor.
    for id in ["skeleton", "stray"] {
        let rig = rig(id);
        assert!(
            rig.part_by_name.contains_key("hat"),
            "{id}: vanilla hat cube"
        );
    }
}

// MODEL-OPUS-N8: left/right mirror flags now come from the 26.3 layers. Each tuple is
// (rig, bone, expected mirror flags per cube) as the dump bakes them; the pre-fix rigs
// carried the opposite flags on these cubes.
#[test]
fn left_side_mirror_flags_follow_the_26_3_layers() {
    fn find<'a>(
        bones: &'a [super::krasnokamen_models::BoneSpec],
        name: &str,
    ) -> Option<&'a super::krasnokamen_models::BoneSpec> {
        bones.iter().find_map(|bone| {
            (bone.name == name)
                .then_some(bone)
                .or_else(|| find(&bone.bones, name))
        })
    }
    let cases: [(&str, &str, &[bool], &str); 6] = [
        (
            "cat",
            "left_hind_leg",
            &[false],
            include_str!("../../assets/krasnokamen/models/entity/cat.json"),
        ),
        (
            "cat",
            "left_front_leg",
            &[false],
            include_str!("../../assets/krasnokamen/models/entity/cat.json"),
        ),
        (
            "chicken",
            "left_leg",
            &[false],
            include_str!("../../assets/krasnokamen/models/entity/chicken.json"),
        ),
        (
            "chicken",
            "left_wing",
            &[false],
            include_str!("../../assets/krasnokamen/models/entity/chicken.json"),
        ),
        (
            "wolf",
            "left_hind_leg",
            &[false],
            include_str!("../../assets/krasnokamen/models/entity/wolf.json"),
        ),
        (
            "wolf",
            "right_hind_leg",
            &[true],
            include_str!("../../assets/krasnokamen/models/entity/wolf.json"),
        ),
    ];
    for (rig, bone, expected, source) in cases {
        let model = super::krasnokamen_models::ModelFile::parse(source).unwrap();
        let spec = find(&model.bones, bone).unwrap_or_else(|| panic!("{rig}: {bone}"));
        let flags: Vec<bool> = spec.cubes.iter().map(|cube| cube.mirror).collect();
        assert_eq!(flags, expected, "{rig}: {bone} mirror flags");
    }
}

/// MODEL-SHIELD-GUI: the GUI cell rendered 10x16 because the authored plate was a
/// 10x16x1 box at texOffs(0,6) plus a two-cube grip. Vanilla 26.3 `ShieldModel.createLayer`
/// is plate `addBox(-6,-11,-2,12,22,1)` texOffs(0,0) and handle `addBox(-1,-3,-1,2,6,6)`
/// texOffs(26,0); `receipts/XHarnessGates/models/vanilla_layers_26_3.json`
/// (`minecraft:shield#main`) carries the same boxes, bone names and quads.
#[test]
fn shield_plate_and_handle_match_the_vanilla_263_model() {
    let shield = rig("shield");
    let pose = rest(shield);
    assert_eq!(
        bounds(shield, &pose, "plate"),
        (
            Vec3::new(-6., -11., -2.) / 16.,
            Vec3::new(6., 11., -1.) / 16.
        ),
        "plate: ShieldModel addBox(-6,-11,-2,12,22,1)"
    );
    assert_eq!(
        bounds(shield, &pose, "handle"),
        (Vec3::new(-1., -3., -1.) / 16., Vec3::new(1., 3., 5.) / 16.),
        "handle: ShieldModel addBox(-1,-3,-1,2,6,6)"
    );
    // Vanilla names the grip cube `handle`; a renamed bone would silently drop the
    // banner-pattern overlay, which is attached to `plate`.
    assert!(
        !shield.part_by_name.contains_key("grip"),
        "the old two-cube `grip` bone is gone"
    );
    // Box unwrap in pixels of the 64x64 sheet: plate covers 0..26 x 0..23, handle
    // 26..42 x 0..12.
    for (part, expected) in [
        ("plate", [0., 0., 26., 23.]),
        ("handle", [26., 0., 42., 12.]),
    ] {
        let bone = &shield.bones[shield.part_by_name[part]];
        let mut span = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for quad in &bone.quads {
            let uv = uv_bounds(quad);
            span = [
                span[0].min(uv[0]),
                span[1].min(uv[1]),
                span[2].max(uv[2]),
                span[3].max(uv[3]),
            ];
        }
        assert_eq!(
            [span[0] * 64., span[1] * 64., span[2] * 64., span[3] * 64.],
            expected,
            "{part}: vanilla texOffs unwrap"
        );
    }
}
