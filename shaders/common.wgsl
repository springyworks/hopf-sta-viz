// shaders/common.wgsl — STA field evaluators and shared bindings.
//
// We work in 3-D Geometric Algebra Cl(3,0) (the algebra of physical space).
// The electromagnetic Faraday bivector is
//     F = E + I c B,    I = e1 e2 e3 (unit pseudoscalar)
//
// The "unified 4D" picture says a plane-wave photon, a flying-donut toroidal
// pulse, and a Rañada hopfion are conformal cousins of a single Bateman seed.
// Here each preset is a different *closed-form projection* of that seed into
// physical 3-space.  All field formulas share the same (preset, time, scale)
// parameter set and the same (E, B) output, so streamlines and particles
// don't care which preset is active.

struct Params {
    time:           f32,
    scale:          f32,
    step_len:       f32,
    speed:          f32,
    field_mode:     u32,   // 0=E, 1=B, 2=Poynting
    steps_per_line: u32,
    seed_count:     u32,
    dt:             f32,
    bbox_radius:    f32,
    preset:         u32,   // 0=Hopfion 1=PhotonHopfion 2=Donut 3=PlanePhoton 4=CPPhoton 5=Trefoil
    flow_phase:     f32,
    _pad0:          u32,
};

@group(0) @binding(0) var<uniform> params: Params;

struct EB { e: vec3<f32>, b: vec3<f32> };

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let m = length(v);
    if (m < 1e-6) { return vec3<f32>(0.0); }
    return v / m;
}

// 0) Rañada hopfion (textbook Hopf-link projection).
fn hopfion_field(p_world: vec3<f32>) -> EB {
    let p = p_world / max(params.scale, 1e-4);
    let x = p.x; let y = p.y; let z = p.z;
    let r2 = x*x + y*y + z*z;
    let inv = 1.0 / ((1.0 + r2) * (1.0 + r2));
    let h  = vec3<f32>(2.0*(x*z + y), 2.0*(y*z - x), 1.0 - x*x - y*y + z*z) * inv;
    let hp = vec3<f32>(2.0*(x*z - y), 2.0*(y*z + x), 1.0 - x*x - y*y + z*z) * inv;
    let c = cos(params.time);
    let s = sin(params.time);
    var o: EB;
    o.e =  c * hp + s * h;
    o.b = -s * hp + c * h;
    return o;
}

// 2) Flying donut (Hellwarth-Nouchi-style toroidal pulse).
//    E azimuthal, B poloidal — perpendicular but linking number 0.
fn donut_field(p_world: vec3<f32>) -> EB {
    let p = p_world / max(params.scale, 1e-4);
    let rho2 = p.x*p.x + p.y*p.y;
    let r2   = rho2 + p.z*p.z;
    let env  = 1.0 / ((1.0 + r2) * (1.0 + r2));
    let rho = sqrt(max(rho2, 1e-12));
    let e_phi = vec3<f32>(-p.y, p.x, 0.0);
    let e_pol = vec3<f32>(p.x * p.z / rho, p.y * p.z / rho, 1.0 - rho2 - p.z*p.z);
    let c = cos(params.time);
    let s = sin(params.time);
    var o: EB;
    o.e = ( c * e_phi + s * e_pol) * env;
    o.b = (-s * e_phi + c * e_pol) * env;
    return o;
}

// 3) Plane-wave photon, linearly polarized, propagating +z, Gaussian waist.
fn plane_photon_field(p_world: vec3<f32>) -> EB {
    let s   = max(params.scale, 1e-4);
    let waist = 2.0 * s;
    let k   = 2.0 / s;
    let w   = 2.0;
    let phase = k * p_world.z - w * params.time;
    let g = exp(-(p_world.x*p_world.x + p_world.y*p_world.y) / (waist*waist));
    let amp = cos(phase) * g;
    var o: EB;
    o.e = vec3<f32>(amp, 0.0, 0.0);
    o.b = vec3<f32>(0.0, amp, 0.0);
    return o;
}

// 4) Circularly-polarized photon (single-photon Fock envelope, spin-1).
fn cp_photon_field(p_world: vec3<f32>) -> EB {
    let s   = max(params.scale, 1e-4);
    let waist = 1.5 * s;
    let k   = 2.0 / s;
    let w   = 2.0;
    let phase = k * p_world.z - w * params.time;
    let g = exp(-(p_world.x*p_world.x + p_world.y*p_world.y) / (waist*waist));
    let cx = cos(phase);
    let sy = sin(phase);
    var o: EB;
    o.e = vec3<f32>( cx,  sy, 0.0) * g;
    o.b = vec3<f32>(-sy,  cx, 0.0) * g;
    return o;
}

// 5) Trefoil hopfion (Hopf index 2). Squaring the Bateman scalars doubles
//    the linking number (Kedia, Bialynicki-Birula et al. 2013).
fn trefoil_field(p_world: vec3<f32>) -> EB {
    let p = p_world / max(params.scale, 1e-4);
    let x = p.x; let y = p.y; let z = p.z;
    let r2 = x*x + y*y + z*z;
    let inv = 1.0 / ((1.0 + r2) * (1.0 + r2) * (1.0 + r2));
    let a1 = x*z + y;
    let a2 = y*z - x;
    let b1 = x*z - y;
    let b2 = y*z + x;
    let h  = vec3<f32>(4.0*(a1*a1 - a2*a2),       8.0*a1*a2,       4.0*(a1*a1 + a2*a2) - (1.0 - r2)*(1.0 - r2)) * inv;
    let hp = vec3<f32>(4.0*(b1*b1 - b2*b2),       8.0*b1*b2,       4.0*(b1*b1 + b2*b2) - (1.0 - r2)*(1.0 - r2)) * inv;
    let c = cos(params.time);
    let s = sin(params.time);
    var o: EB;
    o.e =  c * hp + s * h;
    o.b = -s * hp + c * h;
    return o;
}

fn field_eval(p: vec3<f32>) -> EB {
    switch (params.preset) {
        case 0u:  { return hopfion_field(p); }
        case 1u:  { return hopfion_field(p); }      // tight photon-hopfion (scale set on host)
        case 2u:  { return donut_field(p); }
        case 3u:  { return plane_photon_field(p); }
        case 4u:  { return cp_photon_field(p); }
        case 5u:  { return trefoil_field(p); }
        default:  { return hopfion_field(p); }
    }
}

fn field_dir(p: vec3<f32>) -> vec3<f32> {
    let eb = field_eval(p);
    if (params.field_mode == 0u) { return eb.e; }
    if (params.field_mode == 1u) { return eb.b; }
    return cross(eb.e, eb.b);
}

fn field_mag(p: vec3<f32>) -> f32 {
    return length(field_dir(p));
}
