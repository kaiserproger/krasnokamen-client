# Battle resource attribution

The shield and weapon item models (red/blue heater, iron kite and tower shields;
longsword, spear, lance, halberd, flanged mace, longbow, heavy crossbow) and their
textures (`battle_parts.png`, four shield paintings) were created for Krasnokamen by
`tools/battle_items_gen.py` and contain no third-party bytes. The shield board and
handle dimensions (plate 12x22x1, handle 2x6x6) and the idle/blocking display
transforms are numbers read from the local vanilla `ShieldModel` class and
`shield.json` / `shield_blocking.json`; no Mojang texture, model or sound is copied.
The earlier flat sprites and Spartan Shields element models (Apache-2.0, Spartan
Weaponry / Spartan Shields by ObliviousSpartan) are no longer shipped, so their
licence files were removed.

The `battle_holy_*` and `battle_iron_plate_*` entity models are Krasnokamen-schema
conversions of the normal-width armor models from
[Armor of the Ages](https://github.com/Dawn-of-Time-Project/armoroftheages),
commit `f26bba5f6f`, MIT, copyright 2023 Pierre C; see
`licenses/ArmorOfTheAges-MIT.txt`. Upstream credits Poulpinou and TheGoldenWorld
for development and artwork, and Hahdrim for Iron Plate. Original texture copies
are retained in `source-textures`. The Iron Plate equipment texture was modified
by adding eight transparent rows above it, and its model UVs shifted accordingly,
to retain a source cuboid with a negative V coordinate. Its red plume pixels were
moved into a greyscale dyeable layer, `iron_plate_plume.png`, and replaced by grey
in the base texture. Upstream animated decorations are represented at their rest pose,
except the Holy chest's animated effect planes (the gold rings and white crosses
around the torso and arms), which are omitted: drawn static at full size they cut a
flat cloud through the wearer.
For mounted and close-up shots the front face of the Holy helm cuboid was opened
(the cross visor's texels in front of the wearer's eyes, nose and mouth made
transparent, keeping a brow band and a nasal bar) so the wearer's face shows inside
the helm, as it already does through the Iron Plate helm.

The red and blue surcoats, caparisons and crested helm
geometry and textures were created for Krasnokamen (the crested helm's face is open in
the same way as above). The caparisons are drawn on the horse-armour box layout of
the adult horse (saddle cloth, flank crosses, neck cover and a short skirt over the
upper legs; head, mane and tail stay uncovered) by a script, not derived from any
Mojang texture. These resources have not yet
been validated in the native client; copying them into this directory does not
itself register them with Loader or activate them in the texture atlas.
