# hopf-sta-viz

Realtime 3D visualization of the **Rañada electromagnetic hopfion** using
**Spacetime Algebra** (Clifford `Cl(3,0)`) field evaluation on the GPU.

Built in Rust with `wgpu` (Vulkan backend on Linux, DirectX12 on Windows).
All field evaluation, streamline integration and particle advection happens
in WGSL compute shaders — the CPU only manages the camera, UI sliders and
pipeline dispatch.

## Why wgpu and not burn?

The user prompt suggested possibly using `burn` tensors. `burn` is excellent
for ML/autograd workloads but adds a tensor-framework abstraction layer
that is unnecessary (and slower) for a dense streaming visualization where
we want direct control over compute dispatches, vertex/index buffers and
the swapchain. We talk to the same hardware (RTX 2070, Vulkan) more
directly with raw `wgpu`.

## Physical model

We evaluate the Faraday bivector

    F = E + I c B           (STA, with I = e1·e2·e3 the unit pseudoscalar)

using a closed-form analytical hopfion built from the Hopf map
R³ → S². At t = 0 the field is

    Λ      = 1 + x² + y² + z²
    H (x)  = (1/Λ²) ( 2(xz + y), 2(yz − x), 1 − x² − y² + z² )
    H'(x)  = (1/Λ²) ( 2(xz − y), 2(yz + x), 1 − x² − y² + z² )

with `H` chosen as **B** and `H'` (the dual Hopf field, orthogonal to `H`
everywhere) chosen as **E**. Both fields are tangent to two mutually
linked families of Hopf circles — the topological signature of the
hopfion. Time evolution is approximated as a rotation of `F` inside the
(E,B) plane:

    E(t) =  cos(ωt) E₀ + sin(ωt) B₀
    B(t) = −sin(ωt) E₀ + cos(ωt) B₀

which preserves `|F|² = |E|² + |B|²` (the field energy density).

The **Poynting vector** is computed directly:

    S = E × B          (normalized units, μ₀ = ε₀ = 1)

## Display modes

Toggle between **wireframe streamlines** and **flowing particles**, and
choose what to integrate:

| Mode      | Field used         | Colour     |
|-----------|--------------------|------------|
| E         | electric `E`       | cyan       |
| B         | magnetic `B`       | magenta    |
| E & B     | both, drawn at once| cyan + magenta |
| Poynting  | `S = E × B`        | amber      |

## Controls

Right-mouse drag (or middle-mouse drag): orbit camera.
Mouse wheel: zoom.
Sliders in the on-screen panel:

* `time` — animation phase (auto-advances when **Play** is on).
* `time scale` — speed of auto-advance.
* `hopfion scale R` — overall radius of the structure.
* `seed density` — number of streamlines / particles.
* `step length` — RK4 step in streamline integration.
* `steps per line` — number of integration steps per streamline.
* `particle speed` — multiplier on field magnitude.
* `field mode` — E / B / E&B / Poynting.
* `render mode` — Lines / Particles.

## Build & run

    cargo run --release

`--release` is strongly recommended; the streamline compute pass is
several million RK4 steps per frame at full slider settings.

## Files

    src/main.rs        entry point
    src/app.rs         winit ApplicationHandler, wgpu surface, frame loop
    src/gpu.rs         adapter/device with raised RTX 2070 limits
    src/camera.rs      orbit camera + view/proj uniform
    src/field.rs       hopfion parameter uniform
    src/streamlines.rs RK4 line-tracing compute pipeline + line render
    src/particles.rs   particle advection compute pipeline + point render
    src/ui.rs          egui sliders / mode selectors
    shaders/common.wgsl       hopfion E,B,S evaluator + bindings
    shaders/streamlines.wgsl  one workgroup per seed, writes M vertices
    shaders/particles.wgsl    advect + respawn particles
    shaders/render_line.wgsl  vertex/fragment for line strips
    shaders/render_point.wgsl vertex/fragment for particle points
