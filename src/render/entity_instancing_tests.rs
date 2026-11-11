//! The GPU crowd path against the CPU entity path on a real device (no mocks): the same
//! armoured zombie, emitted as dynamic geometry or culled, posed and drawn instanced,
//! must produce the same frame, shadows included.
use super::entity_instancing::{lod, planes, LOD_PIXELS};
use crate::api::{DynamicGeometry, Vertex};
use super::shader_tests::pixels;
use super::*;

fn offscreen() -> (Renderer, std::sync::MutexGuard<'static, ()>) {
    sized(64)
}

fn sized(side: u32) -> (Renderer, std::sync::MutexGuard<'static, ()>) {
    let guard = gpu_test_lock();
    let renderer = pollster::block_on(Renderer::new_offscreen(
        side,
        side,
        PathBuf::from(".analysis/crowd-regression"),
    ))
    .unwrap();
    (renderer, guard)
}

const SPRITE: crate::api::SkyTexture = crate::api::SkyTexture { uv: [0., 0., 1., 1.], layer: 0 };

fn base() -> Scene {
    Scene {
        lightmap: Some(crate::api::LightmapTexture { revision: 1, rgba: Arc::new([255; 1024]) }),
        texture_rgba: Some(Arc::new(vec![255; 4])),
        texture_width: 1,
        texture_height: 1,
        texture_layers: 1,
        texture_revision: 1,
        camera_position: [0., 1.4, -2.6],
        environment: crate::api::WorldEnvironment {
            display_encoded: true,
            sky_kind: SkyKind::Overworld,
            sky_color: [0.47, 0.65, 1.0],
            fog_color: [0.75, 0.85, 1.0],
            sun_angle: 30.,
            moon_angle: 210.,
            fog_start: 200.,
            fog_end: 400.,
            sun_texture: Some(SPRITE),
            moon_texture: Some(SPRITE),
            light_directions: Some([[0.2, 1.0, -0.7], [-0.2, 1.0, 0.7]]),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Pixels whose channels differ by more than 8 levels.
fn differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4).zip(b.chunks(4)).filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 8)).count()
}

#[test]
fn crowd_frames_match_cpu_entity_frames() {
    let (mut renderer, _guard) = offscreen();
    if renderer.crowd.is_none() {
        eprintln!("adapter without compute/indirect/vertex storage: crowd path unavailable");
        return;
    }
    let (cpu, crowd) = crate::entities::crowd::tests::parity_scenes();
    for preset in [GraphicsPreset::Low, GraphicsPreset::Medium] {
        renderer.set_graphics_preset(preset);
        let mut empty = base();
        empty.revision = 1;
        let blank = pixels(&mut renderer, &empty);
        let mut reference = base();
        reference.revision = 2;
        reference.dynamic = cpu.clone();
        let expected = pixels(&mut renderer, &reference);
        assert_eq!(renderer.stats.crowd_instances, 0);
        let mut instanced = base();
        instanced.revision = 3;
        instanced.crowd = crowd.clone();
        let actual = pixels(&mut renderer, &instanced);
        assert_eq!(renderer.stats.crowd_instances, 1, "{preset:?}");
        assert!(renderer.stats.crowd_draws > 0, "{preset:?}");
        let covered = differing(&blank, &expected);
        assert!(covered > 200, "{preset:?}: the zombie covers the frame ({covered} px)");
        let wrong = differing(&expected, &actual);
        assert!(wrong * 100 <= covered, "{preset:?}: {wrong} of {covered} entity pixels differ");
    }
}

/// Sixteen instances of two composites with interleaved bone ranges, some crossing the
/// frame edge: every instance lands in its composite's region and samples its own bones.
#[test]
fn crowd_grid_matches_cpu_grid() {
    let (mut renderer, _guard) = sized(160);
    if renderer.crowd.is_none() {
        return;
    }
    let (cpu, crowd) = crate::entities::crowd::tests::parity_grid(4, 1.2);
    assert_eq!((crowd.instances.len(), crowd.counts.as_slice()), (16, &[8, 8][..]));
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let mut empty = base();
    empty.revision = 1;
    let blank = pixels(&mut renderer, &empty);
    let mut reference = base();
    reference.revision = 2;
    reference.dynamic = cpu;
    let expected = pixels(&mut renderer, &reference);
    let mut instanced = base();
    instanced.revision = 3;
    instanced.crowd = crowd;
    let actual = pixels(&mut renderer, &instanced);
    assert_eq!(renderer.stats.crowd_instances, 16);
    let covered = differing(&blank, &expected);
    assert!(covered > 4000, "the grid covers the frame ({covered} px)");
    let wrong = differing(&expected, &actual);
    assert!(wrong * 100 <= covered, "{wrong} of {covered} entity pixels differ");
}

