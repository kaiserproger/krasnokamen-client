# Solaris entity models

Entity and block-entity geometry is **authored by Solaris** and shipped inside the client binary
(`assets/solaris/models/entity/<id>.json`, embedded by `build.rs`, baked by
`src/entities/solaris_models.rs`). Entity metadata (protocol ids, dimensions, metadata accessors,
texture/variant selection, head/body turn limits) is the Solaris-owned data table
`assets/solaris/entities/entity-facts.json`, embedded via `include_str!` in
`src/entities/resources.rs`. Nothing is read from `dist/entities` any more.

Models are UV-compatible with the vanilla entity textures, which the player supplies at runtime.
Most boxes were authored from the unwrapped regions of the vanilla PNG. The
previous cushion and sulfur-cube candidate rigs and rendering-specific facts
were removed pending independently authored replacements.
Sleeping living entities use received pose and sleeping-position metadata with the facing of
the bed block in the received world. Passenger links bend the legs using the ordinary rig animation.

For chickens, cows, pigs and zombie nautiluses whose variant metadata has not been received, the
renderer selects the temperate entry from the received variant registry, or its first entry if
temperate is absent. An explicitly received variant must resolve in that registry; it is not replaced by a default.
Received on-fire metadata renders camera-facing, full-bright flame sprites for
visible entities; invisible entities do not receive the overlay. Native
front/side views covered a burning baby zombie and a helmeted nonburning
control. Other sizes, motion and matched-vanilla appearance remain unverified.
Bee and wolf anger textures use the received anger-end game tick and refresh when that
deadline expires. A native scene showed an attacked bee's red eyes; other wolf states
and matched-vanilla visuals remain unverified.

The armor stand has a ground-level base and a wooden crossbar-and-post body. Its
arm and base-plate geometry follows the received `ShowArms` and `NoBasePlate` flags;
small stands scale the stand and its equipment together about the feet.
Baby zombie-family and piglin-family armor uses shells fitted to each authored body and its
animation pivots. Baby chest-shell arm fronts and caps sample opaque regions of the iron base
texture. A native scene with a baby zombie in iron armor and a separate unarmored control
showed the previously exposed cyan shoulder strips covered; other species, armor materials,
animation and matched-vanilla appearance remain unverified.
Adult horse body armor and saddles use separate independently authored overlay rigs and the
received equippable asset's `horse_body` and `horse_saddle` texture layers. Overlay joints
follow the corresponding authored horse bones; unsupported species and missing resources stay
explicitly unrendered. The candidate's native equip/strip replay and movement remain unverified.
Adult wolf body armor uses an independently authored overlay rig and the received equippable
asset's `wolf_body` layers, including dyeable layers. A natural equip/strip candidate replay,
armor damage crack overlays, and matched vanilla visuals remain unverified.
Other adult animals select their species' runtime equippable layer (pig, strider, camel and
camel husk, donkey, mule, skeleton and zombie horse saddles; llama and trader llama carpets;
the happy ghast harness; nautilus and zombie nautilus saddles and armor) and draw it with an
independently authored overlay rig sharing the animal's root and joints. Baby animals and other
species stay explicitly unrendered. Players draw elytra panels from the worn `wings` layer
(published profile elytra or cape textures take priority), spread while fall-flying; a published
profile cape hangs from the shoulders with a small movement sway and is hidden under elytra.
Native equip/strip, gliding and matched-vanilla visuals remain unverified.

