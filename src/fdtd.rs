// src/fdtd.rs
//
// Real Spacetime-Algebra FDTD on a 3-D collocated grid.
//
//   * Per cell we store the Faraday bivector  F = E + I c B  as two
//     vec4<f32> (32 bytes, 16-byte aligned, GPU-friendly).
//   * Yee-style leapfrog:   E_{n+1} = E_n + dt * curl(B)
//                            B_{n+1} = B_n - dt * curl(E_{n+1})
//     (collocated, central differences; small numerical damping is applied
//     to suppress the well-known checkerboard instability of collocated grids).
//   * Outer shell is an absorbing layer (poor man's PML).
//   * The mirror is a perfect electric conductor (PEC) mask that zeros the
//     tangential E inside the marked cells — that is what makes the donut
//     bounce.
//   * Particle advection samples the live grid by trilinear interpolation
//     and moves each particle along the Poynting vector S = E × B, so what
//     you see in 3D is genuine simulated energy transport, not analytic
//     decoration.
//
// CPU work that uses every i7 core (via rayon):
//   * Initial donut seeding (parallel over Z slices).
//   * Mirror-mask construction (parallel over Z slices).

use bytemuck::{Pod, Zeroable};
use rayon::prelude::*;
use wgpu::util::DeviceExt;

// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct FdtdParams {
    pub grid_n:        u32,
    pub pml_layers:    u32,
    pub enable_mirror: u32,
    pub time_step:     u32,

    pub dt:            f32,
    pub dx:            f32,
    pub sigma:         f32,
    pub pml_strength:  f32,

    pub mirror_min:    [f32; 4],
    pub mirror_max:    [f32; 4],

    pub seed_center:   [f32; 4],
    pub seed_axis:     [f32; 4],
    pub seed_width:    f32,
    pub seed_amp:      f32,
    pub seed_kick:     f32,
    pub world_extent:  f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct AdvectParams {
    pub step:        f32,
    pub drift_scale: f32,
    pub n_substeps:  u32,
    pub max_age:     f32,
    pub seed_lo:     [f32; 4],
    pub seed_hi:     [f32; 4],
    pub density_gate: f32,
    pub _pad0:       f32,
    pub _pad1:       f32,
    pub _pad2:       f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug, Default)]
pub struct Cell {
    pub e: [f32; 4],
    pub b: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct FdtdParticle {
    pub pos: [f32; 3],
    pub age: f32,
}

// ---------------------------------------------------------------------------

pub struct FdtdState {
    pub grid_n:             u32,
    pub world_extent:       f32,
    pub time_step:          u32,
    pub mirror_enabled:     bool,
    pub mirror_min_world:   [f32; 3],
    pub mirror_max_world:   [f32; 3],
    pub mirror_gap:         f32,
    pub seed_center_world:  [f32; 3],
    pub seed_axis:          [f32; 3],
    pub seed_radius_world:  f32,
    pub seed_width_world:   f32,
    pub seed_amp:           f32,

    pub particle_count:     u32,

    // GPU
    params_buf:   wgpu::Buffer,
    grid_buf:     wgpu::Buffer,
    mask_buf:     wgpu::Buffer,
    parts_buf:    wgpu::Buffer,
    adv_buf:      wgpu::Buffer,

    fdtd_bind:    wgpu::BindGroup,

    pub update_e: wgpu::ComputePipeline,
    pub update_b: wgpu::ComputePipeline,
    pub advect:   wgpu::ComputePipeline,

    // mirror wireframe (LineList vertices for the existing render_line pipeline)
    pub mirror_verts:        wgpu::Buffer,
    pub mirror_index_buf:    wgpu::Buffer,
    pub mirror_index_count:  u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct LineVertex { pos: [f32; 3], mag: f32 }

impl FdtdState {
    pub fn new(
        device:        &wgpu::Device,
        queue:         &wgpu::Queue,
        grid_n:        u32,
        world_extent:  f32,
        particle_count: u32,
    ) -> Self {
        assert!(grid_n >= 32 && grid_n <= 320);
        let cells = (grid_n as u64).pow(3);

        // ---- buffers ---------------------------------------------------------
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fdtd-params"),
            size: std::mem::size_of::<FdtdParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let grid_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fdtd-grid"),
            size: cells * std::mem::size_of::<Cell>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mask_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fdtd-mask"),
            size: cells * std::mem::size_of::<u32>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let parts_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fdtd-particles"),
            size: (particle_count as u64) * std::mem::size_of::<FdtdParticle>() as u64,
            usage: wgpu::BufferUsages::STORAGE
                 | wgpu::BufferUsages::VERTEX
                 | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let adv_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fdtd-adv-params"),
            size: std::mem::size_of::<AdvectParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ---- bind group ------------------------------------------------------
        let fdtd_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fdtd-bgl"),
            entries: &[
                bgl_uniform(0),
                bgl_storage_rw(1),
                bgl_storage_ro(2),
                bgl_storage_rw(3),
                bgl_uniform(4),
            ],
        });

