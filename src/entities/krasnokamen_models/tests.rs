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
        std::env::var_os("KRASNOKAMEN_CLIENT_RESOURCE_ROOT").map(PathBuf::from),
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
    assert!(!EMBEDDED.is_empty(), "no Krasnokamen entity models embedded");
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
                .join("assets/battle_video/assets/krasnokamen_battle/textures").join(path);
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
        let texture = if model.id.starts_with("battle_") {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/battle_video/assets/krasnokamen_battle/textures")
                .join(&model.texture.path)
        } else {
            root.join(&model.texture.path)
        };
        let (width, height, opaque) = png_opaque(&texture);
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

/// Read literal demands from the production selectors rather than copying a test catalog.
fn selection_contract() -> Vec<serde_json::Value> {
    fn visit(dir: &std::path::Path, rows: &mut Vec<serde_json::Value>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { visit(&path, rows); continue; }
            let name = path.file_name().unwrap().to_str().unwrap();
            if path.extension().is_none_or(|ext| ext != "rs")
                || name == "tests.rs" || name.ends_with("_tests.rs") { continue; }
            let source = std::fs::read_to_string(&path).unwrap();
            let relative = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display().to_string();
            let mut function = "";
            let mut test_item = false;
            for (line, text) in source.lines().enumerate() {
                let trimmed = text.trim_start();
                if trimmed == "#[cfg(test)]" { test_item = true; continue; }
                if test_item {
                    if trimmed.starts_with("#[") { continue; }
                    test_item = false;
                    // External test modules can precede production code. Inline test bodies
                    // and test-only helpers end production in these entity modules.
                    if trimmed.ends_with(';') { continue; }
                    break;
                }
                if text.trim_start().starts_with("//") { continue; }
                if let Some((_, tail)) = text.split_once("fn ") {
                    function = tail.split('(').next().unwrap();
                }
                for key in text.split('"').skip(1).step_by(2).filter(|key|
                    key.starts_with("minecraft:") && !key.contains('{')
                        && (key.contains('#') || matches!(*key,
                            "minecraft:wall_banner" | "minecraft:standing_banner")))
                {
                    let key = if key.contains('#') { key.to_owned() } else { format!("{key}#main") };
                    let class = if relative.contains("render_blocks") { "block-special" }
                        else if relative.ends_with("equipment.rs") { "equipment" }
                        else if function == "special_item" { "item-special" }
                        else { "literal" };
                    rows.push(serde_json::json!({
                        "id":format!("literal/{relative}/{}/{key}",line + 1),
                        "class":class,"source":format!("{relative}:{}",line + 1),
                        "selector":function,"required_rig":key,"selected_rig":null,
                        "scope":"production literal demand; branch reachability/pixels unverified"
                    }));
                }
            }
        }
    }
    let mut rows = super::super::resources::rig_selection_contract();
    visit(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/entities"), &mut rows);
    rows.sort_by(|left, right| left["id"].as_str().unwrap().cmp(right["id"].as_str().unwrap()));
    rows
}

fn resolve_contract(
    rows: &[serde_json::Value],
    sources: &[(&str, &str)],
) -> Vec<serde_json::Value> {
    fn normalize_texture(name: &str) -> &str {
        name.trim_end_matches(".png").trim_start_matches("minecraft:").trim_start_matches("textures/")
    }
    let catalog = Catalog::from_sources(sources);
    rows.iter().cloned().map(|mut row| {
        let Some(key) = row["required_rig"].as_str() else {
            row["classification"] = "non-rig-unverified".into();
            row["coverage_pass"] = false.into();
            row["accepted"] = false.into();
            return row;
        };
        let id = model_id(key);
        let rig = catalog.rigs.get(&id);
        let classification = if rig.is_some() {
            "simplified"
        } else if sources.iter().any(|(file, _)| *file == format!("{id}.json")) {
            "rejected"
        } else {
            "missing"
        };
        let selection_matches = row["selected_rig"].as_str().is_none_or(|selected| selected == key);
        let selected = row["class"] == "selection";
        let texture_matches = row["required_texture"].as_str().is_none_or(|expected|
            row["selected_texture"].as_str().is_some_and(|actual|
                normalize_texture(actual) == normalize_texture(expected)));
        let passed = rig.is_some_and(|rig| !rig.is_placeholder())
            && selection_matches && texture_matches && (!selected || row["selection_error"].is_null());
        row["model_id"] = id.clone().into();
        row["model_file"] = format!("assets/krasnokamen/models/entity/{id}.json").into();
        row["classification"] = classification.into();
        row["coverage_pass"] = passed.into();
        // Presence and UV coverage cannot prove a native or oracle frame.
        row["accepted"] = false.into();
        row["native_frame"] = serde_json::Value::Null;
        row["java_frame"] = serde_json::Value::Null;
        row
    }).collect()
}