The ender dragon uses its named base material from the family facts. Copper golem statues use
the block state's pose and oxidation stage when selecting their authored rig and texture.
Banner and shield items use their selected base colors and pattern layers; parent item-graph
transforms compose with child transforms before the selected geometry is drawn.
Third-person left-hand equipment uses the mirrored resource display transform; the right hand,
first-person and GUI transforms are unchanged. Native replay `20261102T120044-scene-shot-mts2d990`
on client `08553e` showed received shields and swords on opposite sides of three static,
command-equipped subjects, including a stand with the hands swapped. The earlier recenter-only
candidate `20261102T115505-scene-shot-t2p29roa` still crossed the shield to the sword side.
Chat partly obscures these views; movement, natural equipment interaction and matched vanilla
visual parity remain unverified.
Placed standing and wall banners use the banner block-entity renderer in the native scene,
including received base colors and pattern layers; their flag animation uses world time.
The banner rigs are independent approximations authored from local texture images, Solaris
renderer/schema and native screenshots; patterned banners and matched-vanilla parity are
not established by those screenshots.
Standing and wall banner flags share the full 20×40 texel box unwrap; their poles and bars
use texel-sized unwraps. Sign bodies in 26.3 come from terrain block models; the
block-entity pass emits their text. Earlier native scenes checked oak-sign text and glow
on standing and wall signs and text on wall and ceiling hanging signs. Those captures
predate the 26.3 integration and do not prove the integrated renderer.
Bell bodies are centered within their block before the received swing animation is applied.
Placed shulker boxes and decorated pots use their block-entity renderers in the native scene,
including shulker lid progress and received pot sherd selection. Pot rigs are independent
approximations authored from local texture images, Solaris renderer/schema and native
screenshots. A 26.3 native scene showed an angler pattern on a placed pot and a blank
control; other patterns, orientations, open animation and matched-vanilla parity remain
unverified.
Decorated pot items compose the authored base and four side rigs, selecting each side's
texture from received pot decorations. An unpatterned GUI item was visually exercised;
patterned item visuals and a complete first-person view remain unverified.
Boat and raft roots anchor their authored hull bottoms to the received entity position;
boat wall geometry meets that bottom. Spruce boat, oak chest boat and bamboo chest raft
were visually checked from shore and water, without a matched vanilla oracle.
Item frames and glow item frames resolve their block-model geometry and contained item from
received metadata, including map, clock and glint rendering. Their position and interaction
bounds reconstruct the hanging cell and facing. Earlier captures exercised empty frames on
six faces and south-facing item insertion and rotation; they predate the integrated frame
geometry. The integrated frame orientations, maps, glow and invisible frames need live checks.
Clock items use received world time when rendered in frames and as dropped entities. A
native-client scene checked the same framed clock at day and night against a framed stone
control; dropped-clock visuals and matched-vanilla appearance remain unverified.
Experience orbs render as camera-facing sprites using received XP value and activated
resources, with a camera-rotated lighting normal. A native-client scene exercised one
received value-1 orb from two views against a no-orb control. The sprite was small in
these views; texture details, other values and matched-vanilla appearance remain unverified.
Projectile tridents transform their authored pole axis into the same orientation basis as
arrows. A received downward-pitch state was visually checked; flight and vanilla parity
remain unverified.
Enchanting tables render their animated book and conduits render their inactive shell or
active cage, wind and camera-facing eye in the native scene. A dry inactive conduit and an
enchanting table were visually exercised; active conduit and matched-vanilla parity were not.
Bed item graphs select the authored head and foot geometry separately before their graph transforms.
Placed skulls render in the native scene. A player head whose profile (UUID, else name) is a
listed player with a published skin renders that skin with its hat layer; other profiles stay
reported unrendered, because the scene does not fetch textures for arbitrary head profiles.

Creepers count the received swell direction (or ignition) through a 30-tick fuse: the model
widens and grows about its feet while blinking white at a quickening rate, and returns to normal
when defused. Charged creepers draw a scrolling `creeper_armor` swirl on an enlarged copy of
their pose (`creeper_charge`), also while invisible. Primed TNT and falling blocks draw their
received block model (TNT flashes white and swells in its last ticks); lightning bolts draw
independently authored seeded jagged geometry while the strike flashes; leashed mobs draw a
sagging brown rope from the neck to the holder's hand or knot. Sizes, rates and colours of these
effects are Solaris approximations of the minecraft.wiki descriptions, not game constants.

## File format (schema 1)

