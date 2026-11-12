# Battle resource attribution

The seven `iron_*` weapon sprites in `assets/krasnokamen_battle/textures/item/`
are unmodified copies from [Spartan Weaponry](https://github.com/ObliviousSpartan/SpartanWeaponry),
commit `e45342c53c`, by ObliviousSpartan and contributors, distributed under
Apache License 2.0; see `licenses/SpartanWeaponry-APACHE-2.0.txt`. The accompanying
item model and item definition JSON files were created for Krasnokamen and are not
copies of the upstream model files.

The Iron kite and tower shield item models are converted and transformed from
[Spartan Shields](https://github.com/ObliviousSpartan/SpartanShields), commit
`89e376d70b`, by ObliviousSpartan and contributors, Apache License 2.0; see
`licenses/SpartanShields-APACHE-2.0.txt`. Their unmodified shared texture is
`assets/krasnokamen_battle/textures/item/iron_tower_shield.png`. Coordinates,
model format, item definitions and item display transforms were changed for
Krasnokamen. Rendering and UV orientation are not yet visually verified.

The `battle_holy_*` and `battle_iron_plate_*` entity models are Krasnokamen-schema
conversions of the normal-width armor models from
[Armor of the Ages](https://github.com/Dawn-of-Time-Project/armoroftheages),
commit `f26bba5f6f`, MIT, copyright 2023 Pierre C; see
`licenses/ArmorOfTheAges-MIT.txt`. Upstream credits Poulpinou and TheGoldenWorld
for development and artwork, and Hahdrim for Iron Plate. Original texture copies
are retained in `source-textures`. The Iron Plate equipment texture was modified
by adding eight transparent rows above it, and its model UVs shifted accordingly,
to retain a source cuboid with a negative V coordinate. Upstream animated
decorations are represented at their rest pose.

The red and blue surcoats, caparisons, heater shield sprites, and crested helm
geometry and textures were created for Krasnokamen. These resources have not yet
been validated in the native client; copying them into this directory does not
itself register them with Loader or activate them in the texture atlas.
