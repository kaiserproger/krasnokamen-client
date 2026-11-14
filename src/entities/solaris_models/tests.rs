use super::*;
use glam::Vec3;
use std::path::PathBuf;

fn cube(origin: [f32; 3], size: [f32; 3], uv: [f32; 2], mirror: bool, inflate: f32) -> CubeSpec {
    CubeSpec {
        origin,
        size,
        uv,
        mirror,
        inflate,
        faces: BTreeMap::new(),
    }
}
fn centroid(quad: &[[f32; 5]; 4]) -> Vec3 {
    quad.iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .sum::<Vec3>()
        / 4.
}

#[test]
fn rc2_authored_village_models_have_coherent_body_parts() {
    let model = |id: &str| {
        let (_, source) = EMBEDDED.iter().find(|(name, _)| *name == format!("{id}.json")).unwrap();
        ModelFile::parse(source).unwrap()
    };
    let child = model("villager_baby");
    let body = child.bones.iter().find(|b| b.name == "body").unwrap();
    assert!(body.cubes[0].size[1] > 2., "child torso must not be a flat strip");
    let golem = model("iron_golem");
    let torso = golem.bones.iter().find(|b| b.name == "body").unwrap();
    assert!(torso.cubes.len() >= 2 && torso.cubes[0].size[0] > torso.cubes[1].size[0],
        "golem shoulders must taper to a narrower waist");
    let head = golem.bones.iter().find(|b| b.name == "head").unwrap();
    let chin = head.pivot[1] + head.cubes[0].origin[1] + head.cubes[0].size[1];
    assert!(head.pivot[2] < torso.pivot[2] && chin == torso.pivot[1] + torso.cubes[0].origin[1],
        "golem head must sit forward on the shoulder line, as on the oracle");
    for name in ["right_arm", "left_arm"] {
        let arm = golem.bones.iter().find(|b| b.name == name).unwrap();
        assert_eq!(arm.cubes[0].size, [4., 30., 6.], "arm texture net dimensions");
        assert!(arm.pivot[1] + arm.cubes[0].size[1] >= 23., "arms reach near the feet");
    }
    assert!(!golem.bones.iter().any(|b| b.name.ends_with("_hand") || b.name.ends_with("_foot") || b.name.ends_with("_heel")),
        "free-floating hands and feet cannot follow the limbs");
    let left = golem.bones.iter().find(|b| b.name == "left_leg").unwrap();
    let right = golem.bones.iter().find(|b| b.name == "right_leg").unwrap();
    assert!(right.pivot[0] + right.cubes[0].origin[0] + right.cubes[0].size[0] < left.pivot[0] + left.cubes[0].origin[0],
        "golem legs must remain visibly separate");
}

#[test]
fn goat_horn_spacing_tracks_observed_front_silhouette() {
    let rig = authored("minecraft:goat#main").unwrap();
    let right = rig.bones[rig.part_by_name["right_horn"]].pivot.x;
    let left = rig.bones[rig.part_by_name["left_horn"]].pivot.x;
    // Matched 26.3 front shots: observed horn centres are about 27 pixels
    // apart, versus 22 pixels with the previous 2.5-unit separation.
    let spacing_in_model_units = (left - right).abs() * 16.;
    assert!((3.0..=3.2).contains(&spacing_in_model_units), "{spacing_in_model_units}");
}

#[test]
fn goat_rest_horns_are_vertical_in_the_oracle_side_view() {
    let rig = authored("minecraft:goat#main").unwrap();
    for name in ["right_horn", "left_horn"] {
        let bone = &rig.bones[rig.part_by_name[name]];
        let axis = crate::entities::resources::transform(bone.pivot, bone.rotation, bone.scale)
            .transform_vector3(Vec3::Y)
            .normalize();
        // The matched 26.3 side screenshot shows upright, not rear-leaning, horns.
        assert!(axis.z.abs() < 0.02, "{name} horn axis: {axis:?}");
    }
}

