// shaders/render_line.wgsl — render the streamline vertex buffer as line list,
// with a traveling-wave colour modulation so flow is visible even though the
// underlying field geometry is the same each frame.

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
    @location(1) mag: f32,
};

struct VsOut {
    @builtin(position) clip:  vec4<f32>,
    @location(0)       color: vec4<f32>,
};

@vertex
fn vs_main(v: VertexIn, @builtin(vertex_index) vi: u32) -> VsOut {
    var o: VsOut;
    o.clip = cam.view_proj * vec4<f32>(v.pos, 1.0);

    // arc-position along this streamline in [0,1)
    let step_i = vi % max(rp.steps_per_line, 1u);
    let u = f32(step_i) / max(f32(rp.steps_per_line), 1.0);

    // traveling brightness wave + slow base intensity from |field|
    let tw = 0.5 + 0.5 * sin(u * 28.0 - rp.flow_phase * 6.0);
    let m  = clamp(v.mag * 1.5, 0.0, 1.0);
    let intensity = mix(0.18, 1.0, m) * mix(0.35, 1.0, tw);

    o.color = vec4<f32>(rp.color.rgb * intensity, rp.color.a * intensity);
    return o;
}

@fragment
fn fs_main(o: VsOut) -> @location(0) vec4<f32> {
    return o.color;
}
