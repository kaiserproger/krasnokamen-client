use super::format::{Header, RecordingOptions};
use super::roster::{self, Roster, Watch};
use crate::{content_paths, session::Session};
use std::{fs::File, io::Read, path::{Path,PathBuf}, sync::{Arc, mpsc}, time::{Duration, Instant}};

pub(crate) fn cli(mut args: impl Iterator<Item=String>) -> Result<(), String> {
    let (mut server, mut username, mut output, mut header, mut ready, mut data) = (None, None, None, None, None, None);
    let mut duration = None;
    let mut roster = None;
    let (mut roster_file,mut roster_ready_file)=(None,None);
    let mut roster_timeout=None;
    while let Some(arg) = args.next() {
        if arg == "--help" { crate::launch::print_help(); return Ok(()); }
        let value = args.next().ok_or_else(|| format!("record missing value for {arg}"))?;
        match arg.as_str() {
            "--server" => server=Some(value), "--username" => username=Some(value),
            "--output" => output=Some(PathBuf::from(value)), "--header" => header=Some(PathBuf::from(value)),
            "--ready-file" => ready=Some(PathBuf::from(value)), "--data-dir" => data=Some(PathBuf::from(value)),
            "--duration" => duration=Some(value.parse::<f64>().map_err(|e| e.to_string())?),
            "--ready-entity-uuids" => roster=Some(PathBuf::from(value)),
            "--roster-file" => roster_file=Some(PathBuf::from(value)),
            "--roster-ready-file" => roster_ready_file=Some(PathBuf::from(value)),
            "--roster-timeout" => roster_timeout=Some(value.parse::<f64>().map_err(|e|e.to_string())?),
            _ => return Err(format!("unknown record argument {arg}")),
        }
    }
    let server = server.ok_or("record requires --server")?;
    let username = username.ok_or("record requires --username")?;
    let output = output.ok_or("record requires --output")?;
    let duration = duration.filter(|v| v.is_finite() && *v > 0. && *v <= 86_400.)
        .ok_or("record requires --duration in (0,86400] seconds")?;
    if output.extension().is_none_or(|extension| extension != "solrec") { return Err("record output must end in .solrec".into()); }
    if ready.as_ref().is_some_and(|path| path.exists()) { return Err("record ready-file already exists; choose a new take name".into()); }
    let dynamic=roster_file.is_some();
    if dynamic!=roster_ready_file.is_some() || (dynamic && roster.is_some()) {
        return Err("choose static --ready-entity-uuids or paired --roster-file and --roster-ready-file".into());
    }
    if roster_timeout.is_some() && !dynamic {return Err("--roster-timeout requires paired post-join roster mode".into());}
    let roster_timeout=roster_timeout.unwrap_or(120.);
    if !roster_timeout.is_finite() || roster_timeout<=0. || roster_timeout>86_400. {
        return Err("--roster-timeout must be in (0,86400] seconds".into());
    }
    if roster_ready_file.as_ref().is_some_and(|path|path.exists()
        || ready.as_ref()==Some(path) || roster_file.as_ref()==Some(path)) {
        return Err("roster-ready-file must be a fresh path distinct from ready-file and roster-file".into());
    }
    let header = header.ok_or("record requires --header")?;
    let mut bytes=Vec::new();
    File::open(&header).map_err(|e| format!("open {}: {e}",header.display()))?.take(16*1024+1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len()>16*1024 { return Err("record header exceeds 16 KiB".into()); }
    let header: Header=serde_json::from_slice(&bytes).map_err(|e| format!("record header: {e}"))?;
    let mut required_entities=roster.as_deref().map(roster::load).transpose()?.unwrap_or_default();
    let empty=Roster::new();
    let root=data.map(Ok).unwrap_or_else(content_paths::default_root)?;
    content_paths::apply_defaults(&root);
    content_paths::validate(false)?;
    let mut session=Session::new_mode("recorder".into(),false);
    session.set_recording(Some(RecordingOptions {path:output.clone(),header}));
    let (send, receive)=mpsc::sync_channel(1);
    let wake:Arc<dyn Fn()+Send+Sync>=Arc::new(move || { let _=send.try_send(()); });
    session.set_wake(wake.clone());
    let mut watcher=roster_file.clone().map(|path|Watch::start(path,wake)).transpose()?;
    let mut roster_loaded=!dynamic;
    session.connect(&server,&username)?;
    let result=(|| {
        let startup_deadline=Instant::now()+Duration::from_secs(120);
        let mut deadline=None;
        let mut joined=false;
        let mut roster_deadline=None;
        let mut next_tick=Instant::now();
        loop {
            session.process_events();
            session.replay_error()?;
            let now=Instant::now();
            if let Some(watch)=&watcher && let Some(roster)=watch.take_result()? {
                required_entities=roster;
                roster_loaded=true;
                watcher=None;
            }
            let join_roster=if dynamic {&empty} else {&required_entities};
            if !joined && session.recording_ready(join_roster) {
                publish_ready(ready.as_deref(),&output,&session,"joined",join_roster.len())?;
                eprintln!("solrec ready: {}",output.display());
                joined=true;
                if dynamic {roster_deadline=Some(now+Duration::from_secs_f64(roster_timeout));}
                else {deadline=Some(now+Duration::from_secs_f64(duration));}
            }
            if joined && dynamic && deadline.is_none() && roster_loaded && session.recording_ready(&required_entities) {
                publish_ready(roster_ready_file.as_deref(),&output,&session,"roster",required_entities.len())?;
                eprintln!("solrec roster ready: {} UUIDs",required_entities.len());
                deadline=Some(now+Duration::from_secs_f64(duration));
            }
            let end=deadline.or(roster_deadline).unwrap_or(startup_deadline);
            if now>=end {
                if !joined {return Err("recorder never reached initial Play+pose+marker readiness within 120s".into());}
                if deadline.is_none() {
                    return Err(format!("recorder roster readiness timed out after {roster_timeout}s: published={roster_loaded}, required UUIDs={}",required_entities.len()));
                }
                return Ok(());
            }
            if now>=next_tick {
                session.tick_after_events(true);
                session.replay_error()?;
                next_tick=now+session.tick_interval();
            }
            match receive.recv_timeout(next_tick.min(end).saturating_duration_since(Instant::now())) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(error) => return Err(format!("recorder wake channel closed: {error}")),
            }
        }
    })();
    // Run even on a failed take: successful exit always means flushed durable bytes.
    let finish=session.finish_recording(result.is_ok());
    result.and(finish)
}

