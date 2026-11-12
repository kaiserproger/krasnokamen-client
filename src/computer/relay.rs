//! Optional, permission-gated desktop pictures and app audio. This protocol never carries input.
use super::{Face, Frame};
use crate::protocol::Reader;
use openh264::{
    OpenH264API,
    decoder::Decoder,
    encoder::{BitRate, Encoder as H264, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType},
    formats::{RgbSliceU8, YUVBuffer, YUVSource},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) const CHANNEL: &str = "krasnokamen:screen";
const MAX_FRAME: usize = 131_072;
const PART: usize = 30_000;
/// Target bitrate of the H.264 stream (OpenH264 Quality mode treats it as a ceiling).
const BITRATE: u32 = 4_000_000;
/// `KRASNOKAMEN_SCREEN_BITRATE` (bit/s) overrides the cap for measurements.
fn bitrate() -> u32 {
    std::env::var("KRASNOKAMEN_SCREEN_BITRATE").ok().and_then(|v| v.parse().ok()).unwrap_or(BITRATE)
}
/// `FRAME` codec byte: H.264 Annex B (Constrained Baseline, OpenH264), one picture per frame.
const H264_CODEC: u8 = 1;
/// Server -> owner `KEYFRAME_REQUEST` and viewer -> server `KEY_REQUEST` share this kind.
const KEY: u8 = 8;

