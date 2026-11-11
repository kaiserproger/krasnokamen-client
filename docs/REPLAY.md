# Packet recording and cinematic export

The `record` command is a normal headless native-client session: the network
owner logs each incoming Login/Configuration/Play packet before handing it to
the session. It does not simulate the battle or invent entity positions. The
separate Status connection and outbound client traffic are not replay packets.
Official online-mode authentication remains unsupported by this client's network
adapter; use the Solaris offline recorder account. No GPU or audio is opened.

## Record a take

The server must list the login name in `[recorder] players = ["BattleRecorder"]`.
By default that name must connect **directly from loopback** (`127.0.0.1`, `::1`,
or IPv4-mapped `::ffff:127.0.0.1` on a dual-stack listener, without a proxy):
run this recorder on the server host. Remote/proxied recorder
access additionally requires `[recorder] allow_remote = true`; a listed name
from elsewhere otherwise joins as a normal player. E1 uses these server-side
conditions, not a client custom-payload handshake. Connect the viewer and
position it at the battle centre before staging. It records only what that
viewer actually receives: chunks and entities outside server tracking/view
distance cannot be recovered by a later camera path. Treat the file as private
server-session data (it can include chat, player identities and custom payloads).

Prepare a header JSON with the actual server build, staging seed and start tick:

```json
{"protocol_version":777,"server_build":"actual-core-commit","seed":42,"start_tick":0,"wall_time":0}
```

`wall_time` is replaced with actual UNIX-epoch milliseconds when the file opens.
`start_tick` is replaced by the first received `solaris:tick`; a reserved-width
JSON header line is rewritten without moving packet records. The other metadata
is supplied by the operator, not inferred from a status MOTD.

```sh
"$SOLARIS_CLIENT_BINARY" record --server 127.0.0.1:25565 \
  --username BattleRecorder --output /work/takes/battle.solrec \
  --header /work/takes/header.json --duration 180 \
  --ready-file /work/takes/battle.ready --data-dir /work/client-data
```

The ready file is published after Play, an authoritative player pose and the
first tick marker, not merely TCP connect. `--duration` counts from that point; startup has a
120-second deadline. A normal exit closes IO, drains the bounded file queue,
flushes and fsyncs the recording. Do not kill the process to finish a successful
take. Disk saturation or a writer error fails the take instead of silently
losing packets or blocking the IO thread. Choose fresh output/ready filenames.

For BattleVideo, **join before setup** using the two-stage barrier:

```sh
"$SOLARIS_CLIENT_BINARY" record --server 127.0.0.1:25565 \
  --username BattleRecorder --output /work/takes/battle.solrec \
  --header /work/takes/header.json --duration 180 --data-dir /work/client-data \
  --ready-file /work/takes/take.ready \
  --roster-file /work/takes/roster.txt --roster-ready-file /work/takes/roster.ready \
  --roster-timeout 900
```

The roster parent directory must already exist; all three signal/input paths
must be fresh. The Linux inotify parent watch is installed **before connect
and initial readiness**, closing the setup/notification race. B4 waits for
`take.ready` (Play + received pose + first marker), stages the paused army,
obtains its exact soldier/horse UUID roster from op-only `battle roster`, then
atomically renames the complete roster to `roster.txt`. Closing a directly
written roster also publishes it; merely creating/opening it does not.

Only after **every listed UUID is currently present** does C3 publish
`roster.ready`; B4 may then start the battle. There is no entity-count
approximation or ever-seen set. `--duration` starts at this second readiness,
so setup time does not consume the requested take. Initial join has a 120-second
deadline. The separate post-join barrier uses `--roster-timeout SECONDS`
(default 120, finite positive maximum 86400; B4 uses 900 for large setups).
Recording remains non-rendering and stores all packets
during both stages. Signal JSON identifies `stage` (`joined` or `roster`),
`presentation:false`, required UUID/current received-entity counts, and integer `tick`: the
latest applied server marker at that causal barrier, never a local counter.

The roster has one UUID per line (1..100000 unique canonicalizable UUIDs,
at most 4 MiB). The one-shot filesystem worker blocks on publication/cancel
events and wakes the session; there is no file polling. Watch loss, overflow,
invalid input or timeout fails the take. Post-join notifications require Linux.