#[test]
fn entity_selection_coverage_report() {
    let rows = resolve_contract(&selection_contract(), EMBEDDED);
    let ids: std::collections::BTreeSet<_> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), rows.len(), "coverage rows must have stable unique identities");
    for class in ["family", "selection", "pose", "enum", "texture", "armor", "skin",
        "painting-variant", "literal", "block-special", "item-special", "equipment", "non-family"] {
        assert!(rows.iter().any(|row| row["class"] == class), "missing contract class {class}");
    }
    if let Some(path) = std::env::var_os("KRASNOKAMEN_ENTITY_COVERAGE_REPORT") {
        let report = serde_json::json!({
            "schema":1,"game_version":"26.3","embedded_models_digest":embedded_digest(),
            "boundary":"Catalog/selection evidence only; simplified means authored, not oracle-verified. Non-family rows do not imply a missing rig.",
            "rows":rows
        });
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}

#[test]
fn entity_selection_coverage_detects_removed_and_rejected_required_entry() {
    let contract = selection_contract();
    let row_id = "selection/minecraft:cow/adult/received/minecraft:cold";
    let find = |rows: Vec<serde_json::Value>| rows.into_iter().find(|row| row["id"] == row_id).unwrap();
    let good = find(resolve_contract(&contract, EMBEDDED));
    assert_eq!(good["required_rig"], "minecraft:cold_cow#main");
    assert_eq!(good["selected_rig"], good["required_rig"], "{good}");
    assert_eq!(good["coverage_pass"], true, "{good}");
    let removed: Vec<_> = EMBEDDED.iter().copied().filter(|(name, _)| *name != "cold_cow.json").collect();
    let missing = find(resolve_contract(&contract, &removed));
    assert_eq!(missing["classification"], "missing", "{missing}");
    assert_eq!(missing["coverage_pass"], false, "{missing}");
    eprintln!("negative control removed required entry: {missing}");
    let rejected: Vec<_> = EMBEDDED.iter().map(|(name, source)|
        (*name, if *name == "cold_cow.json" { "{invalid" } else { *source })).collect();
    let bad = find(resolve_contract(&contract, &rejected));
    assert_eq!(bad["classification"], "rejected", "{bad}");
    assert_eq!(bad["coverage_pass"], false, "{bad}");
    eprintln!("negative control rejected required entry: {bad}");
}

#[test]
fn received_visual_variants_resolve_their_required_authored_entry() {
    for row in resolve_contract(&selection_contract(), EMBEDDED) {
        if row["class"] == "selection"
            && row["case"].as_str().is_some_and(|case| case.starts_with("received/"))
        {
            assert_eq!(row["coverage_pass"], true, "{row}");
        }
    }
}

#[test]
fn entity_selection_coverage_cold_cow_fringe_samples_the_fur_net() {
    let model = embedded("cold_cow");
    let body = model.bones.iter().find(|bone| bone.name == "body").unwrap();
    let fringe = body.cubes.last().unwrap();
    let samples_fur = |cube: &CubeSpec| {
        let quads = cube_quads(cube, 64., 64.);
        let mut sides = 0;
        for (quad, normal) in quads {
            if normal[1].abs() < 0.9 { continue; }
            sides += 1;
            if quad.iter().any(|vertex| {
                !(30. ..=42.).contains(&(vertex[3] * 64.))
                    || !(32. ..=42.).contains(&(vertex[4] * 64.))
            }) { return false; }
        }
        sides == 2
    };
    assert_eq!(fringe.size[1], 0., "the coat front is a local-Y plane");
    assert!(samples_fur(fringe), "both emitted faces must sample the fur, not the head net");
    // A Front/Back override on this plane affects only its degenerate faces. Without the
    // Up/Down fix the actual emitted faces fall back to the unrelated default UV net.
    let mut wrong_faces = cube(fringe.origin, fringe.size, fringe.uv, fringe.mirror, fringe.inflate);
    wrong_faces.faces.insert(Face::Front, [30., 32., 42., 42.]);
    wrong_faces.faces.insert(Face::Back, [30., 32., 42., 42.]);
    assert!(!samples_fur(&wrong_faces), "the original face-selection mutant must be detected");
}

