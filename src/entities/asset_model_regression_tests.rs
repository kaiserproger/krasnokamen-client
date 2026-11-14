//! Asset regressions for MODEL-OPUS-6/8/9/N8/N9/N10 and the shulker shell finding.
//!
//! These inspect the baked geometry/UVs consumed by rendering, not JSON spellings.
//! They protect silhouette, attachment and texture handedness rather than freezing
//! every authored coordinate. Original observations: model-review-opus-20261102
//! sections 3.2/3.4 and model-review-opus-finish-20261102 N8–N10. Texture islands
//! were inspected in the locally supplied 26.3 atlas; no oracle pixels are embedded.
//! Native/Java frame comparisons and isolated negative controls belong to Main.

use super::{resources::{self, Rig}, solaris_models};
use glam::{Mat4, Vec3};

type Quad = [[f32; 5]; 4];

fn rig(id: &str) -> &'static Rig {
    solaris_models::authored(&format!("minecraft:{id}#main"))
        .unwrap_or_else(|| panic!("missing embedded asset {id}"))
}

fn rest(rig: &Rig) -> Vec<Mat4> {
    let mut transforms = Vec::with_capacity(rig.bones.len());
    for bone in &rig.bones {
        let parent = bone.parent.map_or(Mat4::IDENTITY, |index| transforms[index]);
        transforms.push(parent * resources::transform(bone.pivot, bone.rotation, bone.scale));
    }
    transforms
}

fn bounds(rig: &Rig, transforms: &[Mat4], name: &str) -> (Vec3, Vec3) {
    let index = rig.part_by_name[name];
    let bone = &rig.bones[index];
    assert!(!bone.quads.is_empty(), "{name} must emit geometry");
    bone.quads.iter().flatten().fold((Vec3::MAX, Vec3::MIN), |(low, high), vertex| {
        let point = transforms[index].transform_point3(Vec3::new(vertex[0], vertex[1], vertex[2]));
        (low.min(point), high.max(point))
    })
}

fn face<'a>(rig: &'a Rig, name: &str, normal: Vec3) -> &'a Quad {
    let bone = &rig.bones[rig.part_by_name[name]];
    bone.quads.iter().zip(&bone.normals)
        .find(|(_, n)| Vec3::from_array(**n).dot(normal) > 0.99)
        .map(|(quad, _)| quad)
        .unwrap_or_else(|| panic!("{name}: missing face {normal}"))
}

fn uv_bounds(quad: &Quad) -> [f32; 4] {
    quad.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |mut span, v| {
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
            assert!(front_inset > 0., "{id}: front hoof must not project to the chest edge");
            assert!((front_inset - hind_inset).abs() < 1e-5, "{id}: unequal leg alignment");
            assert!((front.1.y - hind.1.y).abs() < 1e-5, "{id}: hooves share a floor");
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
        let normal = (p(front[1]) - p(front[0])).cross(p(front[2]) - p(front[0])).normalize();
        let pivot = pose[bone].transform_point3(Vec3::ZERO);
        let radial = Vec3::new(pivot.x, 0., pivot.z).normalize();
        assert!(normal.dot(radial) < -0.999, "{name}: texture panel twists around the limb");
    }
}

#[test]
fn bed_legs_use_individual_unmirrored_texture_nets() {
    for id in ["bed_head", "bed_foot"] {
        let rig = rig(id);
        for normal in [-Vec3::Z, Vec3::Z] {
            let left = face(rig, "leg_left", normal);
            let right = face(rig, "leg_right", normal);
            assert!(texture_right(left).x * texture_right(right).x > 0., "{id}: wood grain reversed");
            assert_ne!(uv_bounds(left), uv_bounds(right), "{id}: legs have separate atlas islands");
        }
    }
}

