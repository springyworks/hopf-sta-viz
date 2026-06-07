// src/web_gl.rs
//
// Browser-only WebGL2 variant of the visualizer.
//
// Why this exists
// ---------------
// The main app (`app.rs` + `fdtd.rs`) runs the whole Spacetime-Algebra Maxwell
// simulation in **compute shaders** with **storage buffers**.  Neither compute
// shaders nor storage buffers exist in WebGL2, so that path can only run on a
// WebGPU-capable browser.
//
// Plenty of browsers expose a solid WebGL2 but a flaky / disabled WebGPU.  For
// those we keep everything that *does* work on WebGL2:
//
//   * the GPU does the **rendering** (wgpu's GL backend → a real WebGL2
//     context drawn from the page <canvas>), reusing the exact same
//     `render_point.wgsl` / `render_line.wgsl` shaders as the WebGPU build, and
//   * the CPU (Rust compiled to WASM) does the **physics**: it advects a cloud
//     of particles along the analytic hopfion Poynting vector  S = E × B  and
//     streams the positions into a GPU vertex buffer every frame.
//
// So this is still "Rust + WASM + GPU in the browser" — just with the heavy
// math on the CPU and WebGL2 doing the drawing, instead of WebGPU compute.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use web_time::Instant;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::camera::OrbitCamera;
use crate::web_fdtd::Shape;

/// Cross-section half-extent of the flight box (the Y/Z half-size, world units).
const WORLD_R: f32 = 4.0;
/// Default tracer-particle count the CPU advects + the GPU draws.
const DEFAULT_PARTICLES: usize = 45_000;
/// Seconds before a tracer respawns at the −X source (drives the render fade).
const MAX_AGE: f32 = 8.0;
/// Default transverse FDTD grid resolution (cells across Y/Z). The X axis is
/// stretched to match the longer physical box. Live-tunable from the panel
/// ("Grid (cross cells)") so you can crank the per-cell Maxwell crunching up
/// until the machine works for it; the Yee solver runs single-threaded on the
/// CPU in WASM (no GPU compute on WebGL2).
const CROSS_N: usize = 34;
/// Live field-line build sizes (E and/or B field-line tracing).
const STREAM_SEEDS: usize = 900;
const STREAM_STEPS: usize = 40;
const STREAM_DS:    f32   = 0.14;
const STREAM_CAP:   usize = STREAM_SEEDS * STREAM_STEPS * 2;
/// Vertex capacity for the tilted end-mirror wireframe.
const MIRROR_CAP: usize = 64;

/// Two-colour field-line tints used by the E / B / E&B views.
const E_LINE_RGB: [f32; 3] = [0.30, 0.85, 1.00]; // electric → cyan
const B_LINE_RGB: [f32; 3] = [1.00, 0.35, 0.85]; // magnetic → magenta

// ---------------------------------------------------------------------------
// UI-facing settings (driven by the egui control panel).
// ---------------------------------------------------------------------------

/// The launch-source shape fed into the FDTD grid near the −X entrance. Each is
/// a Spacetime-Algebra **null** field (see `web_fdtd::Shape`); the real Yee
/// solver then propagates a genuine electromagnetic field from it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum WebPreset {
    Hopfion,
    PhotonHopfion,
    FlyingDonut,
    RadialDonut,
    PlanePhoton,
    CpPhoton,
    Trefoil,
    PhasedArray,
}

impl WebPreset {
    const ALL: &'static [WebPreset] = &[
        WebPreset::Hopfion, WebPreset::PhotonHopfion, WebPreset::FlyingDonut,
        WebPreset::RadialDonut, WebPreset::PlanePhoton, WebPreset::CpPhoton,
        WebPreset::Trefoil, WebPreset::PhasedArray,
    ];
    fn label(self) -> &'static str {
        match self {
            WebPreset::Hopfion       => "Hopfion · linked null ring (index 1)",
            WebPreset::PhotonHopfion => "Photon hopfion · tight core",
            WebPreset::FlyingDonut   => "Flying donut · Hellwarth–Nouchi",
            WebPreset::RadialDonut   => "Radial donut · TM (E outward)",
            WebPreset::PlanePhoton   => "Plane photon · linear pol",
            WebPreset::CpPhoton      => "CP photon · circular pol",
            WebPreset::Trefoil       => "Trefoil · m=2 twist (index 2)",
            WebPreset::PhasedArray   => "Phased array · steered beam",
        }
    }

    /// Map the UI preset to its Spacetime-Algebra null-field pattern.
    fn shape(self) -> Shape {
        match self {
            WebPreset::Hopfion       => Shape::Hopfion,
            WebPreset::PhotonHopfion => Shape::PhotonHopfion,
            WebPreset::FlyingDonut   => Shape::FlyingDonut,
            WebPreset::RadialDonut   => Shape::RadialDonut,
            WebPreset::PlanePhoton   => Shape::PlanePhoton,
            WebPreset::CpPhoton      => Shape::CpPhoton,
            WebPreset::Trefoil       => Shape::Trefoil,
            WebPreset::PhasedArray   => Shape::PhasedArray,
        }
    }

    /// Ring / torus radius (world units) for the ring-type shapes; 0 for spots.
    fn radius(self) -> f32 {
        match self {
            WebPreset::Hopfion       => 1.6,
            WebPreset::PhotonHopfion => 1.0,
            WebPreset::FlyingDonut   => 1.9,
            WebPreset::RadialDonut   => 1.6,
            WebPreset::Trefoil       => 2.2,
            WebPreset::PlanePhoton | WebPreset::CpPhoton | WebPreset::PhasedArray => 0.0,
        }
    }

    /// Build the FDTD source spec (transverse null-field pattern) for this shape.
    fn source_spec(self, power: f32) -> crate::web_fdtd::SourceSpec {
        crate::web_fdtd::SourceSpec { amp: power, radius: self.radius(), shape: self.shape() }
    }

    /// The next shape in the cycle (wraps) — drives the [Next demo] button.
    fn next(self) -> WebPreset {
        let all = WebPreset::ALL;
        let i = all.iter().position(|&p| p == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
}

/// Which slice of the Faraday bivector F = E + I·c·B to draw, and how.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum WebField {
    /// Energy flow: a tracer cloud riding the live S = E × B (preset tint).
    Poynting,
    /// Electric field lines (cyan).
    E,
    /// Magnetic field lines (magenta).
    B,
    /// Both E (cyan) and B (magenta) at once — the linked two-colour view.
    Both,
}

