//! Resource-scoped demand registry for skull properties. Downloads use the player resolver.
use super::{MAX_PROPERTY, ProfileProperty, ProfileTextureRequest, ProfileTextureTarget, Result};
use crate::protocol::content::TextureRegion;
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

const MAX_HEADS: usize = 256;
#[derive(Default)]
pub(crate) struct HeadTextures(Mutex<Entries>);
#[derive(Default)]
struct Entries {
    skins: BTreeMap<[u8; 32], Option<TextureRegion>>,
    pending: VecDeque<ProfileTextureRequest>,
}
impl HeadTextures {
    /// None means no own property; Err means own property unresolved (do not use PlayerInfo).
    pub(crate) fn skin(&self, profile: &Value) -> Result<Option<TextureRegion>> {
        let Some(properties) = profile.get("properties") else {
            return Ok(None);
        };
        let properties = properties
            .as_array()
            .ok_or("head properties must be a list")?;
        if properties.len() > 16 {
            return Err("head property count bound".into());
        }
        let Some(property) = properties.iter().find(|p| p["name"] == "textures") else {
            return Ok(None);
        };
        let value = property["value"]
            .as_str()
            .ok_or("head texture value missing")?;
        let signature = match property.get("signature") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().ok_or("head texture signature malformed")?),
        };
        if value.len() > MAX_PROPERTY || signature.is_some_and(|s| s.len() > 8192) {
            return Err("head texture property size bound".into());
        }
        let mut hash = Sha256::new();
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
        hash.update([u8::from(signature.is_some())]);
        if let Some(signature) = signature {
            hash.update(signature.as_bytes());
        }
        let key: [u8; 32] = hash.finalize().into();
        let mut entries = self.0.lock();
        if let Some(skin) = entries.skins.get(&key) {
            return skin
                .map(Some)
                .ok_or_else(|| "head texture not published".into());
        }
        if entries.skins.len() >= MAX_HEADS {
            return Err("head texture registry full".into());
        }
        entries.skins.insert(key, None);
        entries.pending.push_back(ProfileTextureRequest {
            target: ProfileTextureTarget::Head(key),
            generation: 0,
            property: Arc::new(ProfileProperty {
                name: "textures".into(),
                value: value.into(),
                signature: signature.map(str::to_owned),
            }),
            // PlayerSkinRenderCache accepts unsigned skull properties; remote avatars do not.
            require_secure: false,
        });
        Err("head texture pending".into())
    }
    pub(crate) fn has_requests(&self) -> bool {
        !self.0.lock().pending.is_empty()
    }
    pub(crate) fn pop_request(&self) -> Option<ProfileTextureRequest> {
        self.0.lock().pending.pop_front()
    }
    pub(crate) fn retry(&self, request: ProfileTextureRequest) {
        self.0.lock().pending.push_front(request);
    }
    pub(crate) fn publish(&self, key: &[u8; 32], skin: TextureRegion) -> bool {
        let mut entries = self.0.lock();
        let Some(entry) = entries.skins.get_mut(key) else {
            return false;
        };
        *entry = Some(skin);
        true
    }
    pub(crate) fn clear(&self) {
        *self.0.lock() = Entries::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn skull_parity_head_registry_deduplicates_bounds_and_separates_signatures() {
        let heads = HeadTextures::default();
        let profile = |value: String| json!({"properties":[{"name":"textures","value":value}]});
        let unsigned = profile("same".into());
        assert!(heads.skin(&unsigned).is_err());
        assert!(heads.skin(&unsigned).is_err());
        let request = heads.pop_request().unwrap();
        assert!(heads.pop_request().is_none());
        let ProfileTextureTarget::Head(key) = request.target else {
            panic!();
        };
        let skin = TextureRegion {
            uv: [0., 0., 1., 1.],
            layer: 1,
            width: 64,
            height: 64,
        };
        assert!(heads.publish(&key, skin));
        assert_eq!(heads.skin(&unsigned).unwrap().unwrap().layer, 1);
        let mut signed = unsigned.clone();
        signed["properties"][0]["signature"] = json!("");
        assert!(
            heads.skin(&signed).is_err(),
            "invalid signature must not reuse unsigned publication"
        );
        assert!(heads.pop_request().is_some());
        for index in 2..MAX_HEADS {
            assert!(heads.skin(&profile(index.to_string())).is_err());
        }
        while heads.pop_request().is_some() {}
        assert!(
            heads
                .skin(&profile("overflow".into()))
                .unwrap_err()
                .contains("full")
        );
        assert!(!heads.has_requests());
        heads.clear();
        assert!(heads.skin(&unsigned).is_err());
        assert!(
            heads.has_requests(),
            "resource reload requeues visible heads"
        );
        heads.clear();
        assert!(heads.skin(&profile("x".repeat(MAX_PROPERTY + 1))).is_err());
        assert!(!heads.has_requests());
    }
}
