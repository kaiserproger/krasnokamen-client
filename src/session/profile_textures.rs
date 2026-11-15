//! One asynchronous resolver serves PlayerInfo and skull-property consumers.
use super::Session;
use crate::{
    entities::ProfileTextureTarget,
    profile_textures::{ProfileTextureCompletion, Resolver, ResolverCompletion, SubmitError},
    protocol::Result,
};
use serde_json::json;

impl Session {
    pub(super) fn process_profile_textures(&mut self) -> bool {
        if !self.presentation || self.phase != "play" || self.content.is_none() {
            return false;
        }
        if !self.profile_resolver_started {
            if self.entities.profile_requests.is_empty()
                && !self
                    .content
                    .as_ref()
                    .expect("checked content")
                    .head_textures
                    .has_requests()
            {
                return false;
            }
            self.profile_resolver_started = true;
            match crate::singleplayer::data_root()
                .and_then(|root| Resolver::new(root.join("profile-textures"), self.wake_callback()))
            {
                Ok(resolver) => {
                    resolver.set_fence(self.profile_texture_epoch);
                    self.profile_resolver = Some(resolver);
                }
                Err(error) => {
                    self.event(json!({"type":"profile_texture_error", "error":error}));
                    return true;
                }
            }
        }
        let mut changed = false;
        while let Some(completion) = self
            .profile_resolver
            .as_ref()
            .and_then(Resolver::try_completion)
        {
            let result = match completion {
                ResolverCompletion::ServicesKeys(result) => result.map(|()| false),
                ResolverCompletion::Texture(completion) => self.publish_profile_texture(completion),
            };
            match result {
                Ok(published) => changed |= published,
                Err(error) => {
                    // Cosmetic failures retain the default skin, never disconnect the player.
                    self.event(json!({"type":"profile_texture_error", "error":error}));
                    changed = true;
                }
            }
        }
        if changed {
            self.changed_state();
        }
        let Some(resolver) = &self.profile_resolver else {
            return changed;
        };
        let content = self.content.as_ref().expect("checked content");
        while let Some(request) = content
            .head_textures
            .pop_request()
            .or_else(|| self.entities.pop_profile_texture_request())
        {
            match resolver.submit(request, self.profile_texture_epoch) {
                Ok(()) => {}
                Err(SubmitError::Full(request)) => {
                    match &request.target {
                        ProfileTextureTarget::Player(uuid) => {
                            self.entities.profile_requests.insert(uuid.clone(), request);
                        }
                        ProfileTextureTarget::Head(_) => content.head_textures.retry(request),
                    }
                    break;
                }
                Err(SubmitError::Closed(request) | SubmitError::Stale(request)) => {
                    if let ProfileTextureTarget::Head(key) = request.target {
                        content.head_textures.fail(&key, request.generation);
                    }
                    self.event(json!({"type":"profile_texture_error", "error":"resolver admission unavailable"}));
                    break;
                }
            }
        }
        changed
    }

    fn publish_profile_texture(&mut self, completion: ProfileTextureCompletion) -> Result<bool> {
        let head = match completion.target {
            ProfileTextureTarget::Head(key) => Some(key),
            ProfileTextureTarget::Player(_) => None,
        };
        let generation = completion.generation;
        let result = self.install_profile_texture(completion);
        if result.is_err()
            && let Some(key) = head
            && let Some(content) = &self.content
        {
            content.head_textures.fail(&key, generation);
        }
        result
    }

