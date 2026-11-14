// GPU crowd vertex stages (render/entity_instancing.rs). Appended to shader.wgsl, so a
// crowd vertex leaves through `source_out`, exactly like a CPU-emitted entity vertex.
struct CrowdView {
    // Scene origin relative to the camera (the dynamic geometry's presentation origin).
    origin: vec4<f32>,
};
@group(3) @binding(0) var<uniform> crowd_view: CrowdView;
@group(3) @binding(1) var<storage, read> crowd_instances: array<CrowdInstance>;
// Evaluated bone matrices: three affine rows each (entity_instancing_prepare.wgsl).
@group(3) @binding(2) var<storage, read> crowd_matrices: array<vec4<f32>>;

struct CrowdPoint {
    relative: vec3<f32>,
    normal: vec3<f32>,
};
// A composite vertex placed by its bone; the normal follows `emit_rig_parts` (inverse
// transpose of the bone matrix) or, for items, `emit` (face normal of the placed quad).
fn crowd_point(instance: CrowdInstance, position: vec3<f32>, normal: vec3<f32>, bone_flags: u32) -> CrowdPoint {
    let base = (instance.words.y + (bone_flags & 0xffffu)) * 3u;
    let r0 = crowd_matrices[base];
    let r1 = crowd_matrices[base + 1u];
    let r2 = crowd_matrices[base + 2u];
    let p = vec4<f32>(position, 1.0);
    var out: CrowdPoint;
    out.relative = vec3<f32>(dot(r0, p), dot(r1, p), dot(r2, p)) + crowd_view.origin.xyz;
    let c0 = vec3<f32>(r0.x, r1.x, r2.x);
    let c1 = vec3<f32>(r0.y, r1.y, r2.y);
    let c2 = vec3<f32>(r0.z, r1.z, r2.z);
    // Cofactor matrix: determinant times the inverse transpose.
    var n = mat3x3<f32>(cross(c1, c2), cross(c2, c0), cross(c0, c1)) * normal;
    if (bone_flags & CROWD_VERTEX_ITEM_NORMAL) == 0u {
        n *= sign(dot(c0, cross(c1, c2)));
    }
    let len = length(n);
    out.normal = select(vec3<f32>(0.0), n / len, len > 0.0);
    return out;
}
@vertex fn crowd_vertex(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) color: vec4<f32>, @location(4) layer: u32,
    @location(5) bone_flags: u32, @location(6) instance_index: u32) -> SceneOut {
    let instance = crowd_instances[instance_index];
    let point = crowd_point(instance, position, normal, bone_flags);
    var light = instance.light.xy;
    // `BakedQuad::light_with_emission`.
    let emission = f32(bone_flags >> 24u);
    if emission > 0.0 && !all(light == vec2<f32>(-1.0)) {
        light = max(floor(light), vec2<f32>(emission));
    }
    // Hurt flash on the main rig only, as `draw_entity` sets it before equipment.
    var overlay = vec2<f32>(0.0, 10.0);
    if instance.light.z > 0.5 && (bone_flags & CROWD_VERTEX_BODY) != 0u {
        overlay = vec2<f32>(0.0, 3.0);
    }
    return source_out(point.relative, color.rgb, uv, overlay, vec3<f32>(1.0), color.a, layer, light, point.normal);
}
@vertex fn crowd_shadow_vertex(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) color: vec4<f32>, @location(4) layer: u32,
    @location(5) bone_flags: u32, @location(6) instance_index: u32) -> ShadowOut {
    let point = crowd_point(crowd_instances[instance_index], position, normal, bone_flags);
    var out: ShadowOut;
    out.position = camera.shadow_projection * vec4<f32>(point.relative, 1.0);
    out.uv = uv; out.layer = layer; out.alpha = color.a;
    out.tile_origin = vec2<f32>(0.0, 10.0); out.tile = vec3<f32>(1.0);
    return out;
}
