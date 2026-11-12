//! Authored fixture only: real GPU crowd/readback, spline camera and ffmpeg.
//! This is not a battle/server recording or a cross-driver pixel-identity claim.
use super::*;

#[test]
#[ignore = "requires an offscreen GPU adapter, ffmpeg and ffprobe"]
fn offscreen_padded_rows_and_mp4_pipe_round_trip() {
    const WIDTH: u32=318;
    const HEIGHT: u32=180;
    let _gpu = crate::render::gpu_test_lock();
    let clock = crate::replay::clock::VirtualClock::new();
    let directory = std::env::var_os("KRASNOKAMEN_CAPTURE_TEST_ARTIFACTS")
        .map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
        .join(format!("capture-smoke-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut session = crate::session::Session::new_mode("authored-capture-fixture".into(), false);
    let camera = camera::CameraPath::parse(br#"{"keyframes":[
        {"time":0,"position":[4.5,3.6,7.5],"look_target":[5,0,4],"fov":70},
        {"time":1,"position":[6.5,3.6,7.5],"look_target":[5,0,4],"fov":70}
    ]}"#).unwrap();
    // 636 RGBA pixels occupy 2544 bytes: each mapped row has 16 padding bytes.
    let mut renderer = pollster::block_on(Renderer::new_offscreen(WIDTH*2, HEIGHT*2, directory.clone())).unwrap();
    session.set_crowd_instancing(renderer.crowd_supported());
    assert!(renderer.crowd_supported(), "capture fixture requires GPU crowd support");
    let mut scene=session.scene().clone();
    // Match the crowd fixture's authored white-atlas/lightmap contract.
    scene.lightmap=Some(crate::api::LightmapTexture {revision:1,rgba:std::sync::Arc::new([255;1024])});
    scene.texture_rgba=Some(std::sync::Arc::new(vec![255;4]));
    scene.texture_width=1;
    scene.texture_height=1;
    scene.texture_layers=1;
    scene.texture_revision=1;
    let (_,mut crowd)=crate::entities::crowd::tests::parity_grid(4,1.2);
    for instance in &mut crowd.instances {
        instance.root[0][3]+=5.;
        instance.root[2][3]+=2.;
    }
    scene.crowd=crowd;
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let readback = renderer.replay_readback().unwrap();
    let observation = session.execution_observation();
    let output = directory.join("fixture.mp4");
    let mut encoder = Encoder::start("ffmpeg", WIDTH, HEIGHT, &output).unwrap();
    let mut pixels = Vec::new();
    let mut sum = vec![0; (WIDTH*HEIGHT*4) as usize];
    let mut frame = vec![0; sum.len()];
    let mut render_seconds = Vec::with_capacity(60);
    let mut first_hash = String::new();
    let clip_start = std::time::Instant::now();
    for index in 0..60 {
        let time = frame_time(index,60);
        clock.set(time);
        session.replay_camera(&camera,time.as_secs_f64(),0.).unwrap();
        let camera_scene=session.scene();
        scene.camera_position=camera_scene.camera_position;
        scene.yaw=camera_scene.yaw;
        scene.pitch=camera_scene.pitch;
        scene.fov_deg=camera_scene.fov_deg;
        let start = std::time::Instant::now();
        renderer.replay_rgba(&scene, &observation, &readback, &mut pixels).unwrap();
        render_seconds.push(start.elapsed().as_secs_f64());
        assert_eq!(pixels.len(), (WIDTH*2*HEIGHT*2*4) as usize, "mapped row padding leaked into RGBA");
        let colors: std::collections::BTreeSet<_> = pixels.chunks_exact(4).map(|p| [p[0],p[1],p[2]]).collect();
        assert!(colors.len()>4, "authored fixture did not produce a nonblank image");
        let hash = format!("{:x}", Sha256::digest(&pixels));
        if index==0 {
            first_hash=hash;
            renderer.replay_rgba(&scene, &observation, &readback, &mut pixels).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&pixels)),first_hash,"TAA-off fixed-time frames differ");
        } else if index==59 {
            assert_ne!(hash,first_hash,"spline camera did not change the rendered image");
        }
        sum.fill(0);
        accumulate(&pixels, WIDTH as usize, HEIGHT as usize, &mut sum);
        for (pixel, sum) in frame.iter_mut().zip(&sum) { *pixel=((sum+2)/4) as u8; }
        encoder.frame(&frame).unwrap();
    }
    encoder.finish().unwrap();
    let clip_seconds = clip_start.elapsed().as_secs_f64();
    std::fs::write(directory.join("frame.rgba"), &frame).unwrap();
    let probe = Command::new("ffprobe").args(["-v","error","-count_frames","-select_streams","v:0","-show_entries","stream=width,height,r_frame_rate,nb_read_frames","-of","json"])
        .arg(&output).output().unwrap();
    assert!(probe.status.success(), "ffprobe: {}", String::from_utf8_lossy(&probe.stderr));
    let result: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let stream = &result["streams"][0];
    assert_eq!(stream["width"],WIDTH);
    assert_eq!(stream["height"],HEIGHT);
    assert_eq!(stream["r_frame_rate"],"60/1");
    assert_eq!(stream["nb_read_frames"],"60");
    std::fs::write(directory.join("ffprobe.json"),&probe.stdout).unwrap();
    let mut gpu=serde_json::json!({});
    renderer.update_metrics(&mut gpu);
    assert_eq!(gpu["crowd_instances"],16);
    assert!(gpu["crowd_draw_calls"].as_u64().unwrap()>0);
    let measured=serde_json::json!({"scope":"authored cubes and 16 armoured crowd fixtures, not a battle recording or hardware GPU performance",
        "frames":60,"render_size":[WIDTH*2,HEIGHT*2],"output_size":[WIDTH,HEIGHT],"fps":60,"taa":false,"supersampling":2,
        "render_seconds_per_frame":render_seconds.iter().sum::<f64>()/60.,"render_seconds":render_seconds,
        "clip_seconds_including_encode":clip_seconds,"clip_seconds_per_frame":clip_seconds/60.,"gpu":gpu});
    std::fs::write(directory.join("timing.json"),serde_json::to_vec_pretty(&measured).unwrap()).unwrap();
    eprintln!("authored offscreen capture artifacts: {}",directory.display());
    eprintln!("render seconds/frame: {}",measured["render_seconds_per_frame"]);
}
