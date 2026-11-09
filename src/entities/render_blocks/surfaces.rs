use super::super::presentation::emit;
use super::*;
use crate::api::Vertex;
use crate::ui::render::{TextAlignment, WorldTextOptions};
fn identifier_path(value: &str) -> (&str, &str) {
    value.split_once(':').unwrap_or(("minecraft", value))
}
fn registry_asset<'a>(c: &'a Content, registry: &str, name: &str) -> Result<&'a Value> {
    c.registry_entries(registry)?
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("source registry asset not observed: {registry}/{name}"))
}
fn paint_banner_part(
    rig: &Rig,
    root: Mat4,
    style: crate::entities::presentation::RigStyle,
    flag_angle: f32,
    flag_only: bool,
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let crate::entities::presentation::RigStyle { tex, color, layer, light, .. } = style;
    transforms.clear();
    transforms.reserve(rig.bones.len());
    for bone in &rig.bones {
        let mut rotation = bone.rotation;
        if bone.name == "flag" {
            rotation.x = flag_angle;
        }
        let parent = bone.parent.map(|i| transforms[i]).unwrap_or(root);
        let matrix = parent * resources::transform(bone.pivot, rotation, bone.scale);
        transforms.push(matrix);
        if (bone.name == "flag") != flag_only {
            continue;
        }
        let normal_matrix = matrix.inverse().transpose();
        for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
            let positions = quad.map(|v| matrix.transform_point3(Vec3::new(v[0], v[1], v[2])).to_array());
            let uv = quad.map(|v| [
                tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
            ]);
            emit(positions, uv, color, tex.layer, layer, 1., light, out)?;
            let normal = normal_matrix.transform_vector3(Vec3::from_array(*normal)).normalize_or_zero().to_array();
            let first = out.vertices.len() - 4;
            for vertex in &mut out.vertices[first..] {
                vertex.normal = normal;
            }
        }
    }
    Ok(())
}

