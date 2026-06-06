# hopf-sta-viz

### An artist's impression of dynamics-views from a 4-dimensional world

> **The 4D world** here is **Minkowski spacetime** $\mathbb{R}^{1,3}$, the flat
> spacetime of special relativity. It is described mathematically by
> **Spacetime Algebra (STA)** — the real geometric (Clifford) algebra
> $C\ell_{1,3}(\mathbb{R})$ introduced by David Hestenes. In STA the whole
> electromagnetic field is a single object, the **Faraday bivector**
>
> $$F = \mathbf{E} + I\,\mathbf{B} \qquad (c = 1,\; I = \gamma_0\gamma_1\gamma_2\gamma_3)$$
>
> and the structure we draw is a **Rañada–Hopf electromagnetic knot** (a
> *hopfion*): a topologically non-trivial, null "knotted light" solution of
> Maxwell's equations whose electric, magnetic and Poynting field lines all
> lie on linked circles of the **Hopf fibration** $S^3 \to S^2$.

`hopf-sta-viz` is a realtime, GPU-accelerated 3D visualizer written in **Rust**
with [`wgpu`](https://wgpu.rs) (Vulkan on Linux, DirectX 12 on Windows, Metal on
macOS). All field evaluation, FDTD time-stepping and particle advection run in
**WGSL compute shaders** — the CPU only drives the camera, the egui control
panel and the pipeline dispatches.

> _A screenshot/GIF will live here once captured. For now, build & run the native
> app, or open the [online artist's impression](#online-demo-artist-impression)._

> **Honesty note — what is "real" and what is "an artist's impression":**
> see the [Real physics vs. artist's impression](#real-physics-vs-artist-impression)
> section. The short version: the **FDTD mode is a genuine Maxwell solver**; the
> closed-form analytic presets and the browser demo are *artistic / analytic
> impressions*, not full PDE solutions.

---

## What it can do

### Live electromagnetism on the GPU
- **FDTD Maxwell solver** — a Yee leapfrog finite-difference time-domain
  integrator on a **192³ collocated grid**, advancing $\mathbf{E}$ and
  $\mathbf{B}$ every frame entirely in compute shaders.
- **Knotted-light seeding** — the grid is seeded with a Rañada/Bateman hopfion
  donut that then propagates and evolves under the discrete Maxwell equations.
- **Particle advection along the Poynting flux** — thousands of tracer
  particles flow along $\mathbf{S} = \mathbf{E}\times\mathbf{B}$, making the
  energy transport visible.
- **Movable PEC mirror** — a perfect-electric-conductor wall you can slide
  toward or away from the field cube (`mirror gap` slider, `0 = touching`) so
  you can watch the pulse **bounce** and interfere with itself.
- **Auto-pulse engine** — emits a fresh knot on an adjustable rhythm so there is
  always something happening, plus a one-shot **⟳ Pulse now** trigger.

### Analytic hopfion presets (closed form)
Several closed-form Spacetime-Algebra field configurations built from the Hopf
map, evolved by rotating $F$ inside the $(\mathbf{E},\mathbf{B})$ plane
(energy-preserving). Useful for clean topology views:
- Fundamental Hopfion, Photon Hopfion, Donut, Plane Photon, CP Photon, Trefoil,
  and an STA "crunch" stress preset.

### Visualization controls
- **Render modes:** FDTD field · streamline **Lines** · flowing **Particles**.
- **Field modes:** `E` (cyan) · `B` (magenta) · `E & B` · `Poynting S` (amber).
- **Transport bar** (a tape-deck at the top): **▶ Play / ⏸ Pause · ⏭ Step ·
  ⟳ Pulse now · ⊡ Fit · ⏹ Quit**, plus a **sim-speed ×** slider.
- **Camera:** right-mouse / middle-mouse drag to orbit, wheel to zoom, **⊡ Fit**
  to reframe.
- **In-app remark log (F1):** press F1 for help; logged UI remarks are written to
  `ui-remarks.json` (an MCP-style feedback artifact, git-ignored).
- **Preset export:** save the current parameters to a timestamped
  `preset-*.json` (git-ignored).

---

## Real physics vs. artist impression

| Layer | What it is | Honesty |
|-------|------------|---------|
| **FDTD mode** | Yee leapfrog solver for Maxwell's equations on a 192³ grid, with PEC boundary, seeding, advection. | **Real** discretized physics. Numerical, but it actually integrates Maxwell. |
| **Analytic presets** | Closed-form Hopf-map fields with time modeled as a rotation in the $(\mathbf{E},\mathbf{B})$ plane. | **Artist's / analytic impression.** Correct topology, *approximate* dynamics — not a full solution of the evolution PDE. |
| **`docs/index.html` web demo** | Three.js torus-knot meshes that *suggest* linked E/M field tubes. | **Pure artist's impression.** Decorative geometry, no field solver at all. |
| **`docs/notes/gemini-exploration.md`** | The original brainstorming transcript (theory + throwaway HTML prototypes). | Exploratory notes / "the fakes," kept for provenance — clearly labeled. |

The repository never pretends the impressions are the physics. The FDTD path is
the scientific core; everything else is there to make a 4D idea graspable to the
eye.

---

## Build & run (native)

Requires a recent Rust toolchain and a Vulkan/DX12/Metal-capable GPU.

```bash
cargo run --release
```

`--release` is strongly recommended — the FDTD pass does millions of grid
updates per frame.

### Project layout
```
src/main.rs        entry point
src/app.rs         winit ApplicationHandler, wgpu surface, SimSettings, frame loop
src/camera.rs      orbit camera + view/proj uniform (+ fit())
src/fdtd.rs        192³ Yee FDTD solver, seeding, PEC mirror, particle advection
src/ui.rs          egui transport bar, sliders, presets, F1 remark log
shaders/           WGSL: fdtd_update_e/b, fdtd_particles, hopfion eval, render passes
web/src/main.ts    TypeScript source for the GitHub Pages artist's-impression demo
docs/              published GitHub Pages site (index.html + generated main.js)
docs/notes/        DESIGN.md (architecture note) + gemini-exploration.md (the "fakes")
```

The web demo is authored in TypeScript under `web/src/` and compiled to the
generated `docs/main.js` with `tsc -p web/tsconfig.json` — never hand-edit the
generated `.js`.

---

## Online demo (artist's impression)

A native wgpu app cannot run on GitHub Pages, so the published demo at

**→ https://springyworks.github.io/hopf-sta-viz/**

is a self-contained **Three.js artist's impression** of the linked
electric/magnetic field tubes of a Rañada hopfion. It is intentionally
*illustrative*, not a Maxwell solver — see the banner on the page itself. For
the real FDTD physics, build and run the native Rust app above.

---

## The mathematics, briefly

Working in Spacetime Algebra, the source-free Maxwell equations collapse to one
equation for the Faraday bivector,

$$\nabla F = 0,$$

with $\nabla = \gamma^\mu \partial_\mu$ the spacetime vector derivative. A
hopfion is a **null** field, $F^2 = 0$, built (Bateman construction) from two
complex scalars $\alpha,\beta$ via $F = \nabla\alpha \times \nabla\beta$, whose
level sets foliate space into the linked circles of the Hopf fibration. The
electric, magnetic and Poynting field lines are each closed and **pairwise
linked with linking number 1** — the topological fingerprint we render.

---

## License

[MIT](LICENSE) © springyworks