#[test]
fn goat_body_origin_tracks_observed_front_silhouette() {
    let rig = authored("minecraft:goat#main").unwrap();
    let body = &rig.bones[rig.part_by_name["body"]];
    let parent = &rig.bones[body.parent.unwrap()];
    // The fixed-distance 26.3 front capture places the body four screen pixels
    // to the right of the former centered rig; this is the fitted model-space offset.
    assert!((parent.pivot.x * 16. - 0.5).abs() < 0.01, "goat body parent: {:?}", parent.pivot);
}

#[test]
fn goat_ears_and_beard_stay_upright_at_rest() {
    let rig = authored("minecraft:goat#main").unwrap();
    let face = &rig.bones[rig.part_by_name["face"]];
    let face_transform = crate::entities::resources::transform(face.pivot, face.rotation, face.scale);
    for name in ["right_ear", "left_ear", "beard"] {
        let bone = &rig.bones[rig.part_by_name[name]];
        let part_transform = crate::entities::resources::transform(bone.pivot, bone.rotation, bone.scale);
        let axis = (face_transform * part_transform).transform_vector3(Vec3::Y).normalize();
        // Java 26.3 front capture: ears are full-height and the beard hangs vertically.
        assert!(axis.z.abs() < 0.02, "{name} vertical axis: {axis:?}");
    }
    let left = rig.bones[rig.part_by_name["left_ear"]].pivot.x;
    let right = rig.bones[rig.part_by_name["right_ear"]].pivot.x;
    assert!(((left - right).abs() * 16. - 8.).abs() < 0.01, "ear spacing");
}

#[test]
fn spider_legs_form_single_sloped_oracle_silhouettes() {
    let (_, source) = EMBEDDED.iter().find(|(name, _)| *name == "spider.json").unwrap();
    let model = ModelFile::parse(source).unwrap();
    for index in 0..8 {
        let name = format!("leg{index}");
        let leg = model.bones.iter().find(|bone| bone.name == name).unwrap();
        // Java 26.3 reference: the visible legs are straight, sloped bars,
        // not horizontal bars with vertical downward extensions at their ends.
        assert_eq!(leg.cubes.len(), 1, "{name} must not have an extra vertical foot");
        assert!((28. ..=40.).contains(&leg.rotation[2].abs()), "{name} slope");
    }
}

#[test]
fn quads_wind_counter_clockwise_and_face_outward() {
    for mirror in [false, true] {
        for inflate in [0., 0.5] {
            let c = cube([-4., -4., -6.], [8., 8., 6.], [0., 0.], mirror, inflate);
            let quads = cube_quads(&c, 64., 32.);
            assert_eq!(quads.len(), 6);
            let centre = Vec3::new(0., 0., -3.) / 16.;
            for (quad, normal) in quads {
                let p = quad.map(|v| Vec3::new(v[0], v[1], v[2]));
                let geometric = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
                assert!(
                    (geometric - Vec3::from_array(normal)).length() < 1e-5,
                    "mirror={mirror} normal {normal:?} vs {geometric:?}"
                );
                assert!(
                    (centroid(&quad) - centre).dot(geometric) > 0.,
                    "face points inward"
                );
            }
        }
    }
}

#[test]
fn box_unwrap_matches_texture_strip_layout() {
    let c = cube([0., 0., 0.], [8., 4., 6.], [10., 20.], false, 0.);
    let quads = cube_quads(&c, 64., 64.);
    let rect = |index: usize| {
        let uv: Vec<[f32; 2]> = quads[index]
            .0
            .iter()
            .map(|v| [v[3] * 64., v[4] * 64.])
            .collect();
        let min = |axis: usize| uv.iter().map(|p| p[axis]).fold(f32::MAX, f32::min);
        let max = |axis: usize| uv.iter().map(|p| p[axis]).fold(f32::MIN, f32::max);
        [min(0), min(1), max(0), max(1)]
    };
    // up, down, right, front, left, back — d = 6, w = 8, h = 4.
    assert_eq!(rect(0), [16., 20., 24., 26.]);
    assert_eq!(rect(1), [24., 20., 32., 26.]);
    assert_eq!(rect(2), [10., 26., 16., 30.]);
    assert_eq!(rect(3), [16., 26., 24., 30.]);
    assert_eq!(rect(4), [24., 26., 30., 30.]);
    assert_eq!(rect(5), [30., 26., 38., 30.]);
}

