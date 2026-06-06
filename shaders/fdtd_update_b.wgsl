// shaders/fdtd_update_b.wgsl  —  Yee-step B update from -curl(E).
//   B_{n+1} = (B_n - dt * curl(E_{n+1})) * (1 - dt*sigma) * boundary_damp

@compute @workgroup_size(8, 8, 4)
fn update_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = fp.grid_n;
    if (gid.x >= n || gid.y >= n || gid.z >= n) { return; }

    let xi = i32(gid.x);
    let yi = i32(gid.y);
    let zi = i32(gid.z);
    let i  = lin(xi, yi, zi);

    let exp_ = grid[lin(xi + 1, yi, zi)].e.xyz;
    let exm  = grid[lin(xi - 1, yi, zi)].e.xyz;
    let eyp  = grid[lin(xi, yi + 1, zi)].e.xyz;
    let eym  = grid[lin(xi, yi - 1, zi)].e.xyz;
    let ezp  = grid[lin(xi, yi, zi + 1)].e.xyz;
    let ezm  = grid[lin(xi, yi, zi - 1)].e.xyz;

    let inv2dx = 0.5 / fp.dx;
    let curl_e = vec3<f32>(
        (eyp.z - eym.z) * inv2dx - (ezp.y - ezm.y) * inv2dx,
        (ezp.x - ezm.x) * inv2dx - (exp_.z - exm.z) * inv2dx,
        (exp_.y - exm.y) * inv2dx - (eyp.x - eym.x) * inv2dx,
    );

    var b = grid[i].b.xyz - fp.dt * curl_e;
    let damp = (1.0 - fp.dt * fp.sigma) * boundary_damp(xi, yi, zi);
    b = b * damp;

    grid[i].b = vec4<f32>(b, 0.0);
}