```json
{
  "schema": 1,
  "id": "cow",
  "texture": {"path": "entity/cow/cow_temperate.png", "size": [64, 64]},
  "roles": {"head": ["head"], "right_leg": ["right_hind_leg", "left_front_leg"], "left_leg": [...]},
  "bones": [
    {"name": "head", "pivot": [0, 4, -8], "rotation": [0, 0, 0], "scale": [1, 1, 1],
     "cubes": [{"origin": [-4, -4, -6], "size": [8, 8, 6], "uv": [0, 0], "mirror": false,
                "inflate": 0, "faces": {"front": [u0, v0, u1, v1]}}],
     "bones": []}
  ]
}
```

* Lengths are pixels (1/16 block), +Y down, feet at `y = 24`, front is -Z, model -X is the
  entity's right. `pivot` is relative to the parent bone; cube `origin` relative to its bone;
  rotations are degrees (Z, then Y, then X).
* `texture.path` is relative to `assets/minecraft/textures/` and names the model's default vanilla
  texture; `size` must equal the PNG size. Runtime texture choice for variants (colour, profession,
  baby, tamed...) stays with the family/variant code and only needs the same layout.
* Cubes use the classic box unwrap (`uv` + size `w x h x d`); `mirror` mirrors the cube about its
  centre keeping texel assignment; `inflate` grows the box; `faces` replaces a face's rectangle
  (swapped corners flip) for flat planes.
* `roles` lists the bones the generic animation drives: `head` (look pitch/yaw), `right_leg` /
  `left_leg` (opposite swing phase; quadrupeds list diagonal pairs), `right_arm` / `left_arm`,
  `wing_negative_x` / `wing_positive_x` (opposed flap; chickens and parrots fold their wings
  while received collision lies under their feet and flap only in midair), `crossed_arms` (villager-family arms bone:
  held-item anchor of the source `CrossedArmsItemLayer`; not animated) and `nose` (witch nose: the potion anchor, tilts forward
  while the main hand holds an item). Bone names must be unique; code also special-cases `body` (sneak) and the
  humanoid names for armor stands and first-person arms.
  Zombie-family arm roles additionally use the received aggressive and baby/equipment state
  for their forward pose and attack swing, rather than the generic walking arm swing.

## Rig keys

The renderer asks for rigs by key (`minecraft:cow#renderer_main`, `minecraft:cow_baby#main`,
`minecraft:oak_chest_boat#renderer/model`, `minecraft:zombie#layer3/modelSet/head`...).
`solaris_models::model_id` maps a key to a model id (file stem): qualifiers are dropped, wood
variants collapse (`boat`, `chest_boat`, `raft`, `chest_raft`, every `*_minecart` -> `minecart`),
armor-layer slots map to `armor_<slot>` / `armor_baby_<slot>`, with species-specific baby
shells where body pivots differ.

### Sign text belongs to the block-entity pass

In 26.3 the resource-pack block models supply standing, wall and hanging sign
bodies through terrain. The block-entity renderer supplies only front/back text,
using the pinned sign text transformations. It must not request the removed
`entity/signs` textures or a second body rig; those dependencies prevent any text
from being emitted even while the terrain board is visible.

Sign mesh regressions deliberately provide no legacy sign texture or body rig.
They receive both NBT text payload forms, assert text material indices and outward
winding on both faces, and reject duplicate body indices. Ordinary non-glowing
and glowing text are covered; windowed pixels remain a separate acceptance gate.

Chest and bell rigs use the same +Y-down authoring convention as living
entities. Their block-entity roots convert that space to world coordinates;
the bell's two cuboids follow the body and rim regions of its supplied texture.
Double-chest halves each occupy their own block and share the centered latch.
The base's sides start one texel below its own top rim row (faces `v = 34`), so
the lid's bottom rim is the single dark band at the seam; the latch hangs two
pixels above and two below that seam.

### Oracle-measured silhouettes

Box sizes and UV rectangles are authored by inspecting the texture nets supplied
by the local resource pack, not by importing a game model or renderer. Relative
positions are fitted to the Java oracle screenshots: the golem is 43 authored
model units tall, with its head on the shoulder line and its arms ending 3.5
units above the feet; the child's nose covers the lowest two face rows.
These are our reconstruction parameters, not extracted Mojang model constants.
Tests in `solaris_models/tests.rs` pin that authored reconstruction.

