// GPU crowd data shared by the vertex stage (entity_instancing.wgsl) and the cull and
// bone pass (entity_instancing_prepare.wgsl). Layouts mirror entities/crowd.rs.
struct CrowdInstance {
    // Rows of the affine model root, relative to the scene origin.
    root: array<vec4<f32>, 3>,
    // Head pitch, head yaw, right leg, left leg.
    pose: vec4<f32>,
    // Arm AxisMap offsets; arms[0].w is the clip leg factor.
    arms: array<vec4<f32>, 2>,
    // Seconds, weight of the action layer, then of the flinch layer.
    layers: vec4<f32>,
    // Sky light, block light, hurt overlay, unused.
    light: vec4<f32>,
    // Composite, first bone matrix, flags, action clip | flinch clip << 16.
    words: vec4<u32>,
};
const CROWD_VERTEX_BODY: u32 = 0x10000u;
const CROWD_VERTEX_ITEM_NORMAL: u32 = 0x20000u;
