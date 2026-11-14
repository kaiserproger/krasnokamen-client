use super::*;

// 26.3 ItemFrameRenderer uses the packet block anchor, direction rotation and
// centered block-model transform; frame geometry comes from the user's pack.
fn frame_root(anchor: [f64; 3], origin: [f64; 3], direction: i32) -> Result<Mat4> {
    let (pitch, yaw) = match direction {
        0 => (std::f32::consts::FRAC_PI_2, std::f32::consts::PI),
        1 => (-std::f32::consts::FRAC_PI_2, std::f32::consts::PI),
        2 => (0., 0.),
        3 => (0., std::f32::consts::PI),
        4 => (0., std::f32::consts::FRAC_PI_2),
        5 => (0., -std::f32::consts::FRAC_PI_2),
        _ => return Err("item frame direction invalid".into()),
    };
    Ok(translation(anchor.map(|coordinate| coordinate.floor() + 0.5), origin)
        * Mat4::from_rotation_x(pitch) * Mat4::from_rotation_y(yaw))
}

// The edge sends hanging frames at the floored attachment cell, not their
// physical center. Reconstruct the native server's center within that cell.
pub(in crate::entities) fn center(position: [f64; 3], direction: i32) -> Result<[f64; 3]> {
    let face = match direction {
        0 => [0., -1., 0.], 1 => [0., 1., 0.],
        2 => [0., 0., -1.], 3 => [0., 0., 1.],
        4 => [-1., 0., 0.], 5 => [1., 0., 0.],
        _ => return Err("item frame direction invalid".into()),
    };
    Ok(std::array::from_fn(|i| position[i].floor() + 0.5 - face[i] * (15. / 32.)))
}

pub(super) struct ItemFrameResources<'a> {
    pub cache: &'a mut Cache,
    pub items: &'a mut ItemModels,
    pub maps: crate::ui::MapRenderView<'a>,
}