### Labels and shadows

`text_display` entities (server settlement labels) are drawn by the name-tag pass
(`render_nameplates.rs`): received text, line width, billboard, transformation,
background, opacity, style flags and brightness override; the backdrop rests on the
entity position and the text is lit by the received light. Living entities cast a
blob shadow (`shadows.rs`): the runtime-supplied `misc/shadow` disc on full-block
tops under the feet. Opacity 0.5, height fade 2 blocks, distance fade 16 blocks
and squared light falloff are chosen approximation parameters, not measured
vanilla formulae. The radius fit (`0.55 * width + 0.17`, golem 0.7) and extra baby
factor 0.5 approximate the observed discs; other mobs are not individually
measured yet.

Text-display style bits and packed brightness are interpreted from server-sent
metadata, using the existing display transform owner. Font scale 0.025 reuses the
authored nameplate convention; the 10-unit line step and bottom-aligned backdrop
are fitted to the visible settlement label. These values are not read from Java
bytecode. The NoAI head-glance gate uses the server's mob flags rather than
inventing a local look target for a stationary oracle fixture.

## Missing models

`Models::rig(key)` never fails. A key with no authored model renders a **magenta, entity-sized
placeholder box** (family rigs are sized from the entity's bounding box at load; other keys use
a fixed box). Placeholders are tinted in `presentation::emit_rig`.

## Tooling and tests

* `python3 tools/entity_models.py fmt|check|preview` - canonical formatting, validation
  (schema, UV bounds, texture existence/size, unique bones, roles, texel coverage) and a textured
  four-view + UV-map preview using the vanilla PNG (`SOLARIS_TEXTURES` overrides the texture root).
* `cargo test --lib solaris_models` - geometry winding/unwrap/mirror tests, key resolution, and a
  sanity test over every shipped model (UVs inside the declared texture, roles/bones valid, and -
  when vanilla textures are available under `SOLARIS_CLIENT_RESOURCE_ROOT` or `dist/resources` -
  the texture exists with the declared size).

## Coverage and known limits