#[test]
fn entity_selection_coverage_excludes_proven_unreachable_nautilus_babies() {
    let rows = resolve_contract(&selection_contract(), EMBEDDED);
    assert!(!rows.iter().any(|row| row["entity"] == "minecraft:zombie_nautilus" && row["age"] == "baby"),
        "26.3 ZombieNautilus.canBeABaby is false; do not demand invented baby assets");
    let row = |id| rows.iter().find(|row| row["id"] == id).unwrap();
    let calf = row("selection/minecraft:cow/baby/received/minecraft:cold");
    assert_eq!(calf["vanilla_reachability"], "declared_family");
    assert_eq!(calf["accepted"], false, "a declared baby rig still needs actual frame proof");
}

#[test]
fn entity_selection_coverage_wither_skull_material_tracks_received_dangerous_flag() {
    let rows = resolve_contract(&selection_contract(), EMBEDDED);
    for (case, texture) in [
        ("default", "minecraft:entity/wither/wither"),
        ("DATA_DANGEROUS/true", "minecraft:entity/wither/wither_invulnerable"),
    ] {
        let id = format!("selection/minecraft:wither_skull/adult/{case}");
        let row = rows.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["selection_error"], serde_json::Value::Null, "{row}");
        assert_eq!(row["selected_texture"], texture, "{row}");
        assert_eq!(row["coverage_pass"], true, "{row}");
    }
}

#[test]
fn baby_zombie_head_and_helmet_follow_the_matched_control_height() {
    let helmet = embedded("armor_baby_zombie_head");
    // The clean paired 26.3 control fits a six-unit child head below the
    // adult-sized head's old joint; armor uses the same joint, not an offset shell.
    assert_eq!(extent(&helmet, "head", 1), (8.5, 14.5));
    assert_eq!(extent(&helmet, "head", 0), (-3., 3.));
    for name in ["zombie_baby", "husk_baby"] {
        let head = embedded(name);
        assert_eq!(extent(&head, "head", 1), (8.5, 14.5));
        assert_eq!(extent(&head, "head", 0), (-3., 3.));
        assert_eq!(head.bones[0].pivot, helmet.bones[0].pivot);
    }
    assert_eq!(helmet.bones[0].cubes[0].inflate, 0.75);
}

#[test]
fn baby_zombie_and_shared_husk_armor_keep_child_body_proportions() {
    for name in ["zombie_baby", "husk_baby"] {
        let model = embedded(name);
        for (part, pivot, origin, size) in [
            ("body", [0., 16., 0.], [-2., 0., -1.], [4., 4., 2.]),
            ("right_arm", [-3., 17., 0.], [-1., -1., -1.], [2., 4., 2.]),
            ("left_arm", [3., 17., 0.], [-1., -1., -1.], [2., 4., 2.]),
            ("right_leg", [-1., 20., 0.], [-1., 0., -1.], [2., 4., 2.]),
            ("left_leg", [1., 20., 0.], [-1., 0., -1.], [2., 4., 2.]),
        ] {
            let bone = model.bones.iter().find(|bone| bone.name == part).unwrap();
            assert_eq!(bone.pivot, pivot, "{name} {part}");
            assert_eq!(bone.cubes[0].origin, origin, "{name} {part}");
            assert_eq!(bone.cubes[0].size, size, "{name} {part}");
        }
    }
    let chest = embedded("armor_baby_zombie_chest");
    for bone in &chest.bones {
        assert_eq!(bone.cubes[0].inflate, 0.5, "outer shell follows the child scale");
        if bone.name.ends_with("_arm") {
            // The supplied child equipment atlas has three opaque shoulder rows
            // followed by transparent forearm rows, not a solid leg-texture strip.
            assert_eq!(bone.cubes[0].faces[&Face::Front], [28., 20., 32., 28.]);
        }
    }
}

#[test]
fn decorated_pot_body_supports_its_neck_at_the_matched_control_height() {
    let base = embedded("decorated_pot_base");
    let walls = embedded("decorated_pot_sides");
    // Fitted from the clean paired block-special control: full-height body walls
    // meet a flat shoulder and a single broad neck, without a protruding foot band.
    let bottom = &base.bones[0].cubes[0];
    assert_eq!((bottom.origin, bottom.size), ([1., 0., 1.], [14., 0., 14.]));
    let shoulder = &base.bones[0].cubes[1];
    assert_eq!((shoulder.origin, shoulder.size), ([1., 16., 1.], [14., 0., 14.]));
    assert_eq!(base.bones[0].cubes.len(), 3);
    let neck = &base.bones[0].cubes[2];
    assert_eq!((neck.origin, neck.size), ([4., 16., 4.], [8., 4., 8.]));
    assert_eq!(neck.faces[&Face::Front], [8., 12., 16., 8.]);
    assert_eq!(extent(&base, "base", 1), (0., 20.));
    for wall in &walls.bones {
        assert_eq!(wall.cubes[0].origin[1], 0., "wall reaches the bottom cap");
        assert_eq!(wall.cubes[0].origin[1] + wall.cubes[0].size[1], shoulder.origin[1]);
    }
    assert_eq!(extent(&walls, "front", 0), (1., 15.));
}