        let fdtd_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fdtd-bg"),
            layout: &fdtd_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: grid_buf.as_entire_binding()   },
                wgpu::BindGroupEntry { binding: 2, resource: mask_buf.as_entire_binding()   },
                wgpu::BindGroupEntry { binding: 3, resource: parts_buf.as_entire_binding()  },
                wgpu::BindGroupEntry { binding: 4, resource: adv_buf.as_entire_binding()    },
            ],
        });

        // ---- shaders ---------------------------------------------------------
        let common_src   = include_str!("../shaders/fdtd_common.wgsl");
        let update_e_src = concat_wgsl(common_src, include_str!("../shaders/fdtd_update_e.wgsl"));
        let update_b_src = concat_wgsl(common_src, include_str!("../shaders/fdtd_update_b.wgsl"));
        let advect_src   = concat_wgsl(common_src, include_str!("../shaders/fdtd_particles.wgsl"));

        let mod_e = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fdtd-update-e"),
            source: wgpu::ShaderSource::Wgsl(update_e_src.into()),
        });
        let mod_b = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fdtd-update-b"),
            source: wgpu::ShaderSource::Wgsl(update_b_src.into()),
        });
        let mod_a = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fdtd-advect"),
            source: wgpu::ShaderSource::Wgsl(advect_src.into()),
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fdtd-pl"),
            bind_group_layouts: &[&fdtd_bgl],
            push_constant_ranges: &[],
        });

        let update_e = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fdtd-update-e-pipe"),
            layout: Some(&pl),
            module: &mod_e,
            entry_point: "update_e",
            compilation_options: Default::default(),
            cache: None,
        });
        let update_b = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fdtd-update-b-pipe"),
            layout: Some(&pl),
            module: &mod_b,
            entry_point: "update_b",
            compilation_options: Default::default(),
            cache: None,
        });
        let advect = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fdtd-advect-pipe"),
            layout: Some(&pl),
            module: &mod_a,
            entry_point: "advect",
            compilation_options: Default::default(),
            cache: None,
        });

        // ---- mirror wireframe placeholders ---------------------------------
        let (mirror_verts, mirror_index_buf, mirror_index_count) =
            build_mirror_wire(device, [0.0_f32; 3], [0.0_f32; 3]);

        let mut s = Self {
            grid_n,
            world_extent,
            time_step: 0,
            mirror_enabled: true,
            mirror_min_world: [ world_extent * 0.55, -world_extent * 0.55, -world_extent * 0.55],
            mirror_max_world: [ world_extent * 0.60,  world_extent * 0.55,  world_extent * 0.55],
            mirror_gap:        world_extent * 0.40,
            seed_center_world: [-world_extent * 0.55, 0.0, 0.0],
            seed_axis:         [1.0, 0.0, 0.0],
            seed_radius_world: world_extent * 0.18,
            seed_width_world:  world_extent * 0.10,
            seed_amp:          1.0,
            particle_count,
            params_buf, grid_buf, mask_buf, parts_buf, adv_buf,
            fdtd_bind,
            update_e, update_b, advect,
            mirror_verts, mirror_index_buf, mirror_index_count,
        };

        s.write_params(queue, 0.45, 0.001, 0.04, 8);
        s.write_adv_params(queue, 0.03, 6.0, 4, 4.0, 0.0, 1.0, 0.0);
        s.reseed(device, queue);
        s
    }

    pub fn write_params(
        &mut self,
        queue:        &wgpu::Queue,
        dt:           f32,
        sigma:        f32,
        pml_strength: f32,
        pml_layers:   u32,
    ) {
        let dx = 1.0;
        let p = FdtdParams {
            grid_n:        self.grid_n,
            pml_layers,
            enable_mirror: self.mirror_enabled as u32,
            time_step:     self.time_step,
            dt, dx, sigma, pml_strength,
            mirror_min: pad4(self.mirror_min_world),
            mirror_max: pad4(self.mirror_max_world),
            seed_center: [
                self.seed_center_world[0], self.seed_center_world[1],
                self.seed_center_world[2], self.seed_radius_world],
            seed_axis: pad4(self.seed_axis),
            seed_width: self.seed_width_world,
            seed_amp:   self.seed_amp,
            seed_kick:  0.0,
            world_extent: self.world_extent,
        };
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&p));
    }

    pub fn write_adv_params(
        &self,
        queue:           &wgpu::Queue,
        step:            f32,
        drift_scale:     f32,
        n_substeps:      u32,
        max_age:         f32,
        density_gate:    f32,
        respawn_scale:   f32,
        respawn_offset_x: f32,
    ) {
        let we = self.world_extent;
        let cx = self.seed_center_world[0] + respawn_offset_x;
        // Respawn box: thin slab at the donut starting position so particles
        // continuously seed where the energy actually is.  `respawn_scale`
        // grows/shrinks the slab live without touching the GPU buffers.
        let half_x = self.seed_width_world  * respawn_scale.max(0.01);
        let half_t = self.seed_radius_world * 1.4 * respawn_scale.max(0.01);
        let lo = [
            cx - half_x,
            self.seed_center_world[1] - half_t,
            self.seed_center_world[2] - half_t,
        ];
        let hi = [
            cx + half_x,
            self.seed_center_world[1] + half_t,
            self.seed_center_world[2] + half_t,
        ];
        // clamp inside the world box
        let lo = clamp3(lo, -we * 0.95, we * 0.95);
        let hi = clamp3(hi, -we * 0.95, we * 0.95);
        let ap = AdvectParams {
            step, drift_scale, n_substeps, max_age,
            seed_lo: pad4(lo),
            seed_hi: pad4(hi),
            density_gate,
            _pad0: 0.0, _pad1: 0.0, _pad2: 0.0,
        };
        queue.write_buffer(&self.adv_buf, 0, bytemuck::bytes_of(&ap));
    }

    /// Build the donut field and the mirror mask on the CPU using rayon
    /// (parallel over Z slices), then upload to the GPU.
    pub fn reseed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let n = self.grid_n as usize;
        let we = self.world_extent;
        let sc = self.seed_center_world;
        let r0 = self.seed_radius_world;
        let w  = self.seed_width_world.max(1e-3);
        let a  = self.seed_amp;

        let cell_to_world = |i: usize| -> f32 {
            (i as f32) / (n as f32 - 1.0) * 2.0 * we - we
        };

        // Parallel over Z slices: real multi-core CPU work.
        let cells: Vec<Cell> = (0..n).into_par_iter()
            .flat_map_iter(|zi| {
                let z = cell_to_world(zi);
                let mut row: Vec<Cell> = Vec::with_capacity(n * n);
                for yi in 0..n {
                    let y = cell_to_world(yi);
                    for xi in 0..n {
                        let x = cell_to_world(xi);
                        let dx = x - sc[0];
                        let dy = y - sc[1];
                        let dz = z - sc[2];
                        let rho2 = dy*dy + dz*dz;
                        let rho  = rho2.sqrt().max(1e-6);
                        let env  = a * (-((rho - r0).powi(2) + dx*dx) / (w*w)).exp();

                        // E azimuthal around the +x axis
                        let ey = -dz / rho * env;
                        let ez =  dy / rho * env;

                        // B poloidal: B = x̂ × E ⇒ E × B points in +x  → propagates +x
                        let by = -dy / rho * env;
                        let bz = -dz / rho * env;

                        row.push(Cell {
                            e: [0.0, ey, ez, 0.0],
                            b: [0.0, by, bz, 0.0],
                        });
                    }
                }
                row.into_iter()
            })
            .collect();
        queue.write_buffer(&self.grid_buf, 0, bytemuck::cast_slice(&cells));

        // Parallel mirror mask.
        let mm0 = self.mirror_min_world;
        let mm1 = self.mirror_max_world;
        let mask: Vec<u32> = (0..n).into_par_iter()
            .flat_map_iter(|zi| {
                let z = cell_to_world(zi);
                let mut row = Vec::with_capacity(n * n);
                for yi in 0..n {
                    let y = cell_to_world(yi);
                    for xi in 0..n {
                        let x = cell_to_world(xi);
                        let inside =
                            x >= mm0[0] && x <= mm1[0] &&
                            y >= mm0[1] && y <= mm1[1] &&
                            z >= mm0[2] && z <= mm1[2];
                        row.push(inside as u32);
                    }
                }
                row.into_iter()
            })
            .collect();
        queue.write_buffer(&self.mask_buf, 0, bytemuck::cast_slice(&mask));

        // Reset particle ages so they all respawn on the next advect pass.
        let parts: Vec<FdtdParticle> = (0..self.particle_count)
            .map(|_| FdtdParticle { pos: [we * 2.0, 0.0, 0.0], age: 1e9 })
            .collect();
        queue.write_buffer(&self.parts_buf, 0, bytemuck::cast_slice(&parts));

        // Rebuild the mirror wireframe to match current bounds.
        let (mv, mi, mc) = build_mirror_wire(device, mm0, mm1);
        self.mirror_verts = mv;
        self.mirror_index_buf = mi;
        self.mirror_index_count = mc;

        self.time_step = 0;
    }

    /// Position the PEC mirror as a flat wall sitting `gap` world-units in from
    /// the cube's far (+x) face. `gap == 0` → the wall is flush against the cube
    /// (touching). Larger gap pulls the wall inward toward the seed, so the
    /// pulse meets it sooner and the bounce is easier to see. The reflecting
    /// mask only changes on the next reseed; the wireframe moves immediately.
    pub fn set_mirror_gap(&mut self, device: &wgpu::Device, gap: f32) {
        if (gap - self.mirror_gap).abs() < 1e-5 { return; }
        let we = self.world_extent;
        let thick = we * 0.05;                       // slab thickness (a few cells)
        // Keep the wall in front of the seed (which sits at x = -0.55·we).
        let outer = (we - gap).clamp(-0.35 * we + thick, we);
        let inner = outer - thick;
        let ext = we * 0.85;                         // full-height wall in y/z
        self.mirror_min_world = [inner, -ext, -ext];
        self.mirror_max_world = [outer,  ext,  ext];
        self.mirror_gap = gap;
        self.rebuild_mirror_wire(device);
    }

    /// Rebuild the pink wireframe to match the current mirror bounds.
    pub fn rebuild_mirror_wire(&mut self, device: &wgpu::Device) {
        let (mv, mi, mc) =
            build_mirror_wire(device, self.mirror_min_world, self.mirror_max_world);
        self.mirror_verts = mv;
        self.mirror_index_buf = mi;
        self.mirror_index_count = mc;
    }

    #[allow(dead_code)]
    pub fn bind_group(&self) -> &wgpu::BindGroup { &self.fdtd_bind }

    /// Dispatch `substeps` of Yee leapfrog (each = update_e + update_b).
    pub fn step(&mut self, encoder: &mut wgpu::CommandEncoder, substeps: u32) {
        let n  = self.grid_n;
        let wx = (n + 7) / 8;
        let wy = (n + 7) / 8;
        let wz = (n + 3) / 4;
        for _ in 0..substeps {
            {
                let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("fdtd-e"),
                    timestamp_writes: None,
                });
                cp.set_pipeline(&self.update_e);
                cp.set_bind_group(0, &self.fdtd_bind, &[]);
                cp.dispatch_workgroups(wx, wy, wz);
            }
            {
                let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("fdtd-b"),
                    timestamp_writes: None,
                });
                cp.set_pipeline(&self.update_b);
                cp.set_bind_group(0, &self.fdtd_bind, &[]);
                cp.dispatch_workgroups(wx, wy, wz);
            }
            self.time_step = self.time_step.wrapping_add(1);
        }
    }

    pub fn dispatch_advect(&self, encoder: &mut wgpu::CommandEncoder) {
        let groups = (self.particle_count + 63) / 64;
        let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fdtd-advect"),
            timestamp_writes: None,
        });
        cp.set_pipeline(&self.advect);
        cp.set_bind_group(0, &self.fdtd_bind, &[]);
        cp.dispatch_workgroups(groups, 1, 1);
    }

    pub fn particle_buffer(&self) -> &wgpu::Buffer { &self.parts_buf }
}