#[test]
fn mirror_reflects_geometry_but_keeps_texture_assignment() {
    let plain = cube_quads(
        &cube([1., 0., 0.], [4., 4., 4.], [0., 0.], false, 0.),
        16.,
        16.,
    );
    let mirrored = cube_quads(
        &cube([1., 0., 0.], [4., 4., 4.], [0., 0.], true, 0.),
        16.,
        16.,
    );
    // Box spans x 1..5 (centre 3): front face (index 3) stays at z = 0 but its u runs backwards.
    let front_plain = &plain[3].0;
    let front_mirror = &mirrored[3].0;
    let mut by_uv: Vec<_> = front_plain
        .iter()
        .map(|v| ((v[3] * 16.) as i32, (v[4] * 16.) as i32, v[0]))
        .collect();
    by_uv.sort_by_key(|k| (k.0, k.1));
    let mut by_uv_m: Vec<_> = front_mirror
        .iter()
        .map(|v| ((v[3] * 16.) as i32, (v[4] * 16.) as i32, v[0]))
        .collect();
    by_uv_m.sort_by_key(|k| (k.0, k.1));
    for (a, b) in by_uv.iter().zip(&by_uv_m) {
        assert_eq!((a.0, a.1), (b.0, b.1));
        assert!(
            (a.2 + b.2 - 6. / 16.).abs() < 1e-6,
            "x reflected about the box centre"
        );
    }
    assert_eq!(mirrored[2].1, [1., 0., 0.]);
}

#[test]
fn zero_thickness_planes_and_explicit_faces_bake() {
    let mut c = cube([-4., -4., 0.], [8., 8., 0.], [0., 0.], false, 0.);
    c.faces.insert(Face::Front, [8., 0., 0., 8.]); // swapped corners flip
    let quads = cube_quads(&c, 16., 16.);
    let front = &quads[3].0;
    assert_eq!((front[0][3] * 16., front[1][3] * 16.), (0., 8.));
    assert!(
        quads
            .iter()
            .flat_map(|q| q.0)
            .flatten()
            .all(|v| v.is_finite())
    );
}

#[test]
fn rig_key_resolution() {
    for (key, id) in [
        ("minecraft:cow#renderer_main", "cow"),
        ("minecraft:cow_baby#main", "cow_baby"),
        ("minecraft:cold_cow#main", "cold_cow"),
        ("minecraft:player#main", "player"),
        ("minecraft:player_slim#main", "player_slim"),
        ("minecraft:oak_boat#renderer/model", "boat"),
        ("minecraft:dark_oak_chest_boat#renderer/model", "chest_boat"),
        ("minecraft:bamboo_raft#renderer/model", "raft"),
        ("minecraft:bamboo_chest_raft#renderer/model", "chest_raft"),
        (
            "minecraft:oak_boat#renderer/waterPatchModel",
            "boat_water_patch",
        ),
        ("minecraft:tnt_minecart#renderer/model", "minecart"),
        ("minecraft:conduit#cage", "conduit_cage"),
        ("minecraft:conduit_shell#main", "conduit_shell"),
        ("minecraft:breeze_wind_charge#renderer/model", "wind_charge"),
        ("minecraft:zombie#layer3/modelSet/head", "armor_head"),
        ("minecraft:armor_stand#boots", "armor_feet"),
        ("minecraft:armor_stand_small#chestplate", "armor_baby_chest"),
        ("minecraft:armor_stand#renderer_main", "armor_stand"),
        ("minecraft:double_chest_left#main", "double_chest_left"),
        ("minecraft:poplar_boat#renderer/model", "boat"),
        ("minecraft:poplar_chest_boat#renderer/model", "chest_boat"),
    ] {
        assert_eq!(model_id(key), id, "{key}");
    }
    // 26.3 families resolve to shipped geometry, not the placeholder.
    for key in [
        "minecraft:poplar_boat#renderer/model",
        "minecraft:poplar_chest_boat#renderer/model",
    ] {
        assert!(authored(key).is_some(), "{key}");
    }
}