/// `a` followed by `b` moved by `dx` along x (one dynamic geometry).
fn with_shifted(a: &DynamicGeometry, b: &DynamicGeometry, dx: f32) -> DynamicGeometry {
    let mut out = a.clone();
    let base = out.vertices.len() as u32;
    out.vertices.extend(b.vertices.iter().map(|v| Vertex {
        position: [v.position[0] + dx, v.position[1], v.position[2]],
        ..*v
    }));
    for (material, indices) in b.indices.iter().enumerate() {
        out.indices[material].extend(indices.iter().map(|index| index + base));
    }
    out
}

/// Crowd and CPU entities in one frame: the crowd draws first in the entity stage and
/// must leave the dynamic-geometry state (instance slot, buffers, pipelines) intact.
#[test]
fn crowd_and_cpu_entities_share_a_frame() {
    let (mut renderer, _guard) = offscreen();
    if renderer.crowd.is_none() {
        return;
    }
    let (cpu, crowd) = crate::entities::crowd::tests::parity_scenes();
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let mut reference = base();
    reference.revision = 1;
    reference.dynamic = with_shifted(&cpu, &cpu, 1.2);
    let expected = pixels(&mut renderer, &reference);
    let mut cpu_only = base();
    cpu_only.revision = 2;
    cpu_only.dynamic = with_shifted(&DynamicGeometry::default(), &cpu, 1.2);
    let half = pixels(&mut renderer, &cpu_only);
    let mut mixed = base();
    mixed.revision = 3;
    mixed.dynamic = cpu_only.dynamic.clone();
    mixed.crowd = crowd;
    let actual = pixels(&mut renderer, &mixed);
    assert_eq!(renderer.stats.crowd_instances, 1);
    let covered = differing(&half, &expected);
    assert!(covered > 200, "the instanced zombie covers the frame ({covered} px)");
    let wrong = differing(&expected, &actual);
    assert!(wrong * 100 <= covered, "{wrong} of {covered} pixels differ");
}

#[test]
fn culled_crowd_draws_nothing_and_leaves_the_frame_unchanged() {
    let (mut renderer, _guard) = offscreen();
    if renderer.crowd.is_none() {
        return;
    }
    let (_, crowd) = crate::entities::crowd::tests::parity_scenes();
    let mut empty = base();
    empty.yaw = 180.;
    empty.revision = 1;
    let blank = pixels(&mut renderer, &empty);
    let mut behind = base();
    behind.yaw = 180.;
    behind.revision = 2;
    behind.crowd = crowd;
    let frame = pixels(&mut renderer, &behind);
    assert_eq!(renderer.stats.crowd_instances, 1);
    assert_eq!(differing(&blank, &frame), 0, "an instance behind the camera is culled");
}

#[test]
fn cull_planes_and_detail_levels() {
    let projection = Mat4::perspective_rh(70_f32.to_radians(), 1., 0.05, 2048.);
    let planes = planes(projection);
    let inside = |p: Vec3, r: f32| planes.iter().all(|plane| Vec3::from_slice(&plane[..3]).dot(p) + plane[3] >= -r);
    assert!(inside(Vec3::new(0., 0., -10.), 0.1), "ahead of a right-handed camera");
    assert!(!inside(Vec3::new(0., 0., 10.), 1.), "behind");
    assert!(!inside(Vec3::new(30., 0., -10.), 1.), "far to the side");
    assert!(inside(Vec3::new(10.5, 0., -10.), 4.), "a sphere overlapping the side plane");
    let pixels_per_unit = projection.y_axis.y * 540.;
    assert_eq!(lod(1., 5., pixels_per_unit), 0);
    let at = |pixels: f32| 2. * pixels_per_unit / pixels;
    assert_eq!(lod(1., at(LOD_PIXELS[0] + 1.), pixels_per_unit), 0);
    assert_eq!(lod(1., at(LOD_PIXELS[0] - 1.), pixels_per_unit), 1);
    assert_eq!(lod(1., at(LOD_PIXELS[1] - 1.), pixels_per_unit), 2);
}
