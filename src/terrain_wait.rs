use crate::api::Request;
use serde_json::Value;
use std::{collections::BTreeSet, time::{Duration, Instant}};

pub(crate) struct Candidate {
    pub frame: u64,
    pub revisions: Vec<u64>,
    pub selected: Vec<bool>,
    pub camera: ([f64; 3], f32, f32),
}
impl Candidate {
    pub fn matches(&self, revisions: Option<&[u64]>, camera: ([f64; 3], f32, f32)) -> bool {
        revisions == Some(self.revisions.as_slice()) && camera == self.camera
    }
    pub fn completed(&self, frame: u64) -> bool {
        frame >= self.frame
    }
}


pub(crate) struct Wait {
    pub request: Request,
    pub keys: Vec<[i32;  3]>,
    pub epochs: (u64, u64, u64),
    pub deadline: Instant,
    pub candidate: Option<Candidate>,
}

pub(crate) fn parse(arguments: &Value) -> Result<(Vec<[i32; 3]>, Duration), String> {
    let sections = arguments["sections"].as_array().ok_or("terrain_sections_required")?;
    if sections.is_empty() || sections.len() > 64 {
        return Err("terrain_sections_count_out_of_range".into());
    }
    let mut seen = BTreeSet::new();
    let mut keys = Vec::with_capacity(sections.len());
    for section in sections {
        let values = section.as_array().filter(|v| v.len() == 3).ok_or("invalid_terrain_section")?;
        let mut key = [0; 3];
        for (i, value) in values.iter().enumerate() {
            key[i] = value.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or("invalid_terrain_section")?;
        }
        if !seen.insert(key) {
            return Err("duplicate_terrain_section".into());
        }
        keys.push(key);
    }
    let seconds = match arguments.get("timeout_seconds") {
        None => 30.,
        Some(value) => value.as_f64().filter(|n| n.is_finite() && (0.1..=120.).contains(n)).ok_or("invalid_terrain_timeout")?,
    };
    Ok((keys, Duration::from_secs_f64(seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn candidate_requires_matching_revision_pose_and_completed_frame() {
        let camera = ([1., 2., 3.], 10., 0.);
        let candidate = Candidate { frame: 12, revisions: vec![7, 9], selected: vec![true, false], camera };
        assert!(candidate.matches(Some(&[7, 9]), camera));
        assert!(!candidate.matches(None, camera));
        assert!(!candidate.matches(Some(&[7, 10]), camera));
        assert!(!candidate.matches(Some(&[7, 9]), ([1., 2., 4.], 10., 0.)));
        assert!(!candidate.completed(11));
        assert!(candidate.completed(12));
    }

    #[test]
    fn validates_section_keys_and_deadline() {
        let (keys, timeout) = parse(&json!({"sections":[[62,10,62],[62,9,62]]})).unwrap();
        assert_eq!(keys, [[62,10,62],[62,9,62]]);
        assert_eq!(timeout, Duration::from_secs(30));
        assert!(parse(&json!({"sections":[]})).is_err());
        assert!(parse(&json!({"sections":[[1,2,3],[1,2,3]]})).is_err());
        assert!(parse(&json!({"sections":[[1,2,2147483648i64]]})).is_err());
        assert!(parse(&json!({"sections":[[1,2,3]],"timeout_seconds":0})).is_err());
    }
}
