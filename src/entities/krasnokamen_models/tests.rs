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
        ("minecraft:sign/wall/oak#main", "sign_wall"),
        (
            "minecraft:hanging_sign/oak/ceiling#main",
            "hanging_sign_ceiling",
        ),
        ("minecraft:zombie#layer3/modelSet/head", "armor_head"),
        (
            "minecraft:piglin#layer3/babyModelSet/feet",
            "armor_baby_feet",
        ),
        ("minecraft:armor_stand#boots", "armor_feet"),
        ("minecraft:armor_stand_small#chestplate", "armor_baby_chest"),
        ("minecraft:armor_stand#renderer_main", "armor_stand"),
        ("minecraft:double_chest_left#main", "double_chest_left"),
    ] {
        assert_eq!(model_id(key), id, "{key}");
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
    assert!(
        !EMBEDDED.is_empty(),
        "no Krasnokamen entity models embedded"
    );
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
        if let Some(root) = &textures {
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
    assert!(
        (parent.pivot.x * 16. - 0.5).abs() < 0.01,
        "goat body parent: {:?}",
        parent.pivot
    );
}

#[test]
fn goat_ears_and_beard_stay_upright_at_rest() {
    let rig = authored("minecraft:goat#main").unwrap();
    let face = &rig.bones[rig.part_by_name["face"]];
    let face_transform =
        crate::entities::resources::transform(face.pivot, face.rotation, face.scale);
    for name in ["right_ear", "left_ear", "beard"] {
        let bone = &rig.bones[rig.part_by_name[name]];
        let part_transform =
            crate::entities::resources::transform(bone.pivot, bone.rotation, bone.scale);
        let axis = (face_transform * part_transform)
            .transform_vector3(Vec3::Y)
            .normalize();
        // Java 26.3 front capture: ears are full-height and the beard hangs vertically.
        assert!(axis.z.abs() < 0.02, "{name} vertical axis: {axis:?}");
    }
    let left = rig.bones[rig.part_by_name["left_ear"]].pivot.x;
    let right = rig.bones[rig.part_by_name["right_ear"]].pivot.x;
    assert!(
        ((left - right).abs() * 16. - 8.).abs() < 0.01,
        "ear spacing"
    );
}

#[test]
fn spider_legs_form_single_sloped_oracle_silhouettes() {
    let (_, source) = EMBEDDED
        .iter()
        .find(|(name, _)| *name == "spider.json")
        .unwrap();
    let model = ModelFile::parse(source).unwrap();
    for index in 0..8 {
        let name = format!("leg{index}");
        let leg = model.bones.iter().find(|bone| bone.name == name).unwrap();
        // Java 26.3 reference: the visible legs are straight, sloped bars,
        // not horizontal bars with vertical downward extensions at their ends.
        assert_eq!(
            leg.cubes.len(),
            1,
            "{name} must not have an extra vertical foot"
        );
        assert!(
            (28. ..=40.).contains(&leg.rotation[2].abs()),
            "{name} slope"
        );
    }
}

/// ISSUE-1-18: the adult sheep and its wool layer had the legs at z = 6 / -2 and the head 4
/// pixels too low. Source `SheepModel`/`QuadrupedModel`: head offset (0, 6, -8), hind legs
/// z = +7, front legs z = -5 (x = +-3, y = 12), body xRot 90 degrees; the wool layer shares
/// every part pose of the base model.
#[test]
fn sheep_and_wool_use_the_vanilla_quadruped_part_offsets() {
    for file in ["sheep.json", "sheep_wool.json"] {
        let (_, source) = EMBEDDED.iter().find(|(name, _)| *name == file).unwrap();
        let model = ModelFile::parse(source).unwrap();
        let part = |name: &str| model.bones.iter().find(|bone| bone.name == name).unwrap();
        for (name, x, z) in [
            ("right_hind_leg", -3., 7.),
            ("left_hind_leg", 3., 7.),
            ("right_front_leg", -3., -5.),
            ("left_front_leg", 3., -5.),
        ] {
            let pivot = part(name).pivot;
            assert_eq!([pivot[0], pivot[1], pivot[2]], [x, 12., z], "{file} {name}");
        }
        assert_eq!(part("head").pivot[1], 6., "{file} head height");
        assert_eq!(part("body").rotation[0], 90., "{file} body lies horizontal");
    }
}

/// ISSUE-1-01: the legs sampled the arm region of the texture, so profession layers drew
/// translucent garbage inside the robe. Adult legs are single mirrored box-unwrap cubes
/// that begin under the robe hem and leave the feet visible.
#[test]
fn villager_legs_use_one_mirrored_box_unwrap_beneath_the_robe() {
    let (_, source) = EMBEDDED
        .iter()
        .find(|(name, _)| *name == "villager.json")
        .unwrap();
    let model = ModelFile::parse(source).unwrap();
    let right = model
        .bones
        .iter()
        .find(|bone| bone.name == "right_leg")
        .unwrap();
    let left = model
        .bones
        .iter()
        .find(|bone| bone.name == "left_leg")
        .unwrap();
    let body = model.bones.iter().find(|bone| bone.name == "body").unwrap();
    let robe = body
        .bones
        .iter()
        .find(|bone| bone.name == "jacket")
        .unwrap();
    assert_eq!(right.cubes.len(), 1);
    assert_eq!(left.cubes.len(), 1);
    let (a, b) = (&right.cubes[0], &left.cubes[0]);
    assert_eq!(a.size, b.size);
    assert_eq!(a.uv, b.uv);
    assert!(!a.mirror && b.mirror);
    assert!(
        a.faces.is_empty() && b.faces.is_empty(),
        "legs use the box unwrap, not custom texture faces"
    );
    assert_eq!(right.pivot[2], body.pivot[2]);
    assert_eq!(left.pivot[2], body.pivot[2]);
    let hem = body.pivot[1] + robe.pivot[1] + robe.cubes[0].origin[1] + robe.cubes[0].size[1];
    let hip = right.pivot[1] + a.origin[1];
    let feet = hip + a.size[1];
    assert!(
        hip < hem && hem < feet,
        "the robe covers the upper legs but leaves the feet visible"
    );
}

/// ISSUE-1-01: the child villager kept the pre-fix legacy rig (8x2x4 body, 4x13x4 arms, 4x8x4
/// legs inside them). The oracle-checked child rig has 2x3x2 legs hanging from the jacket
/// hem, one cube each, using the box unwrap.
#[test]
fn baby_villager_legs_hang_from_the_jacket_hem_as_single_cubes() {
    let (_, source) = EMBEDDED
        .iter()
        .find(|(name, _)| *name == "villager_baby.json")
        .unwrap();
    let model = ModelFile::parse(source).unwrap();
    let right = model
        .bones
        .iter()
        .find(|bone| bone.name == "right_leg")
        .unwrap();
    let left = model
        .bones
        .iter()
        .find(|bone| bone.name == "left_leg")
        .unwrap();
    let body = model.bones.iter().find(|bone| bone.name == "body").unwrap();
    let jacket = body
        .bones
        .iter()
        .find(|bone| bone.name == "jacket")
        .unwrap();
    for leg in [right, left] {
        assert_eq!(leg.cubes.len(), 1, "{}", leg.name);
        assert_eq!(leg.cubes[0].size, [2., 3., 2.], "{}", leg.name);
        assert!(
            leg.cubes[0].faces.is_empty(),
            "{} uses the box unwrap",
            leg.name
        );
    }
    let hem = body.pivot[1] + jacket.pivot[1] + jacket.cubes[0].origin[1] + jacket.cubes[0].size[1];
    assert!(
        right.pivot[1] + right.cubes[0].origin[1] >= hem,
        "legs start at the jacket hem"
    );
}