pub(super) fn banner(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let name = block_name(e, c)?.trim_start_matches("minecraft:");
    let wall = name.contains("_wall_banner");
    let color = name.trim_end_matches("_banner").trim_end_matches("_wall");
    let key = if wall {
        "minecraft:wall_banner"
    } else {
        "minecraft:standing_banner"
    };
    let angle = if wall {
        yaw(prop(e, c, "facing").unwrap_or("north"))
    } else {
        prop(e, c, "rotation")
            .unwrap_or("0")
            .parse::<f32>()
            .map_err(|_| "banner rotation")?
            * 22.5
    };
    let root = root
        * Mat4::from_translation(Vec3::new(0.5, 0., 0.5))
        * Mat4::from_rotation_y(-angle.to_radians())
        * Mat4::from_scale(Vec3::new(2. / 3., -2. / 3., -2. / 3.));
    let phase = ((i64::from(e.position[0]) * 7
        + i64::from(e.position[1]) * 9
        + i64::from(e.position[2]) * 13
        + f.game_time)
        .rem_euclid(100) as f32
        + f.partial)
        / 100.;
    let flag_angle = math::banner_angle(phase);
    let banner = rig(m, &format!("{key}#main"))?;
    paint_banner_part(banner, root, crate::entities::presentation::RigStyle { tex: texture(c, "entity/banner/banner_base")?, color: [1.; 3], layer: EntityMaterial::EntitySolid, alpha: 1., light: f.light }, flag_angle, false, t, out)?;
    paint_banner_part(banner, root, crate::entities::presentation::RigStyle { tex: texture(c, "entity/banner/banner_base")?, color: [1.; 3], layer: EntityMaterial::EntitySolid, alpha: 1., light: f.light }, flag_angle, true, t, out)?;
    paint_banner_part(banner, root, crate::entities::presentation::RigStyle { tex: texture(c, "entity/banner/base")?, color: rgb(dye(color).0), layer: EntityMaterial::BannerPattern, alpha: 1., light: f.light }, flag_angle, true, t, out)?;
    if let Some(patterns) = e.data_value()["patterns"].as_array() {
        for pattern in patterns.iter().take(16) {
            let id = pattern["pattern"].as_str().ok_or("banner pattern holder")?;
            let asset = registry_asset(c, "minecraft:banner_pattern", id)?["asset_id"]
                .as_str()
                .ok_or("banner pattern asset id")?;
            let (namespace, path) = identifier_path(asset);
            let material = format!("{namespace}:entity/banner/{path}");
            paint_banner_part(banner, root, crate::entities::presentation::RigStyle { tex: texture(c, &material)?, color: rgb(dye(pattern["color"].as_str().unwrap_or("white")).0), layer: EntityMaterial::BannerPattern, alpha: 1., light: f.light }, flag_angle, true, t, out)?;
        }
    }
    Ok(BlockRenderStatus::Rendered)
}
pub(super) fn pot(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let mut root = root
        * around(
            Vec3::splat(0.5),
            Mat4::from_rotation_y(
                (180. - yaw(prop(e, c, "facing").unwrap_or("north"))).to_radians(),
            ),
        );
    if let Some((style, start)) = e.animation.source.wobble {
        let duration = if style == 0 { 7. } else { 10. };
        let p = (e.animation.source.ticks.saturating_sub(start) as f32 + f.partial) / duration;
        // Original extractRenderState never assigns wobbleStyle: the submit path
        // therefore uses the negative branch for either received event style.
        let angles = math::pot_angles(p, false);
        root *= around(
            Vec3::new(0.5, 0., 0.5),
            Mat4::from_rotation_x(angles[0])
                * Mat4::from_rotation_z(angles[2])
                * Mat4::from_rotation_y(angles[1]),
        );
    }
    paint_rig(rig(m, "minecraft:decorated_pot_base#main")?, root, crate::entities::presentation::RigStyle { tex: texture(c, "entity/decorated_pot/decorated_pot_base")?, color: [1.; 3], layer: EntityMaterial::EntitySolid, alpha: 1., light: f.light }, t, out, |_, _, _| {})?;
    let sides = rig(m, "minecraft:decorated_pot_sides#main")?;
    for side in ["back", "left", "right", "front"] {
        let item = e.data_value()["sherds"]
            .get(side)
            .and_then(|template| template["id"].as_str())
            .unwrap_or("minecraft:brick");
        let material = if let Some(pattern) = item
            .strip_prefix("minecraft:")
            .and_then(|v| v.strip_suffix("_pottery_sherd"))
        {
            format!("entity/decorated_pot/{pattern}_pottery_pattern")
        } else {
            "entity/decorated_pot/decorated_pot_side".into()
        };
        let tex = texture(c, &material)?;
        t.clear();
        for bone in &sides.bones {
            let parent = bone.parent.map(|i| t[i]).unwrap_or(root);
            let matrix = parent * resources::transform(bone.pivot, bone.rotation, bone.scale);
            t.push(matrix);
            if bone.name != side {
                continue;
            }
            for quad in &bone.quads {
                let positions = quad.map(|v| {
                    matrix
                        .transform_point3(Vec3::new(v[0], v[1], v[2]))
                        .to_array()
                });
                let uv = quad.map(|v| {
                    [
                        tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                        tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                    ]
                });
                emit(
                    positions,
                    uv,
                    [1.; 3],
                    tex.layer,
                    EntityMaterial::EntitySolid,
                    1.,
                    f.light,
                    out,
                )?;
            }
        }
    }
    Ok(BlockRenderStatus::Rendered)
}
fn beam(
    c: &Content,
    root: Mat4,
    time: f32,
    start: i32,
    height: i32,
    color: u32,
    scale: f32,
    radius: f32,
    glow_radius: f32,
    material: &str,
    out: &mut DynamicGeometry,
) -> Result<()> {
    if height == 0 {
        return Ok(());
    }
    let tex = texture(c, material)?;
    let scroll = if height < 0 { time } else { -time };
    let offset = (scroll * 0.2 - (scroll * 0.1).floor()).rem_euclid(1.) - 1.;
    let center = root * Mat4::from_translation(Vec3::new(0.5, 0., 0.5));
    for glow in [false, true] {
        let (matrix, points, top, layer, alpha) = if glow {
            (
                center,
                [
                    [-glow_radius, -glow_radius],
                    [glow_radius, -glow_radius],
                    [glow_radius, glow_radius],
                    [-glow_radius, glow_radius],
                ],
                height as f32 * scale + offset,
                EntityMaterial::BeaconBeamTranslucent,
                32. / 255.,
            )
        } else {
            (
                center * Mat4::from_rotation_y((time * 2.25 - 45.).to_radians()),
                [[0., radius], [radius, 0.], [0., -radius], [-radius, 0.]],
                height as f32 * scale * (0.5 / radius) + offset,
                EntityMaterial::BeaconBeamOpaque,
                1.,
            )
        };
        for i in 0..4 {
            let a = points[i];
            let b = points[(i + 1) % 4];
            let pos = [
                [a[0], (start + height) as f32, a[1]],
                [a[0], start as f32, a[1]],
                [b[0], start as f32, b[1]],
                [b[0], (start + height) as f32, b[1]],
            ]
            .map(|v| matrix.transform_point3(Vec3::from_array(v)).to_array());
            emit(
                pos,
                [[1., top], [1., offset], [0., offset], [0., top]],
                rgb(color),
                tex.layer,
                layer,
                alpha,
                [15.; 2],
                out,
            )?;
            let len = out.vertices.len();
            for vertex in &mut out.vertices[len - 4..] {
                vertex.overlay_uv = [tex.uv[0], tex.uv[1]];
                vertex.overlay_tint = [tex.uv[2] - tex.uv[0], tex.uv[3] - tex.uv[1], 0.];
            }
        }
    }
    Ok(())
}
const FACES: [[[f32; 3]; 4]; 6] = [
    [[0., 0., 1.], [0., 0., 0.], [1., 0., 0.], [1., 0., 1.]],
    [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    [[1., 1., 0.], [1., 0., 0.], [0., 0., 0.], [0., 1., 0.]],
    [[0., 1., 1.], [0., 0., 1.], [1., 0., 1.], [1., 1., 1.]],
    [[0., 1., 0.], [0., 0., 0.], [0., 0., 1.], [0., 1., 1.]],
    [[1., 1., 1.], [1., 0., 1.], [1., 0., 0.], [1., 1., 0.]],
];
fn cube(
    root: Mat4,
    color: [f32; 3],
    alpha: f32,
    layer: EntityMaterial,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let color = color.map(|v| f32::from((v * 255.) as u8) / 255.);
    let alpha = f32::from((alpha * 255.) as u8) / 255.;
    for face in FACES {
        emit(
            face.map(|v| root.transform_point3(Vec3::from_array(v)).to_array()),
            [[0.; 2]; 4],
            color,
            u32::MAX,
            layer,
            alpha,
            [-1.; 2],
            out,
        )?;
    }
    Ok(())
}
fn wire(
    root: Mat4,
    low: [f32; 3],
    high: [f32; 3],
    color: [f32; 3],
    out: &mut DynamicGeometry,
) -> Result<()> {
    if out.vertices.len().saturating_add(48) > 2_000_000 {
        return Err("structure overlay geometry vertex bound reached".into());
    }
    let color = color.map(|v| f32::from((v * 255.) as u8) / 255.);
    let normal_transform = glam::Mat3::from_mat4(root).inverse().transpose();
    for axis in 0..3 {
        for a in 0..2 {
            for b in 0..2 {
                let mut p = low;
                let mut q = low;
                p[(axis + 1) % 3] = if a == 0 {
                    low[(axis + 1) % 3]
                } else {
                    high[(axis + 1) % 3]
                };
                p[(axis + 2) % 3] = if b == 0 {
                    low[(axis + 2) % 3]
                } else {
                    high[(axis + 2) % 3]
                };
                q = p;
                q[axis] = high[axis];
                let first = u32::try_from(out.vertices.len()).map_err(|_| "gizmo index bound")?;
                let normal = (normal_transform
                    * (Vec3::from_array(q) - Vec3::from_array(p)).normalize_or_zero())
                .normalize_or_zero()
                .to_array();
                for (v, side) in [(p, 1.), (p, -1.), (q, 1.), (q, -1.)] {
                    out.vertices.push(Vertex {
                        position: root.transform_point3(Vec3::from_array(v)).to_array(),
                        color,
                        alpha: 1.,
                        uv: [2.5, 0.],
                        overlay_uv: [side, 0.],
                        overlay_tint: [1.; 3],
                        texture_layer: u32::MAX,
                        light: [-1.; 2],
                        normal,
                    });
                }
                out.indices[EntityMaterial::Lines as usize].extend([
                    first,
                    first + 1,
                    first + 2,
                    first + 3,
                    first + 2,
                    first + 1,
                ]);
            }
        }
    }
    Ok(())
}
fn vector(value: &Value) -> Result<[i32; 3]> {
    Ok(std::array::from_fn(|i| {
        value.get(i).and_then(Value::as_i64).unwrap_or(0) as i32
    }))
}
fn structure_box(data: &Value) -> ([i32; 3], [i32; 3]) {
    let origin = [
        data["posX"].as_i64().unwrap_or(0).clamp(-48, 48) as i32,
        data["posY"].as_i64().unwrap_or(1).clamp(-48, 48) as i32,
        data["posZ"].as_i64().unwrap_or(0).clamp(-48, 48) as i32,
    ];
    let size = [
        data["sizeX"].as_i64().unwrap_or(0).clamp(0, 48) as i32,
        data["sizeY"].as_i64().unwrap_or(0).clamp(0, 48) as i32,
        data["sizeZ"].as_i64().unwrap_or(0).clamp(0, 48) as i32,
    ];
    let mirror = data["mirror"].as_str().unwrap_or("NONE");
    let rotation = data["rotation"].as_str().unwrap_or("NONE");
    let x = if mirror == "FRONT_BACK" {
        -size[0]
    } else {
        size[0]
    };
    let z = if mirror == "LEFT_RIGHT" {
        -size[2]
    } else {
        size[2]
    };
    let (x0, z0, dx, dz) = match rotation {
        "CLOCKWISE_90" => (
            origin[0] + i32::from(z >= 0),
            origin[2] + i32::from(x < 0),
            -z,
            x,
        ),
        "CLOCKWISE_180" => (
            origin[0] + i32::from(x >= 0),
            origin[2] + i32::from(z >= 0),
            -x,
            -z,
        ),
        "COUNTERCLOCKWISE_90" => (
            origin[0] + i32::from(z < 0),
            origin[2] + i32::from(x >= 0),
            z,
            -x,
        ),
        _ => (
            origin[0] + i32::from(x < 0),
            origin[2] + i32::from(z < 0),
            x,
            z,
        ),
    };
    (
        [x0.min(x0 + dx), origin[1], z0.min(z0 + dz)],
        [dx.abs(), size[1], dz.abs()],
    )
}

pub(super) fn structure(
    e: &BlockEntity,
    c: &Content,
    world: &World,
    root: Mat4,
    visible: bool,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let data = &e.data;
    let mode = data["mode"].as_str().or_else(|| prop(e, c, "mode")).unwrap_or("");
    let flag = |key: &str, default: bool| {
        data[key].as_bool().or_else(|| data[key].as_i64().map(|v| v != 0)).unwrap_or(default)
    };
    let save = mode.eq_ignore_ascii_case("save");
    let load = mode.eq_ignore_ascii_case("load");
    if !visible || !(save || load && flag("showboundingbox", true)) {
        return Ok(BlockRenderStatus::SourceHidden);
    }
    let (low, size) = structure_box(data);
    if size.iter().any(|&v| v < 1) {
        return Ok(BlockRenderStatus::SourceHidden);
    }
    if save && flag("showair", false) {
        for x in 0..size[0] {
            for y in 0..size[1] {
                for z in 0..size[2] {
                    let p = [low[0] + x, low[1] + y, low[2] + z];
                    if world.block(std::array::from_fn(|i| e.position[i] + p[i])).is_none() {
                        return Ok(BlockRenderStatus::WorldPending);
                    }
                }
            }
        }
    }
    let high = std::array::from_fn(|i| low[i] + size[i]);
    wire(root, low.map(|v| v as f32), high.map(|v| v as f32), [0.9; 3], out)?;
    if save && flag("showair", false) {
        for x in 0..size[0] {
            for y in 0..size[1] {
                for z in 0..size[2] {
                    let p = [low[0] + x, low[1] + y, low[2] + z];
                    let absolute = std::array::from_fn(|i| e.position[i] + p[i]);
                    let state = world.block(absolute).ok_or("structure overlay world changed")?;
                    let name = c.name(state).ok_or("structure overlay unknown block state")?;
                    let (inset, color) = match name {
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => (0.4, [0.5, 0.5, 1.]),
                        "minecraft:structure_void" => (0.45, [1., 0.75, 0.75]),
                        "minecraft:barrier" => (0.45, [1., 0., 0.]),
                        "minecraft:light" => (0.45, [1., 1., 0.]),
                        _ => continue,
                    };
                    let lo = p.map(|v| v as f32 + inset);
                    let hi = p.map(|v| v as f32 + 1. - inset);
                    wire(root, lo, hi, color, out)?;
                }
            }
        }
    }
    Ok(BlockRenderStatus::Rendered)
}

pub(super) struct TestInstanceFrame<'a> {
    pub origin: [f64; 3],
    pub camera: [f64; 3],
    pub camera_rotation: [f32; 2],
    pub game_time: i64,
    pub partial: f32,
    pub scoping: bool,
    pub visible: bool,
    pub text: Option<&'a TextRenderer>,
    pub cache: &'a mut Cache,
}