#[test]
fn placeholder_is_a_visible_entity_sized_box() {
    let rig = placeholder(0.6, 1.8);
    assert!(rig.is_placeholder());
    assert_eq!(rig.bones[0].quads.len(), 6);
    let ys: Vec<f32> = rig.bones[0].quads.iter().flatten().map(|v| v[1]).collect();
    assert!((ys.iter().cloned().fold(f32::MIN, f32::max) - 24. / 16.).abs() < 1e-5);
    assert!((ys.iter().cloned().fold(f32::MAX, f32::min) - (24. - 28.8) / 16.).abs() < 1e-4);
    assert!(authored("minecraft:no_such_entity#main").is_none());
}

#[test]
fn parser_rejects_malformed_models() {
    let ok = r#"{"schema":1,"id":"t","texture":{"path":"entity/t.png","size":[16,16]},
        "roles":{"head":["a"]},
        "bones":[{"name":"a","cubes":[{"origin":[0,0,0],"size":[1,1,1]}]}]}"#;
    assert!(ModelFile::parse(ok).unwrap().bake().is_ok());
    assert!(ModelFile::parse(&ok.replace("\"schema\":1", "\"schema\":2")).is_err());
    assert!(ModelFile::parse(&ok.replace("\"roles\"", "\"rolez\"")).is_err());
    assert!(ModelFile::parse(&ok.replace("\"origin\"", "\"orign\"")).is_err());
    assert!(
        ModelFile::parse(&ok.replace("[\"a\"]", "[\"missing\"]"))
            .unwrap()
            .bake()
            .is_err()
    );
    assert!(
        ModelFile::parse(&ok.replace("[16,16]", "[0,16]"))
            .unwrap()
            .bake()
            .is_err()
    );
}

fn texture_root() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("SOLARIS_CLIENT_RESOURCE_ROOT").map(PathBuf::from),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist/resources")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|root| root.join("assets/minecraft/textures"))
        .find(|root| root.is_dir())
}
fn png_size(path: &std::path::Path) -> [u32; 2] {
    let info = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
        .read_info()
        .unwrap();
    [info.info().width, info.info().height]
}

#[test]
fn every_shipped_model_is_well_formed() {
    let textures = texture_root();
    assert!(!EMBEDDED.is_empty(), "no Solaris entity models embedded");
    let mut ids = std::collections::BTreeSet::new();
    for (file, source) in EMBEDDED {
        let model = ModelFile::parse(source).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(
            format!("{}.json", model.id),
            *file,
            "id must match file name"
        );
        assert!(ids.insert(model.id.clone()), "duplicate id {file}");
        let path = &model.texture.path;
        assert!(
            path.ends_with(".png") && !path.contains("..") && !path.starts_with('/'),
            "{file}: texture path {path}"
        );
        let [tw, th] = model.texture.size.map(|v| v as f32);
        for (bone, [u0, v0, u1, v1]) in model.uv_rects() {
            let (lo_u, hi_u, lo_v, hi_v) = (u0.min(u1), u0.max(u1), v0.min(v1), v0.max(v1));
            assert!(
                lo_u >= 0. && lo_v >= 0. && hi_u <= tw && hi_v <= th,
                "{file}: bone {bone} samples [{u0},{v0},{u1},{v1}] outside {tw}x{th}"
            );
        }
        let names = model.bone_names();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{file}: duplicate bone names");
        let rig = model.bake().unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(
            rig.bones.iter().any(|bone| !bone.quads.is_empty()),
            "{file}: no geometry"
        );
        for bone in &rig.bones {
            assert_eq!(bone.quads.len(), bone.normals.len());
            for normal in &bone.normals {
                let length: f32 = normal.iter().map(|v| v * v).sum();
                assert!((length - 1.).abs() < 1e-5);
            }
        }
        if model.id.starts_with("battle_") {
            let png = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/battle_video/assets/solaris_battle/textures").join(path);
            assert!(png.is_file(), "{file}: battle texture {path} does not exist");
            assert_eq!(png_size(&png), model.texture.size, "{file}: texture size");
        } else if let Some(root) = &textures {
            let png = root.join(path);
            assert!(
                png.is_file(),
                "{file}: vanilla texture {path} does not exist"
            );
            assert_eq!(png_size(&png), model.texture.size, "{file}: texture size");
        }
    }
    if textures.is_none() {
        eprintln!("vanilla textures not available: texture existence/size check skipped");
    }
}