For other recordings with an already-published roster, the mutually exclusive
`--ready-entity-uuids /work/takes/roster.txt` mode waits for those UUIDs before
the single ready signal. Without either roster option, readiness establishes
only the connection, not completion of a large entity spawn burst.

### Final recording receipt

After file drain/flush/fsync, C3 atomically publishes **`<output>.result.json`**
(for example `/work/takes/battle.solrec.result.json`, retaining `.solrec`).
The root schema is:

- `clean_completion`: boolean; true only for explicitly completed requested
  takes with successful network stop and writer finalization.
- `first_tick`, `last_tick`: u64 first/last recorded marker over the **whole**
  take, including join/setup/paused time.
- `markers`: u64 number of recorded markers.
- `marker_gaps`: u64 integer count `sum(next_tick - previous_tick - 1)`,
  **not** an array of ranges. Duplicate/regressing markers fail the writer.
- `packets`, `packet_bytes`: u64 packet count and decompressed packet bytes.
- `last_packet_ms`: u32 timestamp of the final written packet.
- `error`: null on clean completion, otherwise a failure description.

If writer finalization fails, unknown counts/ranges are null, not fabricated
zeroes. Missing/false receipts or nonzero recorder exit must fail the take.
For gap-free battle acceptance also require integer `marker_gaps == 0`.
Use B4's authoritative battle-start tick and the receipt's `last_tick` to
measure recorded battle duration; `last_tick - first_tick` includes setup and
must not be mistaken for battle ticks. These facts describe the actual packet
file, not how long the server happened to run after its last recorded marker.
The `solaris:tick` markers in the recording, not telemetry's `recorders` key,
are authoritative for which ticks reached this recorder. Do not infer delivery
or fill marker gaps from that telemetry key's presence or absence.

### File contract

`.solrec` starts with one UTF-8 JSON header line (at most 16 KiB). Records are
`u32 big-endian ms_since_start | u32 big-endian byte_length | packet bytes`.
Packet bytes are the **exact decompressed packet-ID VarInt plus payload**, with
original ID encoding retained, not TCP compression/length envelopes. There is no
added phase byte: playback starts at Login and follows the recorded
LoginFinished/FinishConfiguration/StartConfiguration transitions. Lengths are
bounded by the existing protocol `MAX_FRAME`; timestamps must not decrease.
Truncated records are errors; EOF is legal only at a record boundary.

The Play `solaris:tick` custom payload has exactly two big-endian u64 values:
`tick`, then `wall_ns`. The latter is the core tick's `wall_start_unix_ns`,
identical to the dashboard and host sampler value, not the later edge
publish/receive/encoding time. These are server-owned telemetry labels. A
pump-skipped tick or recorder world queue with no free frame slot produces no
marker; a byte-full queue with a free slot may send only its marker. Gaps remain
jumps in tick number. The marker precedes that tick's world FIFO packets, except
asynchronous `solaris:lod/tile` payloads (only when LOD tiles were requested),
which may arrive between any markers and do not belong to tick T. Reliable-queue
packets use the latest observed marker. Receive milliseconds define replay time;
markers join frames to server metrics without estimating a tick from frame count
or wall-clock drift.

## Camera JSON

Times are seconds on the selected `--timebase` (`wall` by default, or `tick`).
The selected render range is half-open. Paths need 2..4096 strictly increasing keys, finite values, and a FOV
between 1 and 179 degrees. Each path uses either `look_target` on every key or
`yaw` and `pitch` on every key, never both. Uniform Catmull–Rom interpolation
uses duplicated endpoint keys; optional `ease: true` on a key smoothsteps its
outgoing segment. Angle paths unwrap yaw across ±180 degrees.

This shot assumes a staging fixture whose actual world spawn is `[0,80,0]`;
translate **both** positions and targets by the real staging centre delta.
Verify terrain clearance in the selected world before a final take.

```json
{"keyframes":[
  {"time":6,"position":[0,140,-180],"look_target":[0,80,0],"fov":65,"ease":true},
  {"time":7,"position":[12,140,-180],"look_target":[0,80,0],"fov":65}
]}
```