    fn install_profile_texture(&mut self, completion: ProfileTextureCompletion) -> Result<bool> {
        if completion.resource_generation != self.profile_texture_epoch {
            return Ok(false);
        }
        let Some(content) = &self.content else {
            return Ok(false);
        };
        if let ProfileTextureTarget::Player(uuid) = &completion.target {
            if self
                .entities
                .player_profile(uuid)
                .is_none_or(|p| p.generation != completion.generation)
            {
                return Ok(false);
            }
        }
        if let ProfileTextureTarget::Head(key) = &completion.target
            && !content.head_textures.is_current(key, completion.generation)
        {
            // An evicted request may finish after the same key has been admitted again.
            return Ok(false);
        }
        let textures = completion.result?;
        let mut install = |texture: Option<crate::profile_textures::ResolvedTexture>| {
            texture
                .map(|texture| -> Result<_> {
                    // Content-addressed sprites are shared by all consumers and uploaded only once.
                    if let Some(region) = content.texture(&texture.id) {
                        return Ok(region);
                    }
                    let (region, patch) = content.install_dynamic_texture(
                        &texture.id,
                        &texture.rgba,
                        texture.width,
                        texture.height,
                    )?;
                    self.scene.texture_patches.push(patch);
                    Ok(region)
                })
                .transpose()
        };
        let skin = install(textures.skin)?;
        let published = match completion.target {
            ProfileTextureTarget::Head(key) => {
                let skin = skin.ok_or("head profile contains no skin")?;
                let published = content.head_textures.publish(&key, completion.generation, skin);
                if published {
                    // Refresh the shared item selection snapshots (GUI, hands, worn/crowd).
                    self.entities.profiles_revision =
                        self.entities.profiles_revision.wrapping_add(1);
                }
                published
            }
            ProfileTextureTarget::Player(uuid) => {
                let cape = install(textures.cape)?;
                let elytra = install(textures.elytra)?;
                self.entities.set_profile_texture(
                    &uuid,
                    skin,
                    cape,
                    elytra,
                    textures.model,
                    textures.secure,
                    completion.generation,
                )?
            }
        };
        Ok(published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        profile_textures::{ResolvedProfileTextures, ResolvedTexture},
        protocol::content::Content,
    };
    use std::sync::Arc;

    #[test]
    fn skull_parity_completion_publishes_once_invalidates_items_and_rejects_stale_atlas() {
        let mut session = Session::new_mode("skull-parity".into(), false);
        let mut content = Content::test_cube();
        content.width = 256;
        content.height = 256;
        session.install_atlas(&content);
        let profile = json!({"properties":[{"name":"textures","value":"own"}]});
        assert!(content.head_textures.skin(&profile).is_err());
        let request = content.head_textures.pop_request().unwrap();
        session.content = Some(content);
        let completion = |epoch| ProfileTextureCompletion {
            target: request.target.clone(),
            generation: request.generation,
            resource_generation: epoch,
            result: Ok(ResolvedProfileTextures {
                skin: Some(ResolvedTexture {
                    id: "profile:skin_authored".into(),
                    rgba: Arc::from(vec![255; 64 * 64 * 4]),
                    width: 64,
                    height: 64,
                }),
                cape: None,
                elytra: None,
                model: None,
                secure: false,
            }),
        };
        let epoch = session.profile_texture_epoch;
        let revision = session.entities.profiles_revision;
        assert!(
            !session
                .publish_profile_texture(completion(epoch - 1))
                .unwrap()
        );
        assert!(session.scene.texture_patches.is_empty());
        assert!(session.publish_profile_texture(completion(epoch)).unwrap());
        assert_eq!(session.entities.profiles_revision, revision + 1);
        assert_eq!(session.scene.texture_patches.len(), 1);
        assert!(
            session
                .content
                .as_ref()
                .unwrap()
                .head_textures
                .skin(&profile)
                .unwrap()
                .is_some()
        );
        assert!(session.publish_profile_texture(completion(epoch)).unwrap());
        assert_eq!(
            session.scene.texture_patches.len(),
            1,
            "deduplicate texture upload"
        );
        let content = session.content.take().unwrap();
        session.install_atlas(&content);
        session.content = Some(content);
        assert!(!session.publish_profile_texture(completion(epoch)).unwrap());
        assert!(session.scene.texture_patches.is_empty());
        assert!(
            session
                .content
                .as_ref()
                .unwrap()
                .head_textures
                .skin(&profile)
                .is_err()
        );
        assert!(
            session
                .content
                .as_ref()
                .unwrap()
                .head_textures
                .has_requests()
        );
        session.disconnect();
        assert!(session.content.is_none());
        assert!(!session.publish_profile_texture(completion(epoch)).unwrap());
    }

    #[test]
    fn skull_parity_dynamic_atlas_rejects_overflow_instead_of_unallocated_layers() {
        let mut content = Content::test_cube();
        content.width = 128;
        content.height = 128;
        let pixels = vec![255; 64 * 64 * 4];
        content
            .install_dynamic_texture("profile:first", &pixels, 64, 64)
            .unwrap();
        assert!(
            content
                .install_dynamic_texture("profile:second", &pixels, 64, 64)
                .is_err()
        );
        content.reset_profile_textures();
        content.width = 64;
        assert!(
            content
                .install_dynamic_texture("profile:small", &pixels, 64, 64)
                .is_err()
        );
    }

    #[test]
    fn head_registry_evicted_completion_cannot_allocate_atlas_or_poison_readmission() {
        let mut session = Session::new_mode("head-registry".into(), false);
        let content = Content::test_cube();
        let profile = |index| json!({"properties":[{"name":"textures","value":format!("head-{index}")}]});
        assert!(content.head_textures.skin(&profile(0)).is_err());
        let old = content.head_textures.pop_request().unwrap();
        for index in 1..=256 {
            assert!(content.head_textures.skin(&profile(index)).is_err());
        }
        assert!(content.head_textures.skin(&profile(0)).is_err());
        session.content = Some(content);
        assert!(!session.publish_profile_texture(ProfileTextureCompletion {
            target: old.target, generation: old.generation,
            resource_generation: session.profile_texture_epoch,
            result: Ok(ResolvedProfileTextures {
                skin: Some(ResolvedTexture {
                    id: "profile:evicted".into(),
                    rgba: Arc::from(vec![255; 64 * 64 * 4]),
                    width: 64, height: 64,
                }),
                cape: None, elytra: None,
                model: None,
                secure: false,
            }),
        }).unwrap());
        assert!(session.scene.texture_patches.is_empty());
        assert!(session.content.as_ref().unwrap().texture("profile:evicted").is_none());
    }

    #[test]
    fn head_registry_failed_completions_preserve_preloaded_skin() {
        let mut session = Session::new_mode("head-registry".into(), false);
        let mut content = Content::test_cube();
        content.width = 256;
        content.height = 256;
        let profile = json!({"properties":[{"name":"textures","value":"preloaded"}]});
        assert!(content.head_textures.skin(&profile).is_err());
        let request = content.head_textures.pop_request().unwrap();
        session.content = Some(content);
        assert!(session.publish_profile_texture(ProfileTextureCompletion {
            target: request.target, generation: request.generation,
            resource_generation: session.profile_texture_epoch,
            result: Ok(ResolvedProfileTextures {
                skin: Some(ResolvedTexture {
                    id: "profile:preloaded".into(),
                    rgba: Arc::from(vec![255; 64 * 64 * 4]),
                    width: 64, height: 64,
                }),
                cape: None, elytra: None, model: None, secure: false,
            }),
        }).unwrap());
        for index in 0..261 {
            let content = session.content.as_ref().unwrap();
            assert!(content.head_textures.skin(&json!({"properties":[
                {"name":"textures","value":format!("failed-{index}")}
            ]})).is_err());
            let request = content.head_textures.pop_request().unwrap();
            assert!(session.publish_profile_texture(ProfileTextureCompletion {
                target: request.target, generation: request.generation,
                resource_generation: session.profile_texture_epoch,
                result: if index % 2 == 0 {
                    Err("invalid property".into())
                } else {
                    Ok(ResolvedProfileTextures {
                        skin: None, cape: None, elytra: None, model: None, secure: false,
                    })
                },
            }).is_err());
        }
        assert!(session.content.as_ref().unwrap().head_textures.skin(&profile).unwrap().is_some());
        assert_eq!(session.scene.texture_patches.len(), 1);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::protocol::content::Content;

    #[test]
    fn failed_head_registry_saturation_admits_new_valid_profile() {
        let mut session = Session::new_mode("head-registry".into(), false);
        session.content = Some(Content::test_cube());
        for index in 0..256 {
            let content = session.content.as_ref().unwrap();
            assert!(content.head_textures.skin(&json!({"properties":[
                {"name":"textures","value":format!("invalid-{index}")}
            ]})).is_err());
            let request = content.head_textures.pop_request().unwrap();
            // Actual production completion path: a decoding error, not a download/atlas allocation.
            assert!(session.publish_profile_texture(ProfileTextureCompletion {
                target: request.target, generation: request.generation,
                resource_generation: session.profile_texture_epoch,
                result: Err("profile texture payload base64: invalid".into()),
            }).is_err());
        }
        assert!(session.scene.texture_patches.is_empty());
        let content = session.content.as_ref().unwrap();
        assert!(!content.head_textures.has_requests());
        assert!(content.head_textures.skin(&json!({"properties":[
            {"name":"textures","value":"eyJ0ZXh0dXJlcyI6e319"}
        ]})).is_err());
        assert!(content.head_textures.pop_request().is_some(),
            "failed completions must not permanently deny a new profile admission");
    }
}