#[test]
fn malformed_and_duplicate_files_are_skipped_not_fatal() {
    let good = r#"{"schema":1,"id":"ok","texture":{"path":"entity/t.png","size":[16,16]},
        "bones":[{"name":"a","cubes":[{"origin":[0,0,0],"size":[1,1,1]}]}]}"#;
    let bad_face = good.replace(
        "\"size\":[1,1,1]",
        "\"size\":[1,1,1],\"faces\":{\"bottom\":[0,0,1,1]}",
    );
    let catalog = Catalog::from_sources(&[
        ("bad.json", "{ not json"),
        ("face.json", &bad_face),
        ("ok.json", good),
        ("ok2.json", good),
    ]);
    assert_eq!(catalog.rigs.keys().collect::<Vec<_>>(), ["ok"]);
    assert_eq!(catalog.textures["ok"], "entity/t.png");
}

fn embedded(id: &str) -> ModelFile {
    let (_, source) = EMBEDDED.iter().find(|(name, _)| *name == format!("{id}.json")).unwrap();
    ModelFile::parse(source).unwrap()
}
/// `(min, max)` of a top-level bone's cubes along `axis`, in model pixels.
fn extent(model: &ModelFile, bone: &str, axis: usize) -> (f32, f32) {
    let bone = model.bones.iter().find(|b| b.name == bone).unwrap();
    bone.cubes.iter().fold((f32::MAX, f32::MIN), |(low, high), c| {
        let start = bone.pivot[axis] + c.origin[axis];
        (low.min(start), high.max(start + c.size[axis]))
    })
}

#[test]
fn iron_golem_matches_the_oracle_silhouette() {
    // Measured on the vanilla Java oracle (golem-ortho side and front shots)
    // in model pixels: +Y down, feet at 24, front -Z.
    let golem = embedded("iron_golem");
    let top = golem
        .bones
        .iter()
        .map(|b| extent(&golem, &b.name, 1).0)
        .fold(f32::MAX, f32::min);
    // 43 px = 2.69 blocks: the golem stands its full 2.7-block hitbox height.
    assert_eq!(top, -19., "head top");
    assert_eq!(extent(&golem, "head", 2), (-9.5, 0.5), "head and nose sit forward of the torso");
    assert_eq!(extent(&golem, "body", 1), (-9., 8.), "chest over waist");
    assert_eq!(extent(&golem, "body", 2), (-6., 5.));
    assert_eq!(extent(&golem, "right_arm", 1), (-9.5, 20.5), "arms hang from the shoulders");
    assert_eq!(extent(&golem, "right_leg", 1), (8., 24.));
    assert_eq!(extent(&golem, "right_leg", 0), (-7.5, -1.5));
}

#[test]
fn child_villager_nose_is_a_small_bump_over_the_mouth() {
    // Texture net: the child nose is a 2x2x1 box at (23, 0); the oracle side
    // and front shots put it on the two lowest face rows, over the mouth.
    let child = embedded("villager_baby");
    let head = child.bones.iter().find(|b| b.name == "head").unwrap();
    let nose = &head.cubes[1];
    assert_eq!((nose.size, nose.uv), ([2., 2., 1.], [23., 0.]));
    let face = &head.cubes[0];
    assert_eq!(nose.origin[1] + nose.size[1], face.origin[1] + face.size[1], "nose ends at the chin");
    assert_eq!(nose.origin[2] + nose.size[2], face.origin[2], "nose protrudes one pixel");
    // Limbs follow their texture nets instead of stretched strips.
    for (bone, size) in [("body", [4., 5., 3.]), ("right_leg", [2., 3., 2.]), ("left_leg", [2., 3., 2.])] {
        let cube = &child.bones.iter().find(|b| b.name == bone).unwrap().cubes[0];
        assert_eq!(cube.size, size, "{bone}");
        assert!(cube.faces.is_empty(), "{bone} uses the plain box unwrap");
    }
}