An optional top-level `"attach_entity":123` makes positions and look targets
world-axis offsets from that received entity's interpolated position, including
mount attachment. Missing/despawned entities fail the shot; there is no fallback
camera. The JSON does not alter gameplay or the recorded viewer's position.
`replay::Player::open(path, presentation, timebase)` selects the same timeline as
the exporter. `seek` is a **shot/cut boundary**, never an operation within a shot.
Backwards seeks rebuild the initial packet stream, retaining the explicit radius
and crowd capability. Every received packet and world/client clock tick is still
applied; the uncaptured prefix skips local player prediction, entity presentation
and scene extraction. The final 80 client ticks (four seconds at 20 Hz, or all
available history for an earlier cut) warm the normal entity presentation owner
before one target scene refresh. Frozen world ticks do not advance that owner.
The actual warmed world-tick count is `seek_warmup_ticks` in the frame sidecar.
80 is provisional: on the authored moving-entity fixture an 80-tick warm-up
matches a 400-tick uncut run (interpolated position within 1e-12 blocks, body
yaw within 1e-4 degrees, walk speed within 1e-6). It has not been measured on a
real take; `replay::seek_tests::captured_take_warmup_convergence` (ignored,
see Focused checks) does that against a cut-free presentation reference.
Received positions/state remain authoritative; cumulative presentation age/gait
phase may restart at a cut and are not promised identical to playing from login.
Use `advance_to`/`step(frame)` inside the shot, preserving that warmed history.
`step` uses rational `frame/60` time without accumulated floating-point drift;
it rejects backwards time (use an explicit cut-boundary `seek`).

### Timebase and coverage

`--timebase wall` preserves packet receive times and the existing client tick
cadence. A server running below 20 TPS therefore plays in slow motion.
`--timebase tick` maps each `solaris:tick` group to
`(tick - first_actual_marker_tick) * 0.05` seconds. Packets before the first
marker are configuration at zero; a marker itself starts its new group, and
all subsequent packets keep that group's time until the next marker. Missing
ticks remain gaps; duplicate/decreasing markers and timeline overflow fail.
The raw recorded bytes/timestamps and `last_applied_tick`/`wall_ns` labels are
never rewritten. Client ticks run every 50 ms in this mode. Camera keys and
`--start`/`--end` must use **tick seconds**, not receipt-wall seconds. For a slow
server, label the final video “20 ticks/s playback; actual server TPS on graph”;
the telemetry overlay must still join on the original `last_applied_tick`.

`--render-distance N` sets both entity and terrain radius to `N * 16` blocks,
including cinematic fog, within the normal client bounds 2..32 chunks. Without
the flag, replay follows the server radius received in the recording, falling
back to the deterministic client default 8 until one is announced. It never
reads or writes the render host's persisted options. An override can reveal
received corner entities beyond the recorder's spherical fog radius; it cannot
recover chunks/entities that the recorder never received. Inspect `known`,
`eligible` and the frame itself, rather than treating prepared instances as
proof of visible GPU pixels.

## Export a one-second smoke shot

Install normal client content with `setup` (see the main README); reuse the same
content metadata/assets as the recorder. Use an immutable release client from
Main's `tools/dev/pin-release --client --tree <client-tree>`. The binary path
below is the emitted `SOLARIS_CLIENT_BINARY`, not a guessed target artifact.

First record at least 12 seconds with the command above (`--duration 12`) and
save the shot JSON as `/work/takes/smoke.json`. The selected 6..7-second range
must already be in Play and have received chunks, lighting and tick markers.

Host software Vulkan (lavapipe; no display server is required):

```sh
WGPU_BACKEND=vulkan VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
  "$SOLARIS_CLIENT_BINARY" render-replay \
  --recording /work/takes/battle.solrec --camera /work/takes/smoke.json \
  --output /work/takes/smoke-lvp.mp4 --size 320x180 \
  --start 6 --end 7 --data-dir /work/client-data
ffprobe -v error -count_frames -select_streams v:0 \
  -show_entries stream=width,height,r_frame_rate,nb_read_frames \
  -of json /work/takes/smoke-lvp.mp4
```

If the host runtime requires X11 for another dependency, run that same export
under `xvfb-run -a`; the renderer still creates **no** window or surface. On the
owner's GPU, omit the lavapipe override (and any inherited software ICD selector):

