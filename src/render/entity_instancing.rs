//! GPU crowd renderer: draws `Scene::crowd` (data model in `entities/crowd.rs`).
//!
//! Per frame: one compute dispatch culls every instance against the camera and each
//! shadow cascade, picks its detail level and evaluates its bone matrices
//! (`entity_instancing_prepare.wgsl`); a second writes the instance counts into indirect
//! draw arguments. Each view then issues one indirect instanced draw per composite,
//! material and detail level, with that view's visible-instance list bound as the
//! per-instance vertex buffer. Vertices leave through the same `source_out` and Source
//! material pipelines as CPU-emitted entities.
use super::material::MATERIALS as DESCRIPTORS;
use crate::entities::{crowd as model, CrowdScene};
use glam::{Mat4, Vec3, Vec4};
use std::sync::Arc;

/// Projected bounding-sphere diameters (pixels) below which detail levels 1 and 2 apply.
pub(super) const LOD_PIXELS: [f32; 2] = [32., 13.];
/// The camera, then up to `MAX_CASCADES` shadow cascades.
pub(super) const MAX_VIEWS: usize = 1 + super::graphics::MAX_CASCADES;
const WORKGROUP: u32 = 64;
const MATERIALS: usize = model::MATERIALS.len();
const ARGS_BYTES: u64 = 20;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Group {
    bone_first: u32,
    bone_count: u32,
    prefix: u32,
    count: u32,
    hip: [f32; 3],
    radius: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    planes: [[f32; 4]; 6 * MAX_VIEWS],
    eye: [f32; 4],
    lod: [f32; 4],
    sizes: [u32; 4],
    tables: [u32; 4],
}

/// Whether an adapter can run the crowd renderer (and `SOLARIS_CLIENT_CROWD` is not 0).
pub(super) fn supported(downlevel: wgpu::DownlevelFlags, limits: &wgpu::Limits) -> bool {
    downlevel.contains(
        wgpu::DownlevelFlags::COMPUTE_SHADERS
            | wgpu::DownlevelFlags::INDIRECT_EXECUTION
            | wgpu::DownlevelFlags::VERTEX_STORAGE,
    ) && limits.max_storage_buffers_per_shader_stage >= 8
        && std::env::var_os("SOLARIS_CLIENT_CROWD").is_none_or(|value| value != "0")
}

/// Six normalized planes `(n, d)` with `n·p + d >= 0` inside the clip volume of `m`
/// (WebGPU depth 0..1).
pub(super) fn planes(m: Mat4) -> [[f32; 4]; 6] {
    let row = |i: usize| Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|plane| {
        let length = plane.truncate().length();
        (if length > 0. { plane / length } else { plane }).to_array()
    })
}

/// Detail level of a bounding sphere of `radius` at `distance`, with the projection's
/// `pixels_per_unit` at distance 1 (mirrors `crowd_prepare`).
#[cfg(test)]
pub(super) fn lod(radius: f32, distance: f32, pixels_per_unit: f32) -> usize {
    let pixels = 2. * radius * pixels_per_unit / distance.max(0.05);
    usize::from(pixels < LOD_PIXELS[0]) + usize::from(pixels < LOD_PIXELS[1])
}

/// A storage/vertex buffer that grows to the next power of two.
struct Grow {
    buffer: wgpu::Buffer,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl Grow {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST;
        Self { buffer: Self::create(device, label, usage, 256), usage, label }
    }
    fn create(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
    }
    /// Makes room for `size` bytes; `true` when the buffer was replaced.
    fn ensure(&mut self, device: &wgpu::Device, size: u64) -> bool {
        if size <= self.buffer.size() {
            return false;
        }
        self.buffer = Self::create(device, self.label, self.usage, size.next_power_of_two().max(256));
        true
    }
}

struct Binds {
    draw: wgpu::BindGroup,
    prepare: wgpu::BindGroup,
    args: wgpu::BindGroup,
}

/// What this frame draws.
#[derive(Default)]
struct Plan {
    views: u32,
    instances: u32,
    entries: u32,
    meshes: Arc<model::CrowdMeshes>,
    /// Instances and first visible slot of each composite.
    groups: Vec<(u32, u32)>,
}

pub(super) struct Crowd {
    pipelines: Vec<wgpu::RenderPipeline>,
    shadow: wgpu::RenderPipeline,
    prepare: wgpu::ComputePipeline,
    args_pipeline: wgpu::ComputePipeline,
    draw_layout: wgpu::BindGroupLayout,
    prepare_layout: wgpu::BindGroupLayout,
    args_layout: wgpu::BindGroupLayout,
    empty: wgpu::BindGroup,
    view: wgpu::Buffer,
    params: wgpu::Buffer,
    mesh_revision: Option<u64>,
    vertices: Grow,
    indices: Grow,
    bones: Grow,
    clip_revision: Option<u64>,
    clips: Grow,
    clip_headers: u32,
    instances: Grow,
    groups: Grow,
    matrices: Grow,
    visible: Grow,
    counts: Grow,
    args: Grow,
    binds: Option<Binds>,
    plan: Plan,
    /// Bytes written this frame.
    pub uploaded_bytes: u64,
}

