// src/web_fdtd.rs
//
// Real CPU Yee-grid Maxwell FDTD for the browser (WASM, **no** compute shaders).
//
// This is the *same* finite-difference time-domain scheme the native build runs
// on the GPU (see `src/fdtd.rs` + `shaders/fdtd_*.wgsl`), reimplemented on the
// CPU so the WebGL2 page shows a **genuine electromagnetic field**, not an
// analytic look-alike or a ballistic "ball" bounce.
//
//   Leapfrog (normalized units, c = mu0 = eps0 = 1, unit grid dx = 1):
//       E_{n+1} = E_n + dt * curl(B_n)
//       B_{n+1} = B_n - dt * curl(E_{n+1})
//
//   * collocated grid, central differences (inv2dx = 0.5 because dx = 1),
//   * a small numerical-damping term `sigma` tames the checkerboard mode,
//   * an outer absorbing shell (poor-man's PML) on the −X entrance and the four
//     Y/Z side walls — the **+X face is left reflective** so the tilted
//     perfect-electric-conductor (PEC) mask sitting just inside it acts as the
//     mirror (tangential E = 0),
//   * energy transport is read out as the Poynting vector S = E × B, sampled
//     trilinearly so tracer particles ride the live field.
//
// The simulation runs in pure grid-index space (unit spacing on every axis, just
// like the native shader). The physical box is anisotropic (long in X), so the
// world↔grid mapping below is used only for placing the source, carving the PEC
// mirror, and reading the field back out for the particles.

use glam::Vec3;

/// What the launch source looks like in the transverse (Y/Z) plane.
#[derive(Copy, Clone, Debug)]
pub struct SourceSpec {
    /// Drive amplitude.
    pub amp: f32,
    /// Ring radius in world units (`0` ⇒ a filled Gaussian spot).
    pub radius: f32,
    /// `true` ⇒ a full transverse sheet (plane wave) instead of a ring/spot.
    pub plane: bool,
    /// Circular-polarization spin (adds a quadrature E_z / B_y component).
    pub spin: f32,
}

pub struct Fdtd {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Half-extent on Y and Z, world units.
    pub world_r: f32,
    /// Half-extent on X, world units.
    pub half_x: f32,
    pub dt: f32,
    pub pml: usize,
    e: Vec<Vec3>,
    b: Vec<Vec3>,
    mask: Vec<bool>, // true ⇒ PEC cell (the mirror)
    pub steps: u64,
}

impl Fdtd {
    /// Build a fresh grid. `cross_n` is the transverse resolution; the X axis is
    /// stretched to match the (longer) physical box `half_x`.
    pub fn new(cross_n: usize, world_r: f32, half_x: f32) -> Self {
        let ny = cross_n.max(8);
        let nz = cross_n.max(8);
        // Keep the cells roughly cubic: more cells along the long X axis.
        let nx = ((cross_n as f32) * (half_x / world_r)).round().max(16.0) as usize;
        let n = nx * ny * nz;
        Self {
            nx,
            ny,
            nz,
            world_r,
            half_x,
            dt: 0.45,
            pml: 8,
            e: vec![Vec3::ZERO; n],
            b: vec![Vec3::ZERO; n],
            mask: vec![false; n],
            steps: 0,
        }
    }