#[test]
fn chest_rims_are_single_and_the_latch_straddles_the_lid_seam() {
    for id in ["chest", "double_chest_left", "double_chest_right"] {
        let chest = embedded(id);
        let base = &chest.bones.iter().find(|b| b.name == "body").unwrap().cubes[0];
        // The lid's bottom rim is the only rim at the seam: the base's sides
        // start one texel below the base's own top rim row (v = 33).
        for face in [Face::Front, Face::Back, Face::Left, Face::Right] {
            let rect = base.faces[&face];
            assert_eq!((rect[1], rect[3] - rect[1]), (34., base.size[1]), "{id} {face:?}");
        }
        let (_, lid_bottom) = extent(&chest, "lid", 1);
        let (latch_top, latch_bottom) = extent(&chest, "lock", 1);
        assert_eq!(
            (lid_bottom - latch_top, latch_bottom - lid_bottom),
            (2., 2.),
            "{id}: latch hangs across the seam"
        );
    }
}

/// `(width, height, opaque)` of the PNG: a texel is opaque when its alpha is non-zero.
fn png_opaque(path: &std::path::Path) -> (usize, usize, Vec<bool>) {
    let mut decoder =
        png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut bytes).unwrap();
    let channels = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::GrayscaleAlpha => 2,
        _ => 0,
    };
    let (width, height) = (frame.width as usize, frame.height as usize);
    let opaque = (0..width * height)
        .map(|i| channels == 0 || bytes[i * channels + channels - 1] != 0)
        .collect();
    (width, height, opaque)
}

/// A limb whose baked quads sample only fully transparent texels of the rig's default skin is
/// invisible once the cutout material discards them (the zombie, husk and giant lost their left
/// arm and leg this way). Bakes every shipped rig and samples the baked UVs against the locally
/// supplied vanilla skin; skipped when those textures are absent.
#[test]
fn baked_limbs_sample_visible_texels_of_the_default_skin() {
    let Some(root) = texture_root() else {
        eprintln!("vanilla textures not available: limb visibility check skipped");
        return;
    };
    let limbs = [
        BoneRole::RightArm,
        BoneRole::LeftArm,
        BoneRole::RightLeg,
        BoneRole::LeftLeg,
    ];
    for (file, source) in EMBEDDED {
        let model = ModelFile::parse(source).unwrap();
        let rig = model.bake().unwrap_or_else(|e| panic!("{file}: {e}"));
        if !rig.bones.iter().any(|bone| limbs.contains(&bone.role)) {
            continue;
        }
        let (width, height, opaque) = png_opaque(&root.join(&model.texture.path));
        for bone in rig.bones.iter().filter(|bone| limbs.contains(&bone.role)) {
            if bone.quads.is_empty() {
                continue;
            }
            let visible = bone
                .quads
                .iter()
                .map(|quad| {
                    let span = |axis: usize, size: usize| {
                        let values = quad.map(|vertex| vertex[axis] * size as f32);
                        let low = values.iter().copied().fold(f32::MAX, f32::min).round();
                        let high = values.iter().copied().fold(f32::MIN, f32::max).round();
                        (low as usize, (high as usize).min(size))
                    };
                    let (x0, x1) = span(3, width);
                    let (y0, y1) = span(4, height);
                    (y0..y1)
                        .flat_map(|y| (x0..x1).map(move |x| y * width + x))
                        .filter(|&i| opaque[i])
                        .count()
                })
                .sum::<usize>();
            assert!(
                visible > 0,
                "{file}: limb bone {} samples only transparent texels of {}",
                bone.name,
                model.texture.path
            );
        }
    }
}

