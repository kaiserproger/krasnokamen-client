// GPU crowd cull and bone pass (render/entity_instancing.rs).
//
// `crowd_prepare`, one invocation per instance: tests the instance's bounding sphere
// against every view (camera, then shadow cascades), picks its detail level from its
// projected size, appends it to the visible list of each view that sees it, and evaluates
// its composite's bone matrices with the rules of `presentation::evaluate_posed_rig` and
// `clip_pose` (mirrored on the CPU by `crowd::evaluate`).
// `crowd_args`, one invocation per indirect draw: copies the visible counts.
struct CrowdBone {
    pivot: vec3<f32>,
    parent: i32,
    rotation: vec3<f32>,
    role: u32,
    scale: vec3<f32>,
    channel: u32,
    flags: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
struct CrowdGroup {
    bone_first: u32,
    bone_count: u32,
    // First instance of this composite in a view's visible list, and instance count.
    prefix: u32,
    count: u32,
    hip: vec3<f32>,
    radius: f32,
};
struct CrowdParams {
    // Six planes per view (camera, then up to three shadow cascades).
    planes: array<vec4<f32>, 24>,
    // Camera relative to the scene origin; w: pixels per model unit at distance 1.
    eye: vec4<f32>,
    // Projected sizes in pixels below which detail levels 1 and 2 apply.
    lod: vec4<f32>,
    // Instances, composites, views, instances in total (visible list stride per level).
    sizes: vec4<u32>,
    // Clip headers, materials, indirect draws, unused.
    tables: vec4<u32>,
};
@group(0) @binding(0) var<uniform> params: CrowdParams;
@group(0) @binding(1) var<storage, read> instances: array<CrowdInstance>;
@group(0) @binding(2) var<storage, read> groups: array<CrowdGroup>;
@group(0) @binding(3) var<storage, read> bones: array<CrowdBone>;
// Clip headers [first frame, frames, playback, 0], then every sample as two words.
@group(0) @binding(4) var<storage, read> clips: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> matrices: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> visible: array<u32>;
@group(0) @binding(7) var<storage, read_write> counts: array<atomic<u32>>;
@group(0) @binding(8) var<storage, read_write> args: array<u32>;

const NO_CHANNEL: u32 = 0xffffffffu;
const CHANNELS: u32 = 9u;
const BODY: u32 = 1u;
const ROLE_HEAD: u32 = 1u;
const ROLE_RIGHT_ARM: u32 = 2u;
const ROLE_LEFT_ARM: u32 = 3u;
const ROLE_RIGHT_LEG: u32 = 4u;
const ROLE_LEFT_LEG: u32 = 5u;
const ROLE_ITEM: u32 = 6u;
const BONE_CARRIED: u32 = 1u;
const PIVOT_Z_ZERO: u32 = 0x1000u;
const RIDING: u32 = 0x2000u;
const RIDING_LEG_PITCH: f32 = -1.4137167;

struct Affine {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
};
fn mul(a: Affine, b: Affine) -> Affine {
    var out: Affine;
    out.r0 = a.r0.x * b.r0 + a.r0.y * b.r1 + a.r0.z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, a.r0.w);
    out.r1 = a.r1.x * b.r0 + a.r1.y * b.r1 + a.r1.z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, a.r1.w);
    out.r2 = a.r2.x * b.r0 + a.r2.y * b.r1 + a.r2.z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, a.r2.w);
    return out;
}
// `resources::transform`: T(pivot) Rz Ry Rx S.
fn transform(pivot: vec3<f32>, rotation: vec3<f32>, scale: vec3<f32>) -> Affine {
    let sx = sin(rotation.x); let cx = cos(rotation.x);
    let sy = sin(rotation.y); let cy = cos(rotation.y);
    let sz = sin(rotation.z); let cz = cos(rotation.z);
    var out: Affine;
    out.r0 = vec4<f32>(vec3<f32>(cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx) * scale, pivot.x);
    out.r1 = vec4<f32>(vec3<f32>(sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx) * scale, pivot.y);
    out.r2 = vec4<f32>(vec3<f32>(-sy, cy * sx, cy * cx) * scale, pivot.z);
    return out;
}
fn translated(a: Affine, offset: vec3<f32>) -> Affine {
    var out = a;
    out.r0.w += offset.x; out.r1.w += offset.y; out.r2.w += offset.z;
    return out;
}
fn apply_point(a: Affine, p: vec3<f32>) -> vec3<f32> {
    let h = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(a.r0, h), dot(a.r1, h), dot(a.r2, h));
}

