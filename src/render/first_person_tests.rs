use super::*;
use super::shader_tests::pixels;
use bytemuck::Zeroable;

fn quad(geometry: &mut crate::api::DynamicGeometry, depth: f32, half: f32, color: [f32; 3], alpha: f32, family: EntityMaterial) {
    let first = geometry.vertices.len() as u32;
    for position in [[-half, -half, depth], [-half, half, depth], [half, half, depth], [half, -half, depth]] {
        geometry.vertices.push(crate::api::Vertex {
            position,
            color,
            alpha,
            normal: [0., 0., -1.],
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            light: [-1.; 2],
            ..crate::api::Vertex::zeroed()
        });
    }
    geometry.indices[family as usize].extend([first, first + 1, first + 2, first, first + 2, first + 3]);
}

fn rgb(image: &[u8], x: usize, y: usize) -> &[u8] {
    let pixel = (y * 64 + x) * 4;
    &image[pixel..pixel + 3]
}

fn scene() -> Scene {
    Scene {
        lightmap: Some(crate::api::LightmapTexture {
            revision: 1,
            rgba: Arc::new([255; 1024]),
        }),
        environment: crate::api::WorldEnvironment {
            light_directions: Some([[0., 1., 0.], [0., 1., 0.]]),
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn first_person_depth_isolated_from_close_wall_without_revealing_world_or_covering_hud() {
    let _gpu_guard = crate::render::gpu_test_lock();
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64, 64, PathBuf::from(".analysis/first-person-regression"),
    )).unwrap();

    renderer.set_graphics_preset(GraphicsPreset::Low);
    let mut scene = scene();
    quad(&mut scene.dynamic, 0.3, 1., [1., 0., 0.], 1., EntityMaterial::ItemCutout);
    let wall = pixels(&mut renderer, &scene);
    assert!(rgb(&wall, 32, 32)[0] > 32);
    // Ordinary world geometry behind the wall must remain hidden.
    quad(&mut scene.dynamic, 1., 0.3, [0., 0., 1.], 1., EntityMaterial::ItemCutout);
    scene.revision += 1;
    assert_eq!(pixels(&mut renderer, &scene), wall);
    // The hand is geometrically behind that same wall, but belongs to the HUD.
    quad(&mut scene.first_person, 0.7, 0.15, [0., 1., 0.], 1., EntityMaterial::ItemCutout);
    scene.revision += 1;
    let held = pixels(&mut renderer, &scene);
    assert!(rgb(&held, 32, 32)[1] > 32, "held item disappeared behind close world depth");
    assert_eq!(rgb(&held, 4, 32), rgb(&wall, 4, 32), "world occlusion changed outside the hand");
    // The retained crosshair/HUD remains the final painter, above the hand.
    for position in [[30., 30., 0.], [34., 30., 0.], [34., 34., 0.], [30., 34., 0.]] {
        scene.overlay.vertices.push(crate::api::Vertex {
            position, color: [1., 1., 0.], alpha: 1., uv: [-1.; 2],
            overlay_uv: [-1.; 2], overlay_tint: [1.; 3],
            ..crate::api::Vertex::zeroed()
        });
    }
    scene.overlay.indices[0] = vec![0, 1, 2, 0, 2, 3];
    scene.revision += 1;
    assert_eq!(rgb(&pixels(&mut renderer, &scene), 32, 32), &[255, 255, 0]);
    // Camera-mode/scoping eligibility remains the session's responsibility;
    // an empty stream on the next frame must not retain the previous hand.
    scene.first_person = Default::default();
    scene.overlay = Default::default();
    scene.revision += 1;
    assert_eq!(pixels(&mut renderer, &scene), wall);
}

#[test]
fn first_person_preserves_self_depth_and_translucent_sorting() {
    let _gpu_guard = crate::render::gpu_test_lock();
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64, 64, PathBuf::from(".analysis/first-person-regression"),
    )).unwrap();
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let mut scene = scene();
    quad(&mut scene.first_person, 0.6, 0.2, [0., 1., 0.], 1., EntityMaterial::ItemCutout);
    let near = pixels(&mut renderer, &scene);
    assert!(rgb(&near, 32, 32)[1] > 32);
    // Deliberately submit the far opaque hand last: disabling all depth testing
    // would fix wall clipping but fail this self-occlusion regression.
    quad(&mut scene.first_person, 0.9, 0.2, [1., 0., 0.], 1., EntityMaterial::ItemCutout);
    scene.revision += 1;
    assert_eq!(rgb(&pixels(&mut renderer, &scene), 32, 32), rgb(&near, 32, 32));
    scene.first_person = Default::default();
    // Near translucent face first in source order; it must draw after the far
    // face so both contribute, rather than masking the farther face by depth.
    quad(&mut scene.first_person, 0.6, 0.2, [0., 1., 0.], 0.5, EntityMaterial::ItemTranslucent);
    quad(&mut scene.first_person, 0.9, 0.3, [1., 0., 0.], 0.5, EntityMaterial::ItemTranslucent);
    scene.revision += 1;
    let translucent = pixels(&mut renderer, &scene);
    assert!(rgb(&translucent, 32, 32)[0] > 8, "far translucent face was depth-rejected");
    assert!(rgb(&translucent, 32, 32)[1] > rgb(&translucent, 32, 32)[0], "near face must blend last");
}

