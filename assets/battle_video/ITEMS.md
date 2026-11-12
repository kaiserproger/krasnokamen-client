# Battle equipment candidates

These are resource identifiers, not proof of a deliverable bundle. For local
native-client development, `KRASNOKAMEN_CLIENT_RESOURCE_PACKS` accepts this extracted
pack directory; Loader delivery has not been implemented or exercised.

| Appearance | Component | Resource identifier | Suggested vanilla carrier |
|---|---|---|---|
| Holy armor (all four slots) | equippable `{slot:…,asset_id:…}` | `krasnokamen_battle:holy` | matching iron armor piece |
| Iron Plate armor (all four slots) | equippable `{slot:…,asset_id:…}` | `krasnokamen_battle:iron_plate` | matching iron armor piece |
| Crested helm | equippable `{slot:head,asset_id:…}` | `krasnokamen_battle:crested_helm` | `minecraft:iron_helmet` |
| Red surcoat | equippable `{slot:chest,asset_id:…}` | `krasnokamen_battle:red_surcoat` | `minecraft:iron_chestplate` |
| Blue surcoat | equippable `{slot:chest,asset_id:…}` | `krasnokamen_battle:blue_surcoat` | `minecraft:iron_chestplate` |
| Red caparison | equippable `{slot:body,asset_id:…}` | `krasnokamen_battle:red_caparison` | `minecraft:iron_horse_armor` |
| Blue caparison | equippable `{slot:body,asset_id:…}` | `krasnokamen_battle:blue_caparison` | `minecraft:iron_horse_armor` |
| Red heater shield, flat sprite | item_model | `krasnokamen_battle:red_heater_shield` | `minecraft:shield` |
| Blue heater shield, flat sprite | item_model | `krasnokamen_battle:blue_heater_shield` | `minecraft:shield` |
| Iron kite shield | item_model | `krasnokamen_battle:iron_kite_shield` | `minecraft:shield` |
| Iron tower shield | item_model | `krasnokamen_battle:iron_tower_shield` | `minecraft:shield` |
| Longsword | item_model | `krasnokamen_battle:iron_longsword` | `minecraft:iron_sword` |
| Spear | item_model | `krasnokamen_battle:iron_spear` | `minecraft:iron_sword` |
| Lance | item_model | `krasnokamen_battle:iron_lance` | `minecraft:iron_sword` |
| Mace | item_model | `krasnokamen_battle:iron_flanged_mace` | `minecraft:iron_sword` |
| Halberd | item_model | `krasnokamen_battle:iron_halberd` | `minecraft:iron_sword` |
| Longbow, static | item_model | `krasnokamen_battle:iron_longbow_standby` | `minecraft:bow` |
| Heavy crossbow, static | item_model | `krasnokamen_battle:iron_heavy_crossbow_standby` | `minecraft:crossbow` |

Example for a core build containing the equippable codec, using its existing
summon equipment path:

```mcfunction
summon minecraft:zombie ~ ~ ~ {NoAI:1b,equipment:{chest:{id:'minecraft:iron_chestplate',components:{'minecraft:equippable':{slot:'chest',asset_id:'krasnokamen_battle:holy'}}},mainhand:{id:'minecraft:iron_sword',components:{'minecraft:item_model':'krasnokamen_battle:iron_longsword'}}}}
```

All appearances are unverified in a graphical client. The bows and crossbows
use static handheld item models; draw and loaded states are not implemented.
The heater shields are flat sprites, not three-dimensional shield rigs.
Loader asset delivery is not implemented.