fn storage(binding: u32, visibility: wgpu::ShaderStages, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Main-shader source of the crowd vertex stages, appended by `main_shader_source`.
pub(super) fn vertex_source() -> String {
    format!("{}\n{}", include_str!("entity_instancing_types.wgsl"), include_str!("entity_instancing.wgsl"))
}

pub(super) fn prepare_source() -> String {
    format!("{}\n{}", include_str!("entity_instancing_types.wgsl"), include_str!("entity_instancing_prepare.wgsl"))
}

impl Crowd {
    pub(super) fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        atlas_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Solaris material shader with crowd vertex stages"),
            source: wgpu::ShaderSource::Wgsl(super::graphics::main_shader_source().into()),
        });
        let vertex = wgpu::ShaderStages::VERTEX;
        let compute = wgpu::ShaderStages::COMPUTE;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crowd instances and bone matrices"),
            entries: &[uniform(0, vertex), storage(1, vertex, true), storage(2, vertex, true)],
        });
        let prepare_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crowd cull and bone pass"),
            entries: &[
                uniform(0, compute),
                storage(1, compute, true),
                storage(2, compute, true),
                storage(3, compute, true),
                storage(4, compute, true),
                storage(5, compute, false),
                storage(6, compute, false),
                storage(7, compute, false),
            ],
        });
        let args_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crowd indirect arguments"),
            entries: &[uniform(0, compute), storage(7, compute, false), storage(8, compute, false)],
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crowd shadow placeholder"),
            entries: &[],
        });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("crowd shadow placeholder"),
            layout: &empty_layout,
            entries: &[],
        });
        let fancy_layout = super::shadow::layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crowd material layout"),
            bind_group_layouts: &[camera_layout, atlas_layout, &fancy_layout, &draw_layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 4 => Uint32, 5 => Uint32];
        let instance = wgpu::vertex_attr_array![6 => Uint32];
        let buffers = [
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<model::CrowdVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            },
            wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Instance, attributes: &instance },
        ];
        let pipelines = model::MATERIALS
            .iter()
            .map(|material| {
                super::pipelines::create_source(
                    device,
                    &shader,
                    &layout,
                    &buffers,
                    DESCRIPTORS[*material as usize],
                    super::post::HDR,
                    "crowd_vertex",
                )
            })
            .collect();
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crowd shadow caster layout"),
            bind_group_layouts: &[camera_layout, atlas_layout, &empty_layout, &draw_layout],
            push_constant_ranges: &[],
        });
        // Same raster state as the dynamic-geometry caster pipeline (`Shadow::new`).
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crowd sun moon alpha-tested shadow map"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("crowd_shadow_vertex"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("shadow_fragment"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: super::DEPTH,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2., clamp: 0. },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let compute_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crowd cull and bone pass"),
            source: wgpu::ShaderSource::Wgsl(prepare_source().into()),
        });
        let compute_pipeline = |layout: &wgpu::BindGroupLayout, entry: &'static str| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &compute_module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let prepare = compute_pipeline(&prepare_layout, "crowd_prepare");
        let args_pipeline = compute_pipeline(&args_layout, "crowd_args");
        let storage = wgpu::BufferUsages::STORAGE;
        Self {
            pipelines,
            shadow,
            prepare,
            args_pipeline,
            draw_layout,
            prepare_layout,
            args_layout,
            empty,
            view: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("crowd presentation origin"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("crowd cull parameters"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            mesh_revision: None,
            vertices: Grow::new(device, "crowd composite vertices", wgpu::BufferUsages::VERTEX),
            indices: Grow::new(device, "crowd composite indices", wgpu::BufferUsages::INDEX),
            bones: Grow::new(device, "crowd composite bones", storage),
            clip_revision: None,
            clips: Grow::new(device, "crowd clip table", storage),
            clip_headers: 0,
            instances: Grow::new(device, "crowd instances", storage),
            groups: Grow::new(device, "crowd composite cull table", storage),
            matrices: Grow::new(device, "crowd bone matrices", storage),
            visible: Grow::new(device, "crowd visible instances", storage | wgpu::BufferUsages::VERTEX),
            counts: Grow::new(device, "crowd visible counts", storage),
            args: Grow::new(device, "crowd indirect draws", storage | wgpu::BufferUsages::INDIRECT),
            binds: None,
            plan: Plan::default(),
            uploaded_bytes: 0,
        }
    }

    /// Uploads this frame's crowd. `views` are camera-relative clip matrices (camera, then
    /// shadow cascades), `origin` the scene origin relative to the camera and
    /// `pixels_per_unit` the camera projection's vertical scale times half the viewport
    /// height. Returns the number of instances to cull.
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &CrowdScene,
        views: &[Mat4],
        origin: Vec3,
        pixels_per_unit: f32,
    ) -> u32 {
        self.uploaded_bytes = 0;
        self.plan.instances = 0;
        let meshes = &scene.meshes;
        if scene.instances.is_empty() || meshes.groups.is_empty() || views.is_empty() {
            return 0;
        }
        let mut uploaded = 0;
        let mut write = |queue: &wgpu::Queue, buffer: &wgpu::Buffer, bytes: &[u8]| {
            queue.write_buffer(buffer, 0, bytes);
            uploaded += bytes.len() as u64;
        };
        let mut rebind = false;
        if self.mesh_revision != Some(meshes.revision) {
            let vertices: &[u8] = bytemuck::cast_slice(&meshes.vertices);
            let indices: &[u8] = bytemuck::cast_slice(&meshes.indices);
            let bones: &[u8] = bytemuck::cast_slice(&meshes.bones);
            self.vertices.ensure(device, vertices.len() as u64);
            self.indices.ensure(device, indices.len() as u64);
            rebind |= self.bones.ensure(device, bones.len() as u64);
            write(queue, &self.vertices.buffer, vertices);
            write(queue, &self.indices.buffer, indices);
            write(queue, &self.bones.buffer, bones);
            self.mesh_revision = Some(meshes.revision);
        }
        if self.clip_revision != Some(scene.clips.revision()) {
            let (headers, frames) = scene.clips.gpu_tables();
            let mut table: Vec<[u32; 4]> = headers;
            self.clip_headers = table.len() as u32;
            for sample in &frames {
                table.push(bytemuck::cast([sample.rotation[0], sample.rotation[1], sample.rotation[2], sample.weight]));
                table.push(bytemuck::cast([sample.offset[0], sample.offset[1], sample.offset[2], 0_f32]));
            }
            if table.is_empty() {
                table.push([0; 4]);
            }
            let bytes: &[u8] = bytemuck::cast_slice(&table);
            rebind |= self.clips.ensure(device, bytes.len() as u64);
            write(queue, &self.clips.buffer, bytes);
            self.clip_revision = Some(scene.clips.revision());
        }
        let instances = scene.instances.len() as u32;
        let group_count = meshes.groups.len();
        let views_count = views.len().min(MAX_VIEWS) as u32;
        let mut groups = Vec::with_capacity(group_count);
        self.plan.groups.clear();
        let mut prefix = 0;
        for (index, group) in meshes.groups.iter().enumerate() {
            let count = scene.counts.get(index).copied().unwrap_or(0);
            groups.push(Group {
                bone_first: group.bone_first,
                bone_count: group.bone_count,
                prefix,
                count,
                hip: group.hip,
                radius: group.radius,
            });
            self.plan.groups.push((count, prefix));
            prefix += count;
        }
        let entries = views_count * group_count as u32 * model::LODS as u32 * MATERIALS as u32;
        let mut args = Vec::with_capacity(entries as usize * 5);
        for _ in 0..views_count {
            for group in &meshes.groups {
                for lod in 0..model::LODS {
                    for material in 0..MATERIALS {
                        args.extend([
                            group.counts[material][lod],
                            0,
                            group.first_index[material],
                            group.base_vertex as u32,
                            0,
                        ]);
                    }
                }
            }
        }
        let mut params = Params {
            planes: [[0.; 4]; 6 * MAX_VIEWS],
            eye: (-origin).extend(pixels_per_unit).to_array(),
            lod: [LOD_PIXELS[0], LOD_PIXELS[1], 0., 0.],
            sizes: [instances, group_count as u32, views_count, instances],
            tables: [self.clip_headers, MATERIALS as u32, entries, 0],
        };
        let shift = Mat4::from_translation(origin);
        for (view, matrix) in views.iter().take(MAX_VIEWS).enumerate() {
            params.planes[view * 6..view * 6 + 6].copy_from_slice(&planes(*matrix * shift));
        }
        rebind |= self.instances.ensure(device, u64::from(instances) * std::mem::size_of::<model::CrowdInstance>() as u64);
        rebind |= self.groups.ensure(device, (group_count * std::mem::size_of::<Group>()) as u64);
        rebind |= self.matrices.ensure(device, u64::from(scene.bones.max(1)) * 48);
        rebind |= self.visible.ensure(device, u64::from(views_count * 3 * instances) * 4);
        rebind |= self.counts.ensure(device, u64::from(views_count * group_count as u32 * 3) * 4);
        rebind |= self.args.ensure(device, u64::from(entries) * ARGS_BYTES);
        write(queue, &self.instances.buffer, bytemuck::cast_slice(&scene.instances));
        write(queue, &self.groups.buffer, bytemuck::cast_slice(&groups));
        write(queue, &self.args.buffer, bytemuck::cast_slice(&args));
        write(queue, &self.params, bytemuck::bytes_of(&params));
        write(queue, &self.view, bytemuck::bytes_of(&origin.extend(0.).to_array()));
        if rebind || self.binds.is_none() {
            self.binds = Some(self.bind(device));
        }
        self.uploaded_bytes = uploaded;
        self.plan.views = views_count;
        self.plan.instances = instances;
        self.plan.entries = entries;
        self.plan.meshes = Arc::clone(meshes);
        instances
    }

    fn bind(&self, device: &wgpu::Device) -> Binds {
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
        }
        Binds {
            draw: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crowd instances and bone matrices"),
                layout: &self.draw_layout,
                entries: &[entry(0, &self.view), entry(1, &self.instances.buffer), entry(2, &self.matrices.buffer)],
            }),
            prepare: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crowd cull and bone pass"),
                layout: &self.prepare_layout,
                entries: &[
                    entry(0, &self.params),
                    entry(1, &self.instances.buffer),
                    entry(2, &self.groups.buffer),
                    entry(3, &self.bones.buffer),
                    entry(4, &self.clips.buffer),
                    entry(5, &self.matrices.buffer),
                    entry(6, &self.visible.buffer),
                    entry(7, &self.counts.buffer),
                ],
            }),
            args: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crowd indirect arguments"),
                layout: &self.args_layout,
                entries: &[entry(0, &self.params), entry(7, &self.counts.buffer), entry(8, &self.args.buffer)],
            }),
        }
    }

    /// Culls, picks detail levels, evaluates bones and writes the indirect draws.
    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(binds), true) = (&self.binds, self.plan.instances > 0) else { return };
        let groups = self.plan.groups.len() as u64;
        encoder.clear_buffer(&self.counts.buffer, 0, Some(u64::from(self.plan.views) * groups * 3 * 4));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("crowd cull and bone pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.prepare);
        pass.set_bind_group(0, &binds.prepare, &[]);
        pass.dispatch_workgroups(self.plan.instances.div_ceil(WORKGROUP), 1, 1);
        pass.set_pipeline(&self.args_pipeline);
        pass.set_bind_group(0, &binds.args, &[]);
        pass.dispatch_workgroups(self.plan.entries.div_ceil(WORKGROUP), 1, 1);
    }

    /// Draws the crowd in `view` of an open pass whose groups 0-2 are bound. `shadow`
    /// selects the caster pipeline (depth-writing materials only). Leaves vertex slot 1
    /// and the index buffer replaced. Returns the draw calls issued.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, view: usize, shadow: bool) -> u64 {
        let Some(binds) = &self.binds else { return 0 };
        if self.plan.instances == 0 || view as u32 >= self.plan.views {
            return 0;
        }
        let meshes = &self.plan.meshes;
        if shadow {
            pass.set_pipeline(&self.shadow);
            pass.set_bind_group(2, &self.empty, &[]);
        }
        pass.set_bind_group(3, &binds.draw, &[]);
        pass.set_vertex_buffer(0, self.vertices.buffer.slice(..));
        pass.set_index_buffer(self.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
        let total = u64::from(self.plan.instances);
        let group_count = meshes.groups.len();
        let mut calls = 0;
        for (material, &entity_material) in model::MATERIALS.iter().enumerate() {
            let descriptor = DESCRIPTORS[entity_material as usize];
            if shadow && !(descriptor.write_depth && descriptor.shader.kind <= 2) {
                continue;
            }
            if !shadow {
                pass.set_pipeline(&self.pipelines[material]);
            }
            for (g, group) in meshes.groups.iter().enumerate() {
                let (count, prefix) = self.plan.groups[g];
                if count == 0 {
                    continue;
                }
                for lod in 0..model::LODS {
                    if group.counts[material][lod] == 0 {
                        continue;
                    }
                    let region = view as u64 * 3 * total + 3 * u64::from(prefix) + lod as u64 * u64::from(count);
                    let entry = ((view * group_count + g) * model::LODS + lod) * MATERIALS + material;
                    pass.set_vertex_buffer(1, self.visible.buffer.slice(region * 4..(region + u64::from(count)) * 4));
                    pass.draw_indexed_indirect(&self.args.buffer, entry as u64 * ARGS_BYTES);
                    calls += 1;
                }
            }
        }
        calls
    }
}
