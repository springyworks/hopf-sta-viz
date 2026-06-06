// shaders/particles.wgsl — advect points along the selected field; respawn
// when they leave the box or get too slow.

struct Particle {
    pos: vec3<f32>,
    age: f32,
};

@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;

// 3-D hash → uniform random in [0,1]^3 (Hugo Elias style).
fn hash3(p_in: vec3<u32>) -> vec3<f32> {
    var p = p_in;
    p = p * 1664525u + 1013904223u;
    p.x = p.x + p.y * p.z;
    p.y = p.y + p.z * p.x;
    p.z = p.z + p.x * p.y;
    p = p ^ (p >> vec3<u32>(16u));
    p.x = p.x + p.y * p.z;
    p.y = p.y + p.z * p.x;
    p.z = p.z + p.x * p.y;
    return vec3<f32>(
        f32(p.x & 0x00ffffffu) / 16777216.0,
        f32(p.y & 0x00ffffffu) / 16777216.0,
        f32(p.z & 0x00ffffffu) / 16777216.0,
    );
}

fn respawn(i: u32) -> vec3<f32> {
    let seed = vec3<u32>(i * 1973u, i * 9277u + 1u, u32(params.time * 977.0) ^ i);
    let r = hash3(seed) - vec3<f32>(0.5);
    return r * 4.0 * params.scale;
}

@compute @workgroup_size(64)
fn cs_advect(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let n = arrayLength(&particles);
    if (i >= n) { return; }

    var pt = particles[i];

    let v = field_dir(pt.pos);
    pt.pos = pt.pos + params.speed * params.dt * v;
    pt.age = pt.age + params.dt;

    let bound = params.bbox_radius * params.scale;
    if (pt.age > 4.0 || length(pt.pos) > bound) {
        pt.pos = respawn(i);
        pt.age = 0.0;
    }

    particles[i] = pt;
}
