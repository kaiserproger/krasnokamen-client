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
    skins: BTreeMap<[u8; 32], Entry>,
    pending: VecDeque<ProfileTextureRequest>,
    clock: u64,
}
struct Entry {
    state: State,
    generation: u64,
    last_access: u64,
}
enum State {
    Pending,
    Ready(TextureRegion),
    Failed,
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
        entries.clock += 1;
        let access = entries.clock;
        if let Some(entry) = entries.skins.get_mut(&key) {
            entry.last_access = access;
            return match entry.state {
                State::Ready(skin) => Ok(Some(skin)),
                State::Pending | State::Failed => Err("head texture not published".into()),
            };
        }
        if entries.skins.len() >= MAX_HEADS {
            // Retain failures as a negative cache, but never let them deny new demand.
            let victim = *entries.skins.iter()
                .min_by_key(|(_, entry)| (
                    !matches!(entry.state, State::Failed), entry.last_access,
                ))
                .expect("full registry has an eviction candidate").0;
            entries.skins.remove(&victim);
            entries.pending.retain(|request|
                request.target != ProfileTextureTarget::Head(victim));
        }
        entries.skins.insert(key, Entry {
            state: State::Pending, generation: access, last_access: access,
        });
        entries.pending.push_back(ProfileTextureRequest {
            target: ProfileTextureTarget::Head(key),
            generation: access,
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
        let ProfileTextureTarget::Head(key) = request.target else {
            return;
        };
        let mut entries = self.0.lock();
        if entries.skins.get(&key).is_some_and(|entry|
            entry.generation == request.generation && matches!(entry.state, State::Pending))
        {
            entries.pending.push_front(request);
        }
    }
    pub(crate) fn is_current(&self, key: &[u8; 32], generation: u64) -> bool {
        self.0.lock().skins.get(key).is_some_and(|entry| entry.generation == generation)
    }
    pub(crate) fn fail(&self, key: &[u8; 32], generation: u64) {
        if let Some(entry) = self.0.lock().skins.get_mut(key)
            && entry.generation == generation
        {
            entry.state = State::Failed;
        }
    }
    pub(crate) fn publish(&self, key: &[u8; 32], generation: u64, skin: TextureRegion) -> bool {
        let mut entries = self.0.lock();
        let Some(entry) = entries.skins.get_mut(key) else {
            return false;
        };
        if entry.generation != generation {
            return false;
        }
        entry.state = State::Ready(skin);
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
        assert!(heads.publish(&key, request.generation, skin));
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
        assert!(heads.skin(&profile("overflow".into())).is_err());
        assert!(heads.has_requests());
        assert_eq!(heads.0.lock().skins.len(), MAX_HEADS);
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

    fn profile(index: usize) -> Value {
        json!({"properties":[{"name":"textures","value":index.to_string()}]})
    }

    fn key(request: &ProfileTextureRequest) -> [u8; 32] {
        let ProfileTextureTarget::Head(key) = request.target else { panic!() };
        key
    }

    #[test]
    fn head_registry_eviction_prefers_failed_then_least_recently_accessed() {
        let heads = HeadTextures::default();
        assert!(heads.skin(&profile(0)).is_err());
        let ready = heads.pop_request().unwrap();
        let skin = TextureRegion { uv: [0., 0., 1., 1.], layer: 1, width: 64, height: 64 };
        assert!(heads.publish(&key(&ready), ready.generation, skin));
        let mut failed = Vec::new();
        for index in 1..MAX_HEADS {
            assert!(heads.skin(&profile(index)).is_err());
            let request = heads.pop_request().unwrap();
            heads.fail(&key(&request), request.generation);
            failed.push(request);
        }
        // A repeated failed lookup neither retries nor loses its recency.
        assert!(heads.skin(&profile(1)).is_err());
        assert!(!heads.has_requests());
        assert!(heads.skin(&profile(MAX_HEADS)).is_err());
        assert!(heads.is_current(&key(&failed[0]), failed[0].generation));
        assert!(!heads.is_current(&key(&failed[1]), failed[1].generation));
        assert_eq!(heads.skin(&profile(0)).unwrap().unwrap().layer, 1);
        assert_eq!(heads.0.lock().skins.len(), MAX_HEADS);
    }

    #[test]
    fn head_registry_eviction_bounds_queue_and_fences_readmitted_keys() {
        let heads = HeadTextures::default();
        assert!(heads.skin(&profile(0)).is_err());
        let old = heads.pop_request().unwrap();
        for index in 1..=MAX_HEADS {
            assert!(heads.skin(&profile(index)).is_err());
        }
        assert!(!heads.is_current(&key(&old), old.generation));
        assert!(heads.skin(&profile(0)).is_err());
        let new_generation = heads.0.lock().skins[&key(&old)].generation;
        assert_ne!(old.generation, new_generation);
        heads.fail(&key(&old), old.generation);
        let skin = TextureRegion { uv: [0., 0., 1., 1.], layer: 1, width: 64, height: 64 };
        assert!(!heads.publish(&key(&old), old.generation, skin));
        heads.retry(old);
        let entries = heads.0.lock();
        assert_eq!(entries.skins.len(), MAX_HEADS);
        assert_eq!(entries.pending.len(), MAX_HEADS);
        assert!(entries.pending.iter().all(|request| {
            let entry = &entries.skins[&key(request)];
            entry.generation == request.generation && matches!(entry.state, State::Pending)
        }));
    }
}