#[derive(Copy, Clone, Debug)]
struct WebSettings {
    preset:         WebPreset,
    field:          WebField,
    cross_n:        u32,   // transverse FDTD cells (Y/Z); X is stretched to match
    len_mult:       f32,   // box length along +X, in cross-sections (1..5, default 3)
    mirror_deg:     f32,   // tilt of the PEC end mirror (degrees)
    mirror_on:      bool,  // is the reflecting wall present?
    sim_speed:      u32,   // Yee substeps integrated per frame
    source_power:   f32,   // soft-source drive amplitude
    source_freq:    f32,   // soft-source angular frequency (rad / substep)
    absorb:         f32,   // numerical damping sigma (stability / dissipation)
    advect:         f32,   // tracer Poynting-advection gain
    brightness:     f32,   // particle / streamline alpha
    particle_count: u32,   // tracer cloud size (applied on reseed)
    drive_on:       bool,  // keep the continuous soft source running?
    reseed:         bool,
    fire:           bool,  // [Fire]: clear the grid + stamp a fresh STA pulse
    inject:         bool,  // one-shot "Inject pulse" (stamp without clearing)
    clear:          bool,  // "Clear field" request
    fit:            bool,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            preset:         WebPreset::Hopfion,
            field:          WebField::Poynting,
            cross_n:        CROSS_N as u32,
            len_mult:       3.0,
            mirror_deg:     25.0,
            mirror_on:      true,
            sim_speed:      2,
            source_power:   0.6,
            source_freq:    0.55,
            absorb:         0.008,
            advect:         5.0,
            brightness:     1.8,
            particle_count: DEFAULT_PARTICLES as u32,
            drive_on:       true,
            reseed:         false,
            fire:           true,   // launch a pulse on first frame (always animated)
            inject:         false,
            clear:          false,
            fit:            false,
        }
    }
}

// ---------------------------------------------------------------------------
// GPU-facing vertex layouts.  These must stay byte-compatible with the WGSL
// `VertexIn` structs in render_point.wgsl / render_line.wgsl.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct PointVertex {
    pos: [f32; 3],
    age: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct LineVertex {
    pos: [f32; 3],
    mag: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct RenderParams {
    color:          [f32; 4],
    flow_phase:     f32,
    steps_per_line: u32,
    _pad0:          u32,
    _pad1:          u32,
}

// ---------------------------------------------------------------------------
// CPU-side particle.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct Particle {
    /// Absolute world position; advected along the live grid's S = E × B.
    pos: Vec3,
    age: f32,
}

// ---------------------------------------------------------------------------
// Entry point — called from `lib.rs::start()` on wasm.
// ---------------------------------------------------------------------------

pub fn run_webgl() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let app = GlApp::new(&event_loop);
    use winit::platform::web::EventLoopExtWebSys;
    event_loop.spawn_app(app);
}

enum UserEvent {
    Ready(GlState),
}

struct GlApp {
    state: Option<GlState>,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

impl GlApp {
    fn new(event_loop: &EventLoop<UserEvent>) -> Self {
        Self { state: None, proxy: event_loop.create_proxy() }
    }
}

impl ApplicationHandler<UserEvent> for GlApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("hopf-sta-viz · WebGL2");
        let window = Arc::new(el.create_window(attrs).expect("window"));

        // Mount winit's <canvas> into the page so the WebGL2 surface is visible.
        use winit::platform::web::WindowExtWebSys;
        if let Some(canvas) = window.canvas() {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let dst = doc
                    .get_element_by_id("hopf-canvas-host")
                    .or_else(|| doc.body().map(Into::into));
                if let Some(dst) = dst {
                    canvas.set_id("hopf-canvas");
                    let _ = dst.append_child(&canvas);
                }
            }
        }

        // The GL device request is async on the web — build `GlState` off the
        // control flow and hand it back through the loop.
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(state) = GlState::new(window).await {
                let _ = proxy.send_event(UserEvent::Ready(state));
            }
        });
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Ready(state) = event;
        // WebGL2 is live — drop the boot overlay.
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(el) = doc.get_element_by_id("boot-msg") {
                let _ = el.set_attribute("hidden", "");
            }
        }
        state.window.request_redraw();
        self.state = Some(state);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return; };

        // Feed every event to egui first; if it claims the pointer, don't orbit.
        let egui_consumed = state
            .egui_winit
            .on_window_event(&state.window, &event)
            .consumed;

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => state.resize(size),
            WindowEvent::RedrawRequested => {
                state.render();
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let dx = (position.x - state.mouse_pos.x) as f32;
                let dy = (position.y - state.mouse_pos.y) as f32;
                state.mouse_pos = position;
                if !egui_consumed {
                    if state.rmb_down {
                        state.camera.orbit(dx, dy);
                    }
                    if state.mmb_down {
                        state.camera.pan(dx, dy);
                    }
                }
            }
            WindowEvent::MouseInput { state: bstate, button, .. } => {
                let pressed = bstate == ElementState::Pressed;
                match button {
                    MouseButton::Right => state.rmb_down = pressed && !egui_consumed,
                    MouseButton::Middle => state.mmb_down = pressed && !egui_consumed,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !egui_consumed {
                    let s = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 40.0,
                    };
                    let factor = if s > 0.0 { 0.9 } else { 1.1 };
                    state.camera.zoom(factor);
                }
            }
            WindowEvent::Touch(touch) => state.handle_touch(touch, egui_consumed),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// GPU + simulation state.
// ---------------------------------------------------------------------------

struct GlState {
    window:     Arc<Window>,
    surface:    wgpu::Surface<'static>,
    device:     wgpu::Device,
    queue:      wgpu::Queue,
    config:     wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,

    // pipelines (point cloud + line box / streamlines) — reuse render shaders.
    point_pipeline: wgpu::RenderPipeline,
    line_pipeline:  wgpu::RenderPipeline,

    // uniforms / bind groups
    camera_buf:     wgpu::Buffer,
    particle_bind:  wgpu::BindGroup,
    box_bind:       wgpu::BindGroup,
    stream_bind:    wgpu::BindGroup,
    mirror_bind:    wgpu::BindGroup,
    particle_color: wgpu::Buffer,
    box_color:      wgpu::Buffer,
    stream_color:   wgpu::Buffer,
    mirror_color:   wgpu::Buffer,

    // geometry
    particle_vbuf:   wgpu::Buffer,
    box_vbuf:        wgpu::Buffer,
    box_ibuf:        wgpu::Buffer,
    box_index_count: u32,
    mirror_vbuf:       wgpu::Buffer,
    mirror_vert_count: u32,

