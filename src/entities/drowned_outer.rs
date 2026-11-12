//! Drowned clothing layer, using Krasnokamen-authored rigs and the activated entity textures.
use super::{Entity, Models, presentation as p};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content},
};
use glam::Mat4;

pub(super) struct Pose {
    pub tick: u64,
    pub partial: f32,
    pub root: Mat4,
}

/// Append the visible drowned clothing after its body. The caller handles invisibility.
/// `transforms` is scratch space and is replaced with the clothing rig's transforms.
pub(super) fn append_drowned_outer(
    models: &Models,
    content: &Content,
    entity: &Entity,
    pose: Pose,
    transforms: &mut Vec<Mat4>,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let Pose { tick, partial, root } = pose;
    let (key, texture) = if p::boolean(models, entity, "DATA_BABY_ID") {
        ("minecraft:drowned_outer_baby#main", "entity/zombie/drowned_outer_layer_baby")
    } else {
        ("minecraft:drowned_outer#main", "entity/zombie/drowned_outer_layer")
    };
    if !models.has_authored_rig(key) {
        return Err(format!("drowned clothing rig not activated or authored: {key}"));
    }
    let texture = p::texture(content, texture)?;
    let rig = models.rig(key);
    p::evaluate_rig(rig, entity, models, content, tick, partial, root, transforms);
    p::emit_rig(rig, transforms, p::RigStyle { tex: texture, color: [1.; 3], layer: EntityMaterial::EntityTranslucent, alpha: 1., light }, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{animation_tests, resources::{Bone, BoneRole, Rig}},
        protocol::content::TextureRegion,
    };
    use glam::Vec3;
    use std::collections::BTreeMap;

    fn rig(x: f32) -> Rig {
        Rig {
            bones: vec![Bone {
                parent: None,
                pivot: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::ONE,
                role: BoneRole::RightArm,
                name: "right_arm".into(),
                quads: vec![[
                    [x, 0., 0., 0., 0.],
                    [x, 1., 0., 0., 1.],
                    [x + 1., 1., 0., 1., 1.],
                    [x + 1., 0., 0., 1., 0.],
                ]],
                normals: vec![[0., 0., 1.]],
            }],
            part_by_name: BTreeMap::new(),
        }
    }

    fn fixture() -> (Models, Entity, Content) {
        let (mut models, mut entity) = animation_tests::fixture();
        let mut schema = models.schemas.remove(&entity.type_name).unwrap();
        schema.accessors.insert("DATA_BABY_ID".into(), [0, 8]);
        entity.type_name = "minecraft:drowned".into();
        entity.source = None;
        models.schemas.insert(entity.type_name.clone(), schema);
        models.rigs.insert("minecraft:drowned_outer#main".into(), rig(2.));
        models.rigs.insert("minecraft:drowned_outer_baby#main".into(), rig(5.));
        let mut content = Content::test_cube();
        for (path, layer) in [
            ("entity/zombie/drowned_outer_layer", 11),
            ("entity/zombie/drowned_outer_layer_baby", 12),
        ] {
            content.insert_test_texture(path, TextureRegion {
                uv: [0., 0., 1., 1.], layer, width: 64, height: 64,
            });
        }
        (models, entity, content)
    }

    #[test]
    fn missing_clothing_texture_is_an_error() {
        let (models, entity, _) = fixture();
        let mut out = DynamicGeometry::default();
        let error = append_drowned_outer(&models, &Content::test_cube(), &entity,
            Pose { tick: 0, partial: 0., root: Mat4::IDENTITY },
            &mut Vec::new(), [0.; 2], &mut out).unwrap_err();
        assert!(error.contains("entity/zombie/drowned_outer_layer"));
        assert!(out.vertices.is_empty());
    }
}
