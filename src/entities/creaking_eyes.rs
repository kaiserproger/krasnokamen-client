//! Emissive creaking eyes attached to the already animated head.
use super::{Entity, Models, presentation as p, render_living::Modern};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content},
};
use glam::Mat4;

/// `head` is the final base head-bone matrix, including its animated pivot and rotation.
/// The caller must invoke this independently of base-model invisibility. `transforms`
/// is scratch space and is replaced only when the eye layer is visible.
#[allow(clippy::too_many_arguments)]
pub(super) fn append(
    models: &Models,
    content: &Content,
    entity: &Entity,
    head: Mat4,
    transforms: &mut Vec<Mat4>,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let visible = if p::boolean(models, entity, "IS_TEARING_DOWN") {
        match entity.source.as_ref().and_then(|source| source.living.as_ref()).map(|living| &living.modern) {
            Some(Modern::Creaking { eyes_glowing, .. }) => *eyes_glowing,
            _ => return Err("creaking eyes: living creaking source state missing".into()),
        }
    } else {
        p::boolean(models, entity, "IS_ACTIVE")
    };
    if !visible {
        return Ok(());
    }

    let key = "minecraft:creaking_eyes#main";
    if !models.has_authored_rig(key) {
        return Err(format!("creaking eyes rig not activated or authored: {key}"));
    }
    let texture = p::texture(content, "entity/creaking/creaking_eyes")?;
    let rig = models.rig(key);
    transforms.clear();
    // Both eye rig bones are identity transforms in the animated head frame.
    // Evaluating the Head role would apply its look rotation a second time.
    transforms.resize(rig.bones.len(), head);
    p::emit_rig(rig, transforms, texture, [1.; 3], EntityMaterial::Eyes, 1., light, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{Metadata, animation_tests, resources::{Bone, BoneRole, Rig}, source_state},
        protocol::content::TextureRegion,
    };
    use glam::Vec3;
    use std::collections::BTreeMap;

    fn fixture() -> (Models, Entity, Content) {
        let (mut models, mut entity) = animation_tests::fixture();
        let mut schema = models.schemas.remove(&entity.type_name).unwrap();
        let motion = models.motion.remove(&entity.type_name).unwrap();
        schema.accessors.insert("IS_ACTIVE".into(), [8, 0]);
        schema.accessors.insert("IS_TEARING_DOWN".into(), [9, 0]);
        entity.type_name = "minecraft:creaking".into();
        models.schemas.insert(entity.type_name.clone(), schema);
        models.motion.insert(entity.type_name.clone(), motion);
        entity.source = Some(source_state::SourceState::new(&entity, &models).unwrap());
        models.rigs.insert("minecraft:creaking_eyes#main".into(), Rig {
            bones: vec![Bone {
                parent: None,
                pivot: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::ONE,
                role: BoneRole::Head,
                name: "eyes".into(),
                quads: vec![[
                    [0., 0., 0., 0., 0.],
                    [0., 1., 0., 0., 1.],
                    [1., 1., 0., 1., 1.],
                    [1., 0., 0., 1., 0.],
                ]],
                normals: vec![[0., 0., 1.]],
            }],
            part_by_name: BTreeMap::new(),
        });
        let mut content = Content::test_cube();
        content.insert_test_texture("minecraft:entity/creaking/creaking_eyes", TextureRegion {
            uv: [0., 0., 1., 1.], layer: 11, width: 64, height: 64,
        });
        (models, entity, content)
    }

    fn render(models: &Models, entity: &Entity, content: &Content, head: Mat4) -> Result<DynamicGeometry> {
        let mut out = DynamicGeometry::default();
        append(models, content, entity, head, &mut Vec::new(), [0.; 2], &mut out)?;
        Ok(out)
    }

    #[test]
    fn active_invisible_eyes_use_the_animated_head_and_eyes_material() {
        let (models, mut entity, content) = fixture();
        entity.metadata.insert(8, Metadata::Bool(true));
        entity.metadata.insert(0, Metadata::Byte(32));
        let head = Mat4::from_translation(Vec3::new(3., 4., 5.)) * Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let out = render(&models, &entity, &content, head).unwrap();
        assert_eq!(out.indices[EntityMaterial::Eyes as usize].len(), 6);
        assert_eq!(out.vertices.len(), 4);
        let p = Vec3::from_array(out.vertices[1].position);
        assert!((p - head.transform_point3(Vec3::Y)).length() < 1e-5);
    }

    #[test]
    fn teardown_flicker_overrides_active_and_missing_state_is_error() {
        let (models, mut entity, content) = fixture();
        assert!(render(&models, &entity, &content, Mat4::IDENTITY).unwrap().vertices.is_empty());
        entity.metadata.insert(8, Metadata::Bool(true));
        entity.metadata.insert(9, Metadata::Bool(true));
        assert!(render(&models, &entity, &content, Mat4::IDENTITY).unwrap().vertices.is_empty());
        let Modern::Creaking { eyes_glowing, .. } = &mut entity.source.as_mut().unwrap().living.as_mut().unwrap().modern else { panic!("creaking state"); };
        *eyes_glowing = true;
        entity.metadata.insert(8, Metadata::Bool(false));
        assert!(!render(&models, &entity, &content, Mat4::IDENTITY).unwrap().vertices.is_empty());
        entity.source = None;
        assert!(render(&models, &entity, &content, Mat4::IDENTITY).unwrap_err().contains("source state"));
    }

    #[test]
    fn missing_active_layer_assets_are_errors() {
        let (mut models, mut entity, content) = fixture();
        entity.metadata.insert(8, Metadata::Bool(true));
        let err = render(&models, &entity, &Content::test_cube(), Mat4::IDENTITY).unwrap_err();
        assert!(err.contains("entity/creaking/creaking_eyes"));
        models.rigs.remove("minecraft:creaking_eyes#main");
        // A compiled-in authored rig is valid as well; a placeholder is not.
        models.rigs.insert("minecraft:creaking_eyes#main".into(), super::super::solaris_models::placeholder(1., 1.));
        let err = render(&models, &entity, &content, Mat4::IDENTITY).unwrap_err();
        assert!(err.contains("minecraft:creaking_eyes#main"));
    }
}
