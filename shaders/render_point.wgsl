// shaders/render_point.wgsl — render the particle storage buffer as points.

struct Camera {
    view_proj: mat4x4<f32>,
    eye:       vec4<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

struct RenderParams {
    color:          vec4<f32>,
    flow_phase:     f32,
    steps_per_line: u32,
    _pad0:          u32,
    _pad1:          u32,
};
@group(0) @binding(1) var<uniform> rp: RenderParams;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) age: f32,
};

struct VsOut {
    @builtin(position) clip:  vec4<f32>,
    @location(0)       color: vec4<f32>,
};

@vertex
fn vs_main(v: VertexIn) -> VsOut {
    var o: VsOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos, 1.0);
    let life = clamp(v.age * 0.25, 0.0, 1.0);
    let env  = smoothstep(0.0, 0.15, life) * (1.0 - smoothstep(0.7, 1.0, life));
    o.color = vec4<f32>(rp.color.rgb, rp.color.a * env);
    return o;
}

@fragment
fn fs_main(o: VsOut) -> @location(0) vec4<f32> {
    return o.color;
}
