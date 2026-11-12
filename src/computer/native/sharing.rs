use super::*;
use crate::computer::{Frame, audio::AudioSource, relay::{self, Encoder, Id, Request}};
use crate::voice::{self, FRAME};
use std::{collections::{BTreeMap, BTreeSet}, time::{Duration, Instant}};

#[derive(Default)]
pub(super) struct Sharing {
    /// Connection epoch and screen dimension this state belongs to. The server keeps
    /// screens and grants across a same-dimension respawn, so this state does too.
    scope: Option<(u64, Option<u8>)>,
    announced: Option<Id>,
    pub pending: Option<Request>,
    pub viewers: BTreeMap<[u8; 16], String>,
    watching: BTreeSet<Id>,
    next_frame: Option<Instant>,
    encoder: Encoder,
    stats_at: Option<Instant>,
    audio: Option<AudioCapture>,
    /// `source.audio()` returned nothing (or failed) for this announcement: do not retry.
    audio_unavailable: bool,
}
/// Owner side of a monitor's audio: capture -> 20 ms Opus packets -> relay and/or own mixer.
struct AudioCapture {
    source: Box<dyn AudioSource>,
    encoder: voice::Encoder,
    pcm: Vec<f32>,
    seq: u32,
    sent: u64,
    /// Demand went off at this time; the capture stays warm briefly for looks away and back.
    idle_since: Option<Instant>,
}
/// A non-private capture (Windows loopback) is released this long after demand ends.
const AUDIO_GRACE: Duration = Duration::from_secs(3);
/// Never queue more than this much owner audio (240 ms) behind a stalled frame.
const MAX_PENDING_FRAMES: usize = 12;
/// App audio is music-like, not speech: 64 kbit/s (about 160 bytes per 20 ms packet).
const APP_BITRATE: i32 = 64_000;
impl AudioCapture {
    /// Encode every whole 20 ms frame captured so far (oldest audio beyond 240 ms is
    /// dropped). Returns the packets made before any failure, and the failure.
    fn step(&mut self) -> (Vec<(u32, Vec<u8>)>, Option<String>) {
        let mut packets = Vec::new();
        if let Err(error) = self.source.drain(&mut self.pcm) {
            return (packets, Some(error));
        }
        let excess = self.pcm.len().saturating_sub(MAX_PENDING_FRAMES * FRAME);
        self.pcm.drain(..excess - excess % FRAME);
        while self.pcm.len() >= FRAME {
            let frame: [f32; FRAME] = self.pcm[..FRAME].try_into().expect("one frame");
            self.pcm.drain(..FRAME);
            match self.encoder.encode(&frame) {
                Ok(opus) => {
                    self.seq = self.seq.wrapping_add(1);
                    self.sent += 1;
                    packets.push((self.seq, opus));
                }
                Err(error) => return (packets, Some(error)),
            }
        }
        (packets, None)
    }
}
impl Sharing {
    pub fn dialog(&self, selected: usize) -> Option<crate::ui::desktop::DesktopDialog> {
        use crate::ui::desktop::{DesktopAction, DesktopDialog};
        let request = self.pending.as_ref()?;
        Some(DesktopDialog {
            title: "Allow screen and audio sharing?".into(),
            body: vec![format!("{} wants to view your screens.", request.name),
                "Includes app audio, only while looking. No remote input.".into()],
            entries: vec![
                ("Deny".into(), DesktopAction::ViewPermission { request: request.number, allow: false }),
                ("Allow all my open screens and their app audio".into(), DesktopAction::ViewPermission { request: request.number, allow: true }),
            ],
            selected,
        })
    }
}
impl App {
    pub(super) fn answer_screen_request(&mut self, number: u32, allow: bool) -> Result<(), String> {
        if self.computer.sharing.pending.as_ref().is_none_or(|request| request.number != number) {
            return Err("no_pending_screen_request".into());
        }
        let request = self.computer.sharing.pending.take().unwrap();
        self.computer.selection = 0;
        match self.session.send_screen(request.answer(allow)) {
            Ok(()) => {
                if allow { self.computer.sharing.viewers.insert(request.viewer, request.name); }
                self.computer.message = if allow {
                    "Screens and app audio shared while they look. F8 revokes ALL viewers; Shift+Esc closes"
                } else { "Viewing denied; desktop remains private" }.into();
                Ok(())
            }
            Err(error) => {
                self.computer.message = error.clone();
                Err(error)
            }
        }
    }
    pub(super) fn revoke_screen_viewer(&mut self, viewer: [u8; 16]) {
        if self.computer.sharing.viewers.remove(&viewer).is_none() { return; }
        if let Some(id) = self.computer.sharing.announced {
            let mut packet = id.packet(5);
            packet.extend(viewer);
            self.computer.message = match self.session.send_screen(packet) {
                Ok(()) => "Viewer permission revoked".into(),
                Err(error) => error,
            };
        }
        self.computer.selection = 0;
    }
    pub(super) fn sync_screen_sharing(&mut self, enabled: bool) {
        let scope = (
            self.session.context_epochs().0,
            self.session.screen_dimension(),
        );
        if self.computer.sharing.scope != Some(scope) {
            self.computer.sharing = Sharing {
                scope: Some(scope),
                ..Default::default()
            };
        }
        let live = enabled && self.session.phase == "play";
        if live && let Err(error) = self.session.register_screens() {
            self.computer.message = error;
        }
        // Screen audio uses the voice codec and is enabled by the server's VOICE_STATE;
        // a server without it (older, or [voice] off) never receives AUDIO_PART.
        if live && let Err(error) = self.session.register_voice() {
            self.computer.message = error;
        }
        let current = if live && self.computer.source.is_some() && self.computer.size != [0; 2] {
            self.computer.face.zip(self.session.screen_dimension()).map(|(face, dimension)| Id { face, dimension })
        } else { None };
        if current != self.computer.sharing.announced {
            if let Some(old) = self.computer.sharing.announced.take() {
                let _ = self.session.send_screen(old.packet(1));
                self.session.screen_relay.demand.remove(&old);
            }
            if let Some(request) = self.computer.sharing.pending.take() {
                let _ = self.session.send_screen(request.answer(false));
            }
            self.computer.sharing.viewers.clear();
            self.computer.sharing.next_frame = None;
            self.computer.sharing.audio = None;
            self.computer.sharing.audio_unavailable = false;
            if let Some(id) = current {
                match self.session.send_screen(id.packet(0)) {
                    Ok(()) => self.computer.sharing.announced = Some(id),
                    Err(error) => self.computer.message = error,
                }
            }
        }
        if !live {
            for id in std::mem::take(&mut self.computer.sharing.watching) {
                let _ = self.session.send_screen(id.packet(6));
            }
            self.computer.sharing.pending = None;
            for remote in self.session.screen_relay.active.values_mut() {
                remote.frame = None;
                remote.dirty = true;
            }
        }
        if let Some(notice) = self.session.screen_relay.notice.take() {
            self.computer.message = notice;
            self.publish_session();
        }
        while let Some((id, allow)) = self.session.screen_relay.answers.pop_front() {
            if self.computer.sharing.watching.contains(&id) {
                self.computer.message = if allow {
                    "Viewing allowed — look at the screen; F8 leaves"
                } else {
                    "Screen owner denied viewing"
                }
                .into();
                self.publish_session();
            } else if allow {
                // F8 cancelled this request before the owner answered: leave the late grant.
                self.session.screen_relay.cancelled_grants_left += 1;
                let _ = self.session.send_screen(id.packet(6));
                self.publish_session();
            }
        }
        if self.computer.sharing.pending.is_none() {
            while let Some(request) = self.session.screen_relay.requests.pop_front() {
                if live && Some(request.screen) == self.computer.sharing.announced {
                    self.release("desktop viewing permission request");
                    self.computer.selection = 0;
                    self.computer.sharing.pending = Some(request);
                    self.publish_session();
                    break;
                }
                let _ = self.session.send_screen(request.answer(false));
            }
        }
        if let Some(renderer) = &mut self.renderer {
            let own = self.computer.sharing.announced;
            let dimension = self.session.screen_dimension();
            renderer.retain_shared_screens(|id| live && Some(id.dimension) == dimension
                && Some(id) != own && self.session.screen_relay.active.contains_key(&id));
            for (id, remote) in &mut self.session.screen_relay.active {
                if live && Some(id.dimension) == dimension && Some(*id) != own
                    && (remote.dirty || !renderer.has_shared_screen(*id)) {
                    let frame = remote.frame.take().unwrap_or_else(relay::on_frame);
                    renderer.update_shared_screen(*id, &frame);
                    remote.dirty = false;
                }
            }
        }
        if std::env::var_os("SOLARIS_SCREEN_STATS").is_some() {
            let now = Instant::now();
            if self.computer.sharing.stats_at.is_none_or(|at| now >= at) {
                self.computer.sharing.stats_at = Some(now + Duration::from_secs(1));
                eprintln!("{}", relay::stats::line());
            }
        }
    }
    /// `frame` is a fresh capture, or `None` when the source had nothing new (a static window):
    /// then only an owed key frame is answered, from the last picture sent.
    pub(super) fn publish_screen_frame(&mut self, frame: Option<&Frame>) {
        let Some(id) = self.computer.sharing.announced else { return; };
        // Local approval is required as well as server demand. Never trust a
        // demand alone to authorize disclosure of desktop bytes.
        if self.computer.sharing.viewers.is_empty() { return; }
        let Some(demand) = self.session.screen_relay.demand.get(&id).copied() else { return; };
        if frame.is_none() && !self.session.screen_relay.key_wanted.contains(&id) { return; }
        // A requested key frame (new viewer, resync) is answered at once, not at the next frame slot.
        let key = self.session.screen_relay.key_wanted.remove(&id);
        let now = Instant::now();
        if !key && self.computer.sharing.next_frame.is_some_and(|next| now < next) { return; }
        self.computer.sharing.next_frame = Some(now + Duration::from_secs_f64(1. / f64::from(demand.fps)));
        if key { self.computer.sharing.encoder.want_key(); }
        let encoder = &mut self.computer.sharing.encoder;
        let result = match frame {
            Some(frame) => encoder.encode(id, frame, demand.width),
            None => encoder.repeat_key(id, demand.width),
        }
            .and_then(|packets| {
                // An identical picture produced no packets: nothing is sent.
                let sent = !packets.is_empty();
                let key_sent = packets.first().is_some_and(|packet| packet[27] == 1);
                for packet in packets {
                    relay::stats::TX_BYTES.fetch_add(packet.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    self.session.send_screen(packet)?;
                }
                if sent {
                    relay::stats::TX_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if key_sent { relay::stats::TX_KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                }
                Ok(())
            });
        if let Err(error) = result {
            // The encoder's reference chain and what viewers have may differ now: restart with a key frame.
            self.computer.sharing.encoder.want_key();
            self.computer.message = format!("Screen sharing: {error}");
        }
    }
    /// Capture the shared source's audio and relay it while viewers are demanded; a
    /// private-sink app (nested monitor) is also played at its own monitor.
    pub(super) fn pump_screen_audio(&mut self) {
        let announced = self.computer.sharing.announced;
        let demanded = self.session.voice_net.enabled
            && announced.is_some_and(|id| {
                !self.computer.sharing.viewers.is_empty() && self.session.screen_relay.demand.contains_key(&id)
            });
        let private = self.computer.source.as_ref().is_some_and(|source| source.private_audio());
        let now = Instant::now();
        let sharing = &mut self.computer.sharing;
        if announced.is_none() || self.computer.source.is_none() {
            sharing.audio = None;
            self.session.screen_audio_owner.capturing = false;
            return;
        }
        if demanded || private {
            if let Some(capture) = &mut sharing.audio { capture.idle_since = None; }
        } else if let Some(capture) = &mut sharing.audio
            && now.duration_since(*capture.idle_since.get_or_insert(now)) >= AUDIO_GRACE
        {
            sharing.audio = None;
        }
        if sharing.audio.is_none() && (demanded || private) && !sharing.audio_unavailable {
            let native = self.computer.source.as_mut().and_then(|source| source.audio());
            match (crate::computer::audio::resolve(native), voice::Encoder::with_bitrate(APP_BITRATE, true)) {
                (Some(source), Ok(encoder)) => {
                    sharing.audio = Some(AudioCapture { source, encoder, pcm: Vec::new(), seq: 0, sent: 0, idle_since: None });
                }
                (_, encoder) => {
                    sharing.audio_unavailable = true;
                    if let Err(error) = encoder { eprintln!("screen audio: {error}"); }
                }
            }
        }
        let owner = &mut self.session.screen_audio_owner;
        owner.capturing = sharing.audio.is_some();
        owner.unavailable = sharing.audio_unavailable;
        let (Some(id), Some(capture)) = (announced, &mut sharing.audio) else { return };
        let (packets, failure) = capture.step();
        owner.packets_sent = capture.sent;
        let local = private;
        if let Some(error) = failure {
            eprintln!("screen audio: {error}");
            sharing.audio = None;
            sharing.audio_unavailable = true;
        }
        for (seq, opus) in packets {
            if local && let Err(error) = self.session.play_screen_audio(id, seq, &opus) {
                eprintln!("screen audio playback: {error}");
            }
            if demanded {
                let mut packet = id.packet(7);
                packet.extend(seq.to_be_bytes());
                packet.extend(&opus);
                if let Err(error) = self.session.send_screen(packet) {
                    self.computer.message = format!("Screen sharing: {error}");
                    break;
                }
            }
        }
    }
    pub(super) fn request_shared_screen(&mut self, face: Face) -> bool {
        let Some(dimension) = self.session.screen_dimension() else { return false; };
        let id = Id { face, dimension };
        if Some(id) == self.computer.sharing.announced || !self.session.screen_relay.active.contains_key(&id) {
            return false;
        }
        self.computer.release_input();
        let _ = self.session.command(Command::Input(InputIntent::ReleaseAll));
        match self.session.send_screen(id.packet(3)) {
            Ok(()) => {
                self.computer.sharing.watching.insert(id);
                self.computer.message = "Viewing requested. Waiting for the owner; F8 cancels/leaves".into();
            }
            Err(error) => self.computer.message = error,
        }
        self.publish_session();
        true
    }
    pub(super) fn sharing_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.physical_key == PhysicalKey::Code(KeyCode::F8)
            && event.state == ElementState::Pressed && !event.repeat
        {
            self.leave_screen_sharing();
            return true;
        }
        false
    }
    /// F8: revoke every viewer of this player's screen and leave (or cancel the pending
    /// request for) every screen this player watches.
    fn leave_screen_sharing(&mut self) {
        if let Some(id) = self.computer.sharing.announced {
            for (viewer, _) in std::mem::take(&mut self.computer.sharing.viewers) {
                let mut packet = id.packet(5); packet.extend(viewer);
                let _ = self.session.send_screen(packet);
            }
            self.session.screen_relay.demand.remove(&id);
            self.session.screen_relay.key_wanted.remove(&id);
        }
        for id in std::mem::take(&mut self.computer.sharing.watching) {
            let _ = self.session.send_screen(id.packet(6));
        }
        // Clear already-rendered pictures immediately rather than waiting
        // for STOP to return through the network.
        for remote in self.session.screen_relay.active.values_mut() {
            remote.frame = None; remote.dirty = true;
        }
        self.computer.message = "Screen sharing revoked; viewing ended. Local capture continues".into();
        self.publish_session();
    }
    /// Automation `client_screen_share`: the same paths as the player's right-click on a
    /// remote monitor (`request`), the owner's consent dialog (`answer`) and F8 (`leave`).
    pub(crate) fn screen_share_command(&mut self, arguments: &serde_json::Value) -> Result<serde_json::Value, String> {
        use serde_json::json;
        let result = match arguments["action"].as_str() {
            Some("request") => {
                if self.session.screen() != "game" || self.session.dialog_screen_active() {
                    return Err("screen_share_request_requires_game_screen".into());
                }
                let hit = self.computer_hit().ok_or("not_looking_at_a_monitor")?;
                let face = self.monitor_face(&hit)?;
                let dimension = self.session.screen_dimension().ok_or("no_screen_dimension")?;
                let id = Id { face, dimension };
                if !self.request_shared_screen(face) {
                    return Err("not_an_active_remote_screen".into());
                }
                if !self.computer.sharing.watching.contains(&id) {
                    return Err(self.computer.message.clone());
                }
                json!({"action": "request", "block": face.block, "side": face.side,
                    "width": face.width, "height": face.height, "dimension": dimension})
            }
            Some("answer") => {
                let allow = arguments["allow"].as_bool().ok_or("allow_required_for_answer")?;
                let request = self.computer.sharing.pending.as_ref().ok_or("no_pending_screen_request")?;
                let (number, viewer) = (request.number, request.name.clone());
                self.answer_screen_request(number, allow)?;
                json!({"action": "answer", "request": number, "viewer": viewer, "allow": allow})
            }
            Some("leave") => {
                let watching = self.computer.sharing.watching.len();
                let viewers = self.computer.sharing.viewers.len();
                self.leave_screen_sharing();
                json!({"action": "leave", "left_screens": watching, "revoked_viewers": viewers})
            }
            _ => return Err("invalid_arguments: arguments.action is outside the declared enum".into()),
        };
        self.publish_session();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn capture(source: crate::computer::audio::WavSource) -> AudioCapture {
        AudioCapture {
            source: Box::new(source),
            encoder: voice::Encoder::with_bitrate(APP_BITRATE, true).unwrap(),
            pcm: Vec::new(),
            seq: 0,
            sent: 0,
            idle_since: None,
        }
    }
    #[test]
    fn captured_audio_becomes_sequenced_decodable_20ms_opus_packets() {
        use crate::computer::audio::WavSource;
        let mut capture = capture(WavSource::tone(440., 1.).advanced(Duration::from_millis(100)));
        let (packets, failure) = capture.step();
        assert!(failure.is_none());
        assert!((4..=6).contains(&packets.len()), "100 ms is five 20 ms packets, got {}", packets.len());
        assert_eq!(packets.iter().map(|p| p.0).collect::<Vec<_>>(), (1..=packets.len() as u32).collect::<Vec<_>>());
        assert!(packets.iter().all(|(_, opus)| (1..=relay::MAX_OPUS).contains(&opus.len())));
        let mut decoder = voice::Decoder::new().unwrap();
        let peak = packets.iter().skip(1).map(|(_, opus)| {
            decoder.decode(Some(opus)).unwrap().iter().fold(0f32, |m, s| m.max(s.abs()))
        }).fold(0f32, f32::max);
        assert!(peak > 0.2, "decoded tone is audible: {peak}");
        assert_eq!(capture.sent, packets.len() as u64);
        // Nothing new has been captured yet: no packets, no failure.
        assert!(capture.step().0.is_empty());
    }
    #[test]
    fn a_stalled_owner_never_queues_more_than_240_ms_of_audio() {
        use crate::computer::audio::WavSource;
        let mut capture = capture(WavSource::tone(220., 3.).advanced(Duration::from_secs(2)));
        let (packets, _) = capture.step();
        assert_eq!(packets.len(), MAX_PENDING_FRAMES);
        assert!(capture.pcm.len() < FRAME);
    }
    #[test]
    fn permission_prompt_defaults_to_deny_and_discloses_scope() {
        let sharing = Sharing {
            pending: Some(Request {
                screen: Id { face: Face { block: [0, 1, 2], side: 2, width: 1, height: 1 }, dimension: 0 },
                number: 7, viewer: [0; 16], name: "Viewer".into(),
            }),
            ..Default::default()
        };
        assert!(sharing.viewers.is_empty());
        let dialog = sharing.dialog(0).unwrap();
        assert_eq!(dialog.selected, 0);
        assert!(dialog.entries[1].0.contains("all my open screens"));
        assert_eq!(dialog.title, "Allow screen and audio sharing?");
        assert_eq!(dialog.body[1], "Includes app audio, only while looking. No remote input.");
        assert_eq!(dialog.entries[1].0, "Allow all my open screens and their app audio");
        assert_eq!(dialog.entries[1].1, crate::ui::desktop::DesktopAction::ViewPermission { request: 7, allow: true });
        assert_eq!(dialog.entries[0].1, crate::ui::desktop::DesktopAction::ViewPermission { request: 7, allow: false });
    }
    fn screen() -> Id {
        Id { face: Face { block: [0, 1, 2], side: 2, width: 1, height: 1 }, dimension: 0 }
    }
    fn playing_app(name: &str) -> App {
        let options = crate::Options {
            automation: false, bind: "127.0.0.1:0".parse().unwrap(),
            artifacts: format!(".analysis/{name}").into(), headless: Some(true),
            runtime_dir: format!(".analysis/{name}/runtime").into(),
            profile: "fixture".into(), server: None, direct_join: false, username: None,
            width: 1280, height: 720, fullscreen: false,
        };
        let mut app = App::new(options, std::sync::Arc::new(|| {})).unwrap();
        app.session.enter_offline_play_for_test();
        // Settle the sharing scope (connection epoch, dimension) before seeding state.
        app.sync_screen_sharing(true);
        app
    }
    fn answer(allow: bool) -> Vec<u8> {
        let mut packet = screen().packet(5);
        packet.push(u8::from(allow));
        packet
    }
    #[test]
    fn screen_share_answer_runs_the_consent_dialog_path() {
        let mut app = playing_app("screen-share-answer");
        let viewer = [9; 16];
        app.computer.sharing.pending = Some(Request { screen: screen(), number: 7, viewer, name: "Viewer".into() });
        assert_eq!(app.screen_share_command(&json!({"action": "answer"})), Err("allow_required_for_answer".into()));
        assert!(app.computer.sharing.pending.is_some(), "a refused call leaves the dialog open");
        assert_eq!(
            app.screen_share_command(&json!({"action": "answer", "allow": true})),
            Ok(json!({"action": "answer", "request": 7, "viewer": "Viewer", "allow": true}))
        );
        assert!(app.computer.sharing.pending.is_none());
        assert_eq!(app.computer.sharing.viewers.get(&viewer).map(String::as_str), Some("Viewer"));
        assert!(app.computer.message.starts_with("Screens and app audio shared"));
        assert_eq!(
            app.screen_share_command(&json!({"action": "answer", "allow": false})),
            Err("no_pending_screen_request".into())
        );
        assert!(app.screen_share_command(&json!({"action": "watch"})).is_err());
    }
    #[test]
    fn screen_share_request_targets_only_an_active_remote_screen() {
        let mut app = playing_app("screen-share-request");
        assert!(app.screen_share_command(&json!({"action": "request"})).is_err(), "no monitor under the crosshair");
        assert!(app.computer.sharing.watching.is_empty());
        assert!(!app.request_shared_screen(screen().face), "an inactive screen is not requested");
        app.session.screen_relay.active.insert(screen(), Default::default());
        assert!(app.request_shared_screen(screen().face));
        assert!(app.computer.sharing.watching.contains(&screen()));
        assert!(app.computer.message.contains("F8 cancels/leaves"));
    }
    #[test]
    fn screen_share_leave_is_f8_and_a_later_allow_is_counted_and_left() {
        let mut app = playing_app("screen-share-leave");
        app.session.screen_relay.active.insert(screen(), Default::default());
        assert!(app.request_shared_screen(screen().face));
        assert_eq!(
            app.screen_share_command(&json!({"action": "leave"})),
            Ok(json!({"action": "leave", "left_screens": 1, "revoked_viewers": 0}))
        );
        assert!(app.computer.sharing.watching.is_empty(), "F8 cancels the unanswered request");
        assert_eq!(app.computer.message, "Screen sharing revoked; viewing ended. Local capture continues");
        // A server that still grants the cancelled request delivers the owner's Allow.
        app.session.screen_relay.receive(&answer(true)).unwrap();
        app.sync_screen_sharing(true);
        assert_eq!(app.session.screen_relay.cancelled_grants_left, 1);
        assert_eq!(app.session.screen_relay.permission_answers, [1, 0]);
        assert_eq!(app.computer.message, "Screen sharing revoked; viewing ended. Local capture continues",
            "a cancelled request never announces viewing");
        let sharing = &app.session.minecraft_observation()["screen_sharing"];
        assert_eq!(sharing["cancelled_grants_left"], 1);
        assert_eq!(sharing["permission_answers"], json!({"allowed": 1, "denied": 0}));
        assert!(sharing["received_frames"].is_u64() && sharing["decoded_frames"].is_u64());
        // An answer to a request still wanted is announced, not left.
        assert!(app.request_shared_screen(screen().face));
        app.session.screen_relay.receive(&answer(true)).unwrap();
        app.sync_screen_sharing(true);
        assert_eq!(app.session.screen_relay.cancelled_grants_left, 1);
        assert_eq!(app.computer.message, "Viewing allowed — look at the screen; F8 leaves");
    }
}
