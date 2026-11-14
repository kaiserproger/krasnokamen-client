# Entity presentation: which entities tick

Decision record for the "undrawn mobs wait" rule of `Store::tick_presentation`
(`src/entities/physical.rs`). It cites `SPECIFICATION.md`: §5.2 time and threads
(lines 176-199), §5.4 entity storage (line 242), §10.3 entities (lines 395-399),
§12.1 optimization classes (lines 445-449) and row ENT-03 (line 919).

## Rule

The session owner ticks entity presentation once per client tick, whatever the
frame rate (§5.2: client simulation is not tied to render FPS). A frame draws the
entities inside its radius and their vanilla draw distance (`within`,
`src/entities/view_range.rs`). The draw set is chosen by distance only, never by
frustum or occlusion: an entity behind the camera is drawn. `Store::mark_shown`
marks each drawn body and its whole passenger chain with the frame's tick.

A tick runs while frames are drawing when the latest frame is at most
`MAX_TICKS_WITHOUT_FRAME` (4) ticks old. That is the most ticks the owner runs
per turn before it draws again (`src/lib.rs`). In such a tick a living mob
**waits** if the latest frame did not draw it, unless one of these holds:

- it is in the local player's passenger chain;
- it is the ender dragon;
- it is within `BODY_TICK_RADIUS` (16 blocks) of the local body. The player can
  reach, push or collide with it, so its tick state feeds local collision, hit
  and interaction prediction;
- it is in a passenger chain and the latest frame drew any body of that chain.
  This also covers a chain formed after that frame (a mount).

Every entity ticks when no frame is drawing: headless, after a replay seek
(`Store::forget_frames` resets the latest frame) or with a stalled frame owner.
The rule depends on the latest frame, not on the frame rate. At 15 fps (four
ticks per turn) the same mobs wait as at 60 fps.

## Absorb-wait contract

A waiting mob's received body still interpolates. `Entity::presented_transform`
counts the ticks it waited since its current interpolation began
(`lerp_waited`). A new packet starts from the pose on screen. Once the
interpolation would have ended, the mob's motion span is released, so it no
longer pads spatial queries while it waits.

The other tick state catches up in one step, `Store::absorb_wait`. The step runs
the first time the mob is drawn again (`mark_shown`, before any geometry) or
ticks again, whichever comes first. It applies the waited ticks in closed form
to:

- the received-body interpolation;
- the Source `age`;
- the active item use: `prepare_item_use` at the caught-up age, so the crossbow
  charge and the spear raise continue instead of restarting;
- the `EntityUseState` countdowns (`in_love`, `forced_age_timer`,
  `age_lock_particle_timer`) and the stand counter, which drops the standing bit
  when it runs out;
- the shulker peek and polar bear stand steps toward their current targets.
  They converge within 20 and 6 steps.

Item use and countdowns stay as they are when the mob's latest tick found its
blocks not yet received (`pending_world`). A ticked mob stops there too.

Metadata that reaches a waiting mob is stamped with `age + waited`, the age a
ticked mob has at that moment (`source_state::on_metadata`). Damage, death and
swing timing use absolute ticks and need no catch-up.

## Recorded residuals

- **R1, convergence after entry.** These resume where they stopped and converge
  over the ticks that follow: the walk animation (its target is clamped to 1
  and smoothed by 0.4 per tick), body and head smoothing (a moving mob's body
  takes its yaw at once, as in vanilla), wing and flap phases, seat motion and
  `EntityMotion` glance and squash. After a long wait, a landing squash can
  appear without a real landing (`src/motion.rs`).
- **R2, equipment change during a wait.** If an active use changes only by
  equipment, with no metadata, while the mob waits, the use is stamped at the
  absorb step. A ticked mob would stamp it at the first tick after the change.
- **R3, replay hashes.** `cpu_state_sha256` covers the age and pose of every
  received entity, so it depends on the camera path since the cut (which mobs
  waited). The same shot is still deterministic, but hashes pinned before this
  rule must be re-pinned (`docs/REPLAY.md`).

## Class (§12.1)

The rule is claimed as **class A** (incremental work and valid culling). While a
mob waits it is not drawn, so nothing drawn changes. When it is drawn again, its
closed-form state is current. Only R1 differs, and only on entry.

Owner question Q3 is open: accept R1 under class A, or treat the rule as class
B. Class B would gate the rule behind an explicitly chosen profile (§10.3: a
presentation LOD for distant decorative animation only in an explicitly chosen
profile) and test it separately.

The rule is not class C:

- network state consumption is unchanged: packets still apply to waiting mobs;
- local collision, hit and interaction are unchanged (`BODY_TICK_RADIUS`, and
  drawn mobs always tick);
- drawn and nearby mobs keep the full tick rate;
- there are no teleports: interpolation is closed-form;
- render distance is unchanged.

ENT-03: a hidden entity keeps receiving state. When it appears it keeps damage
(absolute ticks), pose (metadata) and equipment, and its item use continues.
The compatibility-matrix row moves only after parent acceptance.

§10.3 asks for animated entities to be counted separately. `PresentationStats`
records `ticked` and `waited`: entity ticks run and waited since the store
began. Replay frames carry them as `presentation.entity_extraction.ticked` and
`presentation.entity_extraction.waited`.

§5.4: the wait test reads per-slot fields only, with no dispatch on type-name
strings.

## Tests

These tests are in `src/entities/animation_tests.rs`:

- `offscreen_mob_waits_and_catches_up_on_entry`
- `mobs_wait_by_the_latest_frame_through_ticks_without_frames`
- `mounted_pair_waits_and_ticks_as_one`
- `mount_and_dismount_after_a_frame_keep_chains_whole`
- `non_living_passenger_ticks_and_draws_its_vehicle`
- `waiting_mob_countdowns_match_a_ticked_mob_when_drawn`
- `crossbow_charge_started_while_waiting_matches_a_ticked_mob`
- `waiting_mob_releases_its_motion_span_when_its_interpolation_ends`
- `waiting_mob_carried_far_is_drawn_fresh_when_the_camera_jumps_to_it`
- `mob_behind_the_camera_within_range_ticks`
- `undrawn_mob_near_the_local_body_ticks`

Seek: `fixed_frames_and_direct_forward_seek_apply_identical_state`
(`src/replay/tests.rs`).

## Rollback

The state lives only in client memory and is never written to disk. Reverting
the change is a full rollback. There is no runtime switch or environment
variable.