struct Sample {
    rotation: vec3<f32>,
    weight: f32,
    offset: vec3<f32>,
    valid: bool,
};
fn frame_sample(index: u32) -> Sample {
    let at = params.tables.x + index * 2u;
    let a = bitcast<vec4<f32>>(clips[at]);
    let b = bitcast<vec4<f32>>(clips[at + 1u]);
    return Sample(a.xyz, a.w, b.xyz, true);
}
fn mirrored(channel: u32) -> u32 {
    if channel >= 2u && channel <= 7u { return channel ^ 1u; }
    return channel;
}
// `ClipLibrary::sample`.
fn clip_sample(clip: u32, seconds: f32, mirror: bool, channel: u32) -> Sample {
    var none: Sample;
    none.valid = false;
    if clip == 0u || clip > params.tables.x { return none; }
    let header = clips[clip - 1u];
    let last = header.y - 1u;
    var frame = seconds * 60.0;
    if !(frame >= 0.0) { return none; }
    if header.z == 1u {
        let length = f32(last + 1u);
        frame = frame - floor(frame / length) * length;
    } else if header.z == 2u {
        frame = min(frame, f32(last));
    } else if frame > f32(last) {
        return none;
    }
    let first = min(u32(floor(frame)), last);
    var second = min(first + 1u, last);
    if header.z == 1u { second = (first + 1u) % (last + 1u); }
    var source = channel;
    if mirror { source = mirrored(channel); }
    let a = frame_sample((header.x + first) * CHANNELS + source);
    let b = frame_sample((header.x + second) * CHANNELS + source);
    let t = frame - f32(first);
    var s = Sample(a.rotation + (b.rotation - a.rotation) * t, a.weight + (b.weight - a.weight) * t,
        a.offset + (b.offset - a.offset) * t, true);
    if mirror {
        s.rotation = vec3<f32>(s.rotation.x, -s.rotation.y, -s.rotation.z);
        s.offset.x = -s.offset.x;
    }
    return s;
}
fn layer_clip(instance: CrowdInstance, layer: u32) -> u32 {
    return (instance.words.w >> (16u * layer)) & 0xffffu;
}
fn layer_sample(instance: CrowdInstance, layer: u32, channel: u32) -> Sample {
    let mirror = (instance.words.z & (0x4000u << layer)) != 0u;
    return clip_sample(layer_clip(instance, layer), instance.layers[layer * 2u], mirror, channel);
}
// `ClipLibrary::delta` as `T(pivot + offset) R(rotation) T(-pivot)`; `valid` false when
// no layer samples the channel.
struct Delta {
    about: Affine,
    valid: bool,
};
fn delta(instance: CrowdInstance, channel: u32, pivot: vec3<f32>) -> Delta {
    var rotation = vec3<f32>(0.0);
    var offset = vec3<f32>(0.0);
    var out: Delta;
    out.valid = false;
    for (var layer = 0u; layer < 2u; layer++) {
        let s = layer_sample(instance, layer, channel);
        if s.valid {
            let w = instance.layers[layer * 2u + 1u];
            rotation += s.rotation * w;
            offset += s.offset * w;
            out.valid = true;
        }
    }
    let r = transform(vec3<f32>(0.0), rotation, vec3<f32>(1.0));
    out.about = translated(r, pivot + offset - apply_point(r, pivot));
    return out;
}
fn decode_scale(flags: u32, shift: u32) -> f32 {
    let code = (flags >> shift) & 3u;
    return select(select(0.5, 1.0, code == 1u), 0.0, code == 0u);
}
fn store(index: u32, a: Affine) {
    matrices[index * 3u] = a.r0;
    matrices[index * 3u + 1u] = a.r1;
    matrices[index * 3u + 2u] = a.r2;
}
fn load(index: u32) -> Affine {
    return Affine(matrices[index * 3u], matrices[index * 3u + 1u], matrices[index * 3u + 2u]);
}