#[test]
fn first_person_bypasses_world_temporal_history_and_tonemapping() {
    let _gpu_guard = crate::render::gpu_test_lock();
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64, 64, PathBuf::from(".analysis/first-person-regression"),
    )).unwrap();
    let mut scene = scene();
    scene.environment.sky_kind = SkyKind::Overworld;
    let sprite = Some(crate::api::SkyTexture { uv: [0., 0., 1., 1.], layer: 0 });
    scene.environment.sun_texture = sprite;
    scene.environment.moon_texture = sprite;
    quad(&mut scene.dynamic, 0.3, 1., [1., 0., 0.], 1., EntityMaterial::ItemCutout);
    quad(&mut scene.first_person, 0.7, 0.15, [0., 1., 0.], 1., EntityMaterial::ItemCutout);
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let low = pixels(&mut renderer, &scene);
    for preset in [GraphicsPreset::Medium, GraphicsPreset::High] {
        renderer.set_graphics_preset(preset);
        let held = std::mem::take(&mut scene.first_person);
        scene.revision += 1;
        let background = pixels(&mut renderer, &scene);
        scene.first_person = held;
        scene.revision += 1;
        for _ in 0..3 {
            let high = pixels(&mut renderer, &scene);
            assert_eq!(rgb(&high, 32, 32), rgb(&low, 32, 32),
                "hand must not inherit world tone mapping, shadows or TAA jitter");
        }
        let held = std::mem::take(&mut scene.first_person);
        scene.revision += 1;
        let cleared = pixels(&mut renderer, &scene);
        assert_eq!(rgb(&cleared, 32, 32), rgb(&background, 32, 32), "removed hand leaked into world history");
        scene.first_person = held;
        scene.revision += 1;
    }
}

#[test]
fn first_person_projection_does_not_follow_world_fov() {
    let _gpu_guard = crate::render::gpu_test_lock();
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64, 64, PathBuf::from(".analysis/first-person-regression"),
    )).unwrap();
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let mut scene = scene();
    quad(&mut scene.first_person, 0.7, 0.15, [0., 1., 0.], 1., EntityMaterial::ItemCutout);
    scene.fov_deg = 70.;
    let reference = pixels(&mut renderer, &scene);
    assert!(rgb(&reference, 32, 32)[1] > 32, "regression requires real hand pixels");
    for fov in [30., 110., 70. * 1.075] {
        scene.fov_deg = fov;
        scene.revision += 1;
        assert!(pixels(&mut renderer, &scene) == reference,
            "world FOV {fov} changed the first-person silhouette");
    }
}

#[test]
fn camera_pole_view_and_held_space_agree_with_nearby_and_cardinal_views() {
    let eye = Vec3::new(3., 5., -7.);
    for yaw in [0_f32, 90., 180., 270.] {
        let angle = yaw.to_radians();
        let heading = Vec3::new(-angle.sin(), 0., angle.cos());
        let right = Vec3::new(-angle.cos(), 0., -angle.sin());
        for pitch in [-90_f32, -89.999, -45., 0., 45., 89.999, 90.] {
            let view = camera_view(eye, yaw, pitch);
            let p = pitch.to_radians();
            let top = heading * p.sin() + Vec3::Y * p.cos();
            let forward = heading * p.cos() - Vec3::Y * p.sin();
            let point = eye + forward * 4. + top + right * 0.5;
            let projected = view.transform_point3(point);
            assert!((projected - Vec3::new(0.5, 1., -4.)).length() < 0.001,
                "world view yaw={yaw} pitch={pitch}: {projected:?}");
            let held_world = view.inverse().transform_point3(Vec3::new(0.5, 1., -4.));
            assert!((held_world - point).length() < 0.001,
                "first-person basis yaw={yaw} pitch={pitch}: {held_world:?} vs {point:?}");
        }
    }
}