    // CPU-built field lines — magnetic B (magenta).
    line_vbuf:       wgpu::Buffer,
    line_vert_count: u32,
    line_scratch:    Vec<LineVertex>,
    // CPU-built field lines — electric E (cyan), for the E / E&B views.
    eline_vbuf:       wgpu::Buffer,
    eline_vert_count: u32,
    eline_scratch:    Vec<LineVertex>,
    eline_color:      wgpu::Buffer,
    eline_bind:       wgpu::BindGroup,

    // CPU simulation — real Yee-grid Maxwell FDTD (see web_fdtd.rs)
    fdtd:      crate::web_fdtd::Fdtd,
    particles: Vec<Particle>,
    upload:    Vec<PointVertex>,
    rng:       u32,

    // live, UI-driven settings
    settings:        WebSettings,
    allocated:       usize,
    last_len:        f32,
    last_cross:      u32,
    last_mirror_deg: f32,
    last_mirror_on:  bool,

    source_phase: f32,
    flow_phase:   f32,
    frame_ms:     f32,
    last_tick:    Instant,

    // camera + input
    camera:    OrbitCamera,
    mouse_pos: PhysicalPosition<f64>,
    rmb_down:  bool,
    mmb_down:  bool,

    // touch (mobile) — active touch points + last 2-finger pinch baseline
    touches:    Vec<(u64, PhysicalPosition<f64>)>,
    pinch_dist: f32,
    pinch_mid:  (f64, f64),
    /// On-screen 3-D viewport in physical px [x, y, w, h]; on a narrow phone
    /// layout this is only the strip above the control sheet. Also sets aspect.
    scene_px:   [f32; 4],

    // UI
    egui_ctx:      egui::Context,
    egui_winit:    egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

impl GlState {
    async fn new(window: Arc<Window>) -> Option<Self> {
        let size = window.inner_size();

        // WebGL2 backend.  The `webgl` feature on wgpu (enabled for wasm32 in
        // Cargo.toml) makes `Backends::GL` resolve to a real WebGL2 context.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");

        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
        {
            Some(a) => a,
            None => {
                log::error!("no WebGL2 adapter available");
                set_boot_status(
                    "⚠️ Could not create a WebGL2 context in this browser/view.",
                );
                return None;
            }
        };

        log::info!(
            "WebGL2 adapter: {} (backend {:?})",
            adapter.get_info().name,
            adapter.get_info().backend,
        );

        // WebGL2 caps: no compute, no storage buffers, tighter limits. We only
        // use uniform + vertex + index buffers, so the downlevel defaults fit.
        let limits = wgpu::Limits::downlevel_webgl2_defaults()
            .using_resolution(adapter.limits());

        let (device, queue) = match adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("hopf-webgl-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                log::error!("WebGL2 device request failed: {e}");
                set_boot_status("⚠️ WebGL2 device initialization failed.");
                return None;
            }
        };

        // Surface configuration.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth_view = make_depth(&device, config.width, config.height);