fn publish_ready(path:Option<&Path>,output:&Path,session:&Session,stage:&str,required:usize) -> Result<(),String> {
    if let Some(path)=path {
        super::format::publish_json(path,&serde_json::json!({"recording":output,"stage":stage,"phase":"play","pose_known":true,
            "presentation":false,"tick":session.recording_tick(),"tick_marker_received":true,
            "required_entities":required,"received_entities":session.entities.len()}))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_roster_flags_are_paired_and_exclusive_with_static_roster() {
        for extra in [
            vec!["--roster-file","roster.txt"],
            vec!["--roster-ready-file","roster.ready"],
            vec!["--ready-entity-uuids","static.txt","--roster-file","roster.txt","--roster-ready-file","roster.ready"],
        ] {
            let args=["--server","127.0.0.1:25565","--username","Recorder","--output","take.solrec","--duration","1"]
                .into_iter().chain(extra).map(str::to_owned);
            assert!(cli(args).unwrap_err().contains("paired"));
        }
    }
    #[test]
    fn ready_signal_contains_latest_received_marker_and_non_rendering_mode() {
        let directory=std::env::temp_dir().join(format!("recorder-ready-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&directory).unwrap();
        let output=directory.join("take.solrec");
        let path=directory.join("take.ready");
        let mut session=Session::replay_fixture();
        session.phase="play".into();
        session.player.pose_known=true;
        session.set_recording(Some(RecordingOptions {path:output.clone(),header:Header {
            protocol_version:777,server_build:"ready-fixture".into(),seed:0,start_tick:0,wall_time:0,
        }}));
        for tick in [9000,9003] {
            let mut body=crate::protocol::Writer::default();
            body.string("krasnokamen:tick");body.i64(tick);body.i64(0);
            session.receive_packet(crate::protocol::network::Phase::Play,
                crate::protocol::packet_ids::play::clientbound::CUSTOM_PAYLOAD,&body.0).unwrap();
        }
        assert!(session.recording_ready(&Roster::new()));
        publish_ready(Some(&path),&output,&session,"joined",0).unwrap();
        let ready:serde_json::Value=serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        assert_eq!(ready["tick"],9003,"first marker must not be mistaken for latest marker");
        assert_eq!(ready["presentation"],false);
        assert_eq!(ready["stage"],"joined");
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;
    #[test]
    fn roster_timeout_accepts_large_finite_setup_but_rejects_unbounded_values() {
        for value in ["0","-1","NaN","inf","86401","900"] {
            let args=["--server","127.0.0.1:25565","--username","Recorder","--output","take.solrec","--duration","1",
                "--roster-file","roster.txt","--roster-ready-file","roster.ready","--roster-timeout",value]
                .into_iter().map(str::to_owned);
            let error=cli(args).unwrap_err();
            if value=="900" {assert!(error.contains("requires --header"),"{error}");}
            else {assert!(error.contains("--roster-timeout must"),"{error}");}
        }
    }
}