// ---------------------------------------------------------------------------

fn bgl_uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn bgl_storage_rw(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn bgl_storage_ro(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pad4(v: [f32; 3]) -> [f32; 4] { [v[0], v[1], v[2], 0.0] }
fn clamp3(v: [f32; 3], lo: f32, hi: f32) -> [f32; 3] {
    [v[0].clamp(lo, hi), v[1].clamp(lo, hi), v[2].clamp(lo, hi)]
}

fn concat_wgsl(a: &str, b: &str) -> String {
    let mut s = String::with_capacity(a.len() + b.len() + 1);
    s.push_str(a);
    s.push('\n');
    s.push_str(b);
    s
}

/// Build a 12-edge wireframe box for the mirror in world coords.  Each vertex
/// is a `LineVertex { pos, mag }` so it pairs with the existing `render_line`
/// pipeline (which expects exactly that layout).  `mag = 1` so the colour
/// modulation stays bright.
fn build_mirror_wire(
    device: &wgpu::Device,
    lo: [f32; 3],
    hi: [f32; 3],
) -> (wgpu::Buffer, wgpu::Buffer, u32) {
    let v = |x: f32, y: f32, z: f32| LineVertex { pos: [x, y, z], mag: 1.0 };
    let verts = [
        v(lo[0], lo[1], lo[2]), v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]), v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]), v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]), v(lo[0], hi[1], hi[2]),
    ];
    let idx: [u32; 24] = [
        0,1, 1,2, 2,3, 3,0,
        4,5, 5,6, 6,7, 7,4,
        0,4, 1,5, 2,6, 3,7,
    ];
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mirror-verts"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mirror-idx"),
        contents: bytemuck::cast_slice(&idx),
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
    });
    (vb, ib, idx.len() as u32)
}