```sh
WGPU_BACKEND=vulkan "$SOLARIS_CLIENT_BINARY" render-replay \
  --recording /work/takes/battle.solrec --camera /work/takes/smoke.json \
  --output /work/takes/smoke-gpu.mp4 --size 1920x1080 \
  --start 6 --end 7 --motion-samples 1 --data-dir /work/client-data
```

Before the first frame the exporter prints the GPU it renders with to stderr,
`render-replay adapter: {"name":…,"vendor":…,"device_type":…,"backend":…,"driver":…,"driver_info":…}`.
On a hybrid laptop check it: the client asks for the high-performance adapter,
but the Vulkan loader decides what is listed (`VK_DRIVER_FILES` restricts it to
one driver manifest, for example `/usr/share/vulkan/icd.d/nvidia_icd.json`).

Expect 60 frames, the chosen output dimensions and 60/1 fps. Inspect decoded
frames for actual terrain, armour, animation and camera movement; ffprobe alone
is not visual acceptance. Execute a second identical shot to a fresh filename
and compare `cpu_state_sha256` in the sidecars. GPU RGBA hashes are recorded but
are not promised identical across devices/drivers. Repeat on lavapipe under
Xvfb when collecting that platform's acceptance receipt.

For a sub-20-TPS battle take, author tick-second camera keys and export with
`--timebase tick --render-distance 20 --motion-samples 1`. Run the two identical
exports sequentially through the current graphical admission wrapper. Compare
both `cpu_state_sha256` and `rgba_sha256` on the **same pin/device/driver**;
cross-driver pixel equality remains outside the contract.

Rendering is fixed-step, not real time: output dimensions are rendered at twice
each axis and box-downsampled (four samples per output pixel). The Medium
HDR/shadow preset has TAA and projection jitter/history off. Optional 1..16
subframes sample the first half-frame (180-degree shutter) and average the
**display-encoded RGBA**, not linear HDR; this is intentionally documented, not
claimed physically exact blur. CPU mesh jobs finish through an event-driven
completion barrier before capture; a timeout is an error. The barrier meshes
sections before their first upload, so a wide `--render-distance` can exceed the
terrain's 512 MiB CPU mesh budget: a result that does not fit then waits, still
in flight, while the exporter renders an **uncaptured upload frame** and
releases the uploaded CPU payloads, and settling continues. Live play reaches
the same backpressure only when its per-frame uploads fall behind. Replay
terrain uses explicit zero-duration upload admission, so it has no
upload-dependent fade-in; an unknown fade policy is not drawable. Raw padded GPU
rows are stripped before 2x reduction and fed directly to ffmpeg stdin as tight RGBA.

The exporter negotiates the C1 crowd-instancing path from the offscreen
renderer and retains that choice across backward seeks. Recording itself
never constructs this renderer. CPU frame fingerprints include crowd instance
and mesh content, not process-global mesh revision IDs; revisions only
invalidate the cached mesh digest. Compare runs with identical content,
client build and rendering capabilities/settings.

The ffmpeg command uses libx264 CRF 18, yuv420p, BT.709 tags and faststart MP4.
`--ffmpeg /absolute/path` selects another installation of the same executable;
no shell command is evaluated. ffmpeg must provide libx264. Even output sizes
from 2x2 through 3840x2160 are supported, subject to adapter limits and the
2-GiB offscreen-target budget. Live-window budgets are unchanged.

For `shot.mp4`, `shot.frames.jsonl` contains frame index, recording time,
last applied tick, marker wall time, sampled ticks, camera pose, CPU-state and
RGBA SHA-256 hashes, SSAA/motion settings and TAA-off status. With motion blur,
`last_applied_tick`/camera describe the final shutter sample; `sample_ticks`
retains all sample labels. Join telemetry by tick, not encoding time. A failed
export can leave a partial MP4/sidecar: only a successful process exit makes a
complete shot. Outputs are never intentionally overwritten.

Each frame also contains `presentation`:

- `crowd_instances`: prepared instances supplied to the crowd renderer, **not**
  a GPU-visible count or proof that every recorded soldier is in camera range.
- `crowd_fallback`: the existing CPU-path counts by crowd kind and reason.
- `entity_extraction`: the session's existing presentation statistics, including
  known/eligible/submitted entities and unrendered reasons.
