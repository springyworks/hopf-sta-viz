// shaders/fdtd_particles.wgsl — advect particles in the live FDTD grid.
//
// Particle position is in WORLD coordinates (the cube [-W, +W]^3).  The grid
// is sampled trilinearly after converting world -> grid index.  Particles
// drift along the Poynting vector S = E × B so the visualization shows
// real energy transport in the simulated field.

struct Particle { pos: vec3<f32>, age: f32 };

@group(0) @binding(3) var<storage, read_write> parts: array<Particle>;

struct AdvectParams {
    step:           f32,     // world-space distance per substep at unit |S|
    drift_scale:    f32,     // multiplier applied to |S|
    n_substeps:     u32,
    max_age:        f32,
    seed_lo:        vec4<f32>,   // world-space respawn box (.xyz)
    seed_hi:        vec4<f32>,
    density_gate:   f32,         // particles with |S| below this respawn
    _pad0:          f32,
    _pad1:          f32,
    _pad2:          f32,
};
@group(0) @binding(4) var<uniform> adv: AdvectParams;

fn world_to_grid(w: vec3<f32>) -> vec3<f32> {
    let we  = fp.world_extent;
    let wex = fp.world_ext_x;
    let gx = (w.x + wex) / (2.0 * wex) * f32(fp.nx - 1u);
    let gy = (w.y + we ) / (2.0 * we ) * f32(fp.ny - 1u);
    let gz = (w.z + we ) / (2.0 * we ) * f32(fp.nz - 1u);
    return vec3<f32>(gx, gy, gz);
}

struct EBPair { e: vec3<f32>, b: vec3<f32> };

fn sample_eb_world(pw: vec3<f32>) -> EBPair {
    let hi = vec3<f32>(f32(fp.nx) - 2.0, f32(fp.ny) - 2.0, f32(fp.nz) - 2.0);
    let pc = clamp(world_to_grid(pw), vec3<f32>(1.0), hi);
    let x0 = i32(floor(pc.x));
    let y0 = i32(floor(pc.y));
    let z0 = i32(floor(pc.z));
    let fr = pc - vec3<f32>(f32(x0), f32(y0), f32(z0));

    var e_sum = vec3<f32>(0.0);
    var b_sum = vec3<f32>(0.0);
    for (var dz = 0; dz < 2; dz = dz + 1) {
        for (var dy = 0; dy < 2; dy = dy + 1) {
            for (var dx = 0; dx < 2; dx = dx + 1) {
                let wx = select(1.0 - fr.x, fr.x, dx == 1);
                let wy = select(1.0 - fr.y, fr.y, dy == 1);
                let wz = select(1.0 - fr.z, fr.z, dz == 1);
                let w  = wx * wy * wz;
                let c  = grid[lin(x0 + dx, y0 + dy, z0 + dz)];
                e_sum = e_sum + c.e.xyz * w;
                b_sum = b_sum + c.b.xyz * w;
            }
        }
    }
    var o: EBPair;
    o.e = e_sum;
    o.b = b_sum;
    return o;
}

fn hash(u: u32) -> f32 {
    var v = u ^ 0x9E3779B9u;
    v = (v ^ (v >> 16u)) * 0x7FEB352Du;
    v = (v ^ (v >> 15u)) * 0x846CA68Bu;
    v = v ^ (v >> 16u);
    return f32(v & 0x00FFFFFFu) / f32(0x01000000u);
}

@compute @workgroup_size(64)
fn advect(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&parts)) { return; }
    var p = parts[i];

    for (var s: u32 = 0u; s < adv.n_substeps; s = s + 1u) {
        let eb  = sample_eb_world(p.pos);
        let poy = cross(eb.e, eb.b);
        p.pos = p.pos + poy * adv.drift_scale * adv.step;
        p.age = p.age + 0.016;
    }

    // Density gate: if the local Poynting magnitude is too weak, the
    // particle is sitting in "empty space" and just clutters the cube.
    // Force it to respawn so dots concentrate where the energy is.
    let eb_final = sample_eb_world(p.pos);
    let s_mag    = length(cross(eb_final.e, eb_final.b));
    if (s_mag < adv.density_gate) {
        p.age = adv.max_age + 1.0;
    }

    // Respawn dead / out-of-bounds particles inside the user-provided seed box.
    let we  = fp.world_extent;
    let wex = fp.world_ext_x;
    let lo_w = vec3<f32>(-wex, -we, -we);
    let hi_w = vec3<f32>( wex,  we,  we);
    let oob = any(p.pos < lo_w) || any(p.pos > hi_w);
    if (p.age > adv.max_age || oob) {
        let h1 = hash(i * 3u + fp.time_step);
        let h2 = hash(i * 3u + 1u + fp.time_step);
        let h3 = hash(i * 3u + 2u + fp.time_step);
        let lo = adv.seed_lo.xyz;
        let hi = adv.seed_hi.xyz;
        p.pos = lo + vec3<f32>(h1, h2, h3) * (hi - lo);
        p.age = 0.0;
    }
    parts[i] = p;
}
