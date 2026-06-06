// shaders/streamlines.wgsl — one thread per streamline, RK4 integrator.
// Vertex layout written: vec4(x, y, z, |field|).  The w component is used
// by the line vertex shader for colour intensity.

struct Vertex {
    pos: vec3<f32>,
    mag: f32,
};

@group(0) @binding(1) var<storage, read>       seeds: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> verts: array<Vertex>;

@compute @workgroup_size(64)
fn cs_trace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let line = gid.x;
    if (line >= params.seed_count) { return; }

    let n = params.steps_per_line;
    let base = line * n;

    var p = seeds[line].xyz;
    let h = params.step_len;

    // forward-only trace; centred seeding gives full lines because the
    // hopfion field lines are closed loops.
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        // RK4 with direction normalization (arc-length parameterization)
        let k1 = safe_normalize(field_dir(p));
        let k2 = safe_normalize(field_dir(p + 0.5 * h * k1));
        let k3 = safe_normalize(field_dir(p + 0.5 * h * k2));
        let k4 = safe_normalize(field_dir(p + h       * k3));
        let dir = safe_normalize(k1 + 2.0*k2 + 2.0*k3 + k4);

        let mag = field_mag(p);
        var v: Vertex;
        v.pos = p;
        v.mag = mag;
        verts[base + i] = v;

        p = p + h * dir;
    }
}