pub(super) fn test_instance(
    e: &BlockEntity,
    c: &Content,
    f: &mut TestInstanceFrame,
    root: Mat4,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let data = &e.data["data"];
    let status = data["status"].as_str().unwrap_or("cleared");
    let needs_test = f.visible || status == "finished" && !data["error_message"].is_null();
    let test = match (needs_test, data["test"].as_str()) {
        (true, Some(name)) => c.registry_entries("minecraft:test_instance")?
            .iter().find(|(id, _)| id == name).map(|(_, value)| value),
        _ => None,
    };
    if test.is_some_and(Value::is_null) {
        return Err("test instance registry value not received".into());
    }
    let color = match status {
        "running" => Some(0x808080),
        "finished" if data["error_message"].is_null() => Some(0x00ff00),
        "finished" if test.is_some_and(|v| v["required"].as_bool() == Some(false)) => Some(0xff8000),
        "finished" => Some(0xff0000),
        _ => None,
    };
    let initial = out.vertices.len();
    if let Some(color) = color {
        let distance = ((f.camera[0] - f64::from(e.position[0]) - 0.5).powi(2)
            + (f.camera[2] - f64::from(e.position[2]) - 0.5).powi(2)).sqrt() as f32;
        let radius = if f.scoping { 1. } else { (distance / 96.).max(1.) };
        beam(c, root, (f.game_time % 40) as f32 + f.partial, 0, 2048, color,
             1., 0.2 * radius, 0.25 * radius, "entity/beacon/beacon_beam", out)?;
    }
    if f.visible {
        let padding = test.and_then(|v| v["padding"].as_i64()).unwrap_or(0) as i32;
        let mut size = vector(&data["size"])?;
        let quarter = |rotation: &str| match rotation {
            "clockwise_90" => 1, "clockwise_180" => 2,
            "counterclockwise_90" => 3, _ => 0,
        };
        let rotation = quarter(data["rotation"].as_str().unwrap_or("none"))
            + quarter(test.and_then(|v| v["rotation"].as_str()).unwrap_or("none"));
        if rotation % 2 != 0 {
            size.swap(0, 2);
        }
        if size.iter().all(|&v| v > 0) {
            let pos = [padding, padding + 1, padding + 1];
            wire(root, pos.map(|v| v as f32),
                std::array::from_fn(|i| (pos[i] + size[i]) as f32), [0.9; 3], out)?;
        }
    }
    if let Some(errors) = e.data["errors"].as_array().filter(|v| !v.is_empty()) {
        let text = f.text.ok_or("source error marker font painter not activated")?;
        let layouts = f.cache.errors(e, text)?;
        for (index, error) in errors.iter().enumerate() {
            let p = vector(&error["pos"])?;
            let matrix = Mat4::from_translation(Vec3::from_array(std::array::from_fn(|i| {
                (f64::from(p[i]) - f.origin[i] - 0.02) as f32
            }))) * Mat4::from_scale(Vec3::splat(1.04));
            cube(matrix, [1., 0., 0.], 0.375, EntityMaterial::DebugFilledBox, out)?;
            let layout = layouts.get(index).ok_or("source error marker layout missing")?;
            let matrix = Mat4::from_translation(Vec3::new(
                (f64::from(p[0]) + 0.5 - f.origin[0]) as f32,
                (f64::from(p[1]) + 1.2 - f.origin[1]) as f32,
                (f64::from(p[2]) + 0.5 - f.origin[2]) as f32,
            )) * Mat4::from_rotation_y(-f.camera_rotation[0].to_radians())
                * Mat4::from_rotation_x(f.camera_rotation[1].to_radians())
                * Mat4::from_scale(Vec3::new(-0.16, -0.16, 0.16))
                * Mat4::from_translation(Vec3::new(-layout.width / 2., 0., 0.));
            text.append_world_layout(c, layout, matrix, 0xffffffff,
                &WorldTextOptions { alignment: TextAlignment::Center,
                    layer: EntityMaterial::TextSeeThrough, ..Default::default() }, out)?;
        }
    }
    Ok(if out.vertices.len() == initial { BlockRenderStatus::SourceHidden } else { BlockRenderStatus::Rendered })
}
pub(super) fn beacon(
    e: &BlockEntity,
    c: &Content,
    root: Mat4,
    camera: [f64; 3],
    time: f32,
    scoping: bool,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    if e.animation.source.pending_world {
        return Ok(BlockRenderStatus::WorldPending);
    }
    if e.animation.source.beacon_levels == 0 {
        return Ok(BlockRenderStatus::SourceHidden);
    }
    let sections = &e.animation.source.beam_sections;
    let distance = ((camera[0] - f64::from(e.position[0]) - 0.5).powi(2)
        + (camera[2] - f64::from(e.position[2]) - 0.5).powi(2))
    .sqrt() as f32;
    let radius = if scoping { 1. } else { (distance / 96.).max(1.) };
    let mut start = 0;
    for (i, section) in sections.iter().enumerate() {
        beam(
            c,
            root,
            time,
            start,
            if i + 1 == sections.len() { 2048 } else { section.height },
            section.color,
            1.,
            0.2 * radius,
            0.25 * radius,
            "entity/beacon/beacon_beam",
            out,
        )?;
        start += section.height;
    }
    Ok(BlockRenderStatus::Rendered)
}