    #[inline]
    fn lin(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.ny + j) * self.nx + i
    }

    // --- world ↔ grid mapping (collocated, endpoints inclusive) ------------

    #[inline]
    fn world_x(&self, i: usize) -> f32 {
        i as f32 / (self.nx as f32 - 1.0) * 2.0 * self.half_x - self.half_x
    }
    #[inline]
    fn world_y(&self, j: usize) -> f32 {
        j as f32 / (self.ny as f32 - 1.0) * 2.0 * self.world_r - self.world_r
    }
    #[inline]
    fn world_z(&self, k: usize) -> f32 {
        k as f32 / (self.nz as f32 - 1.0) * 2.0 * self.world_r - self.world_r
    }

    /// Continuous grid coordinates for a world point (un-clamped).
    #[inline]
    fn to_grid(&self, w: Vec3) -> (f32, f32, f32) {
        let gx = (w.x + self.half_x) / (2.0 * self.half_x) * (self.nx as f32 - 1.0);
        let gy = (w.y + self.world_r) / (2.0 * self.world_r) * (self.ny as f32 - 1.0);
        let gz = (w.z + self.world_r) / (2.0 * self.world_r) * (self.nz as f32 - 1.0);
        (gx, gy, gz)
    }

    /// Zero the field (keeps the mirror mask).
    pub fn clear(&mut self) {
        for v in self.e.iter_mut() {
            *v = Vec3::ZERO;
        }
        for v in self.b.iter_mut() {
            *v = Vec3::ZERO;
        }
    }

    // --- PEC mirror --------------------------------------------------------

    /// Carve a tilted PEC mirror into the +X end. `theta` tilts the reflecting
    /// plane about the Z axis (0 ⇒ a flat wall facing −X). Disabling it removes
    /// every PEC cell so the wave runs straight out into the absorbing shell.
    pub fn set_mirror(&mut self, theta: f32, enabled: bool) {
        for v in self.mask.iter_mut() {
            *v = false;
        }
        if !enabled {
            return;
        }
        // Inward plane normal (points back toward −X for theta = 0).
        let n = Vec3::new(theta.cos(), theta.sin(), 0.0);
        let p0 = Vec3::new(0.80 * self.half_x, 0.0, 0.0);
        let x_min = 0.50 * self.half_x; // confine the mirror to the +X half
        for k in 0..self.nz {
            let z = self.world_z(k);
            for j in 0..self.ny {
                let y = self.world_y(j);
                for i in 0..self.nx {
                    let x = self.world_x(i);
                    let w = Vec3::new(x, y, z);
                    if x > x_min && (w - p0).dot(n) >= 0.0 {
                        let idx = self.lin(i, j, k);
                        self.mask[idx] = true;
                    }
                }
            }
        }
    }

    // --- absorbing shell ---------------------------------------------------

    /// Damping multiplier near the boundaries. The −X entrance and the four Y/Z
    /// side walls absorb (graded over `pml` layers); the +X face is left alone
    /// so the PEC mirror there reflects cleanly.
    #[inline]
    fn boundary_damp(&self, i: usize, j: usize, k: usize, strength: f32) -> f32 {
        let pl = self.pml as i32;
        let d_xlo = i as i32; // −X entrance (absorbing)
        let d_y = (j as i32).min(self.ny as i32 - 1 - j as i32);
        let d_z = (k as i32).min(self.nz as i32 - 1 - k as i32);
        let d = d_xlo.min(d_y).min(d_z);
        if d >= pl {
            1.0
        } else {
            let t = (pl - d) as f32 / pl as f32;
            1.0 - strength * t * t
        }
    }

    // --- curl helpers (central differences on the unit grid) ---------------

    #[inline]
    fn idx_clamp(&self, i: i32, j: i32, k: i32) -> usize {
        let ci = i.clamp(0, self.nx as i32 - 1) as usize;
        let cj = j.clamp(0, self.ny as i32 - 1) as usize;
        let ck = k.clamp(0, self.nz as i32 - 1) as usize;
        self.lin(ci, cj, ck)
    }

    #[inline]
    fn curl(field: &[Vec3], me: &Fdtd, i: usize, j: usize, k: usize) -> Vec3 {
        let (i, j, k) = (i as i32, j as i32, k as i32);
        let xp = field[me.idx_clamp(i + 1, j, k)];
        let xm = field[me.idx_clamp(i - 1, j, k)];
        let yp = field[me.idx_clamp(i, j + 1, k)];
        let ym = field[me.idx_clamp(i, j - 1, k)];
        let zp = field[me.idx_clamp(i, j, k + 1)];
        let zm = field[me.idx_clamp(i, j, k - 1)];
        // inv2dx = 0.5 (dx = 1)
        Vec3::new(
            ((yp.z - ym.z) - (zp.y - zm.y)) * 0.5,
            ((zp.x - zm.x) - (xp.z - xm.z)) * 0.5,
            ((xp.y - xm.y) - (yp.x - ym.x)) * 0.5,
        )
    }

    // --- leapfrog ----------------------------------------------------------

    fn update_e(&mut self, sigma: f32, pml_strength: f32) {
        let decay = 1.0 - self.dt * sigma;
        for k in 0..self.nz {
            for j in 0..self.ny {
                for i in 0..self.nx {
                    let idx = self.lin(i, j, k);
                    if self.mask[idx] {
                        self.e[idx] = Vec3::ZERO; // PEC: tangential E = 0
                        continue;
                    }
                    let cb = Self::curl(&self.b, self, i, j, k);
                    let damp = self.boundary_damp(i, j, k, pml_strength);
                    self.e[idx] = (self.e[idx] + self.dt * cb) * (decay * damp);
                }
            }
        }
    }

    fn update_b(&mut self, sigma: f32, pml_strength: f32) {
        let decay = 1.0 - self.dt * sigma;
        for k in 0..self.nz {
            for j in 0..self.ny {
                for i in 0..self.nx {
                    let idx = self.lin(i, j, k);
                    let ce = Self::curl(&self.e, self, i, j, k);
                    let damp = self.boundary_damp(i, j, k, pml_strength);
                    self.b[idx] = (self.b[idx] - self.dt * ce) * (decay * damp);
                }
            }
        }
    }

    /// Advance the field by `substeps` Yee leapfrog steps.
    pub fn step(&mut self, substeps: u32, sigma: f32, pml_strength: f32) {
        for _ in 0..substeps {
            self.update_e(sigma, pml_strength);
            self.update_b(sigma, pml_strength);
            self.steps += 1;
        }
    }

    // --- sources -----------------------------------------------------------

    /// Range of X cells covered by the soft source slab near the −X entrance.
    #[inline]
    fn source_x_window(&self) -> (usize, usize, f32, f32) {
        let cx = -0.60 * self.half_x; // source plane (world X)
        let wx = 0.08 * self.half_x; // longitudinal Gaussian width
        let lo = self.to_grid(Vec3::new(cx - 3.0 * wx, 0.0, 0.0)).0.floor().max(0.0) as usize;
        let hi = (self.to_grid(Vec3::new(cx + 3.0 * wx, 0.0, 0.0)).0.ceil() as usize).min(self.nx - 1);
        (lo, hi, cx, wx)
    }

    /// Transverse profile of the source at world (y, z) for the given shape.
    #[inline]
    fn transverse(spec: &SourceSpec, y: f32, z: f32) -> f32 {
        if spec.plane {
            // Soft-edged sheet across the cross-section.
            let r = (y * y + z * z).sqrt();
            (-(r * r) / 9.0).exp()
        } else if spec.radius <= 1e-3 {
            let r2 = y * y + z * z;
            (-(r2) / 0.6).exp()
        } else {
            let r = (y * y + z * z).sqrt();
            let d = (r - spec.radius) / 0.45;
            (-(d * d)).exp()
        }
    }

    /// Continuously driven soft source (call each substep with advancing phase).
    /// E_y and B_z are driven in phase so S = E × B points +X (forward beam).
    pub fn drive(&mut self, spec: &SourceSpec, phase: f32) {
        let s = phase.sin();
        let sq = (phase + std::f32::consts::FRAC_PI_2).sin(); // quadrature for spin
        let (lo, hi, cx, wx) = self.source_x_window();
        for i in lo..=hi {
            let lx = (self.world_x(i) - cx) / wx;
            let env = (-(lx * lx)).exp();
            for k in 0..self.nz {
                let z = self.world_z(k);
                for j in 0..self.ny {
                    let y = self.world_y(j);
                    let a = spec.amp * env * Self::transverse(spec, y, z);
                    if a < 1e-4 {
                        continue;
                    }
                    let idx = self.lin(i, j, k);
                    if self.mask[idx] {
                        continue;
                    }
                    self.e[idx].y += a * s;
                    self.b[idx].z += a * s;
                    if spec.spin.abs() > 1e-3 {
                        self.e[idx].z += a * spec.spin * sq;
                        self.b[idx].y -= a * spec.spin * sq;
                    }
                }
            }
        }
    }

    /// Stamp a one-shot forward wave packet (the "Inject pulse" button): a
    /// Gaussian-enveloped single cycle that propagates toward the mirror.
    pub fn inject_packet(&mut self, spec: &SourceSpec) {
        let cx = -0.45 * self.half_x;
        let wx = 0.16 * self.half_x;
        let lo = self.to_grid(Vec3::new(cx - 3.0 * wx, 0.0, 0.0)).0.floor().max(0.0) as usize;
        let hi = (self.to_grid(Vec3::new(cx + 3.0 * wx, 0.0, 0.0)).0.ceil() as usize).min(self.nx - 1);
        let k_wave = std::f32::consts::TAU / (0.55 * wx).max(1.0);
        for i in lo..=hi {
            let lx = self.world_x(i) - cx;
            let env = (-(lx / wx) * (lx / wx)).exp();
            let osc = (k_wave * lx).sin();
            for k in 0..self.nz {
                let z = self.world_z(k);
                for j in 0..self.ny {
                    let y = self.world_y(j);
                    let a = 1.6 * spec.amp * env * osc * Self::transverse(spec, y, z);
                    let idx = self.lin(i, j, k);
                    if self.mask[idx] {
                        continue;
                    }
                    self.e[idx].y += a;
                    self.b[idx].z += a;
                }
            }
        }
    }

    // --- read-out ----------------------------------------------------------

    #[inline]
    fn sample(field: &[Vec3], me: &Fdtd, w: Vec3) -> Vec3 {
        let (gx, gy, gz) = me.to_grid(w);
        let gx = gx.clamp(0.0, me.nx as f32 - 1.0);
        let gy = gy.clamp(0.0, me.ny as f32 - 1.0);
        let gz = gz.clamp(0.0, me.nz as f32 - 1.0);
        let i0 = gx.floor() as usize;
        let j0 = gy.floor() as usize;
        let k0 = gz.floor() as usize;
        let i1 = (i0 + 1).min(me.nx - 1);
        let j1 = (j0 + 1).min(me.ny - 1);
        let k1 = (k0 + 1).min(me.nz - 1);
        let fx = gx - i0 as f32;
        let fy = gy - j0 as f32;
        let fz = gz - k0 as f32;
        let c000 = field[me.lin(i0, j0, k0)];
        let c100 = field[me.lin(i1, j0, k0)];
        let c010 = field[me.lin(i0, j1, k0)];
        let c110 = field[me.lin(i1, j1, k0)];
        let c001 = field[me.lin(i0, j0, k1)];
        let c101 = field[me.lin(i1, j0, k1)];
        let c011 = field[me.lin(i0, j1, k1)];
        let c111 = field[me.lin(i1, j1, k1)];
        let c00 = c000.lerp(c100, fx);
        let c10 = c010.lerp(c110, fx);
        let c01 = c001.lerp(c101, fx);
        let c11 = c011.lerp(c111, fx);
        let c0 = c00.lerp(c10, fy);
        let c1 = c01.lerp(c11, fy);
        c0.lerp(c1, fz)
    }

    /// Poynting vector S = E × B at a world point (the energy-flow direction the
    /// tracer particles ride).
    #[inline]
    pub fn sample_s(&self, w: Vec3) -> Vec3 {
        let e = Self::sample(&self.e, self, w);
        let b = Self::sample(&self.b, self, w);
        e.cross(b)
    }

    /// Magnetic field at a world point (for the field-line / streamline view).
    #[inline]
    pub fn sample_b(&self, w: Vec3) -> Vec3 {
        Self::sample(&self.b, self, w)
    }

    /// `true` if the world point sits inside a PEC mirror cell.
    pub fn is_pec(&self, w: Vec3) -> bool {
        let (gx, gy, gz) = self.to_grid(w);
        if gx < 0.0 || gy < 0.0 || gz < 0.0 {
            return false;
        }
        let i = (gx.round() as usize).min(self.nx - 1);
        let j = (gy.round() as usize).min(self.ny - 1);
        let k = (gz.round() as usize).min(self.nz - 1);
        self.mask[self.lin(i, j, k)]
    }
}