* Authored models cover players (wide/slim), the humanoid and skeleton families, illagers/villagers,
  farm and wild animals with their baby layouts, aquatic mobs, Nether/End mobs, vehicles,
  projectiles, armor shells, armor stand and the block entities the renderer draws (chests,
  beds, banners, skulls, bell, book, conduit, decorated pot, shulker box, shield).
  The native block-entity pass also submits received campfire food through the shared
  item painter, using the campfire's facing and four item slots. Unavailable item
  models are reported as unrendered. An actual-client scene showed a naturally inserted
  raw chicken in slot 0 on an unlit south-facing campfire; other states and vanilla
  parity remain unverified.
  The same pass renders received shelf items and exposed brushable-block items.
  A native scene showed three items in a south-facing shelf and an edge-on item
  on dusted sand, with empty controls. This verifies static presentation only;
  brushing, reload persistence and vanilla visual parity were not checked.
  `sheep_wool` / `sheep_wool_baby` are the dyed coat drawn by `src/entities/sheep.rs`
  (undercoat tinted body once sheared).
  Tamed cats and wolves render their age-specific collar texture, dyed from received collar
  metadata (red when the server leaves it at the vanilla default); untamed pets omit it.
  Received sitting flags select Solaris-authored sitting poses for adult and baby cats and
  wolves; red-collar static standing/sitting views were exercised. Transition animation
  and matched-vanilla poses remain unverified.
  The allay head's faces sample explicit rectangles of the runtime texture; the front one
  contains both eyes. An earlier real-client replay (`20261102T081517-scene-shot-ur5d50bb`)
  showed both eyes on the previous, larger allay rig; the re-authored rig is unverified natively.
  Adult and baby goat and the four sheep rigs (base and coat, adult and lamb) size every cube from the
  net it samples in the runtime atlas and place it by fitting orthographic silhouettes against the stored
  reference pictures (`refsheet.py sheet`; author-static mean IoU 0.92 adult goat, 0.98 baby goat,
  1.00 adult sheep and coat, 0.99 lamb, 0.97 lamb coat). The adult goat head is a tilted `face`
  bone (nose box, ears, beard plane) under the animated `head` bone, with the horns beside it;
  the baby goat's second body net is hidden inside the first and is unused. Vanilla's half-pixel
  sideways offset of the adult goat is not reproduced because `check` requires x-mirrored legs.
  The lamb coat is the base shape with a 0.05 px inflate against coplanar depth fights.
  `goat_and_sheep_rigs_have_no_overlapping_same_facing_coplanar_faces` keeps the earlier goat
  stripe fix (coplanar horn/head and leg/torso faces) true for all six rigs. Adult
  goat horn spacing was fitted from matched 26.3 Java/native front captures;
  motion and complete matched-vanilla parity remain unverified.
  The snifflet rig includes a middle leg pair as well as front and hind legs; its
  gait remains simplified and has not been validated against vanilla motion.
  Baby zombie villager body and arms sample the baby clothing area instead of the
  dark head/leg area; this UV correction still requires post-build visual validation.
  Adult piglin, piglin brute and zombified piglin use independently authored short
  snouts, tusks and side ears parented to the animated head. Native-client static
  front and three-quarter views showed all three rigs; armor interaction, motion
  and matched-vanilla appearance remain unverified.
  Giants render at six times the ordinary humanoid scale about their feet; a
  native-client summon showed the larger model and a 12-block hitbox.
  Equipment and matched-vanilla appearance remain unverified.
  Adult and baby pigs use independently authored texture-sized head, snout, torso and leg
  geometry without separate ears or tail: the adult head sits in front of the body with the
  snout just below the eyes, and the piglet head rises a pixel above its back. The cold pig's
  extra texture layer is not modelled.
  Allay, vex, baby bee, ghastling, rabbit, fox, panda, hoglin, zoglin, strider, guardian, elder
  guardian, cod, salmon and axolotl rigs (with their babies where listed) and the vindicator's
  clothing UVs were re-authored by eye from public minecraft.wiki renders; receipts list the
  references. `post_rc4_reauthored_rigs_keep_their_reference_silhouettes` pins the reported
  defects (legs, wings, gills, fins, spikes, tusks, proportions); native appearance is unverified.
  Slime and magma cube geometry scales about the feet using the received size; the default
  remains one. Slime's face and inner body use cutout rendering, with a separate translucent
  outer shell. The magma cube's orange and yellow eye rows use adjacent texture slices;
  a size-four native-client screenshot confirms contiguous eyes, not exact vanilla parity or motion.
  Pig and slime visual parity remains unverified.
  26.3 families: `cushion` is the `CushionModel` slab, turned to the nearest horizontal
  facing (`Direction.fromYRot`) and textured from the received dye colour. Poplar boats
  reuse the shared `boat`/`chest_boat` hulls with `entity/boat|chest_boat/poplar`.
  Sulfur cube geometry is four rigs: adult and baby outer shells, drawn translucent, and inner cubes.
  Their `cube` bone carries the renderer's age downscale and vertical offset, so the inner
  layer, the swallowed body-slot block (X 180 degrees, (-0.5, -0.518, -0.5), half size for
  babies, worldless `BlockColors` tints) and worn skulls or head items (custom-head transform,
  16/19 counter-scale) share one frame. A body skull, else a head-slot skull, hides the inner
  layer and the block. The received fuse swells the cube in its last ten ticks. Babies use
  the size alone for dimensions, as `AbstractCubeMob.getDefaultDimensions` does. The white fuse
  flash and hurt overlay on the inner layer and block are not drawn. Squish, the 0.999 downscale,
  special-renderer blocks (reported unrendered) and profiled player heads are not modelled.
  Native-client scenes showed adult sulfur-cube shells and selected contents, white
  cushions and a poplar boat. Baby and empty-inner cubes, other cushion colors,
  the chest boat and matched-vanilla appearance remain unverified.
  Humanoid armor emits received trim patterns using registry-resolved material palettes,
  equipment-asset overrides and the slot's normal or leggings rig; vanilla's baby layer
  omits trim. Post-build worn-trim visual validation remains pending.
  `breeze_wind` provides the scrolling wind layer; snow golems with the pumpkin flag render a
  carved-pumpkin block attached to the animated head.
  Creaking eyes use a head-attached emissive layer: active metadata enables it, while
  teardown uses the client's flicker state. The layer is independent of base invisibility.
  Active and teardown visuals have not yet been validated against a server producing those states.
  Chest item and placed rigs convert the authored Y-down frame to their display/world
  frames; each double-chest half uses its own block-local origin.
  Chest item geometry reverses paired vertices and UVs after its reflected coordinate
  transform to preserve outward winding. The single-chest rig was independently authored
  from textures and rendered screenshots, without copying vanilla model geometry.
  Client SHA `e280328c1c2ff5b3472cfe8eee2f336bb21ece9d1abbe3e923cc963e0e333055`
  shows textured lids on received normal and five copper items in GUI and held scenes
  `20261102T052300-scene-shot-_e95twwj` and `20261102T052300-scene-shot-798ot0uv`
  under `/run/media/kaiserroman/games/solaris-work/wt/rc2/.analysis/validation`.
  Held faces are partly obscured, and hidden faces and vanilla parity are unverified.
  After a missing-latch regression, placed closed normal and copper chests show their
  centered latches in `20261102T052257-scene-shot-7e2hkqka`; open animation remains
  unverified by these scenes.
  GUI special heads retain their cutout material while their faces are ordered in
  display-space depth for the depthless overlay. On client SHA
  `1612b34664e52a35e00d933008be5993fa736ca8de74242cf0e0a81be2468fe7`,
  `20261102T053716-scene-shot-8vday4b4` shows six received head variants with
  visible exterior faces and a solid decorated pot. The dragon head is too dark
  for fine-detail assessment; first-person captures are cropped, and vanilla
  parity is not established.