pub(super) fn portal(
    e: &BlockEntity,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let gateway = e.type_name == "minecraft:end_gateway";
    let mut visible = [false; 6];
    for direction in 0..6 {
        if gateway {
            let d = super::super::physical::direction(direction)?;
            let neighbor = std::array::from_fn(|i| e.position[i] + d[i] as i32);
            if neighbor[1] < f.world.min_y || neighbor[1] >= f.world.min_y + f.world.height {
                visible[direction as usize] = true;
                continue;
            }
            let Some(state) = f.world.block(neighbor) else {
                return Ok(BlockRenderStatus::WorldPending);
            };
            visible[direction as usize] = !c.info(state)?.occludes[(direction ^ 1) as usize];
        } else {
            visible[direction as usize] = direction == 0 || direction == 1;
        }
    }
    let layer = c.texture("minecraft:environment/end_sky")
        .ok_or("missing mandatory portal texture minecraft:environment/end_sky")?
        .layer;
    if gateway
        && let Some((scale, height, spawning)) = math::gateway_beam(
            e.animation.source.gateway_age,
            e.animation.source.gateway_cooldown,
            f.partial,
            f.world.min_y + f.world.height - 1,
        )
    {
        beam(
            c,
            root,
            (f.game_time % 40) as f32 + f.partial,
            -height,
            height * 2,
            dye(if spawning { "magenta" } else { "purple" }).0,
            scale,
            0.15,
            0.175,
            "entity/end_portal/end_gateway_beam",
            out,
        )?;
    }
    let matrix = if gateway {
        root
    } else {
        root * Mat4::from_translation(Vec3::new(0., 0.375, 0.))
            * Mat4::from_scale(Vec3::new(1., 0.375, 1.))
    };
    for direction in 0..6 {
        if !visible[direction as usize] {
            continue;
        }
        emit(
            FACES[direction as usize]
                .map(|v| matrix.transform_point3(Vec3::from_array(v)).to_array()),
            [[0.; 2]; 4],
            [1.; 3],
            layer,
            if gateway {
                EntityMaterial::EndGateway
            } else {
                EntityMaterial::EndPortal
            },
            1.,
            [-1.; 2],
            out,
        )?;
    }
    Ok(BlockRenderStatus::Rendered)
}

