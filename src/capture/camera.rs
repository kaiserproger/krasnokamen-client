//! Authored camera JSON; uniform Catmull–Rom with optional smoothstep easing.
//! Positions/look targets are world-space, or offsets from `attach_entity`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
    pub time: f64,
    pub position: [f64; 3],
    pub look_target: Option<[f64; 3]>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub fov: f64,
    #[serde(default)]
    pub ease: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraPath {
    pub attach_entity: Option<i32>,
    pub keyframes: Vec<Keyframe>,
}

fn cubic(p: [f64; 4], t: f64) -> f64 {
    let [a,b,c,d] = p;
    0.5 * (2.*b + (-a+c)*t + (2.*a-5.*b+4.*c-d)*t*t + (-a+3.*b-3.*c+d)*t*t*t)
}
fn near(angle: f64, reference: f64) -> f64 {
    reference + (angle-reference+180.).rem_euclid(360.)-180.
}
impl CameraPath {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 1024 * 1024 { return Err("camera JSON exceeds 1 MiB".into()); }
        let path: Self = serde_json::from_slice(bytes).map_err(|e| format!("camera JSON: {e}"))?;
        if path.keyframes.len() < 2 || path.keyframes.len() > 4096 {
            return Err("camera path needs 2..4096 keyframes".into());
        }
        let targets = path.keyframes[0].look_target.is_some();
        let mut previous = -1.;
        for key in &path.keyframes {
            if !key.time.is_finite() || key.time <= previous || key.time < 0.
                || !key.position.iter().all(|v| v.is_finite())
                || !key.fov.is_finite() || !(1.0..179.0).contains(&key.fov)
                || key.look_target.is_some() != targets
                || (targets && (key.yaw.is_some() || key.pitch.is_some()))
                || (!targets && (key.yaw.is_none_or(|v| !v.is_finite()) || key.pitch.is_none_or(|v| !v.is_finite() || v.abs() > 90.)))
                || key.look_target.is_some_and(|v| !v.iter().all(|x| x.is_finite()) || v == key.position)
            { return Err("camera keys require increasing nonnegative time, finite pose/FOV, and exclusively look_target or yaw+pitch".into()); }
            previous = key.time;
        }
        Ok(path)
    }
    pub fn range(&self) -> [f64; 2] { [self.keyframes[0].time, self.keyframes.last().unwrap().time] }
    pub fn sample(&self, time: f64, attached: Option<[f64; 3]>) -> Result<Pose, String> {
        if !time.is_finite() { return Err("camera sample time must be finite".into()); }
        let offset = match self.attach_entity {
            Some(id) => attached.ok_or_else(|| format!("camera attached entity {id} is absent at {time}"))?,
            None => [0.;3],
        };
        let last = self.keyframes.len()-1;
        let end = self.keyframes.partition_point(|key| key.time <= time).clamp(1, last);
        let indices = [end.saturating_sub(2), end-1, end, (end+1).min(last)];
        let keys = indices.map(|i| &self.keyframes[i]);
        let mut t = ((time-keys[1].time)/(keys[2].time-keys[1].time)).clamp(0.,1.);
        if keys[1].ease { t = t*t*(3.-2.*t); }
        let position = std::array::from_fn(|axis| cubic(keys.map(|key| key.position[axis]),t)+offset[axis]);
        let (yaw,pitch) = if keys[1].look_target.is_some() {
            let target: [f64;3] = std::array::from_fn(|axis| cubic(keys.map(|key| key.look_target.unwrap()[axis]),t)+offset[axis]);
            let delta: [f64;3] = std::array::from_fn(|axis| target[axis]-position[axis]);
            let horizontal = delta[0].hypot(delta[2]);
            if horizontal == 0. && delta[1] == 0. { return Err(format!("camera target intersects position at {time}")); }
            ((-delta[0]).atan2(delta[2]).to_degrees(), (-delta[1]).atan2(horizontal).to_degrees())
        } else {
            let mut yaws = keys.map(|key| key.yaw.unwrap());
            yaws[0] = near(yaws[0], yaws[1]);
            yaws[2] = near(yaws[2], yaws[1]);
            yaws[3] = near(yaws[3], yaws[2]);
            (cubic(yaws,t), cubic(keys.map(|key| key.pitch.unwrap()),t).clamp(-90.,90.))
        };
        Ok(Pose { position, yaw: yaw as f32, pitch: pitch as f32, fov: cubic(keys.map(|key| key.fov),t).clamp(1.,179.) as f32 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spline_reproduces_keys_linear_interior_and_attachment() {
        let path = CameraPath::parse(br#"{"attach_entity":7,"keyframes":[{"time":0,"position":[0,0,0],"yaw":170,"pitch":0,"fov":60},{"time":1,"position":[1,0,0],"yaw":179,"pitch":0,"fov":70},{"time":2,"position":[2,0,0],"yaw":-179,"pitch":0,"fov":80},{"time":3,"position":[3,0,0],"yaw":-170,"pitch":0,"fov":90}]}"#).unwrap();
        assert_eq!(path.sample(1.,Some([10.,2.,3.])).unwrap().position,[11.,2.,3.]);
        let mid=path.sample(1.5,Some([10.,2.,3.])).unwrap();
        assert_eq!(mid.position,[11.5,2.,3.]);
        assert!((mid.yaw-180.).abs()<0.001);
        assert_eq!(mid.fov,75.);
        assert!(path.sample(1.,None).is_err());
    }
    #[test]
    fn target_axes_ease_and_time_validation() {
        let path=CameraPath::parse(br#"{"keyframes":[{"time":0,"position":[0,0,0],"look_target":[0,0,10],"fov":60,"ease":true},{"time":2,"position":[2,0,0],"look_target":[2,0,10],"fov":60}]}"#).unwrap();
        assert_eq!(path.sample(0.,None).unwrap(),Pose {position:[0.,0.,0.],yaw:0.,pitch:0.,fov:60.});
        assert_eq!(path.sample(2.,None).unwrap().position,[2.,0.,0.]);
        assert!(path.sample(0.5,None).unwrap().position[0]<0.5);
        assert!(CameraPath::parse(br#"{"keyframes":[]}"#).is_err());
        assert_eq!(cubic([0.,1.,2.,3.],0.25),1.25);
    }
}