fn evaluate(instance: CrowdInstance, group: CrowdGroup) {
    let root = Affine(instance.root[0], instance.root[1], instance.root[2]);
    let flags = instance.words.z;
    let base = instance.words.y;
    let carry = delta(instance, BODY, group.hip);
    for (var b = 0u; b < group.bone_count; b++) {
        let bone = bones[group.bone_first + b];
        var parent = root;
        if bone.parent >= 0 { parent = load(base + u32(bone.parent)); }
        if bone.role == ROLE_ITEM {
            var item = parent;
            if bone.channel != NO_CHANNEL {
                let hand = bone.channel - 6u;
                let grip = vec3<f32>(select(1.0, -1.0, hand == 0u) / 16.0, 0.625, -0.125);
                let d = delta(instance, bone.channel, grip);
                if d.valid { item = mul(parent, d.about); }
            }
            store(base + b, item);
            continue;
        }
        var rotation = bone.rotation;
        var pivot = bone.pivot;
        if bone.role == ROLE_HEAD {
            rotation.x += instance.pose.x;
            rotation.y += instance.pose.y;
        } else if bone.role == ROLE_RIGHT_LEG {
            rotation.x += instance.pose.z;
        } else if bone.role == ROLE_LEFT_LEG {
            rotation.x += instance.pose.w;
        } else if bone.role == ROLE_RIGHT_ARM || bone.role == ROLE_LEFT_ARM {
            let arm = select(0u, 1u, bone.role == ROLE_LEFT_ARM);
            let scale = vec3<f32>(decode_scale(flags, arm * 6u), decode_scale(flags, arm * 6u + 2u),
                decode_scale(flags, arm * 6u + 4u));
            rotation = rotation * scale + select(instance.arms[0], instance.arms[1], arm == 1u).xyz;
            if (flags & PIVOT_Z_ZERO) != 0u { pivot.z = 0.0; }
        }
        let leg = bone.role == ROLE_RIGHT_LEG || bone.role == ROLE_LEFT_LEG;
        if (flags & RIDING) != 0u && leg {
            rotation.x = RIDING_LEG_PITCH;
        }
        if bone.channel != NO_CHANNEL {
            // `ClipLibrary::apply`.
            let factor = select(1.0, instance.arms[0].w, leg);
            for (var layer = 0u; layer < 2u; layer++) {
                let s = layer_sample(instance, layer, bone.channel);
                if s.valid {
                    let w = instance.layers[layer * 2u + 1u] * factor;
                    rotation = rotation + (bone.rotation - rotation) * (s.weight * w);
                    if bone.channel != BODY {
                        rotation += s.rotation * w;
                        pivot += s.offset * w;
                    }
                }
            }
        }
        if carry.valid && (bone.flags & BONE_CARRIED) != 0u {
            parent = mul(parent, carry.about);
        }
        store(base + b, mul(parent, transform(pivot, rotation, bone.scale)));
    }
}

@compute @workgroup_size(64)
fn crowd_prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= params.sizes.x { return; }
    let instance = instances[index];
    let g = instance.words.x;
    let group = groups[g];
    let c0 = vec3<f32>(instance.root[0].x, instance.root[1].x, instance.root[2].x);
    let c1 = vec3<f32>(instance.root[0].y, instance.root[1].y, instance.root[2].y);
    let c2 = vec3<f32>(instance.root[0].z, instance.root[1].z, instance.root[2].z);
    let center = vec3<f32>(instance.root[0].w, instance.root[1].w, instance.root[2].w);
    let radius = group.radius * max(length(c0), max(length(c1), length(c2)));
    let pixels = 2.0 * radius * params.eye.w / max(distance(center, params.eye.xyz), 0.05);
    var lod = 0u;
    if pixels < params.lod.x { lod = 1u; }
    if pixels < params.lod.y { lod = 2u; }
    var seen = false;
    for (var view = 0u; view < params.sizes.z; view++) {
        var inside = true;
        for (var plane = 0u; plane < 6u; plane++) {
            let p = params.planes[view * 6u + plane];
            if dot(p.xyz, center) + p.w < -radius { inside = false; }
        }
        if inside {
            seen = true;
            let slot = atomicAdd(&counts[(view * params.sizes.y + g) * 3u + lod], 1u);
            visible[view * 3u * params.sizes.w + 3u * group.prefix + lod * group.count + slot] = index;
        }
    }
    if seen { evaluate(instance, group); }
}

// DrawIndexedIndirect argument blocks of five words; the instance count is word 1.
@compute @workgroup_size(64)
fn crowd_args(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.tables.z { return; }
    args[id.x * 5u + 1u] = atomicLoad(&counts[id.x / params.tables.y]);
}