pub(super) fn append(
    entity: &Entity,
    models: &Models,
    resources: ItemFrameResources<'_>,
    content: &Content,
    frame: p::EntityDrawFrame,
    clock_context: Option<&ItemContext>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let p::EntityDrawFrame { origin, transform, light, .. } = frame;
    let ItemFrameResources { cache, items, maps } = resources;
    let direction = match p::value(models, entity, "DATA_DIRECTION") {
        Some(value) => value.int().ok_or("item frame direction metadata invalid")?,
        None => entity.spawn_data,
    };
    let root = frame_root(transform.position, origin, direction)?;
    let stack = match p::value(models, entity, "DATA_ITEM") {
        Some(Metadata::Item(stack)) if !stack.empty() => Some(stack),
        _ => None,
    };
    let map = stack.and_then(|stack| stack.component("minecraft:map_id"))
        .map(|value| value.as_i64().and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id >= 0).ok_or("invalid framed map id"))
        .transpose()?
        .and_then(|id| maps.maps.get(&id));
    let invisible = p::number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 32 != 0;
    let glow = entity.type_name == "minecraft:glow_item_frame";
    // Clocks in frames follow received world time like the dropped clock does.
    let item_model = stack.filter(|_| map.is_none())
        .map(|stack| match clock_context {
            Some(clock) => {
                let mut state = clock.clone();
                state.display = "fixed".into();
                items.model_with(content, stack, &state)
            }
            None => items.model(content, stack, "fixed"),
        })
        .transpose()?;
    if !invisible {
        let model_name = match (glow, map.is_some()) {
            (false, false) => "minecraft:block/item_frame",
            (false, true) => "minecraft:block/item_frame_map",
            (true, false) => "minecraft:block/glow_item_frame",
            (true, true) => "minecraft:block/glow_item_frame_map",
        };
        let model = cache.model(content, model_name)?;
        // Light is [sky, block]; a glow frame raises only its block light to five.
        let mut body_light = light;
        if glow { body_light[1] = body_light[1].max(5.); }
        p::append_baked(model, root * Mat4::from_translation(Vec3::splat(-0.5)), body_light, out)?;
    }
    let rotation = p::number(models, entity, "DATA_ROTATION").unwrap_or(0).rem_euclid(8);
    if let Some(map) = map {
        // 26.3 ItemFrameRenderer.submit: map rotations are quarter turns,
        // followed by 180 degrees, 1/128 scale and (-64,-64,-1) translation.
        let surface = root
            * Mat4::from_translation(Vec3::new(0., 0., if invisible { 0.5 } else { 0.4375 }))
            * Mat4::from_rotation_z((rotation % 4) as f32 * std::f32::consts::FRAC_PI_2 + std::f32::consts::PI)
            * Mat4::from_scale(Vec3::splat(1. / 128.))
            * Mat4::from_translation(Vec3::new(-64., -64., -1.));
        map.append_world_geometry(content, maps.text, surface,
            if glow { [15., 13.] } else { light }, true, out)?;
    } else if let (Some(stack), Some(model)) = (stack, item_model) {
        let item = root
            * Mat4::from_translation(Vec3::new(0., 0., if invisible { 0.5 } else { 0.4375 }))
            * Mat4::from_rotation_z(rotation as f32 * std::f32::consts::FRAC_PI_4)
            * Mat4::from_scale(Vec3::splat(0.5));
        p::append_baked_stack(model, item * p::item_matrix(model, "fixed"),
            if glow { [15.; 2] } else { light }, content, stack, out)?;
    }
    Ok(if invisible && stack.is_none() { Handled::NoGeometry } else { Handled::Rendered })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_model_front_faces_outward_on_all_six_surfaces() {
        for (direction, outward) in [(0, Vec3::NEG_Y), (1, Vec3::Y),
            (2, Vec3::NEG_Z), (3, Vec3::Z), (4, Vec3::NEG_X), (5, Vec3::X)] {
            let anchor = [4., 5., 6.];
            let root = frame_root(anchor, anchor, direction).unwrap();
            assert!((root.transform_vector3(Vec3::NEG_Z) - outward).length() < 0.00001);
            assert!((root.transform_point3(Vec3::ZERO) - Vec3::splat(0.5)).length() < 0.00001);
            // The hitbox center is the middle of the block model's back plate (z 15..16).
            let plate = root.transform_point3(Vec3::new(0., 0., 15. / 32.));
            let hitbox = center(anchor, direction).unwrap();
            let hitbox = Vec3::from_array(std::array::from_fn(|i| (hitbox[i] - anchor[i]) as f32));
            assert!((plate - hitbox).length() < 0.00001, "{direction}: {plate} {hitbox}");
            let actual = Vec3::from_array(center([-3., 64., 8.], direction).unwrap().map(|v| v as f32));
            let expected = Vec3::new(-2.5, 64.5, 8.5) - outward * (15. / 32.);
            assert!((actual - expected).length() < 1e-5);
        }
        assert!(frame_root([0.; 3], [0.; 3], 6).is_err());
        assert!(center([0.; 3], 6).is_err());
    }

    #[test]
    fn frame_hitbox_matches_visible_center_and_received_direction() {
        let (mut models, mut entity) = super::super::super::animation_tests::fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_DIRECTION".into(), [8, 12]);
        entity.type_name = "minecraft:item_frame".into();
        models.schemas.insert(entity.type_name.clone(), schema);
        for direction in 0..6 {
            entity.spawn_data = (direction + 1) % 6;
            entity.metadata.insert(8, Metadata::Direction(direction as u8));
            let normal = super::super::super::physical::direction(direction).unwrap();
            let center = center([0.; 3], direction).unwrap();
            let bounds = super::super::super::physical::bounds(&models, &entity, 0, 0.).unwrap();
            let ray_origin = std::array::from_fn(|i| center[i] + normal[i] * 2.);
            let ray = normal.map(|value| -value);
            assert!(crate::player::collision::ray_box(ray_origin, ray, bounds, 4.).is_some());
            let tangent = normal.iter().position(|value| *value == 0.).unwrap();
            let mut missed = ray_origin;
            missed[tangent] += 0.376;
            assert!(crate::player::collision::ray_box(missed, ray, bounds, 4.).is_none());
        }
    }

    pub(super) fn frame_fixture(kind: &str) -> (Models, Entity, Content, Cache, ItemModels, BakedModel) {
        let (mut models, mut entity) = super::super::super::animation_tests::fixture();
        let mut schema = models.schemas.remove("minecraft:pig").unwrap();
        schema.accessors.insert("DATA_ITEM".into(), [9, 7]);
        schema.accessors.insert("DATA_ROTATION".into(), [10, 1]);
        schema.accessors.insert("DATA_SHARED_FLAGS_ID".into(), [0, 0]);
        entity.type_name = kind.into();
        entity.spawn_data = 2;
        models.schemas.insert(entity.type_name.clone(), schema);
        let mut content = Content::test_cube();
        content.item_names.insert(1, "test:held".into());
        let cube = content.models[0].clone();
        let mut items = ItemModels::new();
        items.insert_test_model("test:held", cube.clone());
        (models, entity, content, Cache::default(), items, cube)
    }

    pub(super) fn held(id: u32) -> Stack {
        let mut stack = Stack::default();
        stack.id = id;
        stack.count = 1;
        stack.components = std::sync::Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(), serde_json::json!("test:held"),
        )]));
        stack
    }

    #[test]
    fn occupied_frame_renders_item_rotation_and_invisibility() {
        let (models, mut entity, content, mut cache, mut items, cube) =
            frame_fixture("minecraft:item_frame");
        cache.models.insert("minecraft:block/item_frame".into(), cube);
        let render = |entity: &Entity, cache: &mut Cache, items: &mut ItemModels| {
            let mut out = DynamicGeometry::default();
            let handled = append(entity, &models, ItemFrameResources { cache, items, maps: crate::ui::MapRenderFixture::empty().view() }, &content,
                p::EntityDrawFrame { origin: [0.; 3], transform: entity.presented_transform(0, 0.), tick: 0, partial: 0., light: [8., 12.], parent: None },
                None, &mut out).unwrap();
            (handled, out)
        };
        let (handled, empty) = render(&entity, &mut cache, &mut items);
        assert_eq!(handled, Handled::Rendered);
        entity.metadata.insert(9, Metadata::Item(held(1)));
        let (_, filled) = render(&entity, &mut cache, &mut items);
        assert!(filled.vertices.len() > empty.vertices.len());
        entity.metadata.insert(10, Metadata::Int(2));
        let (_, rotated) = render(&entity, &mut cache, &mut items);
        // Each received step turns the item 45 degrees about the frame normal.
        let center = Vec3::new(0.5, 0.5, 0.9375);
        for (before, after) in filled.vertices[empty.vertices.len()..].iter()
            .zip(&rotated.vertices[empty.vertices.len()..])
        {
            let expected = center + Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2)
                .transform_vector3(Vec3::from_array(before.position) - center);
            assert!((expected - Vec3::from_array(after.position)).length() < 1e-5);
        }
        entity.metadata.insert(0, Metadata::Byte(0x20));
        let (handled, invisible) = render(&entity, &mut cache, &mut items);
        assert_eq!(handled, Handled::Rendered);
        assert_eq!(invisible.vertices.len(), filled.vertices.len() - empty.vertices.len());
        entity.metadata.remove(&9);
        let (handled, nothing) = render(&entity, &mut cache, &mut items);
        assert_eq!(handled, Handled::NoGeometry);
        assert!(nothing.vertices.is_empty());
    }

    #[test]
    fn glow_frame_raises_block_light_and_unobserved_maps_remain_items() {
        let (models, mut entity, mut content, mut cache, mut items, cube) =
            frame_fixture("minecraft:glow_item_frame");
        let mut map_body = cube.clone();
        for quad in &mut map_body.quads {
            quad.texture_layer = 77;
        }
        cache.models.insert("minecraft:block/glow_item_frame".into(), cube.clone());
        cache.models.insert("minecraft:block/glow_item_frame_map".into(), map_body);
        content.item_names.insert(2, "minecraft:filled_map".into());
        let body = cube.quads.len() * 4;
        for stack in [held(1), held(2)] {
            entity.metadata.insert(9, Metadata::Item(stack));
            let mut out = DynamicGeometry::default();
            append(&entity, &models, ItemFrameResources { cache: &mut cache, items: &mut items, maps: crate::ui::MapRenderFixture::empty().view() }, &content,
                p::EntityDrawFrame { origin: [0.; 3], transform: entity.presented_transform(0, 0.), tick: 0, partial: 0., light: [8., 2.], parent: None },
                None, &mut out).unwrap();
            assert!(out.vertices[..body].iter().all(|v| v.light == [8., 5.]));
            assert!(out.vertices[..body].iter().all(|v| v.texture_layer != 77),
                "an unobserved map uses the ordinary item frame, never invented pixels");
            assert!(out.vertices[body..].iter().all(|v| v.light == [15., 15.]));
        }
    }
}

#[cfg(test)]
#[path = "item_frame_map_tests.rs"]
mod map_tests;