/// Rest-pose `(min, max)` of every bone that has geometry, in model blocks
/// (+Y down, front -Z), from the baked embedded rig.
fn rest_bounds(key: &str) -> BTreeMap<String, (Vec3, Vec3)> {
    let rig = authored(key).unwrap_or_else(|| panic!("{key} has no authored rig"));
    let mut matrices = Vec::with_capacity(rig.bones.len());
    let mut bounds = BTreeMap::new();
    for bone in &rig.bones {
        let matrix = bone.parent.map_or(glam::Mat4::IDENTITY, |parent| matrices[parent])
            * super::super::resources::transform(bone.pivot, bone.rotation, bone.scale);
        matrices.push(matrix);
        if bone.quads.is_empty() {
            continue;
        }
        let corners = bone.quads.iter().flatten().map(|v| matrix.transform_point3(Vec3::new(v[0], v[1], v[2])));
        bounds.insert(bone.name.clone(), corners.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p))));
    }
    bounds
}
fn overall_size(bounds: &BTreeMap<String, (Vec3, Vec3)>) -> Vec3 {
    let (low, high) = bounds.values().fold((Vec3::MAX, Vec3::MIN), |(l, h), (a, b)| (l.min(*a), h.max(*b)));
    high - low
}
/// How far `part` reaches outside `of` on its farthest side.
fn protrusion(part: (Vec3, Vec3), of: (Vec3, Vec3)) -> f32 {
    (of.0 - part.0).max(part.1 - of.1).max_element()
}
fn spec_bone<'a>(bones: &'a [BoneSpec], name: &str) -> Option<&'a BoneSpec> {
    bones.iter().find_map(|bone| (bone.name == name).then_some(bone).or_else(|| spec_bone(&bone.bones, name)))
}
/// Texture rectangle `[u0, v0, u1, v1]` (pixels) of a cube's front face.
fn front_face(cube: &CubeSpec) -> [f32; 4] {
    let [u, v] = cube.uv;
    let [w, h, d] = cube.size;
    cube.faces.get(&Face::Front).copied().unwrap_or([u + d, v + d, u + d + w, v + d + h])
}