        // ---- camera + render-param uniforms --------------------------------
        let camera = OrbitCamera::new(config.width as f32 / config.height as f32);
        let camera_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&camera.uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let particle_color = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle-color"),
            contents: bytemuck::bytes_of(&RenderParams {
                color: [0.55, 1.00, 0.75, 1.8], // mint, alpha = brightness
                flow_phase: 0.0,
                steps_per_line: 256,
                _pad0: 0,
                _pad1: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let box_color = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("box-color"),
            contents: bytemuck::bytes_of(&RenderParams {
                color: [0.40, 0.55, 1.00, 1.0], // cobalt wireframe
                flow_phase: 0.0,
                steps_per_line: 1,
                _pad0: 0,
                _pad1: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let stream_color = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("stream-color"),
            contents: bytemuck::bytes_of(&RenderParams {
                color: [B_LINE_RGB[0], B_LINE_RGB[1], B_LINE_RGB[2], 1.6], // B field → magenta
                flow_phase: 0.0,
                steps_per_line: STREAM_STEPS as u32,
                _pad0: 0,
                _pad1: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let eline_color = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("eline-color"),
            contents: bytemuck::bytes_of(&RenderParams {
                color: [E_LINE_RGB[0], E_LINE_RGB[1], E_LINE_RGB[2], 1.6], // E field → cyan
                flow_phase: 0.0,
                steps_per_line: STREAM_STEPS as u32,
                _pad0: 0,
                _pad1: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let mirror_color = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mirror-color"),
            contents: bytemuck::bytes_of(&RenderParams {
                color: [0.85, 0.95, 1.00, 1.0], // silvery reflecting surface
                flow_phase: 0.0,
                steps_per_line: 1,
                _pad0: 0,
                _pad1: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let particle_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle-bind"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: particle_color.as_entire_binding() },
            ],
        });
        let box_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("box-bind"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: box_color.as_entire_binding() },
            ],
        });
        let stream_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stream-bind"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: stream_color.as_entire_binding() },
            ],
        });
        let eline_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("eline-bind"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: eline_color.as_entire_binding() },
            ],
        });
        let mirror_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mirror-bind"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: mirror_color.as_entire_binding() },
            ],
        });

        // ---- pipelines -----------------------------------------------------
        let point_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render_point.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/render_point.wgsl").into()),
        });
        let line_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render_line.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/render_line.wgsl").into()),
        });
        let pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("render-pll"),
            bind_group_layouts: &[&camera_bgl],
            push_constant_ranges: &[],
        });

        let vlayout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PointVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 12, shader_location: 1 },
            ],
        };

        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        });

        let point_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point-pipeline"),
            layout: Some(&pll),
            vertex: wgpu::VertexState {
                module: &point_module,
                entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[vlayout.clone()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &point_module,
                entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line-pipeline"),
            layout: Some(&pll),
            vertex: wgpu::VertexState {
                module: &line_module,
                entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[vlayout.clone()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &line_module,
                entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // ---- egui control panel -------------------------------------------
        let egui_ctx = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            Some(wgpu::TextureFormat::Depth24Plus),
            1,
            false,
        );

        // ---- settings + FDTD grid + tracer cloud --------------------------
        let settings = WebSettings::default();
        let allocated = settings.particle_count as usize;
        let half_x_init = WORLD_R * settings.len_mult;

        // Real CPU Yee-grid Maxwell solver (the genuine field the page shows).
        let mut fdtd = crate::web_fdtd::Fdtd::new(settings.cross_n as usize, WORLD_R, half_x_init);
        fdtd.set_mirror(settings.mirror_deg.to_radians(), settings.mirror_on);

        let mut rng: u32 = 0xC0FFEE_u32;
        let mut particles = Vec::with_capacity(allocated);
        for i in 0..allocated {
            particles.push(Particle {
                pos: source_seed(&mut rng, half_x_init, settings.preset),
                age: (i as f32 * 0.0137) % 1.0 * MAX_AGE,
            });
        }
        let upload = vec![PointVertex { pos: [0.0; 3], age: 0.0 }; allocated];
        let particle_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-vbuf"),
            size: (allocated * std::mem::size_of::<PointVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Elongated "flight box": 3× as long along +X by default (len_mult).
        let half_x = WORLD_R * settings.len_mult;
        let (box_verts, box_indices) = world_box(half_x, WORLD_R);
        let box_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("box-vbuf"),
            size: (box_verts.len() * std::mem::size_of::<LineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&box_vbuf, 0, bytemuck::cast_slice(&box_verts));
        let box_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("box-ibuf"),
            contents: bytemuck::cast_slice(&box_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let box_index_count = box_indices.len() as u32;

        // Tilted end-mirror wireframe at the +X end (rings reflect off it).
        let mirror_verts = mirror_lines(half_x, WORLD_R, settings.mirror_deg.to_radians());
        let mirror_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mirror-vbuf"),
            size: (MIRROR_CAP * std::mem::size_of::<LineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mirror_vert_count = mirror_verts.len().min(MIRROR_CAP) as u32;
        queue.write_buffer(
            &mirror_vbuf,
            0,
            bytemuck::cast_slice(&mirror_verts[..mirror_vert_count as usize]),
        );

        // Streamline vertex buffer (CPU-rebuilt each frame in field-line mode).
        let line_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stream-vbuf"),
            size: (STREAM_CAP * std::mem::size_of::<LineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let eline_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eline-vbuf"),
            size: (STREAM_CAP * std::mem::size_of::<LineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Frame the camera so the whole long box is visible from the start.
        let mut camera = camera;
        camera.dist = (10.0 + 7.0 * settings.len_mult).clamp(6.0, 120.0);

        Some(Self {
            window,
            surface,
            device,
            queue,
            config,
            depth_view,
            point_pipeline,
            line_pipeline,
            camera_buf,
            particle_bind,
            box_bind,
            stream_bind,
            mirror_bind,
            particle_color,
            box_color,
            stream_color,
            mirror_color,
            particle_vbuf,
            box_vbuf,
            box_ibuf,
            box_index_count,
            mirror_vbuf,
            mirror_vert_count,
            line_vbuf,
            line_vert_count: 0,
            line_scratch: Vec::with_capacity(STREAM_CAP),
            eline_vbuf,
            eline_vert_count: 0,
            eline_scratch: Vec::with_capacity(STREAM_CAP),
            eline_color,
            eline_bind,
            fdtd,
            particles,
            upload,
            rng,
            settings,
            allocated,
            last_len: settings.len_mult,
            last_cross: settings.cross_n,
            last_mirror_deg: settings.mirror_deg,
            last_mirror_on: settings.mirror_on,
            source_phase: 0.0,
            flow_phase: 0.0,
            frame_ms: 16.0,
            last_tick: Instant::now(),
            camera,
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            rmb_down: false,
            mmb_down: false,
            touches: Vec::new(),
            pinch_dist: 0.0,
            pinch_mid: (0.0, 0.0),
            scene_px: [0.0, 0.0, size.width as f32, size.height as f32],
            egui_ctx,
            egui_winit,
            egui_renderer,
        })
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = make_depth(&self.device, size.width, size.height);
        self.camera.aspect = size.width as f32 / size.height as f32;
    }

    /// Mobile multi-touch navigation: **one finger orbits**, **two fingers
    /// pinch-zoom and pan** — the important 3-D gestures on a phone. Gated by
    /// `egui_consumed` so dragging on the control sheet never moves the camera.
    fn handle_touch(&mut self, touch: Touch, egui_consumed: bool) {
        let id = touch.id;
        let loc = touch.location;
        match touch.phase {
            TouchPhase::Started => {
                if !self.touches.iter().any(|(t, _)| *t == id) {
                    self.touches.push((id, loc));
                }
                self.reset_pinch();
            }
            TouchPhase::Moved => {
                let prev = self.touches.iter().find(|(t, _)| *t == id).map(|(_, p)| *p);
                if let Some(slot) = self.touches.iter_mut().find(|(t, _)| *t == id) {
                    slot.1 = loc;
                }
                if egui_consumed {
                    return;
                }
                match self.touches.len() {
                    1 => {
                        if let Some(prev) = prev {
                            let dx = (loc.x - prev.x) as f32;
                            let dy = (loc.y - prev.y) as f32;
                            self.camera.orbit(dx, dy);
                        }
                    }
                    n if n >= 2 => {
                        let a = self.touches[0].1;
                        let b = self.touches[1].1;
                        let dist =
                            (((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()) as f32;
                        let mid = ((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
                        if self.pinch_dist > 1.0 {
                            // spread fingers → zoom in; pinch → zoom out
                            let factor = (self.pinch_dist / dist.max(1.0)).clamp(0.5, 2.0);
                            self.camera.zoom(factor);
                            // two-finger drag pans the target
                            let pdx = (mid.0 - self.pinch_mid.0) as f32;
                            let pdy = (mid.1 - self.pinch_mid.1) as f32;
                            self.camera.pan(pdx, pdy);
                        }
                        self.pinch_dist = dist;
                        self.pinch_mid = mid;
                    }
                    _ => {}
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.touches.retain(|(t, _)| *t != id);
                self.reset_pinch();
            }
        }
    }

    /// Re-baseline the pinch gesture whenever the finger count changes, so the
    /// next move doesn't jump the camera.
    fn reset_pinch(&mut self) {
        if self.touches.len() >= 2 {
            let a = self.touches[0].1;
            let b = self.touches[1].1;
            self.pinch_dist = (((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()) as f32;
            self.pinch_mid = ((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
        } else {
            self.pinch_dist = 0.0;
        }
    }

    fn update(&mut self) {
        self.apply_settings();

        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().clamp(1.0 / 240.0, 0.05);
        self.last_tick = now;
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;

        self.flow_phase += dt * 1.4;

        let half_x = WORLD_R * self.settings.len_mult;

        // --- one-shot field requests from the UI ---------------------------
        if self.settings.clear {
            self.settings.clear = false;
            self.fdtd.clear();
        }
        // [Fire] — the always-do-something button: wipe the grid and stamp a
        // fresh, well-initialized STA pulse for the selected shape so the user
        // always sees a clean launch, however the sliders are set.
        if self.settings.fire {
            self.settings.fire = false;
            self.fdtd.clear();
            let spec = self.settings.preset.source_spec(self.settings.source_power);
            self.fdtd.stamp_pulse(&spec);
        }
        if self.settings.inject {
            self.settings.inject = false;
            let spec = self.settings.preset.source_spec(self.settings.source_power);
            self.fdtd.stamp_pulse(&spec);
        }

        // --- advance the REAL Maxwell field --------------------------------
        // Optionally drive the soft source every substep (a continuous forward
        // beam), then run the Yee leapfrog. The tilted PEC mask reflects it; the
        // −X entrance and the side walls absorb. This is genuine FDTD — E and B
        // evolved by curl equations — not a ballistic "ball" approximation.
        let substeps = self.settings.sim_speed.max(1);
        let spec = self.settings.preset.source_spec(self.settings.source_power);
        let sigma = self.settings.absorb;
        for _ in 0..substeps {
            self.source_phase += self.settings.source_freq;
            if self.settings.drive_on {
                self.fdtd.drive(&spec, self.source_phase);
            }
            self.fdtd.step(1, sigma, 0.06);
        }

        // --- advect tracer particles along the live S = E × B --------------
        let advect = self.settings.advect;
        let preset = self.settings.preset;
        let mut rng = self.rng;
        let step_cap = WORLD_R * 0.18; // keep tracers from tunnelling per frame
        let max_step = step_cap / dt.max(1e-3);
        for p in self.particles.iter_mut() {
            let s = self.fdtd.sample_s(p.pos);
            let v = (s * advect).clamp_length_max(max_step);
            p.pos += v * dt;
            p.age += dt;

            let out = p.pos.x < -half_x || p.pos.x > half_x
                || p.pos.y.abs() > WORLD_R || p.pos.z.abs() > WORLD_R;
            if p.age > MAX_AGE || out || self.fdtd.is_pec(p.pos) {
                p.pos = source_seed(&mut rng, half_x, preset);
                p.age = 0.0;
            }
        }
        self.rng = rng;

        // --- stream tracer positions to the GPU ----------------------------
        for (dst, p) in self.upload.iter_mut().zip(self.particles.iter()) {
            dst.pos = [p.pos.x, p.pos.y, p.pos.z];
            dst.age = p.age;
        }
        self.queue
            .write_buffer(&self.particle_vbuf, 0, bytemuck::cast_slice(&self.upload));

        // Uniforms: camera + per-pass colour / flow phase.
        // Match the camera aspect to the on-screen 3-D viewport (which, on a
        // narrow phone layout, is only the strip above the control sheet).
        self.camera.aspect = self.scene_px[2].max(1.0) / self.scene_px[3].max(1.0);
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&self.camera.uniform()));
        let col = preset_color(preset);
        let pr = RenderParams {
            color: [col[0], col[1], col[2], self.settings.brightness],
            flow_phase: self.flow_phase,
            steps_per_line: 256,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.particle_color, 0, bytemuck::bytes_of(&pr));
        // B field lines → magenta; E field lines → cyan (the two-colour view).
        let br = RenderParams {
            color: [B_LINE_RGB[0], B_LINE_RGB[1], B_LINE_RGB[2], self.settings.brightness * 0.95],
            flow_phase: self.flow_phase,
            steps_per_line: STREAM_STEPS as u32,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.stream_color, 0, bytemuck::bytes_of(&br));
        let er = RenderParams {
            color: [E_LINE_RGB[0], E_LINE_RGB[1], E_LINE_RGB[2], self.settings.brightness * 0.95],
            flow_phase: self.flow_phase,
            steps_per_line: STREAM_STEPS as u32,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.eline_color, 0, bytemuck::bytes_of(&er));
        let bp = RenderParams {
            color: [0.40, 0.55, 1.00, 1.0],
            flow_phase: self.flow_phase,
            steps_per_line: 1,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.box_color, 0, bytemuck::bytes_of(&bp));
        let mp = RenderParams {
            color: [0.85, 0.95, 1.00, 1.0], // silvery reflecting surface
            flow_phase: self.flow_phase,
            steps_per_line: 1,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.mirror_color, 0, bytemuck::bytes_of(&mp));

        match self.settings.field {
            WebField::Poynting => {}
            WebField::E => self.rebuild_field_lines(true, false),
            WebField::B => self.rebuild_field_lines(false, true),
            WebField::Both => self.rebuild_field_lines(true, true),
        }
    }

    /// Apply pending UI changes (box length, ring count, particle count, reseed,
    /// fit) before stepping the physics.
    fn apply_settings(&mut self) {
        // Box length changed → rebuild the Yee grid + rewrite wireframe + mirror.
        if (self.settings.len_mult - self.last_len).abs() > 1e-4 {
            self.last_len = self.settings.len_mult;
            let half_x = WORLD_R * self.settings.len_mult;
            let (verts, _idx) = world_box(half_x, WORLD_R);
            self.queue
                .write_buffer(&self.box_vbuf, 0, bytemuck::cast_slice(&verts));
            // Resize the grid to the new box and re-carve the PEC mirror.
            self.fdtd = crate::web_fdtd::Fdtd::new(self.settings.cross_n as usize, WORLD_R, half_x);
            self.fdtd
                .set_mirror(self.settings.mirror_deg.to_radians(), self.settings.mirror_on);
            self.upload_mirror(half_x);
            self.reseed();
        }

        // Grid resolution changed → rebuild the Yee grid at the new cross count.
        // More cells = far more Maxwell crunching per substep (cost ~ cross_n³),
        // so this is the knob that makes the CPU actually work for the field.
        if self.settings.cross_n != self.last_cross {
            self.last_cross = self.settings.cross_n;
            let half_x = WORLD_R * self.settings.len_mult;
            self.fdtd = crate::web_fdtd::Fdtd::new(self.settings.cross_n as usize, WORLD_R, half_x);
            self.fdtd
                .set_mirror(self.settings.mirror_deg.to_radians(), self.settings.mirror_on);
            self.upload_mirror(half_x);
            self.reseed();
        }

        // Mirror tilt / on-off changed → re-carve the PEC mask + wireframe.
        if (self.settings.mirror_deg - self.last_mirror_deg).abs() > 1e-3
            || self.settings.mirror_on != self.last_mirror_on
        {
            self.last_mirror_deg = self.settings.mirror_deg;
            self.last_mirror_on = self.settings.mirror_on;
            self.fdtd
                .set_mirror(self.settings.mirror_deg.to_radians(), self.settings.mirror_on);
            let half_x = WORLD_R * self.settings.len_mult;
            self.upload_mirror(half_x);
        }

        let mut need_reseed = self.settings.reseed;
        self.settings.reseed = false;

        if self.settings.particle_count as usize != self.allocated {
            self.allocated = self.settings.particle_count as usize;
            self.particle_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle-vbuf"),
                size: (self.allocated * std::mem::size_of::<PointVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.upload = vec![PointVertex { pos: [0.0; 3], age: 0.0 }; self.allocated];
            need_reseed = true;
        }

        if need_reseed {
            self.reseed();
        }

        if self.settings.fit {
            self.settings.fit = false;
            self.frame_camera();
        }
    }

    fn reseed(&mut self) {
        let half_x = WORLD_R * self.settings.len_mult;
        let preset = self.settings.preset;
        let mut rng = self.rng;
        self.particles.clear();
        self.particles.reserve(self.allocated);
        for i in 0..self.allocated {
            self.particles.push(Particle {
                pos: source_seed(&mut rng, half_x, preset),
                age: (i as f32 * 0.0137) % 1.0 * MAX_AGE,
            });
        }
        self.rng = rng;
        if self.upload.len() != self.allocated {
            self.upload = vec![PointVertex { pos: [0.0; 3], age: 0.0 }; self.allocated];
        }
    }

    /// Reframe the camera to take in the whole (possibly very long) flight box.
    fn frame_camera(&mut self) {
        self.camera.fit();
        self.camera.dist = (10.0 + 7.0 * self.settings.len_mult).clamp(6.0, 120.0);
    }

    /// Rebuild + upload the tilted end-mirror wireframe for the current tilt.
    fn upload_mirror(&mut self, half_x: f32) {
        let verts = mirror_lines(half_x, WORLD_R, self.settings.mirror_deg.to_radians());
        let n = verts.len().min(MIRROR_CAP);
        self.queue
            .write_buffer(&self.mirror_vbuf, 0, bytemuck::cast_slice(&verts[..n]));
        self.mirror_vert_count = n as u32;
    }

    /// Trace live field lines through the FDTD grid. `do_e` fills the cyan E
    /// buffer; `do_b` fills the magenta B buffer; both share one seed lattice.
    fn rebuild_field_lines(&mut self, do_e: bool, do_b: bool) {
        let half_x = WORLD_R * self.settings.len_mult;
        if do_e {
            self.eline_scratch.clear();
        }
        if do_b {
            self.line_scratch.clear();
        }
        // Seed a coarse lattice across the box and follow each field from there.
        let nx_seed = 14i32;
        let nr_seed = 8i32;
        for ix in 0..nx_seed {
            let fx = (ix as f32 + 0.5) / nx_seed as f32;
            let x0 = -half_x + fx * 2.0 * half_x;
            for iy in 0..nr_seed {
                for iz in 0..nr_seed {
                    let y0 = (iy as f32 + 0.5) / nr_seed as f32 * 2.0 * WORLD_R - WORLD_R;
                    let z0 = (iz as f32 + 0.5) / nr_seed as f32 * 2.0 * WORLD_R - WORLD_R;
                    let seed = Vec3::new(x0, y0 * 0.85, z0 * 0.85);
                    if do_e {
                        trace_field_line(&self.fdtd, seed, half_x, true, &mut self.eline_scratch);
                    }
                    if do_b {
                        trace_field_line(&self.fdtd, seed, half_x, false, &mut self.line_scratch);
                    }
                }
            }
        }
        if do_e {
            let n = self.eline_scratch.len().min(STREAM_CAP);
            if n > 0 {
                self.queue
                    .write_buffer(&self.eline_vbuf, 0, bytemuck::cast_slice(&self.eline_scratch[..n]));
            }
            self.eline_vert_count = n as u32;
        } else {
            self.eline_vert_count = 0;
        }
        if do_b {
            let n = self.line_scratch.len().min(STREAM_CAP);
            if n > 0 {
                self.queue
                    .write_buffer(&self.line_vbuf, 0, bytemuck::cast_slice(&self.line_scratch[..n]));
            }
            self.line_vert_count = n as u32;
        } else {
            self.line_vert_count = 0;
        }
    }

    fn render(&mut self) {
        // ----- egui control panel -----
        let raw = self.egui_winit.take_egui_input(&self.window);
        let mut settings = self.settings;
        let fps = if self.frame_ms > 0.0 { 1000.0 / self.frame_ms } else { 0.0 };
        let mut scene_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(0.0, 0.0));
        let full = self.egui_ctx.run(raw, |ctx| {
            scene_rect = draw_web_ui(ctx, &mut settings, fps);
        });
        self.settings = settings;
        self.egui_winit
            .handle_platform_output(&self.window, full.platform_output);
        let ppp = self.egui_ctx.pixels_per_point();
        // Convert the scene rect (egui points) to physical px for the GPU
        // viewport, so the 3-D view sits in the strip above the phone controls.
        let fw = self.config.width as f32;
        let fh = self.config.height as f32;
        let vx = (scene_rect.min.x * ppp).clamp(0.0, fw);
        let vy = (scene_rect.min.y * ppp).clamp(0.0, fh);
        let vw = (scene_rect.width() * ppp).clamp(1.0, (fw - vx).max(1.0));
        let vh = (scene_rect.height() * ppp).clamp(1.0, (fh - vy).max(1.0));
        self.scene_px = [vx, vy, vw, vh];
        let jobs = self.egui_ctx.tessellate(full.shapes, ppp);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: ppp,
        };

        // ----- apply UI + step physics -----
        self.update();

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        for (id, image) in &full.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image);
        }
        self.egui_renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.012, g: 0.012, b: 0.022, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Constrain the 3-D view to the scene viewport (the strip above
            // the phone control sheet); on desktop this is the whole surface.
            let [vx, vy, vw, vh] = self.scene_px;
            rp.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            rp.set_scissor_rect(vx as u32, vy as u32, vw as u32, vh as u32);

            // World box wireframe (the elongated flight tube).
            rp.set_pipeline(&self.line_pipeline);
            rp.set_bind_group(0, &self.box_bind, &[]);
            rp.set_vertex_buffer(0, self.box_vbuf.slice(..));
            rp.set_index_buffer(self.box_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..self.box_index_count, 0, 0..1);

            // Tilted end-mirror (the reflecting PEC wall).
            if self.mirror_vert_count > 0 {
                rp.set_bind_group(0, &self.mirror_bind, &[]);
                rp.set_vertex_buffer(0, self.mirror_vbuf.slice(..));
                rp.draw(0..self.mirror_vert_count, 0..1);
            }

            match self.settings.field {
                WebField::Poynting => {
                    rp.set_pipeline(&self.point_pipeline);
                    rp.set_bind_group(0, &self.particle_bind, &[]);
                    rp.set_vertex_buffer(0, self.particle_vbuf.slice(..));
                    rp.draw(0..self.allocated as u32, 0..1);
                }
                WebField::E | WebField::B | WebField::Both => {
                    rp.set_pipeline(&self.line_pipeline);
                    // Magenta B lines.
                    if matches!(self.settings.field, WebField::B | WebField::Both)
                        && self.line_vert_count > 0
                    {
                        rp.set_bind_group(0, &self.stream_bind, &[]);
                        rp.set_vertex_buffer(0, self.line_vbuf.slice(..));
                        rp.draw(0..self.line_vert_count, 0..1);
                    }
                    // Cyan E lines (drawn second so they read over the magenta).
                    if matches!(self.settings.field, WebField::E | WebField::Both)
                        && self.eline_vert_count > 0
                    {
                        rp.set_bind_group(0, &self.eline_bind, &[]);
                        rp.set_vertex_buffer(0, self.eline_vbuf.slice(..));
                        rp.draw(0..self.eline_vert_count, 0..1);
                    }
                }
            }

            // Reset to the full surface so egui (overlay + control sheet)
            // draws across the whole canvas, undistorted.
            rp.set_viewport(0.0, 0.0, fw, fh, 0.0, 1.0);
            rp.set_scissor_rect(0, 0, self.config.width, self.config.height);
            self.egui_renderer
                .render(&mut rp.forget_lifetime(), &jobs, &screen);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        for id in &full.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        frame.present();
    }
}

// ---------------------------------------------------------------------------
// Tracer seeding — particles are born at the −X soft source and ride the live
// FDTD field (S = E × B), so the cloud is a direct read-out of the real
// electromagnetic energy flow, not an analytic stand-in.
// ---------------------------------------------------------------------------

/// Integrate one field line from `seed` through `fdtd` (E if `electric`, else B),
/// pushing line-list segment vertices into `out` (capped at STREAM_CAP).
fn trace_field_line(
    fdtd: &crate::web_fdtd::Fdtd,
    seed: Vec3,
    half_x: f32,
    electric: bool,
    out: &mut Vec<LineVertex>,
) {
    if out.len() >= STREAM_CAP {
        return;
    }
    let mut q = seed;
    let mut prev = q;
    for _ in 0..STREAM_STEPS {
        let f = if electric { fdtd.sample_e(q) } else { fdtd.sample_b(q) };
        let m = f.length();
        if m <= 1e-4 {
            break;
        }
        q += (f / m) * STREAM_DS;
        let mag = (m * 2.0).min(1.0);
        out.push(LineVertex { pos: [prev.x, prev.y, prev.z], mag });
        out.push(LineVertex { pos: [q.x, q.y, q.z], mag });
        prev = q;
        let outside = q.x.abs() > half_x || q.y.abs() > WORLD_R || q.z.abs() > WORLD_R;
        if outside || out.len() >= STREAM_CAP {
            break;
        }
    }
}

/// Spawn a tracer near the −X soft source so the forward beam pushes it along.
/// The transverse spread matches the preset's source profile.
fn source_seed(rng: &mut u32, half_x: f32, preset: WebPreset) -> Vec3 {
    let x = -0.60 * half_x + (lcg(rng) - 0.5) * 0.18 * half_x;
    let (radius, plane) = match preset {
        WebPreset::Hopfion       => (1.6, false),
        WebPreset::PhotonHopfion => (1.0, false),
        WebPreset::FlyingDonut   => (1.9, false),
        WebPreset::RadialDonut   => (1.6, false),
        WebPreset::Trefoil       => (2.2, false),
        WebPreset::PlanePhoton   => (0.0, true),
        WebPreset::CpPhoton      => (0.0, false),
        WebPreset::PhasedArray   => (0.0, true),
    };
    if plane {
        let y = (lcg(rng) - 0.5) * 1.7 * WORLD_R;
        let z = (lcg(rng) - 0.5) * 1.7 * WORLD_R;
        Vec3::new(x, y, z)
    } else if radius <= 1e-3 {
        let y = (lcg(rng) - 0.5) * 1.4;
        let z = (lcg(rng) - 0.5) * 1.4;
        Vec3::new(x, y, z)
    } else {
        let ang = lcg(rng) * std::f32::consts::TAU;
        let rr = radius + (lcg(rng) - 0.5) * 0.9;
        Vec3::new(x, rr * ang.cos(), rr * ang.sin())
    }
}

/// Particle / streamline tint per photon type.
fn preset_color(preset: WebPreset) -> [f32; 3] {
    match preset {
        WebPreset::Hopfion       => [0.55, 1.00, 0.75], // mint
        WebPreset::PhotonHopfion => [0.45, 0.90, 1.00], // cyan
        WebPreset::FlyingDonut   => [1.00, 0.75, 0.40], // amber
        WebPreset::RadialDonut   => [1.00, 0.58, 0.30], // orange
        WebPreset::PlanePhoton   => [0.70, 1.00, 0.50], // lime
        WebPreset::CpPhoton      => [1.00, 0.50, 0.85], // magenta
        WebPreset::Trefoil       => [0.80, 0.60, 1.00], // violet
        WebPreset::PhasedArray   => [1.00, 0.90, 0.45], // gold
    }
}

// ---------------------------------------------------------------------------
// Geometry / RNG helpers.
// ---------------------------------------------------------------------------

/// Inward unit normal of the tilted +X end-mirror (tilt `theta` about the Z axis).
fn mirror_normal(theta: f32) -> Vec3 {
    Vec3::new(-theta.cos(), -theta.sin(), 0.0)
}

/// Wireframe (line list) of the tilted end-mirror quad: outline + diagonals +
/// a couple of mid-lines + a short normal stub so the surface reads clearly.
fn mirror_lines(half_x: f32, r: f32, theta: f32) -> Vec<LineVertex> {
    let n = mirror_normal(theta);
    let p0 = Vec3::new(half_x, 0.0, 0.0);
    let u = Vec3::new(0.0, 0.0, 1.0);                     // in-plane (Z axis)
    let w = Vec3::new(-theta.sin(), theta.cos(), 0.0);   // in-plane (tilted, XY)
    let v = |p: Vec3| LineVertex { pos: [p.x, p.y, p.z], mag: 1.0 };
    let c = [
        p0 - w * r - u * r,
        p0 + w * r - u * r,
        p0 + w * r + u * r,
        p0 - w * r + u * r,
    ];
    let mut out = Vec::with_capacity(MIRROR_CAP);
    for k in 0..4 {
        out.push(v(c[k]));
        out.push(v(c[(k + 1) % 4]));
    }
    out.push(v(c[0])); out.push(v(c[2]));
    out.push(v(c[1])); out.push(v(c[3]));
    out.push(v(p0 - u * r)); out.push(v(p0 + u * r));
    out.push(v(p0 - w * r)); out.push(v(p0 + w * r));
    out.push(v(p0));         out.push(v(p0 + n * (r * 0.6))); // normal stub
    out
}

/// Tiny LCG → uniform f32 in [0, 1). Used for tracer-seed jitter.
fn lcg(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 8) & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// 12-edge wireframe box, elongated to `half_x` along X and `r` on Y/Z.
fn world_box(half_x: f32, r: f32) -> (Vec<LineVertex>, Vec<u32>) {
    let v = |x: f32, y: f32, z: f32| LineVertex { pos: [x, y, z], mag: 1.0 };
    let hx = half_x;
    let verts = vec![
        v(-hx, -r, -r), v(hx, -r, -r), v(hx, r, -r), v(-hx, r, -r),
        v(-hx, -r, r), v(hx, -r, r), v(hx, r, r), v(-hx, r, r),
    ];
    let idx = vec![
        0, 1, 1, 2, 2, 3, 3, 0, // back face
        4, 5, 5, 6, 6, 7, 7, 4, // front face
        0, 4, 1, 5, 2, 6, 3, 7, // connectors
    ];
    (verts, idx)
}

// ---------------------------------------------------------------------------
// egui control panel.
// ---------------------------------------------------------------------------

/// Draw the whole egui surface and return the **scene rect** (in egui points)
/// that the 3-D view should occupy — the strip above the phone control sheet,
/// or the full screen on desktop.
fn draw_web_ui(ctx: &egui::Context, s: &mut WebSettings, fps: f32) -> egui::Rect {
    let screen = ctx.screen_rect();
    let narrow = screen.width() < 560.0;

    // ---- top overlay: [Fit view] + [Next demo], floating over the scene ----
    egui::Area::new(egui::Id::new("top-overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(8, 12, 20, 205))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                .show(ui, |ui| {
                    let h = if narrow { 40.0 } else { 30.0 };
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized([92.0, h], egui::Button::new(
                                egui::RichText::new("Fit view").strong()))
                            .clicked()
                        {
                            s.fit = true;
                        }
                        let next = egui::Button::new(
                            egui::RichText::new("Next demo ▸").strong())
                            .fill(egui::Color32::from_rgb(26, 92, 122));
                        if ui.add_sized([124.0, h], next).clicked() {
                            s.preset = s.preset.next();
                            s.fire = true; // every demo starts with a launch
                        }
                    });
                });
        });

    // ---- controls: bottom sheet on phones, floating window on desktop ----
    if narrow {
        egui::TopBottomPanel::bottom("controls")
            .resizable(false)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(12, 14, 20))
                    .inner_margin(egui::Margin::same(8.0)),
            )
            .show(ctx, |ui| {
                let max_h = (screen.height() * 0.46).max(150.0);
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| controls_ui(ui, s, fps));
            });
        // The 3-D view fills everything above the control sheet.
        ctx.available_rect()
    } else {
        egui::Window::new("hopf-sta-viz · WebGL2 + CPU FDTD")
            .default_pos([12.0, 52.0])
            .resizable(false)
            .show(ctx, |ui| controls_ui(ui, s, fps));
        // The floating window overlaps the scene; the 3-D view uses the whole surface.
        screen
    }
}

/// The shared control widgets, reused by the desktop window and the phone sheet.
fn controls_ui(ui: &mut egui::Ui, s: &mut WebSettings, fps: f32) {
    ui.label(format!("{fps:.0} fps · {} · CPU Yee-FDTD", s.preset.label()));
    ui.separator();

    egui::ComboBox::from_label("Source shape (STA null field)")
        .selected_text(s.preset.label())
        .show_ui(ui, |ui| {
            for p in WebPreset::ALL {
                ui.selectable_value(&mut s.preset, *p, p.label());
            }
        });

    ui.horizontal(|ui| {
        ui.label("Field:");
        ui.selectable_value(&mut s.field, WebField::Poynting, "S = E×B");
        ui.selectable_value(&mut s.field, WebField::E, "E");
        ui.selectable_value(&mut s.field, WebField::B, "B");
        ui.selectable_value(&mut s.field, WebField::Both, "E & B");
    });

    // The always-works [Fire] button: clears the grid and stamps a fresh,
    // well-initialized STA pulse for the selected shape — so it always does
    // something visible, however the sliders are set.
    let fire = egui::Button::new(
        egui::RichText::new("FIRE  —  launch pulse").strong().size(15.0),
    )
    .fill(egui::Color32::from_rgb(150, 32, 44));
    if ui.add_sized([ui.available_width(), 32.0], fire).clicked() {
        s.fire = true;
    }
    ui.checkbox(&mut s.drive_on, "Continuous source (steady beam)");

    ui.separator();
    ui.checkbox(&mut s.mirror_on, "PEC mirror at +X end");
    ui.add(egui::Slider::new(&mut s.mirror_deg, 0.0..=60.0).text("Mirror tilt °"));
    ui.add(egui::Slider::new(&mut s.len_mult, 1.0..=5.0).text("Box length ×"));
    ui.add(
        egui::Slider::new(&mut s.cross_n, 24u32..=72u32)
            .text("Grid (cross cells) · CPU load ∝ n³"),
    );

    ui.separator();
    ui.add(egui::Slider::new(&mut s.sim_speed, 1u32..=4).text("Sim speed (substeps)"));
    ui.add(egui::Slider::new(&mut s.source_power, 0.0..=1.5).text("Source power"));
    ui.add(egui::Slider::new(&mut s.source_freq, 0.15..=1.2).text("Source frequency"));
    ui.add(egui::Slider::new(&mut s.absorb, 0.002..=0.05).text("Absorption σ"));
    ui.add(egui::Slider::new(&mut s.advect, 0.5..=10.0).text("Tracer drift"));
    ui.add(egui::Slider::new(&mut s.brightness, 0.2..=3.0).text("Brightness"));
    ui.add(
        egui::Slider::new(&mut s.particle_count, 12_000u32..=120_000u32)
            .text("Tracers")
            .step_by(2000.0),
    );

    ui.horizontal(|ui| {
        if ui.button("Stamp pulse (add)").clicked() {
            s.inject = true;
        }
        if ui.button("Clear field").clicked() {
            s.clear = true;
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Reseed tracers").clicked() {
            s.reseed = true;
        }
        if ui.button("Fit view").clicked() {
            s.fit = true;
        }
    });

    ui.separator();
    ui.label("Touch: 1 finger orbit · 2 fingers pinch-zoom + pan");
    ui.label("Mouse: RMB orbit · MMB pan · wheel zoom");
}

fn depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth24Plus,
        depth_write_enabled: true,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Post a status line into the page's boot overlay (shown until WebGL2 is live).
fn set_boot_status(html: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("boot-status") {
            el.set_inner_html(html);
        }
    }
}