* Several creatures are simplified silhouettes (ender dragon, wither, warden,
  nautilus, sniffer, parrot...). Drowned have adult and baby outer clothing layers.
  Other overlay-only PNG layers (stray/bogged overlays, horse markings) are not modelled.
* `python3 tools/entity_models.py check` reports texture-texel coverage and structural validity.
  Neither establishes that a rig is selected, that its parts connect in motion, or that it
  looks correct in the client; those require a rendered scene with the relevant entity state.
* Native `minecraft_list_entities` reports the received baby flag and cached rig selection
  separately. A missing metadata value is not `false`, and a cached rig is not proof that a
  frame rendered; retain the entity identity and screenshot diagnostics alongside it.
  `received_metadata` contains raw received values keyed by protocol index; consult the entity
  schema to interpret them. Absence means the client has not received that index.
* End crystals have separate animated glass and core bones and a conditional base. Their beam
  target is not rendered yet.
* Painting entities dispatch to the registry-driven special renderer rather than requiring a
  family rig. A 1×1 painting was visually checked on all four wall facings; larger variants
  and vanilla-server placement semantics remain incompletely exercised.
* Received text displays use the client font renderer and display snapshot for orientation,
  interpolation, text style and background. A server-generated settlement house label was
  visually checked; other display styles and animation are not established by that frame.
* Entity metadata is Solaris-owned data: `assets/solaris/entities/entity-facts.json` (embedded in
  the binary; protocol ids, dimensions, metadata accessors, texture/variant selection, head/body
  turn limits, armor shell keys, motion rules). Pure tables, no meshes/UV/animation keyframes;
  the launcher needs no JDK and nothing is derived from the client jar for entities.
* Spider legs use a Solaris-authored alternating swing driven by received displacement;
  they ease back to their authored resting pose when movement stops. This is not a
  claim of vanilla gait parity.
