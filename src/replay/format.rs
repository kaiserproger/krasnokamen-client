//! PLAN.md .solrec: one JSON line, then BE u32 milliseconds, BE u32 length,
//! decompressed packet bytes (original VarInt ID + body). No transport framing.
use crate::protocol::{MAX_FRAME, Reader, network::Phase, packet_ids::{login, configuration, play}};
use serde::{Deserialize, Serialize};
use std::{fs::{File, OpenOptions}, io::{BufRead, BufReader, BufWriter, Read, Write, Seek, SeekFrom}, path::{Path, PathBuf}, sync::mpsc, thread, time::Instant};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

type Result<T, E = String> = std::result::Result<T, E>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub protocol_version: u32,
    pub server_build: String,
    pub seed: u64,
    pub start_tick: u64,
    /// UNIX epoch milliseconds at recorder start, not a replay time source.
    pub wall_time: u64,
}
impl Header {
    fn validate(&self) -> Result<()> {
        if self.protocol_version != 777 || self.server_build.is_empty() || self.server_build.len() > 1024 {
            return Err("solrec header: expected protocol 777 and server_build of 1..1024 bytes".into());
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet { pub ms: u32, pub bytes: Vec<u8> }
impl Packet {
    pub(crate) fn split(&self) -> Result<(i32, &[u8])> {
        let mut reader = Reader::new(&self.bytes);
        let id = reader.var()?;
        if id < 0 { return Err("solrec: negative packet id".into()); }
        Ok((id, &self.bytes[reader.offset..]))
    }
}

pub(crate) fn phase_after(phase: Phase, id: i32) -> Phase {
    match (phase, id) {
        (Phase::Login, login::clientbound::LOGIN_FINISHED) => Phase::Configuration,
        (Phase::Configuration, configuration::clientbound::FINISH_CONFIGURATION) => Phase::Play,
        (Phase::Play, play::clientbound::START_CONFIGURATION) => Phase::Configuration,
        _ => phase,
    }
}
pub(crate) fn tick_marker(phase: Phase, id: i32, body: &[u8]) -> Result<Option<(u64,u64)>> {
    if phase != Phase::Play || id != play::clientbound::CUSTOM_PAYLOAD { return Ok(None); }
    let mut reader = Reader::new(body);
    if reader.string(32767)? != "krasnokamen:tick" { return Ok(None); }
    let tick = reader.i64()? as u64;
    let wall = reader.i64()? as u64;
    reader.done()?;
    Ok(Some((tick,wall)))
}

pub struct RecordingReader<R> { input: R, pub header: Header, previous_ms: u32 }
impl<R: BufRead> RecordingReader<R> {
    pub fn new(mut input: R) -> Result<Self> {
        let mut line = Vec::new();
        // A malformed file cannot force an unbounded allocation before its newline.
        input.by_ref().take(16 * 1024).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
        if line.last() != Some(&b'\n') { return Err("solrec: header exceeds 16 KiB or has no newline".into()); }
        let header: Header = serde_json::from_slice(&line).map_err(|e| format!("solrec header: {e}"))?;
        header.validate()?;
        Ok(Self { input, header, previous_ms: 0 })
    }
    pub fn next_packet(&mut self) -> Result<Option<Packet>> {
        let mut prefix = [0; 8];
        // EOF is legal only between records, never halfway through one.
        if self.input.read(&mut prefix[..1]).map_err(|e| e.to_string())? == 0 { return Ok(None); }
        self.input.read_exact(&mut prefix[1..]).map_err(|e| format!("solrec truncated frame header: {e}"))?;
        let ms = u32::from_be_bytes(prefix[..4].try_into().unwrap());
        let length = u32::from_be_bytes(prefix[4..].try_into().unwrap()) as usize;
        if ms < self.previous_ms || length == 0 || length > MAX_FRAME {
            return Err(format!("solrec invalid record: ms={ms}, previous={}, length={length}", self.previous_ms));
        }
        let mut bytes = vec![0; length];
        self.input.read_exact(&mut bytes).map_err(|e| format!("solrec truncated packet: {e}"))?;
        let packet = Packet { ms, bytes };
        packet.split()?;
        self.previous_ms = ms;
        Ok(Some(packet))
    }
}
impl RecordingReader<BufReader<File>> {
    pub fn open(path: &Path) -> Result<Self> {
        Self::new(BufReader::new(File::open(path).map_err(|e| format!("open recording {}: {e}", path.display()))?))
    }
}

pub struct RecordingWriter<W: Write> { output: W, previous_ms: u32, header_line_bytes: usize }
impl<W: Write> RecordingWriter<W> {
    pub fn new(mut output: W, header: &Header) -> Result<Self> {
        header.validate()?;
        let bytes = serde_json::to_vec(header).map_err(|e| e.to_string())?;
        output.write_all(&bytes).and_then(|_| output.write_all(b"                    \n")).map_err(|e| e.to_string())?;
        Ok(Self { output, previous_ms: 0, header_line_bytes: bytes.len()+21 })
    }
    pub fn packet(&mut self, packet: &Packet) -> Result<()> {
        if packet.ms < self.previous_ms || packet.bytes.is_empty() || packet.bytes.len() > MAX_FRAME {
            return Err("solrec writer: invalid timestamp or packet length".into());
        }
        self.output.write_all(&packet.ms.to_be_bytes()).and_then(|_| self.output.write_all(&(packet.bytes.len() as u32).to_be_bytes()))
            .and_then(|_| self.output.write_all(&packet.bytes)).map_err(|e| format!("write solrec frame: {e}"))?;
        self.previous_ms = packet.ms;
        Ok(())
    }
    pub fn finish(mut self) -> Result<W> {
        self.output.flush().map_err(|e| format!("flush recording: {e}"))?;
        Ok(self.output)
    }
}
impl<W: Write + Seek> RecordingWriter<W> {
    fn rewrite_header(&mut self, header: &Header) -> Result<()> {
        let mut bytes = serde_json::to_vec(header).map_err(|e| e.to_string())?;
        if bytes.len() >= self.header_line_bytes { return Err("solrec header rewrite exceeded reserved line".into()); }
        bytes.resize(self.header_line_bytes-1, b' ');
        bytes.push(b'\n');
        self.output.flush().map_err(|e| e.to_string())?;
        let position = self.output.stream_position().map_err(|e| e.to_string())?;
        self.output.seek(SeekFrom::Start(0)).and_then(|_| self.output.write_all(&bytes))
            .and_then(|_| self.output.seek(SeekFrom::Start(position))).map_err(|e| format!("rewrite solrec start tick: {e}"))?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct RecordingOptions { pub path: PathBuf, pub header: Header }

#[derive(Default)]
struct Summary {
    first_tick: Option<u64>,
    last_tick: Option<u64>,
    markers: u64,
    marker_gaps: u64,
    packets: u64,
    packet_bytes: u64,
    last_packet_ms: u32,
}
pub(super) fn receipt_path(recording:&Path) -> PathBuf {
    let mut path=recording.as_os_str().to_os_string();
    path.push(".result.json");
    PathBuf::from(path)
}
/// Publish complete ready/result JSON, never a partially written signal.
pub(crate) fn publish_json(path:&Path,value:&impl Serialize) -> Result<()> {
    let mut temporary=path.as_os_str().to_os_string();
    temporary.push(format!(".tmp-{}",std::process::id()));
    let temporary=PathBuf::from(temporary);
    let mut file=OpenOptions::new().create_new(true).write(true).open(&temporary)
        .map_err(|e|format!("create JSON signal {}: {e}",temporary.display()))?;
    serde_json::to_writer(&mut file,value).map_err(|e|format!("write JSON signal {}: {e}",temporary.display()))?;
    file.write_all(b"\n").and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;
    std::fs::rename(&temporary,path).map_err(|e|format!("publish JSON signal {}: {e}",path.display()))?;
    Ok(())
}
/// Disk IO belongs to this bounded worker, never to the socket thread. Saturation
/// fails the take explicitly; recording never drops packets or stalls the network.
const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const PACKET_OVERHEAD: usize = std::mem::size_of::<Packet>() + 64;
pub(crate) struct Recorder {
    sender: mpsc::Sender<Packet>,
    pending_bytes: Arc<AtomicUsize>,
    worker: thread::JoinHandle<Result<Summary>>,
    receipt: PathBuf,
    start: Instant,
}
impl Recorder {
    pub(crate) fn start(mut options: RecordingOptions) -> Result<Self> {
        options.header.wall_time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?.as_millis().try_into().map_err(|_| "recording wall time overflow")?;
        options.header.start_tick = 0; // Replaced by the first authoritative marker.
        let receipt=receipt_path(&options.path);
        if receipt.exists() {return Err(format!("recording receipt {} already exists",receipt.display()));}
        let file = OpenOptions::new().write(true).create_new(true).open(&options.path)
            .map_err(|e| format!("create recording {}: {e}", options.path.display()))?;
        let writer = RecordingWriter::new(BufWriter::new(file), &options.header)?;
        // Bound bytes (including channel overhead), not packet count: a battle
        // can deliver tens of thousands of tiny movement packets in one burst.
        let (sender, receiver) = mpsc::channel::<Packet>();
        let pending_bytes = Arc::new(AtomicUsize::new(0));
        let pending = pending_bytes.clone();
        let worker = thread::Builder::new().name("solrec-writer".into()).spawn(move || {
            let mut writer = writer;
            let mut phase = Phase::Login;
            let mut summary=Summary::default();
            for packet in receiver {
                pending.fetch_sub(packet.bytes.len()+PACKET_OVERHEAD, Ordering::AcqRel);
                let (id, body) = packet.split()?;
                if let Some((tick,_))=tick_marker(phase,id,body)? {
                    if let Some(previous)=summary.last_tick {
                        if tick<=previous {return Err("recorded tick markers are not strictly increasing".into());}
                        summary.marker_gaps+=tick-previous-1;
                    } else {
                        options.header.start_tick=tick;
                        writer.rewrite_header(&options.header)?;
                        summary.first_tick=Some(tick);
                    }
                    summary.last_tick=Some(tick);
                    summary.markers+=1;
                }
                phase = phase_after(phase,id);
                writer.packet(&packet)?;
                summary.packets+=1;
                summary.packet_bytes+=packet.bytes.len() as u64;
                summary.last_packet_ms=packet.ms;
            }
            if summary.first_tick.is_none() { return Err("recording has no krasnokamen:tick marker; configure server recorder account".into()); }
            let file = writer.finish()?.into_inner().map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| format!("sync recording: {e}"))?;
            Ok(summary)
        }).map_err(|e| e.to_string())?;
        Ok(Self { sender, pending_bytes, worker, receipt, start: Instant::now() })
    }
    pub(crate) fn packet(&self, bytes: &[u8]) -> Result<()> {
        let ms = u32::try_from(self.start.elapsed().as_millis()).map_err(|_| "recording exceeds u32 millisecond duration")?;
        let charge = bytes.len()+PACKET_OVERHEAD;
        if self.pending_bytes.fetch_add(charge,Ordering::AcqRel)+charge > MAX_PENDING_BYTES {
            self.pending_bytes.fetch_sub(charge,Ordering::AcqRel);
            return Err("solrec pending bytes exceeded 64 MiB: recording disk cannot keep up".into());
        }
        self.sender.send(Packet { ms, bytes: bytes.to_vec() }).map_err(|_| {
            self.pending_bytes.fetch_sub(charge,Ordering::AcqRel);
            "solrec writer failed: recording stopped".into()
        })
    }
    pub(crate) fn finish(self,take_complete:bool,network_error:Option<&str>) -> Result<()> {
        drop(self.sender);
        let summary=self.worker.join().map_err(|_|"solrec writer panicked".to_string()).and_then(|result|result);
        let error=summary.as_ref().err().cloned()
            .or_else(||network_error.map(str::to_owned))
            .or_else(||(!take_complete).then(||"recording ended before requested take completion".into()));
        let known=summary.as_ref().ok();
        publish_json(&self.receipt,&serde_json::json!({
            "clean_completion":error.is_none(),
            "first_tick":known.and_then(|s|s.first_tick),
            "last_tick":known.and_then(|s|s.last_tick),
            "markers":known.map(|s|s.markers),
            "marker_gaps":known.map(|s|s.marker_gaps),
            "packets":known.map(|s|s.packets),
            "packet_bytes":known.map(|s|s.packet_bytes),
            "last_packet_ms":known.map(|s|s.last_packet_ms),
            "error":error.as_deref()
        }))?;
        match error {Some(error)=>Err(error),None=>Ok(())}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    pub(super) fn header() -> Header { Header { protocol_version: 777, server_build: "authored-test".into(), seed: 5, start_tick: 8, wall_time: 0 } }
    #[test]
    fn round_trip_preserves_noncanonical_id_and_arbitrary_packet_bytes() {
        let packets = [Packet { ms: 0, bytes: vec![0x81, 0, 0xff, 0, b'\n'] }, Packet { ms: 16, bytes: (0..=255).collect() }];
        let mut writer = RecordingWriter::new(Vec::new(), &header()).unwrap();
        for packet in &packets { writer.packet(packet).unwrap(); }
        let bytes = writer.finish().unwrap();
        let offset = bytes.iter().position(|&b| b == b'\n').unwrap() + 1;
        assert_eq!(&bytes[offset..offset+8], &[0,0,0,0,0,0,0,5]);
        let mut reader = RecordingReader::new(io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.header, header());
        for packet in packets { assert_eq!(reader.next_packet().unwrap(), Some(packet)); }
        assert_eq!(reader.next_packet().unwrap(), None);
    }
    #[test]
    fn truncated_or_regressing_records_fail_closed() {
        let writer = RecordingWriter::new(Vec::new(), &header()).unwrap();
        let mut bytes = writer.finish().unwrap();
        bytes.extend([0,0,0]);
        assert!(RecordingReader::new(io::Cursor::new(bytes)).unwrap().next_packet().is_err());
        let mut writer = RecordingWriter::new(Vec::new(), &header()).unwrap();
        writer.packet(&Packet { ms: 7, bytes: vec![1] }).unwrap();
        assert!(writer.packet(&Packet { ms: 6, bytes: vec![1] }).is_err());
    }
}
