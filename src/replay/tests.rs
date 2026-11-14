use super::*;
use crate::protocol::Writer;
use format::{Header, RecordingWriter};
use std::sync::atomic::{AtomicU64, Ordering};

fn fixture_file() -> PathBuf {
    static SERIAL: AtomicU64=AtomicU64::new(0);
    let path=std::env::temp_dir().join(format!("solrec-test-{}-{}.solrec",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
    let file=std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
    let mut output=RecordingWriter::new(file,&Header {protocol_version:777,server_build:"authored-fixture".into(),seed:1,start_tick:123,wall_time:0}).unwrap();
    let mut write=|ms,id,body:Vec<u8>| {let mut packet=Writer::default();packet.var(id);packet.0.extend(body);output.packet(&Packet {ms,bytes:packet.0}).unwrap();};
    let mut login_body=Writer(vec![0;16]);
    login_body.string("ReplayTest");login_body.var(0);login_body.0.extend([0;16]);
    write(0,login::clientbound::LOGIN_FINISHED,login_body.0);
    let mut registry=Writer::default();registry.string("minecraft:entity_type");registry.var(0);
    write(1,configuration::clientbound::REGISTRY_DATA,registry.0);
    let mut tags=Writer::default();tags.var(1);tags.string("minecraft:entity_type");tags.var(1);
    tags.string("minecraft:redirectable_projectile");tags.var(0);
    write(2,configuration::clientbound::UPDATE_TAGS,tags.0);
    write(5,configuration::clientbound::FINISH_CONFIGURATION,vec![]);
    let mut health=Writer::default();health.f32(13.5);health.var(17);health.f32(3.);
    write(30,play::clientbound::SET_HEALTH,health.0);
    let marker=|tick:u64| {let mut body=Writer::default();body.string("krasnokamen:tick");body.0.extend(tick.to_be_bytes());body.0.extend((tick*50_000_000).to_be_bytes());body.0};
    write(50,play::clientbound::CUSTOM_PAYLOAD,marker(9000));
    let mut health=Writer::default();health.f32(5.);health.var(10);health.f32(0.);
    write(90,play::clientbound::SET_HEALTH,health.0);
    write(100,play::clientbound::CUSTOM_PAYLOAD,marker(9003));
    write(500,play::clientbound::START_CONFIGURATION,vec![]);
    output.finish().unwrap();
    path
}
fn player(path:&Path) -> Player {
    let clock=VirtualClock::new();
    let mut input=RecordingReader::open(path).unwrap();
    let next=input.next_packet().unwrap();
    Player {session:Session::replay_fixture(),input,next,path:path.into(),presentation:false,phase:Phase::Login,time:Duration::ZERO,next_tick:Duration::ZERO,last_packet:Duration::ZERO,last_tick:None,timebase:Timebase::Wall,tick_origin:None,render_distance:None,seek_warmup_ticks:0,last_wall_ns:None,transcript:Sha256::new(),crowd_instancing:false,crowd_mesh_fingerprint:Default::default(),clock}
}
#[test]
fn same_recording_produces_identical_cpu_frame_hashes_and_received_health() {
    let path=fixture_file();
    let run=|| {
        let mut replay=player(&path);
        let mut hashes=Vec::new();
        for frame in 0..=20 {
            replay.step(frame).unwrap();
            hashes.push(replay.frame_hash());
            if frame==2 {assert_eq!(replay.session.player.health,Some(13.5));}
            if frame==6 {assert_eq!(replay.session.player.health,Some(5.));assert_eq!(replay.last_tick,Some(9003));}
        }
        assert_ne!(hashes[1],hashes[2]);
        hashes
    };
    assert_eq!(run(),run());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn fixed_frames_and_direct_forward_seek_apply_identical_state() {
    let path=fixture_file();
    let hash={let mut replay=player(&path);for frame in 0..=6 {replay.step(frame).unwrap();}replay.frame_hash()};
    let mut replay=player(&path);
    replay.seek(Duration::from_millis(100)).unwrap();
    assert_eq!(replay.frame_hash(),hash);
    assert_eq!(replay.last_tick,Some(9003));
    assert!(replay.advance_to(Duration::from_millis(20)).is_err());
    assert!(replay.advance_to(Duration::from_secs(1)).is_err());
    assert_eq!(replay.time,Duration::from_millis(500),"EOF must stop simulation at the last received packet");
    assert!(replay.advance_to(Duration::from_secs(u64::MAX)).is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn recording_tap_preserves_original_id_bytes_across_compression() {
    for threshold in [None,Some(2)] {
        let original=vec![0x81,0,8,9,10]; // Deliberately noncanonical, valid VarInt 1.
        let mut wire=Writer::default();
        let payload=if threshold.is_some() {
            use std::io::Write;
            let mut payload=Writer::default();payload.var(original.len() as i32);
            let mut zip=flate2::write::ZlibEncoder::new(Vec::new(),flate2::Compression::fast());
            zip.write_all(&original).unwrap();payload.0.extend(zip.finish().unwrap());payload.0
        } else {original.clone()};
        wire.var(payload.len() as i32);wire.0.extend(payload);
        let mut observed=Vec::new();
        let decoded=crate::protocol::extract_observed(&mut wire.0,threshold,|bytes| {observed.extend_from_slice(bytes);Ok(())}).unwrap().unwrap();
        assert_eq!(observed,original);
        assert_eq!(decoded,(1,vec![8,9,10]));
        assert!(wire.0.is_empty());
    }
}

#[test]
fn recorder_worker_derives_first_tick_without_changing_any_packet_bytes() {
    let source=fixture_file();
    let target=source.with_extension("recorded.solrec");
    let mut reader=RecordingReader::open(&source).unwrap();
    let mut header=reader.header.clone();
    header.start_tick=123; // A pre-login estimate must never survive.
    let recorder=format::Recorder::start(format::RecordingOptions {path:target.clone(),header}).unwrap();
    let mut expected=Vec::new();
    while let Some(packet)=reader.next_packet().unwrap() {
        recorder.packet(&packet.bytes).unwrap();
        expected.push(packet.bytes);
    }
    recorder.finish(true,None).unwrap();
    let mut recorded=RecordingReader::open(&target).unwrap();
    assert_eq!(recorded.header.start_tick,9000);
    assert_ne!(recorded.header.wall_time,0);
    for bytes in expected {assert_eq!(recorded.next_packet().unwrap().unwrap().bytes,bytes);}
    assert!(recorded.next_packet().unwrap().is_none());
    let receipt:serde_json::Value=serde_json::from_reader(File::open(format::receipt_path(&target)).unwrap()).unwrap();
    assert_eq!(receipt["clean_completion"],true);
    assert_eq!(receipt["first_tick"],9000);
    assert_eq!(receipt["last_tick"],9003);
    assert_eq!(receipt["markers"],2);
    assert_eq!(receipt["marker_gaps"],2);
    assert_eq!(receipt["packets"],9);
    std::fs::remove_file(format::receipt_path(&target)).unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();
}

#[test]
fn socket_handoff_records_every_packet_and_graceful_stop_flushes_the_take() {
    use crate::protocol::{frame, network::{Event, Network}};
    use std::{io::{Read, Write}, net::TcpListener, sync::{Arc, mpsc}, thread, time::Instant};
    let source=fixture_file();
    let target=source.with_extension("socket.solrec");
    let mut input=RecordingReader::open(&source).unwrap();
    let header=input.header.clone();
    let mut expected=Vec::new();
    let mut wire=Vec::new();
    while let Some(packet)=input.next_packet().unwrap() {
        let (id,body)=packet.split().unwrap();
        wire.extend(frame(id,body,None).unwrap());
        expected.push(packet.bytes);
    }
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let address=listener.local_addr().unwrap().to_string();
    let peer=thread::spawn(move || {
        // Network opens the Login socket before its separate Status socket.
        let (mut login,_)=listener.accept().unwrap();
        login.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let (mut status,_)=listener.accept().unwrap();
        let mut response=Writer::default();
        response.string(r#"{"version":{"protocol":777}}"#);
        status.write_all(&frame(0,&response.0,None).unwrap()).unwrap();
        login.write_all(&wire).unwrap();
        let mut outbound=Vec::new();
        login.take(65536).read_to_end(&mut outbound).unwrap();
    });
    let (send,wake)=mpsc::sync_channel(16);
    let network=Network::connect(address,"SocketRecorder".into(),8,true,"en_us",
        Arc::new(move || {let _=send.try_send(());}),
        Some(format::RecordingOptions {path:target.clone(),header})).unwrap();
    let deadline=Instant::now()+Duration::from_secs(10);
    'receive: loop {
        while let Ok(event)=network.try_recv() {
            match event {
                Event::Packet(Phase::Play,id,_) if id==play::clientbound::START_CONFIGURATION => break 'receive,
                Event::Error(error) => panic!("socket recorder failed: {error}"),
                _ => {}
            }
        }
        wake.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
    }
    network.finish_recording(true).unwrap();
    peer.join().unwrap();
    let mut recorded=RecordingReader::open(&target).unwrap();
    assert_eq!(recorded.header.start_tick,9000);
    for bytes in expected {assert_eq!(recorded.next_packet().unwrap().unwrap().bytes,bytes);}
    assert!(recorded.next_packet().unwrap().is_none(),"Status or outgoing traffic leaked into recording");
    let receipt:serde_json::Value=serde_json::from_reader(File::open(format::receipt_path(&target)).unwrap()).unwrap();
    assert_eq!(receipt["clean_completion"],true,"the take must be explicitly confirmed before stopping IO");
    assert_eq!(receipt["last_tick"],9003);
    std::fs::remove_file(format::receipt_path(&target)).unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();
}

#[test]
fn failed_or_unconfirmed_recording_never_publishes_a_clean_receipt() {
    for invalid_packet in [false,true] {
        let source=fixture_file();
        let target=source.with_extension("failed.solrec");
        let mut input=RecordingReader::open(&source).unwrap();
        let recorder=format::Recorder::start(format::RecordingOptions {path:target.clone(),header:input.header.clone()}).unwrap();
        if invalid_packet {recorder.packet(&[]).unwrap();}
        else {while let Some(packet)=input.next_packet().unwrap() {recorder.packet(&packet.bytes).unwrap();}}
        assert!(recorder.finish(false,None).is_err());
        let receipt:serde_json::Value=serde_json::from_reader(File::open(format::receipt_path(&target)).unwrap()).unwrap();
        assert_eq!(receipt["clean_completion"],false);
        assert!(receipt["error"].is_string());
        if invalid_packet {
            assert!(receipt["first_tick"].is_null());
            assert!(receipt["last_tick"].is_null());
            assert!(receipt["marker_gaps"].is_null());
        } else {assert_eq!(receipt["last_tick"],9003);}
        std::fs::remove_file(format::receipt_path(&target)).unwrap();
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(target).unwrap();
    }
}

#[test]
fn crowd_mesh_fingerprint_uses_content_not_process_global_revision() {
    let mut meshes=crate::entities::CrowdMeshes::default();
    let fingerprint=crowd_mesh_hash(&meshes);
    meshes.revision=999;
    assert_eq!(crowd_mesh_hash(&meshes),fingerprint);
    meshes.chunks.push(std::sync::Arc::new(crate::entities::CrowdChunk { vertices: vec![crate::entities::CrowdVertex::default()], ..Default::default() }));
    assert_ne!(crowd_mesh_hash(&meshes),fingerprint);
}

#[test]
fn tick_timebase_uses_actual_markers_keeps_gaps_and_original_transcript() {
    let path=fixture_file();
    let mut replay=player(&path);
    replay.timebase=Timebase::Tick;
    replay.advance_to(Duration::ZERO).unwrap();
    assert_eq!(replay.last_tick,Some(9000));
    assert_eq!(replay.session.player.health,Some(5.),
        "packets following marker 9000 belong to that tick, not their wall timestamp");
    assert_eq!(replay.tick_origin,Some(9000));
    replay.advance_to(Duration::from_millis(149)).unwrap();
    assert_eq!(replay.last_tick,Some(9000),"a three-tick gap must not be compressed");
    replay.advance_to(Duration::from_millis(150)).unwrap();
    assert_eq!(replay.last_tick,Some(9003));
    assert_eq!(replay.last_wall_ns,Some(450_150_000_000));
    let transcript=replay.transcript.clone().finalize();
    let mut wall=player(&path);
    wall.advance_to(Duration::from_millis(500)).unwrap();
    assert_eq!(wall.transcript.clone().finalize(),transcript,
        "timeline mapping must not rewrite any received packet or timestamp");
    assert_eq!(replay.tick_interval(),Duration::from_millis(50));
    assert_eq!(replay.partial(),0.);
    assert!(replay.advance_to(Duration::from_millis(151)).unwrap_err().contains("last packet"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn replay_radius_follows_recorded_server_unless_explicitly_overridden() {
    let path=fixture_file();
    let mut replay=player(&path);
    let radius=|distance| {
        let mut body=Writer::default();body.var(distance);
        let mut bytes=Writer::default();bytes.var(play::clientbound::SET_CHUNK_CACHE_RADIUS);
        bytes.0.extend(body.0);
        Packet {ms:0,bytes:bytes.0}
    };
    replay.seek(Duration::from_millis(5)).unwrap();
    replay.apply_packet(&radius(12)).unwrap();
    assert_eq!(replay.session.render_distance(),12);
    assert_eq!(replay.session.scene().render_distance,12);
    replay.set_render_distance(Some(20)).unwrap();
    replay.apply_packet(&radius(6)).unwrap();
    assert_eq!(replay.session.render_distance(),20);
    assert_eq!(replay.session.scene().render_distance,20);
    assert!(replay.set_render_distance(Some(1)).is_err());
    assert!(replay.set_render_distance(Some(33)).is_err());
    replay.set_render_distance(None).unwrap();
    assert_eq!(replay.session.render_distance(),6);
    assert_eq!(replay.session.scene().render_distance,6);
    assert_eq!("tick".parse::<Timebase>().unwrap(),Timebase::Tick);
    assert_eq!("wall".parse::<Timebase>().unwrap(),Timebase::Wall);
    assert!("server".parse::<Timebase>().is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn both_timebases_reject_non_increasing_server_markers() {
    let path=fixture_file();
    let bad=path.with_extension("nonmonotonic.solrec");
    let mut input=RecordingReader::open(&path).unwrap();
    let output=std::fs::OpenOptions::new().write(true).create_new(true).open(&bad).unwrap();
    let mut output=RecordingWriter::new(output,&input.header).unwrap();
    let mut phase=Phase::Login;
    while let Some(mut packet)=input.next_packet().unwrap() {
        let (id,body)=packet.split().unwrap();
        if format::tick_marker(phase,id,body).unwrap().is_some_and(|(tick,_)|tick==9003) {
            let mut bytes=Writer::default();bytes.var(id);bytes.string("krasnokamen:tick");
            bytes.0.extend(9000u64.to_be_bytes());bytes.0.extend(450_000_000_000u64.to_be_bytes());
            packet.bytes=bytes.0;
        }
        phase=format::phase_after(phase,id);
        output.packet(&packet).unwrap();
    }
    output.finish().unwrap();
    for timebase in [Timebase::Wall,Timebase::Tick] {
        let mut replay=player(&bad);replay.timebase=timebase;
        assert!(replay.advance_to(Duration::from_millis(150)).unwrap_err().contains("strictly increasing"));
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(bad).unwrap();
}
