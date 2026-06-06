// shaders/fdtd_common.wgsl
//
// Spacetime Algebra (STA) FDTD on a 3D Yee-style collocated grid.
// The electromagnetic Faraday bivector  F = E + I c B  is stored as
// two vec4<f32> per cell (32 B / cell, 16-byte aligned).
//
// In normalized units we set c = mu0 = eps0 = 1 so Maxwell's equations
// in vacuum reduce to the multivector derivative
//     dE/dt = +curl(B)
//     dB/dt = -curl(E)
// We keep them as separate passes (Yee leapfrog) so PEC masks and
// boundary damping can be applied between half-steps.

struct FdtdParams {
    grid_n:        u32,
    pml_layers:    u32,
    enable_mirror: u32,
    time_step:     u32,

    dt:            f32,
    dx:            f32,
    sigma:         f32,     // bulk numerical damping
    pml_strength:  f32,     // outer-shell absorbing strength

    mirror_min:    vec4<f32>,   // .xyz cell-coords (inclusive)
    mirror_max:    vec4<f32>,   // .xyz cell-coords (exclusive)

    seed_center:   vec4<f32>,   // .xyz cell-coords, .w = ring radius
    seed_axis:     vec4<f32>,   // unit propagation direction (.xyz)
    seed_width:    f32,
    seed_amp:      f32,
    seed_kick:     f32,         // longitudinal kick (boosts +x propagation)
    world_extent:  f32,         // half-side of the rendered cube in world units
};

struct Cell {
    e: vec4<f32>,     // (Ex, Ey, Ez, 0)
    b: vec4<f32>,     // (Bx, By, Bz, 0)
};

@group(0) @binding(0) var<uniform>             fp:   FdtdParams;
@group(0) @binding(1) var<storage, read_write> grid: array<Cell>;
@group(0) @binding(2) var<storage, read>       mask: array<u32>;   // 0 = vacuum, 1 = PEC

fn lin(x: i32, y: i32, z: i32) -> u32 {
    let n = i32(fp.grid_n);
    let xi = max(0, min(n - 1, x));
    let yi = max(0, min(n - 1, y));
    let zi = max(0, min(n - 1, z));
    return u32((zi * n + yi) * n + xi);
}

// Absorbing-shell damping factor for cell (x,y,z): 1.0 in the interior,
// drops smoothly to (1 - pml_strength) at the outermost layer.
fn boundary_damp(x: i32, y: i32, z: i32) -> f32 {
    let n  = i32(fp.grid_n);
    let pl = i32(fp.pml_layers);
    let dx = min(x, n - 1 - x);
    let dy = min(y, n - 1 - y);
    let dz = min(z, n - 1 - z);
    let d  = min(dx, min(dy, dz));
    if (d >= pl) { return 1.0; }
    let t = f32(pl - d) / f32(max(pl, 1));
    return 1.0 - fp.pml_strength * t * t;
}
