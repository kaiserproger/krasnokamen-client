mod wearables {
    use super::*;
    use crate::protocol::{Reader, Writer};
    use serde_json::json;

    fn region(layer: u32) -> TextureRegion {
        TextureRegion {
            layer,
            uv: [0.1, 0.2, 0.8, 0.9],
            width: 64,
            height: 32,
        }
    }

    fn equip(store: &mut crate::entities::Store, content: &Content, slot: u8, id: i32) {
        let mut packet = Writer::default();
        packet.var(4);
        packet.byte(slot);
        packet.var(1);
        packet.var(id);
        packet.var(0);
        packet.var(0);
        store
            .apply(
                crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT,
                &mut Reader::new(&packet.0),
                content,
                3,
            )
            .unwrap();
    }

    #[test]
    fn received_animal_equipment_draws_and_strips_every_supported_species() {
        for (species, slot, kind) in [
            ("pig", 7, "pig_saddle"),
            ("strider", 7, "strider_saddle"),
            ("camel", 7, "camel_saddle"),
            ("camel_husk", 7, "camel_husk_saddle"),
            ("donkey", 7, "donkey_saddle"),
            ("mule", 7, "mule_saddle"),
            ("skeleton_horse", 7, "skeleton_horse_saddle"),
            ("zombie_horse", 7, "zombie_horse_saddle"),
            ("llama", 6, "llama_body"),
            ("trader_llama", 6, "llama_body"),
            ("happy_ghast", 6, "happy_ghast_body"),
            ("nautilus", 6, "nautilus_body"),
            ("nautilus", 7, "nautilus_saddle"),
            ("zombie_nautilus", 6, "nautilus_body"),
            ("zombie_nautilus", 7, "nautilus_saddle"),
        ] {
            let (mut store, models, mut content, mut cache) =
                humanoid_fixture(&format!("minecraft:{species}"));
            for slot in 2..=5 {
                store
                    .apply(
                        crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT,
                        &mut Reader::new(&[4, slot, 0]),
                        &content,
                        2,
                    )
                    .unwrap();
            }
            content.item_names.insert(5, "test:animal_equipment".into());
            content.insert_test_item_components(
                5,
                BTreeMap::from([(
                    "minecraft:equippable".into(),
                    json!({"slot":if slot == 7 {"saddle"} else {"body"}, "asset_id":"test:animal"}),
                )]),
            );
            let asset = json!({"layers":{(kind):[{"texture":"test:animal"}]}}).to_string();
            content.insert_test_resource("assets/test/equipment/animal.json", asset.as_bytes());
            content
                .insert_test_texture(&format!("test:entity/equipment/{kind}/animal"), region(17));
            equip(&mut store, &content, slot, 5);
            let entity = store.get(4).unwrap();
            let key = animal_overlay(&entity.type_name, usize::from(slot))
                .unwrap()
                .0;
            let rig = models.rig(key);
            let mut transforms = Vec::new();
            p::evaluate_rig(rig, entity, &models, &crate::protocol::content::Content::test_cube(), 3, 0., Mat4::IDENTITY, &mut transforms);
            let mut items = crate::ui::items::ItemModels::new();
            let frame = Frame {
                models: &models,
                content: &content,
                root: Mat4::IDENTITY,
                tick: 3,
                partial: 0.,
                light: [15.; 2],
            };
            let mut out = DynamicGeometry::default();
            let mut stats = p::PresentationStats::default();
            p::append_equipment(
                p::EquipmentPose { item_context: None, entity,
                rig,
                transforms: &transforms, },
                &mut items,
                &mut cache,
                frame,
                &mut out,
                &mut stats,
            )
            .unwrap();
            assert!(!out.vertices.is_empty(), "{species}:{slot}");
            assert!(
                out.vertices.iter().all(|v| v.texture_layer == 17),
                "{species}:{slot}"
            );
            assert!(
                stats.unrendered.is_empty(),
                "{species}: {:?}",
                stats.unrendered
            );
            store
                .apply(
                    crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT,
                    &mut Reader::new(&[4, slot, 0]),
                    &content,
                    4,
                )
                .unwrap();
            let mut stripped = DynamicGeometry::default();
            p::append_equipment(
                p::EquipmentPose { item_context: None, entity: store.get(4).unwrap(),
                rig,
                transforms: &transforms, },
                &mut items,
                &mut cache,
                frame,
                &mut stripped,
                &mut stats,
            )
            .unwrap();
            assert!(stripped.vertices.is_empty(), "stripped {species}:{slot}");
        }
    }

    #[test]
    fn unsupported_animal_equipment_is_still_reported_unrendered() {
        let (mut store, models, mut content, mut cache) = humanoid_fixture("minecraft:cow");
        content.item_names.insert(5, "test:saddle".into());
        content.insert_test_item_components(
            5,
            BTreeMap::from([(
                "minecraft:equippable".into(),
                json!({"slot":"saddle", "asset_id":"minecraft:saddle"}),
            )]),
        );
        equip(&mut store, &content, 7, 5);
        let entity = store.get(4).unwrap();
        let rig = models.rig("minecraft:cow#main");
        let mut transforms = Vec::new();
        p::evaluate_rig(rig, entity, &models, &crate::protocol::content::Content::test_cube(), 3, 0., Mat4::IDENTITY, &mut transforms);
        let mut stats = p::PresentationStats::default();
        let mut items = crate::ui::items::ItemModels::new();
        p::append_equipment(
            p::EquipmentPose { item_context: None, entity,
            rig,
            transforms: &transforms, },
            &mut items,
            &mut cache,
            Frame {
                models: &models,
                content: &content,
                root: Mat4::IDENTITY,
                tick: 3,
                partial: 0.,
                light: [15.; 2],
            },
            &mut DynamicGeometry::default(),
            &mut stats,
        )
        .unwrap();
        assert!(stats.unrendered.contains_key("minecraft:cow:equipment:7"));
    }

    #[test]
    fn player_back_uses_wings_asset_profile_priority_gliding_and_cape_exclusion() {
        let (mut store, mut models, mut content, mut cache) = humanoid_fixture("minecraft:player");
        models
            .schemas
            .get_mut("minecraft:player")
            .unwrap()
            .accessors
            .insert("DATA_SHARED_FLAGS_ID".into(), [1, 0]);
        content.item_names.insert(5, "test:wings".into());
        content.insert_test_item_components(
            5,
            BTreeMap::from([(
                "minecraft:equippable".into(),
                json!({"slot":"chest", "asset_id":"test:wings"}),
            )]),
        );
        content.insert_test_resource(
            "assets/test/equipment/wings.json",
            br#"{"layers":{"wings":[{"texture":"test:wings","use_player_texture":true}]}}"#,
        );
        content.insert_test_texture("test:entity/equipment/wings/wings", region(20));
        let frame = Frame {
            models: &models,
            content: &content,
            root: Mat4::IDENTITY,
            tick: 3,
            partial: 0.,
            light: [15.; 2],
        };
        let profile = super::super::super::profiles::ProfileTextures {
            skin: None,
            cape: Some(region(21)),
            elytra: Some(region(22)),
            model: None,
            secure: true,
        };
        let mut absent = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), None, frame, &mut absent)
            .unwrap();
        assert!(absent.vertices.is_empty());
        let mut cape = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), Some(profile), frame, &mut cape)
            .unwrap();
        assert!(!cape.vertices.is_empty());
        assert!(cape.vertices.iter().all(|v| v.texture_layer == 21));
        equip(&mut store, &content, 4, 5);
        let mut folded = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), Some(profile), frame, &mut folded)
            .unwrap();
        assert!(!folded.vertices.is_empty());
        assert!(folded.vertices.iter().all(|v| v.texture_layer == 22));
        let mut no_profile = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), None, frame, &mut no_profile)
            .unwrap();
        assert!(no_profile.vertices.iter().all(|v| v.texture_layer == 20));
        let mut cape_wings = DynamicGeometry::default();
        cache
            .append_player_back(
                store.get(4).unwrap(),
                Some(super::super::super::profiles::ProfileTextures {
                    elytra: None,
                    ..profile
                }),
                frame,
                &mut cape_wings,
            )
            .unwrap();
        assert!(cape_wings.vertices.iter().all(|v| v.texture_layer == 21));
        store
            .get_mut(4)
            .unwrap()
            .metadata
            .insert(1, crate::entities::Metadata::Byte(-128));
        let mut gliding = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), Some(profile), frame, &mut gliding)
            .unwrap();
        let width = |out: &DynamicGeometry| {
            out.vertices
                .iter()
                .map(|v| v.position[0].abs())
                .fold(0., f32::max)
        };
        assert!(width(&gliding) > width(&folded) + 0.2);
        store
            .apply(
                crate::protocol::packet_ids::play::clientbound::SET_EQUIPMENT,
                &mut Reader::new(&[4, 4, 0]),
                &content,
                4,
            )
            .unwrap();
        let mut stripped = DynamicGeometry::default();
        cache
            .append_player_back(store.get(4).unwrap(), Some(profile), frame, &mut stripped)
            .unwrap();
        assert_eq!(stripped.vertices.len(), cape.vertices.len());
        assert!(stripped.vertices.iter().all(|v| v.texture_layer == 21));
    }

    #[test]
    fn received_animal_body_equipment_follows_supported_adult_baby_transition() {
        for (species, kind) in [("wolf", "wolf_body"), ("llama", "llama_body")] {
            let (mut store, mut models, mut content, mut cache) =
                humanoid_fixture(&format!("minecraft:{species}"));
            models.schemas.get_mut(&format!("minecraft:{species}")).unwrap()
                .accessors.insert("DATA_BABY_ID".into(), [23, 8]);
            content.item_names.insert(5, "test:body".into());
            content.insert_test_item_components(5, BTreeMap::from([(
                "minecraft:equippable".into(), json!({"slot":"body","asset_id":"test:body"})
            )]));
            content.insert_test_resource("assets/test/equipment/body.json",
                json!({"layers":{(kind):[{"texture":"test:body"}]}}).to_string().as_bytes());
            content.insert_test_texture(&format!("test:entity/equipment/{kind}/body"), region(30));
            equip(&mut store, &content, 6, 5);
            for baby in [true, false, true] {
                store.get_mut(4).unwrap().metadata.insert(23, crate::entities::Metadata::Bool(baby));
                let entity = store.get(4).unwrap();
                let mut out = DynamicGeometry::default();
                assert!(cache.append(entity, 6, entity.equipment_value(6).unwrap(), Frame {
                    models:&models, content:&content, root:Mat4::IDENTITY, tick:3, partial:0.,light:[15.;2],
                }, &mut out).unwrap());
                assert_eq!(out.vertices.is_empty(), baby, "{species}:{baby}");
                assert!(out.vertices.iter().all(|v| v.texture_layer == 30));
            }
        }
    }
}
