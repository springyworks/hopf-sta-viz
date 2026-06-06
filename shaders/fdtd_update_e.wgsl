// shaders/fdtd_update_e.wgsl  —  Yee-step E update from curl(B).
//   E_{n+1} = (E_n + dt * curl(B)) * (1 - dt*sigma) * boundary_damp
//   PEC mask zeros E inside conductors.

@compute @workgroup_size(8, 8, 4)
fn update_e(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = fp.grid_n;
    if (gid.x >= n || gid.y >= n || gid.z >= n) { return; }

    let xi = i32(gid.x);
    let yi = i32(gid.y);
    let zi = i32(gid.z);
    let i  = lin(xi, yi, zi);

    let bxp = grid[lin(xi + 1, yi, zi)].b.xyz;
    let bxm = grid[lin(xi - 1, yi, zi)].b.xyz;
    let byp = grid[lin(xi, yi + 1, zi)].b.xyz;
    let bym = grid[lin(xi, yi - 1, zi)].b.xyz;
    let bzp = grid[lin(xi, yi, zi + 1)].b.xyz;
    let bzm = grid[lin(xi, yi, zi - 1)].b.xyz;

    let inv2dx = 0.5 / fp.dx;
    let curl_b = vec3<f32>(
        (byp.z - bym.z) * inv2dx - (bzp.y - bzm.y) * inv2dx,
        (bzp.x - bzm.x) * inv2dx - (bxp.z - bxm.z) * inv2dx,
        (bxp.y - bxm.y) * inv2dx - (byp.x - bym.x) * inv2dx,
    );

    var e = grid[i].e.xyz + fp.dt * curl_b;
    let damp = (1.0 - fp.dt * fp.sigma) * boundary_damp(xi, yi, zi);
    e = e * damp;

    if (fp.enable_mirror != 0u && mask[i] != 0u) {
        e = vec3<f32>(0.0);   // perfect electric conductor: tangential E = 0
    }

    grid[i].e = vec4<f32>(e, 0.0);
}
