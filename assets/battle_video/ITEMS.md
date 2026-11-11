# Battle equipment candidates

These are resource identifiers, not proof of a deliverable bundle. For local
native-client development, `SOLARIS_CLIENT_RESOURCE_PACKS` accepts this extracted
pack directory; Loader delivery has not been implemented or exercised.

| Appearance | Component | Resource identifier | Suggested vanilla carrier |
|---|---|---|---|
| Holy armor (all four slots) | equippable `{slot:…,asset_id:…}` | `solaris_battle:holy` | matching iron armor piece |
| Iron Plate armor (all four slots) | equippable `{slot:…,asset_id:…}`; `dyed_color` tints the plume (undyed red) | `solaris_battle:iron_plate` | matching iron armor piece |
| Crested helm | equippable `{slot:head,asset_id:…}`; `dyed_color` tints the crest (undyed red) | `solaris_battle:crested_helm` | `minecraft:iron_helmet` |
| Red surcoat | equippable `{slot:chest,asset_id:…}` | `solaris_battle:red_surcoat` | `minecraft:iron_chestplate` |
| Blue surcoat | equippable `{slot:chest,asset_id:…}` | `solaris_battle:blue_surcoat` | `minecraft:iron_chestplate` |
| Red caparison | equippable `{slot:body,asset_id:…}` | `solaris_battle:red_caparison` | `minecraft:iron_horse_armor` |
| Blue caparison | equippable `{slot:body,asset_id:…}` | `solaris_battle:blue_caparison` | `minecraft:iron_horse_armor` |
| Red heater shield, flat sprite | item_model | `solaris_battle:red_heater_shield` | `minecraft:shield` |
| Blue heater shield, flat sprite | item_model | `solaris_battle:blue_heater_shield` | `minecraft:shield` |
| Iron kite shield | item_model | `solaris_battle:iron_kite_shield` | `minecraft:shield` |
| Iron tower shield | item_model | `solaris_battle:iron_tower_shield` | `minecraft:shield` |
| Longsword | item_model | `solaris_battle:iron_longsword` | `minecraft:iron_sword` |
| Spear | item_model | `solaris_battle:iron_spear` | `minecraft:iron_sword` |
| Lance | item_model | `solaris_battle:iron_lance` | `minecraft:iron_sword` |
| Mace | item_model | `solaris_battle:iron_flanged_mace` | `minecraft:iron_sword` |
| Halberd | item_model | `solaris_battle:iron_halberd` | `minecraft:iron_sword` |
| Longbow, static | item_model | `solaris_battle:iron_longbow_standby` | `minecraft:bow` |
| Heavy crossbow, static | item_model | `solaris_battle:iron_heavy_crossbow_standby` | `minecraft:crossbow` |

The red crest of the crested helm and the red plume pixels of the Iron Plate
humanoid texture are a separate dyeable layer (`crested_helm_plume`,
`iron_plate_plume`, greyscale; the base textures keep a grey copy). Battle
staging dyes it per side: red `0xB92529` for side 1, blue `0x294DB7` for side 2.

Example for a core build containing the equippable codec, using its existing
summon equipment path:

```mcfunction
summon minecraft:zombie ~ ~ ~ {NoAI:1b,equipment:{chest:{id:'minecraft:iron_chestplate',components:{'minecraft:equippable':{slot:'chest',asset_id:'solaris_battle:holy'}}},mainhand:{id:'minecraft:iron_sword',components:{'minecraft:item_model':'solaris_battle:iron_longsword'}}}}
```

All appearances are unverified in a graphical client. The bows and crossbows
use static handheld item models; draw and loaded states are not implemented.
The heater shields are flat sprites, not three-dimensional shield rigs.
Loader asset delivery is not implemented.