/// Post-RC4 rows MODEL-OPUS-2/9/10/11/12/16: silhouettes re-authored by eye from public
/// minecraft.wiki renders. Each assertion is the visible defect the row reported.
#[test]
fn post_rc4_reauthored_rigs_keep_their_reference_silhouettes() {
    let px = 1. / 16.;
    let names = |bounds: &BTreeMap<String, (Vec3, Vec3)>| bounds.keys().cloned().collect::<std::collections::BTreeSet<_>>();

    let pig = rest_bounds("minecraft:pig#main");
    assert!(pig["head"].0.z < pig["body"].0.z - 6.5 * px, "pig head sits in front of its body, not on a neck");
    let piglet = rest_bounds("minecraft:pig_baby#main");
    assert!(piglet["head"].0.y < piglet["body"].0.y, "piglet head rises above its back");

    let allay = rest_bounds("minecraft:allay#main");
    assert!(!allay.contains_key("legs") && overall_size(&allay).y <= 11. * px, "allay is a small legless sprite");
    let vex = rest_bounds("minecraft:vex#main");
    assert!(names(&vex).iter().all(|name| !name.contains("leg") && !name.contains("ear")), "vex has no humanoid legs or ears");
    assert!((vex["head"].1 - vex["head"].0).max_element() < 7. * px, "vex head is small");
    let bee = rest_bounds("minecraft:bee_baby#main");
    assert_eq!(names(&bee).iter().filter(|name| name.ends_with("_leg")).count(), 6, "baby bee legs");
    assert!(bee.contains_key("stinger") && bee.contains_key("left_wing") && bee.contains_key("right_wing"));
    let ghastling = embedded("happy_ghast_baby");
    assert!(spec_bone(&ghastling.bones, "body").unwrap().cubes.iter().any(|cube| front_face(cube) == [16., 16., 32., 32.]),
        "ghastling's outer front face samples the white shell, not the pink interior");

    for key in ["minecraft:rabbit#main", "minecraft:rabbit_baby#main"] {
        let rabbit = rest_bounds(key);
        let leg = rabbit["right_hind_leg"].1.y - rabbit["right_hind_leg"].0.y;
        assert!(leg < rabbit["body"].1.y - rabbit["body"].0.y, "{key}: haunches fold below the body, not deer legs");
    }
    let fox = overall_size(&rest_bounds("minecraft:fox#main"));
    assert!(fox.z > 1.8 * fox.y, "fox is long and low: {fox}");
    for key in ["minecraft:panda#main", "minecraft:panda_baby#main"] {
        let body = rest_bounds(key)["body"];
        assert!(body.1.x - body.0.x > body.1.y - body.0.y, "{key}: broad torso");
    }
    for key in ["minecraft:hoglin#main", "minecraft:hoglin_baby#main", "minecraft:zoglin#main", "minecraft:zoglin_baby#main"] {
        let boar = rest_bounds(key);
        assert!(boar.contains_key("mane") && boar.contains_key("left_ear") && boar.contains_key("right_ear"), "{key}");
        for tusk in ["left_tusk", "right_tusk"] {
            assert!(protrusion(boar[tusk], boar["head"]) > px, "{key}: {tusk} sticks out of the head");
        }
    }
    for key in ["minecraft:strider#main", "minecraft:strider_baby#main"] {
        let strider = rest_bounds(key);
        assert_eq!(names(&strider).iter().filter(|name| name.starts_with("leg")).count(), 2, "{key}: two legs");
        let head = strider["head"].1 - strider["head"].0;
        assert!(head.z > 0.8 * head.x && names(&strider).iter().any(|name| name.contains("bristle")), "{key}: deep body with bristles");
    }

    for key in ["minecraft:guardian#main", "minecraft:elder_guardian#main"] {
        let guardian = rest_bounds(key);
        let body = guardian["body"];
        let spikes: Vec<f32> = guardian.iter().filter(|(name, _)| name.starts_with("spike"))
            .map(|(_, spike)| protrusion(*spike, body)).collect();
        assert_eq!(spikes.len(), 12, "{key}");
        let width = body.1.x - body.0.x;
        assert!(spikes.iter().all(|&out| out > 0.12 * width), "{key}: every spike protrudes: {spikes:?}");
    }
    for key in ["minecraft:cod#main", "minecraft:salmon#main"] {
        let fish = rest_bounds(key);
        let size = overall_size(&fish);
        assert!(size.z > 2.4 * size.y, "{key}: long swimming body: {size}");
        assert!(["left_fin", "right_fin", "dorsal"].iter().all(|fin| fish.contains_key(*fin)), "{key}: fins");
        assert!(fish["head"].0.z < fish["tail"].0.z, "{key}: head leads, tail trails");
    }
    for key in ["minecraft:axolotl#main", "minecraft:axolotl_baby#main"] {
        let axolotl = rest_bounds(key);
        let head = axolotl["head"];
        assert!(axolotl["top_gills"].0.y < head.0.y, "{key}: top gills rise above the head");
        assert!(axolotl["right_gills"].0.x < head.0.x && axolotl["left_gills"].1.x > head.1.x, "{key}: side gills");
    }
    let vindicator = embedded("vindicator");
    let cube = |bone: &str| &spec_bone(&vindicator.bones, bone).unwrap().cubes;
    for leg in ["right_leg", "left_leg"] {
        let [u0, v0, _, v1] = front_face(&cube(leg)[0]);
        assert!(u0 < 16. && (26. ..38.).contains(&v0) && v1 <= 38., "{leg} samples the teal trousers");
    }
    assert!(front_face(&cube("body")[0])[1] >= 46., "torso samples the buttoned coat");
    for arm in ["right_arm", "left_arm"] {
        for sleeve in &cube(arm)[..2] {
            let [u0, v0, _, v1] = front_face(sleeve);
            assert!(u0 >= 44. && (46. ..62.).contains(&v0) && v1 <= 62., "{arm} samples the dark coat sleeve");
        }
    }
}