#[test]
fn skeleton_and_creeper_limb_texture_handedness_is_not_interchangeable() {
    for id in ["skeleton", "stray", "wither_skeleton", "bogged"] {
        let rig = rig(id);
        for limb in ["arm", "leg"] {
            let left = face(rig, &format!("left_{limb}"), -Vec3::Z);
            let right = face(rig, &format!("right_{limb}"), -Vec3::Z);
            assert_eq!(uv_bounds(left), uv_bounds(right), "{id}: shared limb atlas");
            assert!(texture_right(left).x * texture_right(right).x < 0., "{id}: left {limb} texture must reflect");
        }
    }
    let rig = rig("creeper");
    for end in ["front", "hind"] {
        let left = face(rig, &format!("left_{end}_leg"), -Vec3::Z);
        let right = face(rig, &format!("right_{end}_leg"), -Vec3::Z);
        assert!(texture_right(left).x * texture_right(right).x > 0., "creeper legs repeat, not reflect");
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
            assert!(base[1] >= 44. / 64. && base[3] <= 52. / 64., "{id}: base skirt island");
        }
        let floor = uv_bounds(face(rig, "base", Vec3::Y));
        assert!(floor[0] >= 32. / 64. && floor[2] <= 48. / 64.);
        assert!(floor[1] >= 28. / 64. && floor[3] <= 44. / 64., "{id}: bottom spiral island");
        assert!(floor[2] > floor[0] && floor[3] > floor[1]);
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
        assert!(head.0[axis] > base.0[axis] && head.1[axis] < base.1[axis], "head protrudes through closed shell");
    }
    assert!(head.0.y > lid.0.y && head.1.y < base.1.y);
    assert!(lid.1.y > base.0.y, "lid skirt must cover the base's upper fingers");
    assert!((base.1.y - lid.0.y - (base.1.x - base.0.x)).abs() < 1e-5, "closed shell is cubic");
}

#[test]
fn tropical_fish_both_shapes_have_two_sided_fins_and_a_trailing_tail() {
    for id in ["tropical_fish", "tropical_fish_large"] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let right = bounds(rig, &pose, "right_fin");
        let left = bounds(rig, &pose, "left_fin");
        assert!(right.0.x < body.0.x && left.1.x > body.1.x, "{id}: missing side silhouette");
        assert!((right.0.x + left.1.x).abs() < 1e-5, "{id}: unbalanced fins");
        for name in ["left_fin", "right_fin"] {
            let front = uv_bounds(face(rig, name, -Vec3::Z));
            let back = uv_bounds(face(rig, name, Vec3::Z));
            assert_eq!(front, back, "{id}: fin disappears from one side");
            assert!(front[0] >= 2. / 32. && front[2] <= 4. / 32.);
            assert!(front[1] >= 12. / 32. && front[3] <= 18. / 32., "{id}: side fin atlas, not body pixels");
        }
        let tail = bounds(rig, &pose, "tail");
        assert!(tail.0.z >= body.1.z && tail.1.z > body.1.z, "{id}: tail must trail the body");
        assert!(tail.1.x - tail.0.x < (body.1.x - body.0.x) * 0.1, "{id}: tail is a fin, not a box");
    }
    let small = rig("tropical_fish");
    let large = rig("tropical_fish_large");
    let a = bounds(small, &rest(small), "body");
    let b = bounds(large, &rest(large), "body");
    assert!(b.1.y - b.0.y > (a.1.y - a.0.y) * 1.5, "large fish must not reuse the small silhouette");
}

#[test]
fn pufferfish_inflation_changes_body_volume_and_exposes_spines() {
    let mut previous = Vec3::ZERO;
    for id in ["pufferfish_small", "pufferfish_medium", "pufferfish"] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let size = body.1 - body.0;
        assert!(size.cmpgt(previous).all(), "{id}: inflation must grow all three axes");
        previous = size;
        assert!(bounds(rig, &pose, "right_fin").0.x < body.0.x);
        assert!(bounds(rig, &pose, "left_fin").1.x > body.1.x);
        if id == "pufferfish_small" {
            for eye in ["left_eye", "right_eye"] {
                assert!(bounds(rig, &pose, eye).0.y < body.0.y, "deflated eyes rise above the body");
            }
            continue;
        }
        for (name, axis, positive) in [
            ("top_front_spines", 1, false), ("bottom_front_spines", 1, true),
            ("right_front_spines", 0, false), ("left_front_spines", 0, true),
        ] {
            let spine = bounds(rig, &pose, name);
            let protrusion = if positive { spine.1[axis] - body.1[axis] } else { body.0[axis] - spine.0[axis] };
            assert!(protrusion > size[axis] * 0.05, "{id}: {name} hidden inside body");
            assert_eq!(uv_bounds(face(rig, name, -Vec3::Z)), uv_bounds(face(rig, name, Vec3::Z)));
        }
        assert!(!rig.part_by_name.contains_key("tail_base"), "inflated silhouette must not grow the old rear slab");
    }
}