/// Payload bytes of video this client sent (owner) and received (viewer); printed by
/// `KRASNOKAMEN_SCREEN_STATS=1` once a second so sharing traffic can be measured.
pub(crate) mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static TX_BYTES: AtomicU64 = AtomicU64::new(0);
    pub static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
    pub static RX_BYTES: AtomicU64 = AtomicU64::new(0);
    pub static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
    pub static TX_KEYS: AtomicU64 = AtomicU64::new(0);
    pub static RX_KEYS: AtomicU64 = AtomicU64::new(0);
    pub static RX_DECODED: AtomicU64 = AtomicU64::new(0);
    pub fn line() -> String {
        format!("screen_stats unix={} tx_bytes={} tx_frames={} tx_keys={} rx_bytes={} rx_frames={} rx_keys={} rx_decoded={}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            TX_BYTES.load(Relaxed), TX_FRAMES.load(Relaxed), TX_KEYS.load(Relaxed),
            RX_BYTES.load(Relaxed), RX_FRAMES.load(Relaxed), RX_KEYS.load(Relaxed), RX_DECODED.load(Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Id {
    pub face: Face,
    pub dimension: u8,
}
impl Id {
    fn read(r: &mut Reader<'_>) -> Result<Self, String> {
        let block = [r.i32()?, r.i32()?, r.i32()?];
        let side = i32::from(r.u8()?);
        let width = r.u8()?;
        let height = r.u8()?;
        let dimension = r.u8()?;
        if !(2..=5).contains(&side) || !(1..=16).contains(&width) || !(1..=16).contains(&height) || dimension > 2 {
            return Err("invalid shared screen identity".into());
        }
        Ok(Self { face: Face { block, side, width, height }, dimension })
    }
    /// The 16 identity bytes on the wire: the key of this screen's audio speaker.
    pub fn speaker(self) -> [u8; 16] {
        self.packet(0)[2..18].try_into().expect("screen id is 16 bytes")
    }
    pub fn packet(self, kind: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.extend([1, kind]);
        for coordinate in self.face.block { out.extend(coordinate.to_be_bytes()); }
        out.extend([self.face.side as u8, self.face.width, self.face.height, self.dimension]);
        out
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub screen: Id,
    pub number: u32,
    pub viewer: [u8; 16],
    pub name: String,
}
impl Request {
    pub fn answer(&self, allow: bool) -> Vec<u8> {
        let mut out = vec![1, 4];
        out.extend(self.number.to_be_bytes());
        out.push(u8::from(allow));
        out
    }
}
#[derive(Default)]
pub(crate) struct Remote {
    pub frame: Option<Frame>,
    pub dirty: bool,
    sequence: Option<u32>,
    decoder: Option<Decoder>,
    /// A key frame has been decoded since the last (re)start of this picture stream.
    synced: bool,
    /// A `KEY_REQUEST` is outstanding; cleared by the next decoded key frame.
    key_asked: bool,
}
impl Remote {
    fn reset(&mut self) {
        self.frame = None;
        self.dirty = true;
        self.decoder = None;
        self.synced = false;
        self.key_asked = false;
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Demand {
    pub width: u16,
    pub fps: u8,
}
/// One relayed Opus packet of a screen's audio, waiting for the session mixer.
pub(crate) struct AudioIn {
    pub screen: Id,
    pub seq: u32,
    pub opus: Vec<u8>,
}
/// At most this many undelivered packets (1.3 s); the oldest are dropped.
const MAX_AUDIO_QUEUE: usize = 64;
pub(crate) const MAX_OPUS: usize = 1000;
/// Monitor audio is silent beyond this many blocks (the server's looking range).
pub(crate) const AUDIO_RANGE: f32 = 32.;
#[derive(Default)]
pub(crate) struct Relay {
    pub audio: VecDeque<AudioIn>,
    pub registered: bool,
    pub active: BTreeMap<Id, Remote>,
    pub demand: BTreeMap<Id, Demand>,
    pub requests: VecDeque<Request>,
    /// Owner answers to this viewer's requests; the app knows which are still wanted.
    pub answers: VecDeque<(Id, bool)>,
    /// Owner answers the server delivered to this viewer: [allowed, denied].
    pub permission_answers: [u64; 2],
    /// Allows for requests this viewer had already cancelled (F8), which it then left.
    pub cancelled_grants_left: u64,
    pub notice: Option<String>,
    /// Screens the server asked this owner to start a key frame for (consumed by the encoder).
    pub key_wanted: BTreeSet<Id>,
    /// Packets the viewer side needs sent (`KEY_REQUEST`); drained by the session after `receive`.
    pub outbox: Vec<Vec<u8>>,
}
impl Relay {
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_FRAME + 32 { return Err("shared screen payload bound".into()); }
        let mut r = Reader::new(bytes);
        if r.u8()? != 1 { return Err("unsupported shared screen version".into()); }
        let kind = r.u8()?;
        let id = Id::read(&mut r)?;
        match kind {
            0 => {
                let on = r.bool()?;
                let width = r.i16()? as u16;
                let fps = r.u8()?;
                r.done()?;
                if width == 0 || width > 640 || fps == 0 || fps > 5 {
                    return Err("invalid shared screen demand".into());
                }
                if on {
                    if self.demand.len() >= 4 && !self.demand.contains_key(&id) { return Err("screen demand bound".into()); }
                    self.demand.insert(id, Demand { width, fps });
                    // A new demand starts a fresh picture stream: the first frame must be a key frame.
                    self.key_wanted.insert(id);
                } else {
                    self.demand.remove(&id);
                    self.key_wanted.remove(&id);
                }
            }
            1 => {
                let sequence = r.i32()? as u32;
                let width = r.i16()? as u16;
                let height = r.i16()? as u16;
                if r.u8()? != H264_CODEC { return Err("unsupported screen image codec".into()); }
                let key = match r.u8()? { 0 => false, 1 => true, _ => return Err("invalid screen key flag".into()) };
                let data = r.take(r.bytes.len() - r.offset)?;
                let remote = self.active.get_mut(&id).ok_or("frame for inactive screen")?;
                if remote.sequence.is_some_and(|old| sequence.wrapping_sub(old) > i32::MAX as u32 || sequence == old) {
                    return Ok(());
                }
                stats::RX_BYTES.fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                stats::RX_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if key { stats::RX_KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                remote.sequence = Some(sequence);
                if !key && !remote.synced {
                    // The server withholds deltas until a key frame; a stray one is simply not decodable.
                    if !std::mem::replace(&mut remote.key_asked, true) { self.outbox.push(id.packet(KEY)); }
                    return Ok(());
                }
                match decode(&mut remote.decoder, data, width, height) {
                    Ok(picture) => {
                        if let Some(picture) = picture {
                            if key { remote.synced = true; remote.key_asked = false; }
                            stats::RX_DECODED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            remote.frame = Some(picture);
                            remote.dirty = true;
                        }
                    }
                    Err(error) => {
                        remote.decoder = None;
                        remote.synced = false;
                        if !std::mem::replace(&mut remote.key_asked, true) { self.outbox.push(id.packet(KEY)); }
                        return Err(error);
                    }
                }
            }
            2 => {
                let reason = r.u8()?;
                r.done()?;
                if reason > 3 { return Err("invalid screen stop reason".into()); }
                self.audio.retain(|packet| packet.screen != id);
                if let Some(remote) = self.active.get_mut(&id) { remote.reset(); }
                if reason != 0 { self.notice = Some("Screen viewing permission ended".into()); }
            }
            3 => {
                let reason = r.u8()?;
                r.done()?;
                let reason = match reason {
                    0 => "not allowed", 1 => "monitor rectangle changed", 2 => "out of reach",
                    3 => "screen limit", 4 => "another player's screen", 5 => "invalid picture",
                    6 => "screen closed", 7 => "request rate limited",
                    _ => return Err("invalid screen refusal".into()),
                };
                self.demand.remove(&id);
                self.key_wanted.remove(&id);
                self.notice = Some(format!("Screen sharing: {reason}"));
            }
            4 => {
                let number = r.i32()? as u32;
                let viewer = r.take(16)?.try_into().map_err(|_| "invalid viewer UUID")?;
                let length = usize::from(r.u8()?);
                let name = std::str::from_utf8(r.take(length)?).map_err(|_| "invalid viewer name")?;
                if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
                    return Err("invalid viewer name".into());
                }
                r.done()?;
                if self.requests.len() >= 32 { return Err("screen permission request bound".into()); }
                self.requests.push_back(Request { screen: id, number, viewer, name: name.into() });
            }
            5 => {
                let allow = r.bool()?;
                r.done()?;
                if self.answers.len() >= 32 {
                    return Err("screen permission answer bound".into());
                }
                self.answers.push_back((id, allow));
                self.permission_answers[usize::from(!allow)] += 1;
            }
            6 => {
                let on = r.bool()?;
                r.done()?;
                if on {
                    if self.active.len() >= 128 && !self.active.contains_key(&id) { return Err("active screen bound".into()); }
                    self.active.entry(id).or_insert_with(|| Remote { dirty: true, ..Default::default() });
                } else {
                    self.active.remove(&id);
                    self.demand.remove(&id);
                    self.key_wanted.remove(&id);
                    self.audio.retain(|packet| packet.screen != id);
                }
            }
            7 => {
                let seq = r.i32()? as u32;
                let opus = r.take(r.bytes.len() - r.offset)?;
                if opus.is_empty() || opus.len() > MAX_OPUS { return Err("invalid screen audio packet".into()); }
                if self.active.contains_key(&id) {
                    if self.audio.len() >= MAX_AUDIO_QUEUE { self.audio.pop_front(); }
                    self.audio.push_back(AudioIn { screen: id, seq, opus: opus.to_vec() });
                }
            }
            KEY => {
                r.done()?;
                // Only an owner with a live demand has a picture stream to restart.
                if self.demand.contains_key(&id) { self.key_wanted.insert(id); }
            }
            _ => return Err("unknown screen relay message".into()),
        }
        Ok(())
    }
}

fn decode(decoder: &mut Option<Decoder>, data: &[u8], width: u16, height: u16) -> Result<Option<Frame>, String> {
    if data.is_empty() || data.len() > MAX_FRAME || width == 0 || height == 0 || width > 640 || height > 640 {
        return Err("shared image size bound".into());
    }
    if decoder.is_none() { *decoder = Some(Decoder::new().map_err(|e| e.to_string())?); }
    let Some(yuv) = decoder.as_mut().expect("created above").decode(data).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let (w, h) = yuv.dimensions();
    if (w, h) != (usize::from(width), usize::from(height)) {
        return Err("shared image dimensions disagree with header".into());
    }
    let mut rgba = vec![0; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    Ok(Some(Frame { width: u32::from(width), height: u32::from(height), rgba }))
}

#[derive(Default)]
pub(crate) struct Encoder {
    h264: Option<H264>,
    /// The H.264 encoder only accepts `force_intra_frame` after its first picture.
    started: bool,
    rgb: Vec<u8>,
    previous: Vec<u8>,
    previous_size: (u32, u32),
    sequence: u32,
    key_wanted: bool,
    /// Largest side after an oversize picture forced a smaller one; 0 = whatever DEMAND allows.
    limit: u32,
}
impl Encoder {
    /// The next picture is a self-contained key frame (new viewer, resync, new DEMAND).
    pub fn want_key(&mut self) { self.key_wanted = true; }

    /// Only invoked on an explicit server DEMAND and at most five times a second. Returns no
    /// packets when the picture is identical to the last one sent (and no key frame is owed).
    pub fn encode(&mut self, id: Id, frame: &Frame, maximum: u16) -> Result<Vec<Vec<u8>>, String> {
        self.picture(id, Some(frame), maximum)
    }
    /// Re-sends the last picture as a key frame if one is owed: a static window delivers no new
    /// captures, yet a viewer who just joined still needs a picture to start from.
    pub fn repeat_key(&mut self, id: Id, maximum: u16) -> Result<Vec<Vec<u8>>, String> {
        self.picture(id, None, maximum)
    }
    fn picture(&mut self, id: Id, frame: Option<&Frame>, maximum: u16) -> Result<Vec<Vec<u8>>, String> {
        if let Some(frame) = frame
            && (frame.width == 0 || frame.height == 0 || frame.width > 8192 || frame.height > 8192
                || frame.rgba.len() != frame.width as usize * frame.height as usize * 4) {
            return Err("invalid source frame".into());
        }
        if frame.is_none() && (!self.key_wanted || self.previous.is_empty()) { return Ok(Vec::new()); }
        let mut maximum = u32::from(maximum.clamp(2, 640));
        if self.limit != 0 { maximum = maximum.min(self.limit); }
        let (width, height, data, key) = loop {
            let (width, height) = if let Some(frame) = frame {
                let largest = frame.width.max(frame.height);
                let even = |v: u32| (v & !1).max(2);
                let width = even(frame.width.min(frame.width.saturating_mul(maximum) / largest));
                let height = even(frame.height.min(frame.height.saturating_mul(maximum) / largest));
                self.scale(frame, width, height);
                (width, height)
            } else {
                self.rgb.clone_from(&self.previous);
                self.previous_size
            };
            if !self.key_wanted && self.previous_size == (width, height) && self.previous == self.rgb {
                return Ok(Vec::new());
            }
            let Some((data, key)) = self.compress(width, height)? else { return Ok(Vec::new()); };
            if data.len() <= MAX_FRAME { break (width, height, data, key); }
            // The encoder already took this picture as its reference: restart the chain, smaller.
            maximum = width.max(height) * 3 / 4;
            if maximum < 16 { return Err("picture does not fit the frame bound".into()); }
            self.limit = maximum;
            self.key_wanted = true;
            if frame.is_none() {
                // Nothing to rescale from; the next captured picture restarts the stream smaller.
                self.previous.clear();
                return Ok(Vec::new());
            }
        };
        if key { self.key_wanted = false; }
        std::mem::swap(&mut self.previous, &mut self.rgb);
        self.previous_size = (width, height);
        self.sequence = self.sequence.wrapping_add(1);
        let count = data.len().div_ceil(PART);
        let mut packets = Vec::with_capacity(count);
        for (index, part) in data.chunks(PART).enumerate() {
            let mut packet = id.packet(2);
            packet.extend(self.sequence.to_be_bytes());
            packet.extend((width as u16).to_be_bytes());
            packet.extend((height as u16).to_be_bytes());
            packet.extend([H264_CODEC, u8::from(key), index as u8, count as u8]);
            packet.extend(part);
            packets.push(packet);
        }
        Ok(packets)
    }
    fn scale(&mut self, frame: &Frame, width: u32, height: u32) {
        self.rgb.resize(width as usize * height as usize * 3, 0);
        for y in 0..height {
            for x in 0..width {
                let source = ((y * frame.height / height) as usize * frame.width as usize
                    + (x * frame.width / width) as usize) * 4;
                let target = (y as usize * width as usize + x as usize) * 3;
                self.rgb[target..target + 3].copy_from_slice(&frame.rgba[source..source + 3]);
            }
        }
    }
    /// One H.264 picture of `self.rgb`; `None` when the encoder skipped it.
    fn compress(&mut self, width: u32, height: u32) -> Result<Option<(Vec<u8>, bool)>, String> {
        if self.h264.is_none() {
            let config = EncoderConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .rate_control_mode(RateControlMode::Quality)
                .bitrate(BitRate::from_bps(bitrate()))
                .max_frame_rate(FrameRate::from_hz(5.0))
                // Without frame skipping OpenH264 ignores the bitrate in Quality mode.
                .skip_frames(true)
                .num_threads(1);
            self.h264 = Some(H264::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?);
        }
        let h264 = self.h264.as_mut().expect("created above");
        if self.key_wanted && self.started { h264.force_intra_frame(); }
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(&self.rgb, (width as usize, height as usize)));
        let stream = h264.encode(&yuv).map_err(|e| e.to_string())?;
        self.started = true;
        match stream.frame_type() {
            FrameType::IDR | FrameType::I => Ok(Some((stream.to_vec(), true))),
            FrameType::P => Ok(Some((stream.to_vec(), false))),
            FrameType::Skip => Ok(None),
            other => Err(format!("H.264 encoder produced {other:?}")),
        }
    }
}

pub(crate) fn on_frame() -> Frame {
    // No desktop content: a plain illuminated panel and central green power mark.
    let mut rgba = vec![0; 32 * 16 * 4];
    for y in 0..16 { for x in 0..32 {
        let i = (y * 32 + x) * 4;
        let lit = (14..18).contains(&x) && (5..11).contains(&y);
        rgba[i..i + 4].copy_from_slice(if lit { &[30, 230, 90, 255] } else { &[12, 22, 28, 255] });
    }}
    Frame { width: 32, height: 16, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> Id { Id { face: Face { block: [-2, 4, 9], side: 2, width: 3, height: 2 }, dimension: 0 } }
    #[test]
    fn wire_consent_and_active_do_not_create_picture_data() {
        let mut relay = Relay::default();
        let mut active = id().packet(6); active.push(1);
        relay.receive(&active).unwrap();
        assert!(relay.active[&id()].frame.is_none());
        let mut request = id().packet(4);
        request.extend(17u32.to_be_bytes()); request.extend([9; 16]); request.push(3); request.extend(b"Bob");
        relay.receive(&request).unwrap();
        let request = relay.requests.pop_front().unwrap();
        assert_eq!(request.answer(false), [1, 4, 0, 0, 0, 17, 0]);
        assert_eq!(request.name, "Bob");
        assert!(relay.demand.is_empty());
        active.push(0);
        assert!(relay.receive(&active).is_err());
    }
    #[test]
    fn owner_answers_are_queued_with_their_screen_and_counted() {
        let mut relay = Relay::default();
        let answer = |allow: u8| { let mut packet = id().packet(5); packet.push(allow); packet };
        relay.receive(&answer(1)).unwrap();
        relay.receive(&answer(0)).unwrap();
        relay.receive(&answer(1)).unwrap();
        assert_eq!(relay.answers.pop_front(), Some((id(), true)));
        assert_eq!(relay.answers.pop_front(), Some((id(), false)));
        assert_eq!(relay.permission_answers, [2, 1], "[allowed, denied] delivered by the server");
        assert!(relay.notice.is_none(), "the app decides whether an answer is still wanted");
        assert!(relay.receive(&id().packet(5)).is_err(), "an answer carries its allow byte");
        assert_eq!(relay.permission_answers, [2, 1]);
    }
    /// The server forwards owner parts as one FRAME: id, seq/size/codec/key header, joined data.
    fn relayed(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut frame = id().packet(1);
        frame.extend_from_slice(&packets[0][18..28]);
        for part in packets { frame.extend_from_slice(&part[30..]); }
        frame
    }
    fn page(offset: usize) -> Frame {
        let (width, height) = (640usize, 360usize);
        let mut rgba = vec![255; width * height * 4];
        for y in 0..height { for x in 0..width {
            let row = y + offset;
            if row % 12 < 8 && (x / 7 + row / 12 * 5) % 4 < 2 {
                rgba[(y * width + x) * 4..][..3].fill(25);
            }
        }}
        Frame { width: width as u32, height: height as u32, rgba }
    }
    fn active() -> Relay {
        let mut relay = Relay::default();
        let mut active = id().packet(6); active.push(1); relay.receive(&active).unwrap();
        relay
    }
    #[test]
    fn pictures_are_bounded_decoded_and_removed_on_stop() {
        let frame = Frame { width: 900, height: 700, rgba: vec![180; 900 * 700 * 4] };
        let packets = Encoder::default().encode(id(), &frame, 640).unwrap();
        assert!(!packets.is_empty() && packets.len() <= 5);
        assert!(packets.iter().all(|p| p.len() <= PART + 30));
        assert_eq!(packets[0][27], 1, "first picture is a key frame");
        let received = relayed(&packets);
        let mut relay = active();
        relay.receive(&received).unwrap();
        let decoded = relay.active[&id()].frame.as_ref().unwrap();
        assert_eq!((decoded.width, decoded.height), (640, 496));
        let mut stop = id().packet(2); stop.push(2); relay.receive(&stop).unwrap();
        assert!(relay.active[&id()].frame.is_none());
        let mut corrupt = received;
        corrupt[22..24].copy_from_slice(&641u16.to_be_bytes());
        // New sequence makes the dimension error observable rather than a duplicate.
        corrupt[18..22].copy_from_slice(&2u32.to_be_bytes());
        assert!(relay.receive(&corrupt).is_err());
        assert!(relay.active[&id()].frame.is_none());
        assert_eq!(relay.outbox, [id().packet(KEY)], "a failed decode asks for a key frame, once");
    }
    #[test]
    fn scrolling_text_decodes_close_to_the_source_and_deltas_are_small() {
        let mut encoder = Encoder::default();
        let mut relay = active();
        let mut sizes = Vec::new();
        for step in 0..6 {
            let packets = encoder.encode(id(), &page(step * 6), 640).unwrap();
            assert_eq!(packets[0][26], H264_CODEC);
            sizes.push(packets.iter().map(|p| p.len() - 30).sum::<usize>());
            relay.receive(&relayed(&packets)).unwrap();
            let decoded = relay.active[&id()].frame.as_ref().unwrap();
            let error: u64 = decoded.rgba.chunks_exact(4).zip(encoder.previous.chunks_exact(3))
                .map(|(a, b)| a[..3].iter().zip(b).map(|(x, y)| u64::from(x.abs_diff(*y))).sum::<u64>()).sum();
            assert!(error / (640 * 360 * 3) < 12, "decoded picture differs from the encoded one: {error}");
        }
        assert!(relay.outbox.is_empty());
        assert!(sizes[1..].iter().all(|size| *size < sizes[0]), "deltas must be smaller than the key frame: {sizes:?}");
    }
    #[test]
    fn an_unchanged_picture_sends_nothing_until_a_key_frame_is_wanted() {
        let mut encoder = Encoder::default();
        assert!(!encoder.encode(id(), &page(0), 640).unwrap().is_empty());
        assert!(encoder.encode(id(), &page(0), 640).unwrap().is_empty());
        assert!(!encoder.encode(id(), &page(6), 640).unwrap().is_empty());
        encoder.want_key();
        let key = encoder.encode(id(), &page(6), 640).unwrap();
        assert_eq!(key[0][26], H264_CODEC);
        assert_eq!(key[0][27], 1, "a requested key frame is sent even for an unchanged picture");
        // A viewer that joined mid-stream decodes it without any earlier frame.
        let mut late = active();
        late.receive(&relayed(&key)).unwrap();
        assert!(late.active[&id()].frame.is_some());
        assert!(encoder.encode(id(), &page(6), 640).unwrap().is_empty());
    }
    #[test]
    fn a_delta_before_any_key_frame_is_not_decoded_and_requests_one_key() {
        let mut encoder = Encoder::default();
        encoder.encode(id(), &page(0), 640).unwrap();
        let delta = encoder.encode(id(), &page(6), 640).unwrap();
        assert_eq!(delta[0][27], 0);
        let mut late = active();
        late.receive(&relayed(&delta)).unwrap();
        assert!(late.active[&id()].frame.is_none());
        assert_eq!(late.outbox, [id().packet(KEY)]);
        let again = encoder.encode(id(), &page(12), 640).unwrap();
        late.receive(&relayed(&again)).unwrap();
        assert_eq!(late.outbox.len(), 1, "one outstanding request is enough");
        encoder.want_key();
        late.receive(&relayed(&encoder.encode(id(), &page(18), 640).unwrap())).unwrap();
        assert!(late.active[&id()].frame.is_some());
        assert!(late.active[&id()].synced && !late.active[&id()].key_asked);
    }
    #[test]
    fn demand_and_key_requests_arm_the_owner_encoder() {
        let mut relay = Relay::default();
        let mut request = id().packet(KEY);
        relay.receive(&request).unwrap();
        assert!(relay.key_wanted.is_empty(), "no demand, no picture stream to restart");
        let mut demand = id().packet(0); demand.extend([1, 2, 128, 5]);
        relay.receive(&demand).unwrap();
        assert!(relay.key_wanted.remove(&id()));
        relay.receive(&request).unwrap();
        assert!(relay.key_wanted.contains(&id()));
        request.push(0);
        assert!(relay.receive(&request).is_err());
    }
    #[test]
    fn relayed_audio_is_queued_only_for_active_screens_bounded_and_dropped_on_stop() {
        let mut relay = Relay::default();
        let audio = |seq: u32, opus: &[u8]| {
            let mut packet = id().packet(7);
            packet.extend(seq.to_be_bytes());
            packet.extend(opus);
            packet
        };
        relay.receive(&audio(1, b"early")).unwrap();
        assert!(relay.audio.is_empty(), "no ACTIVE screen: ignored, not an error");
        let mut active = id().packet(6); active.push(1); relay.receive(&active).unwrap();
        for seq in 0..70 { relay.receive(&audio(seq, &[seq as u8; 20])).unwrap(); }
        assert_eq!(relay.audio.len(), MAX_AUDIO_QUEUE);
        assert_eq!(relay.audio.front().unwrap().seq, 6, "oldest dropped");
        assert_eq!(relay.audio.back().unwrap().opus, [69; 20]);
        assert_eq!(relay.audio[0].screen.speaker(), id().packet(0)[2..18]);
        assert!(relay.receive(&audio(1, b"")).is_err());
        assert!(relay.receive(&audio(1, &[0; MAX_OPUS + 1])).is_err());
        let mut stop = id().packet(2); stop.push(0); relay.receive(&stop).unwrap();
        assert!(relay.audio.is_empty(), "STOP discards queued audio");
    }
    #[test]
    fn screen_identity_places_height_before_dimension_without_legacy_fallback() {
        let mut wire = id().packet(0);
        assert_eq!(wire.len(), 18);
        assert_eq!(&wire[14..18], &[2, 3, 2, 0]);
        let mut reader = Reader::new(&wire[2..]);
        assert_eq!(Id::read(&mut reader).unwrap(), id());
        reader.done().unwrap();
        wire[16] = 0;
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
        wire[16] = 17;
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
        wire.remove(16);
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
    }
}