- `cpu_entity_extract_ms`: measured CPU extraction time accumulated across that
  frame's shutter samples and replay advancement. The first frame includes the
  single target refresh that ends its cut-boundary seek; this is not GPU time.

Counts/reasons describe the final shutter sample, like the camera pose. Camera
range and received tracking data still bound extraction; these counters do not
expand the view or recover unrecorded entities.

`terrain.upload_frames` counts that frame's uncaptured upload frames and
`terrain.cpu_mesh_resident_peak_bytes` is the largest CPU mesh payload held so
far in the process (never above the budget).

## Focused checks and provenance

`cargo test -p solaris-client --lib replay`,
`cargo test -p solaris-client --lib capture::`, and the live-no-block session test
cover original packet bytes (including noncanonical IDs and compression),
record reader/writer round trip and truncation, same-stream CPU frame hashes,
fixed-frame versus forward seek, cut-boundary seek versus uncut playback and the
unchanged live tick, tick timebase gaps/monotonic markers, replay radius,
authoritative tick gaps/received health, camera endpoints/yaw wrap/attachment/
easing, and SSAA/shutter arithmetic. `cargo test -p solaris-client --lib terrain::`
includes the replay settle beyond the CPU mesh budget (several upload rounds equal
one round). These do not replace the captured real-world smoke shot or a full
20k battle.

The warm-up measurement on a real take (offscreen Vulkan; slow on lavapipe):

```sh
SOLARIS_CINEMA_RECORDING=/work/takes/battle.solrec SOLARIS_CINEMA_RECEIPTS=/work/warmup \
SOLARIS_CINEMA_DATA=/work/client-data SOLARIS_CINEMA_RENDER_DISTANCE=20 \
SOLARIS_CINEMA_TICK_CAMERA=/work/takes/tick-shot.json \
SOLARIS_CINEMA_WALL_CAMERA=/work/takes/wall-shot.json WGPU_BACKEND=vulkan \
cargo test --release -p solaris-client --lib \
  replay::seek_tests::captured_take_warmup_convergence -- --exact --ignored --nocapture
```

Each camera's first key is the cut target. For each timebase it plays a
cut-free presentation reference to that target, then cuts with 0..160 warm-up
ticks; received state, packet and clock state must be exact, and
`<receipts>/{tick,wall}/convergence.json` records position/seat/body-yaw/walk
differences and the 480x270 image difference against the reference.

The explicit authored-fixture GPU/ffmpeg check (not a battle capture) is:

```sh
WGPU_BACKEND=vulkan VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
SOLARIS_CAPTURE_TEST_ARTIFACTS=/work/takes \
cargo test -p solaris-client --lib capture::gpu_tests -- --ignored --nocapture
```

It renders a one-second spline-camera clip of authored cubes and 16 armoured
crowd fixtures using their white test atlas/lightmap, not production armour
assets. It requires crowd support and checks 16 submitted crowd instances and
nonzero crowd draws, padded GPU row removal and fixed-time pixel identity with
TAA off. It encodes 60 MP4 frames, validates them with ffprobe and writes
`timing.json` containing every measured render duration plus seconds/frame.
The 636x360 render / 318x180 output is a deliberately small llvmpipe integration
check, not a real recording or 20k-battle performance.

Design sources (ideas only, no borrowed GPL/ARR code):
[ReplayMod docs](https://www.replaymod.com/docs/),
[wgpu 27 buffer mapping](https://docs.rs/wgpu/27.0.1/wgpu/struct.Buffer.html),
[wgpu row alignment](https://docs.rs/wgpu/27.0.1/wgpu/constant.COPY_BYTES_PER_ROW_ALIGNMENT.html),
[ffmpeg rawvideo](https://ffmpeg.org/ffmpeg-formats.html#rawvideo), and the
[cubic Hermite/Catmull–Rom formula](https://en.wikipedia.org/wiki/Cubic_Hermite_spline#Catmull%E2%80%93Rom_spline).
The BattleVideo research-capture section D supplies composition/master recipes.

Post-join roster publication follows
[Linux inotify close-write and moved-to events](https://man7.org/linux/man-pages/man7/inotify.7.html).