#[test]
fn ghast_bodies_are_cubic_and_sample_the_complete_face() {
    for (id, min_width, texture_height) in [("ghast", 4.4, 64.), ("happy_ghast", 3.9, 128.)] {
        let rig = rig(id);
        let pose = rest(rig);
        let body = bounds(rig, &pose, "body");
        let size = body.1 - body.0;
        assert!(size.x > min_width && size.x < min_width + 0.2, "{id}: world-scale silhouette");
        assert!((size.x - size.y).abs() < 1e-5 && (size.x - size.z).abs() < 1e-5, "{id}: slab instead of cube");
        let face = uv_bounds(face(rig, "body", -Vec3::Z));
        assert_eq!(face, [32. / 128., 32. / texture_height, 64. / 128., 64. / texture_height], "{id}: complete eyes/mouth face island");
        for index in 0..9 {
            let tentacle = bounds(rig, &pose, &format!("tentacle_{index}"));
            assert!(tentacle.0.y < body.1.y && tentacle.1.y > body.1.y, "{id}: disconnected or buried tentacle");
            assert!(tentacle.0.x >= body.0.x && tentacle.1.x <= body.1.x);
            assert!(tentacle.0.z >= body.0.z && tentacle.1.z <= body.1.z);
        }
    }
}

#[test]
fn corrected_fish_assets_keep_all_emitted_uvs_inside_the_atlas() {
    for id in ["pufferfish", "pufferfish_small", "pufferfish_medium", "tropical_fish", "tropical_fish_large"] {
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
                    assert!(vertex.iter().all(|v| v.is_finite()), "{id}: nonfinite vertex");
                    assert!((0. ..=1.).contains(&vertex[3]) && (0. ..=1.).contains(&vertex[4]), "{id}/{}: out-of-atlas texel", bone.name);
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
    assert!(up[1] >= 13. / 48. && up[3] <= 20. / 48., "pink tongue island");
    let throat = bounds(rig, &pose, "croaking_body");
    let body = bounds(rig, &pose, "body");
    assert!(throat.0.z < body.0.z && throat.1.z <= body.1.z, "throat expands below the mouth");
    assert_eq!(rig.bones[rig.part_by_name["croaking_body"]].parent, Some(rig.part_by_name["body"]));
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
        assert!((high.y - 1.5).abs() < 1e-5, "{id}: rolling shell must rest on the feet plane");
        for normal in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let span = uv_bounds(face(rig, "cube", normal));
            assert!(span[2] > span[0] && span[3] > span[1], "{id}: missing shell face");
        }
        if id == "armadillo" {
            adult_width = size.x;
        } else {
            assert!(size.x < adult_width * 0.75, "baby must not reuse adult shell scale");
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
        assert!((high - low - expected).abs().max_element() < 1e-5, "{id}: lost species scale");
        let feet = pose[rig.part_by_name["layer_root"]].transform_point3(Vec3::new(0., 1.5, 0.));
        assert!((feet - Vec3::new(0., 1.5, 0.)).length() < 1e-5, "{id}: scaling moves the floor");
    }
}

#[test]
fn warden_has_long_arms_and_a_head_above_its_full_height_torso() {
    let rig = rig("warden");
    let pose = rest(rig);
    let body = bounds(rig, &pose, "body");
    let head_index = rig.part_by_name["head"];
    let head_front = face(rig, "head", -Vec3::Z);
    let head_bottom = head_front.iter().map(|v| {
        pose[head_index].transform_point3(Vec3::new(v[0], v[1], v[2])).y
    }).fold(f32::MIN, f32::max);
    assert!(head_bottom < 0. && (head_bottom - body.0.y).abs() < 1e-5);
    for side in ["right", "left"] {
        let arm = bounds(rig, &pose, &format!("{side}_arm"));
        let leg = bounds(rig, &pose, &format!("{side}_leg"));
        assert!(arm.1.y - arm.0.y > body.1.y - body.0.y, "warden arms exceed torso height");
        assert!(arm.1.y > leg.0.y && arm.1.y < leg.1.y, "hands reach below the hips, above feet");
        assert!((leg.1.y - 1.5).abs() < 1e-5);
    }
    let face_uv = uv_bounds(head_front);
    assert!(face_uv[0] >= 10. / 128. && face_uv[2] <= 26. / 128.);
    assert!(face_uv[1] >= 42. / 128. && face_uv[3] <= 58. / 128., "face must not use chest texels");
}

#[test]
fn polar_bear_has_a_long_two_section_torso_and_grounded_paws() {
    let rig = rig("polar_bear");
    let pose = rest(rig);
    let body = bounds(rig, &pose, "body");
    let head = bounds(rig, &pose, "head");
    assert!(body.1.z - body.0.z > 1.8, "adult bear must not have the short cub-like torso");
    assert!(head.0.z < body.0.z && head.1.z >= body.0.z, "head attaches at the front of the torso");
    for side in ["right", "left"] {
        for end in ["front", "hind"] {
            let paw = bounds(rig, &pose, &format!("{side}_{end}_leg"));
            assert!((paw.1.y - 1.5).abs() < 1e-5);
            assert!(paw.1.z - paw.0.z > paw.1.x - paw.0.x, "long paws, not narrow posts");
        }
    }
}