#[test]
fn bell_wall_texels_follow_the_clean_control_orientation() {
    let bell = embedded("bell");
    for (cube, (low_v, high_v)) in bell.bones[0].cubes.iter().zip([(13., 6.), (23., 21.)]) {
        let mut sides = 0;
        for (quad, normal) in cube_quads(cube, 32., 32.) {
            if normal[1].abs() > 0.9 { continue; }
            sides += 1;
            for vertex in quad {
                let y = vertex[1] * 16.;
                let expected = if (y - cube.origin[1]).abs() < 0.001 { low_v } else { high_v };
                assert_eq!(vertex[4] * 32., expected, "emitted wall vertex {vertex:?}");
            }
        }
        assert_eq!(sides, 4);
    }
}

#[test]
fn obsolete_bed_entity_texture_rigs_are_not_shipped_in_26_3() {
    assert!(EMBEDDED.iter().all(|(name, _)| !matches!(*name, "bed_head.json" | "bed_foot.json")));
}

#[test]
fn baby_chicken_body_and_planar_feet_follow_the_26_3_texture_net() {
    // The 26.3 child is a body/beak assembly with plane legs, toes and wings,
    // not the adult-style head/wattle over a second inflated body.
    for id in ["chicken_baby", "cold_chicken_baby"] {
        let model = embedded(id);
        assert_eq!(model.texture.size, [16, 16]);
        assert!(model.roles.head.is_empty());
        assert_eq!(model.bones.len(), 5);
        assert!(!model.bones.iter().any(|bone| bone.name == "head"));
        let body = model.bones.iter().find(|bone| bone.name == "body").unwrap();
        assert_eq!(body.cubes.len(), 2);
        assert_eq!(body.cubes[0].size, [4., 4., 4.]);
        assert_eq!(body.cubes[1].uv, [10., 8.], "beak uses its own two-pixel island");
        assert_eq!(extent(&model, "body", 1), (18., 22.));
        for name in ["right_leg", "left_leg"] {
            let leg = model.bones.iter().find(|bone| bone.name == name).unwrap();
            assert_eq!(leg.cubes.len(), 2);
            assert_eq!(leg.cubes[0].size, [1., 2., 0.], "shank is a textured plane");
            assert_eq!(leg.cubes[1].size, [1., 0., 1.], "toe lies on the floor");
            assert_eq!(extent(&model, name, 1), (22., 24.));
            assert!(leg.cubes.iter().all(|cube| cube.inflate == 0.));
        }
        for name in ["right_wing", "left_wing"] {
            let wing = model.bones.iter().find(|bone| bone.name == name).unwrap();
            assert_eq!(wing.cubes[0].size, [1., 0., 2.]);
            assert_eq!(wing.cubes[0].inflate, 0.);
        }
    }
    if let Some(root) = texture_root() {
        let model = embedded("chicken_baby");
        for climate in ["temperate", "warm", "cold"] {
            let path = root.join(format!("entity/chicken/chicken_{climate}_baby.png"));
            let (width, height, opaque) = png_opaque(&path);
            for bone in model.bones.iter().filter(|bone| bone.name.ends_with("_leg")) {
                for cube in &bone.cubes {
                    for (quad, _) in cube_quads(cube, width as f32, height as f32) {
                        let point = |v: [f32; 5]| Vec3::new(v[0], v[1], v[2]);
                        if (point(quad[1]) - point(quad[0])).cross(point(quad[2]) - point(quad[0])).length_squared() < 1e-12 {
                            continue;
                        }
                        let span = |axis: usize, size: usize| {
                            let values = quad.map(|v| v[axis] * size as f32);
                            let low = values.iter().copied().fold(f32::INFINITY, f32::min) as usize;
                            let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max) as usize;
                            low..high
                        };
                        for y in span(4, height) {
                            for x in span(3, width) {
                                assert!(opaque[y * width + x], "{climate} {} foot texel ({x}, {y})", bone.name);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn climate_cow_horns_and_cold_muzzle_follow_the_clean_controls() {
    let cold = embedded("cold_cow");
    let muzzle = &cold.bones[0].cubes[1];
    assert_eq!(muzzle.uv, [8., 32.]);
    assert_eq!(muzzle.size, [6., 3., 2.]);
    assert_eq!(face_rects(muzzle).into_iter().find(|(face, _)| *face == Face::Front).unwrap().1,
        [10., 34., 16., 37.], "pink muzzle, not the gray horn net");
    for (id, outer, top) in [("cold_cow", 6., 0.), ("warm_cow", 8., -2.)] {
        let bounds = rest_bounds(&format!("minecraft:{id}#main"));
        for (part, sign) in [("right_horn", -1.), ("left_horn", 1.)] {
            let (low, high) = bounds[part];
            assert!((low.y * 16. - top).abs() < 0.001);
            assert!((high.y * 16. - 2.).abs() < 0.001);
            let edge = if sign < 0. { low.x } else { high.x };
            assert!((edge * 16. - outer * sign).abs() < 0.001);
        }
    }
    for id in ["cold_cow_baby", "warm_cow_baby"] {
        let model = embedded(id);
        assert_eq!(model.bones[0].bones.len(), 2, "{id} has visible horn stubs");
        for horn in &model.bones[0].bones {
            assert_eq!(horn.cubes[0].size, [1., 2., 1.]);
            assert_eq!(horn.cubes[0].uv, [4., 29.]);
        }
    }
}

#[test]
fn warm_nautilus_shell_and_face_follow_both_matched_views() {
    let model = embedded("warm_zombie_nautilus");
    assert_eq!(model.bones[0].rotation, [0., 180., 0.], "head is on the far side in the front control");
    let bounds = rest_bounds("minecraft:warm_zombie_nautilus#main");
    assert!((bounds["shell"].1.y * 16. - 24.).abs() < 0.001, "shell reaches the feet");
    assert!(bounds["head"].0.z > 0., "head is in front of the source-facing shell");
    for i in 0..8 {
        assert!(bounds[&format!("arm{i}")].1.z * 16. > 18., "tentacle {i} reaches beyond the face");
    }
    let shell = &model.bones[0].bones[0];
    // The PNG contains two shell nets, not one ten-pixel band stretched over
    // eighteen model pixels. The lower net includes the red/blue skirt and
    // transparent opening around the head seen in the side control.
    assert_eq!(shell.cubes.len(), 2);
    let upper = &shell.cubes[0];
    let lower = &shell.cubes[1];
    assert_eq!((upper.origin, upper.size), ([-7., 0., -8.], [14., 10., 16.]));
    assert_eq!((lower.origin, lower.size), ([-7., 10., -12.], [14., 8., 20.]));
    assert_eq!(upper.faces[&Face::Back], [16., 16., 30., 26.]);
    assert_eq!(upper.faces[&Face::Left], [0., 16., 16., 26.]);
    assert_eq!(upper.faces[&Face::Up], [30., 16., 16., 0.]);
    assert_eq!(lower.faces[&Face::Back], [20., 46., 34., 54.]);
    assert_eq!(lower.faces[&Face::Left], [0., 46., 20., 54.]);
    assert_eq!(lower.faces[&Face::Up], [20., 26., 34., 46.]);
    assert_eq!(lower.faces[&Face::Down], [34., 26., 48., 46.]);
    for (fan, (pivot, width, v0, v1, planes)) in shell.bones.iter().zip([
        ([6., 18., 5.], 5., 102., 112., 2),
        ([-5.7, 8., 3.], 6., 84., 93., 2),
        ([5., 5.5, -6.], 6., 93., 102., 1),
        ([-8.5, 18., -5.], 4., 112., 122., 2),
    ]) {
        assert_eq!(fan.pivot, pivot, "{} attaches to its observed shell side", fan.name);
        assert_eq!(fan.cubes.len(), planes, "{} preserves the observed plane count", fan.name);
        for cube in &fan.cubes {
            assert_eq!(cube.size[0].max(cube.size[2]), width, "{} uses its actual PNG island width", fan.name);
            assert_eq!(cube.faces.len(), 2, "{} paints both sides of its plane", fan.name);
            for rect in cube.faces.values() {
                assert_eq!(*rect, [0., v0, width, v1], "{} excludes adjacent color islands", fan.name);
            }
        }
    }
    assert_eq!(shell.bones[2].cubes[0].size, [0., 8., 6.]);
}
