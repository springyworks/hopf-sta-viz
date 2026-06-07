use std::sync::Arc;
use web_time::Instant;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::camera::OrbitCamera;
use crate::ui;

// ---------------------------------------------------------------------------
// Public types shared with the UI.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FieldMode { E, B, Both, Poynting }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RenderMode { Lines, Particles, Fdtd }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Preset {
    Hopfion,
    PhotonHopfion,
    Donut,
    PlanePhoton,
    CpPhoton,
    Trefoil,
    StaCrunch,
}

impl Preset {
    pub const ALL: &'static [Preset] = &[
        Preset::Hopfion, Preset::PhotonHopfion, Preset::Donut,
        Preset::PlanePhoton, Preset::CpPhoton, Preset::Trefoil, Preset::StaCrunch,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Preset::Hopfion       => "Hopfion (Rañada link)",
            Preset::PhotonHopfion => "Single-photon hopfion",
            Preset::Donut         => "Flying donut (link=0)",
            Preset::PlanePhoton   => "Plane-wave photon",
            Preset::CpPhoton      => "Circular-pol photon",
            Preset::Trefoil       => "Trefoil hopfion (k=2)",
            Preset::StaCrunch     => "STA crunch (stress test)",
        }
    }
    pub fn shader_id(self) -> u32 {
        match self {
            Preset::Hopfion       => 0,
            Preset::PhotonHopfion => 1,
            Preset::Donut         => 2,
            Preset::PlanePhoton   => 3,
            Preset::CpPhoton      => 4,
            Preset::Trefoil       => 5,
            Preset::StaCrunch     => 0,
        }
    }
    /// Returns (scale, seed_count, steps_per_line, step_len, particle_count, particle_speed, time_scale).
    pub fn defaults(self) -> PresetDefaults {
        match self {
            Preset::Hopfion       => PresetDefaults { scale: 1.5, seeds: 2048,  steps: 256, step_len: 0.04,  particles:  131_072, particle_speed: 1.2, time_scale: 0.6 },
            Preset::PhotonHopfion => PresetDefaults { scale: 0.6, seeds: 2048,  steps: 256, step_len: 0.018, particles:  131_072, particle_speed: 0.6, time_scale: 0.9 },
            Preset::Donut         => PresetDefaults { scale: 1.4, seeds: 1536,  steps: 256, step_len: 0.04,  particles:  131_072, particle_speed: 1.0, time_scale: 0.5 },
            Preset::PlanePhoton   => PresetDefaults { scale: 1.0, seeds:  768,  steps: 220, step_len: 0.06,  particles:   65_536, particle_speed: 1.6, time_scale: 1.5 },
            Preset::CpPhoton      => PresetDefaults { scale: 1.0, seeds: 1536,  steps: 256, step_len: 0.04,  particles:  131_072, particle_speed: 1.6, time_scale: 1.5 },
            Preset::Trefoil       => PresetDefaults { scale: 1.5, seeds: 2048,  steps: 320, step_len: 0.035, particles:  131_072, particle_speed: 1.0, time_scale: 0.7 },
            Preset::StaCrunch     => PresetDefaults { scale: 1.5, seeds: 8192,  steps: 512, step_len: 0.025, particles:  600_000, particle_speed: 1.2, time_scale: 0.6 },
        }
    }
}

pub struct PresetDefaults {
    pub scale:           f32,
    pub seeds:           u32,
    pub steps:           u32,
    pub step_len:        f32,
    pub particles:       u32,
    pub particle_speed:  f32,
    pub time_scale:      f32,
}

#[derive(Debug)]
pub struct SimSettings {
    pub time:       f32,
    pub time_scale: f32,
    pub playing:    bool,

    // ---- simulation transport (Play / Pause / Step / Quit / Fit) -------
    pub sim_speed:      f32,   // master speed multiplier (analytic time + FDTD substeps)
    pub step_once:      bool,  // advance exactly one frame while paused
    pub quit_requested: bool,  // user pressed Quit → app exits next frame
    pub fit_requested:  bool,  // user pressed Fit → camera reframes the scene

    // ---- auto-pulse: fire a fresh FDTD donut on a steady rhythm ---------
    pub auto_pulse:     bool,  // keep launching pulses so there is always motion
    pub pulse_interval: f32,   // seconds between automatic pulses
    pub pulse_clock:    f32,   // internal accumulator (seconds since last pulse)

    pub scale:      f32,
    pub field_mode: FieldMode,
    pub render_mode: RenderMode,

    pub seed_count_request:     u32,
    pub steps_per_line_request: u32,
    pub step_len:               f32,
    pub topology_dirty:         bool,

    pub particle_count_request: u32,
    pub particle_speed:         f32,
    pub particles_dirty:        bool,

    pub preset:          Preset,
    pub preset_dirty:    bool,
    pub demo_cycle:      bool,
    pub demo_clock:      f32,
    pub last_flow_phase: f32,
    pub export_requested: bool,
    pub last_export_path: Option<String>,

    pub last_frame_ms: f32,

    // ---- FDTD ----------------------------------------------------------
    pub fdtd_substeps:        u32,
    pub fdtd_mirror_enabled:  bool,
    pub fdtd_show_mirror:     bool,
    pub fdtd_mirror_gap:      f32,    // wall distance in from the cube's far face (frac of world_extent; 0 = touching)
    pub fdtd_reseed_requested: bool,
    pub fdtd_drift_scale:     f32,
    pub fdtd_step:            f32,
    pub fdtd_dt:              f32,
    pub fdtd_sigma:           f32,
    pub fdtd_total_steps:     u32,

    // ---- FDTD visual tuning (live, no rebuild) -------------------------
    pub fdtd_brightness:      f32,    // alpha multiplier for the dot color
    pub fdtd_density_gate:    f32,    // hide particles where |E x B| < gate
    pub fdtd_max_age:         f32,    // seconds before a particle respawns
    pub fdtd_respawn_scale:   f32,    // scale of the respawn box half-extents
    pub fdtd_respawn_x:       f32,    // -1..+1, normalized world_extent offset
    pub fdtd_color:           [f32; 3],
    pub fdtd_pml_strength:    f32,

    // ---- FDTD seed shape (applied on next RESEED) ----------------------
    pub fdtd_seed_radius_frac: f32,   // fraction of world_extent
    pub fdtd_seed_width_frac:  f32,   // fraction of world_extent
    pub fdtd_seed_amp:         f32,

    // ---- F1 context help + user remark log -----------------------------
    pub help: HelpLog,
}

/// One user-written remark, tagged with the object-ID of the control that
/// was being manipulated. Serialized into `ui-remarks.json`.
#[derive(Debug, Clone, Default)]
pub struct Remark {
    pub object_id:   String,
    pub value:       String,
    pub note:        String,
    pub unix_ms:     u128,
    pub preset:      String,
    pub render_mode: String,
    pub field_mode:  String,
}

/// Live state for the F1 context-help popup and the accumulated remark log.
#[derive(Debug, Default)]
pub struct HelpLog {
    /// object-ID of the control whose remark popup is currently open.
    pub open_id:        Option<String>,
    pub open_help:      String,
    pub open_value:     String,
    pub draft:          String,
    pub remarks:        Vec<Remark>,
    pub flush_requested: bool,
    pub last_log_path:  Option<String>,
}

impl Default for SimSettings {
    fn default() -> Self {
        let d = Preset::Hopfion.defaults();
        Self {
            time: 0.0,
            time_scale: d.time_scale,
            playing: true,

            sim_speed:      1.0,
            step_once:      false,
            quit_requested: false,
            fit_requested:  false,

            auto_pulse:     true,
            pulse_interval: 2.0,
            // Start "due" so the very first frame fires a pulse → the app
            // always opens with a donut already flying.
            pulse_clock:    2.0,

            scale: d.scale,
            field_mode: FieldMode::Both,
            render_mode: RenderMode::Fdtd,

            seed_count_request:     d.seeds,
            steps_per_line_request: d.steps,
            step_len:               d.step_len,
            topology_dirty:         false,

            particle_count_request: d.particles,
            particle_speed:         d.particle_speed,
            particles_dirty:        false,

            preset:          Preset::Hopfion,
            preset_dirty:    false,
            demo_cycle:      true,
            demo_clock:      0.0,
            last_flow_phase: 0.0,
            export_requested: false,
            last_export_path: None,

            last_frame_ms: 16.0,

            // FDTD defaults — sized to warm an RTX 2070 to roughly 60 °C.
            fdtd_substeps:        8,
            fdtd_mirror_enabled:  true,
            fdtd_show_mirror:     true,
            fdtd_mirror_gap:      0.45,
            fdtd_reseed_requested: false,
            fdtd_drift_scale:     8.0,
            fdtd_step:            0.012,
            fdtd_dt:              0.45,
            fdtd_sigma:           0.001,
            fdtd_total_steps:     0,

            // Visual tuning — start bright with a density gate that hides
            // the empty corners of the cube so you actually see the donut.
            fdtd_brightness:      1.8,
            fdtd_density_gate:    0.04,
            fdtd_max_age:         3.5,
            fdtd_respawn_scale:   1.0,
            fdtd_respawn_x:       0.0,
            fdtd_color:           [0.55, 1.00, 0.75],   // mint
            fdtd_pml_strength:    0.06,

            fdtd_seed_radius_frac: 0.18,
            fdtd_seed_width_frac:  0.10,
            fdtd_seed_amp:         1.0,

            help: HelpLog::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// GPU-side structs (kept byte-compatible with the WGSL definitions).
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Params {
    time:           f32,
    scale:          f32,
    step_len:       f32,
    speed:          f32,
    field_mode:     u32,
    steps_per_line: u32,
    seed_count:     u32,
    dt:             f32,
    bbox_radius:    f32,
    preset:         u32,
    flow_phase:     f32,
    _pad0:          u32,
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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Vertex { pos: [f32; 3], mag: f32 }

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
struct Particle { pos: [f32; 3], age: f32 }

// ---------------------------------------------------------------------------
// One renderable + computable "channel" (E, B or Poynting).
// ---------------------------------------------------------------------------

struct Channel {
    #[allow(dead_code)] field_mode_id: u32,
    color:         [f32; 4],

    // streamlines
    verts:                wgpu::Buffer,
    streamline_bind:      wgpu::BindGroup,

    // particles
    particles:            wgpu::Buffer,
    particle_bind:        wgpu::BindGroup,

    // render uniform (color + flow phase + steps_per_line)
    color_buf:            wgpu::Buffer,
    line_render_bind:     wgpu::BindGroup,
    particle_render_bind: wgpu::BindGroup,
}

// ---------------------------------------------------------------------------
// State.
// ---------------------------------------------------------------------------

pub(crate) struct State {
    window:   Arc<Window>,
    surface:  wgpu::Surface<'static>,
    device:   wgpu::Device,
    queue:    wgpu::Queue,
    config:   wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,

    // simulation params
    sim: SimSettings,
    seed_count:     u32,
    steps_per_line: u32,
    particle_count: u32,

    // uniforms / bind groups (shared)
    params_buf:          wgpu::Buffer,
    camera_buf:          wgpu::Buffer,
    #[allow(dead_code)] camera_bgl: wgpu::BindGroupLayout,

    // streamline pipeline
    streamline_compute:  wgpu::ComputePipeline,
    streamline_render:   wgpu::RenderPipeline,
    streamline_bgl:      wgpu::BindGroupLayout,
    seeds_buf:           wgpu::Buffer,
    index_buf:           wgpu::Buffer,
    index_count:         u32,

    // particle pipeline
    particle_compute:    wgpu::ComputePipeline,
    particle_render:     wgpu::RenderPipeline,
    particle_bgl:        wgpu::BindGroupLayout,

    // channels: E, B, Poynting
    channel_e: Channel,
    channel_b: Channel,
    channel_s: Channel,

    // ---- FDTD subsystem (real Yee-grid Maxwell simulator) ----
    fdtd:                   crate::fdtd::FdtdState,
    fdtd_color_buf:         wgpu::Buffer,
    #[allow(dead_code)] fdtd_mirror_color_buf:  wgpu::Buffer,
    fdtd_render_bind:       wgpu::BindGroup,
    fdtd_mirror_render_bind: wgpu::BindGroup,

    // camera + input
    camera: OrbitCamera,
    mouse_pos: PhysicalPosition<f64>,
    rmb_down:  bool,
    mmb_down:  bool,

    // egui
    egui_ctx:      egui::Context,
    egui_winit:    egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    last_tick: Instant,
}

// ---------------------------------------------------------------------------
// App.
// ---------------------------------------------------------------------------

/// Event delivered to the winit loop once the (async) GPU state is ready.
/// On the web the device/queue must be requested asynchronously, so we build
/// `State` off the main control flow and hand it back through this event.
/// (Constructed only on wasm; native fills `App::state` synchronously.)
#[allow(dead_code)]
pub(crate) enum UserEvent { StateReady(State) }

pub(crate) struct App {
    state: Option<State>,
    #[allow(dead_code)] // read only on the wasm async path
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

impl App {
    pub(crate) fn new(event_loop: &winit::event_loop::EventLoop<UserEvent>) -> Self {
        Self { state: None, proxy: event_loop.create_proxy() }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        let attrs = Window::default_attributes()
            .with_title("hopf-sta-viz · Rañada EM hopfion · STA · wgpu")
            .with_inner_size(winit::dpi::LogicalSize::new(1400, 900));
        let window = Arc::new(el.create_window(attrs).expect("window"));

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.state = pollster::block_on(State::new(window));
        }

        #[cfg(target_arch = "wasm32")]
        {
            // Mount winit's <canvas> into the page so the surface is visible.
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
            // Request the GPU device asynchronously, then deliver `State` back
            // through the event loop (we must not block on the web).
            let proxy = self.proxy.clone();
            wasm_bindgen_futures::spawn_local(async move {
                // On success hand `State` back through the loop; on failure the
                // fallback notice is already shown and we simply stop here —
                // no panic, no red console errors.
                if let Some(state) = State::new(window).await {
                    let _ = proxy.send_event(UserEvent::StateReady(state));
                }
            });
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::StateReady(state) = event;
        // WebGPU is live — drop the "initializing / needs WebGPU" overlay.
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                if let Some(el) = doc.get_element_by_id("boot-msg") {
                    let _ = el.set_attribute("hidden", "");
                }
            }
        }
        state.window.request_redraw();
        self.state = Some(state);
    }

    fn window_event(
        &mut self,
        el: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else { return; };

        // Let egui consume events first.
        let response = state.egui_winit.on_window_event(&state.window, &event);
        let consumed = response.consumed;
        if response.repaint { state.window.request_redraw(); }

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => state.resize(size),
            WindowEvent::RedrawRequested => {
                state.update();
                state.render();
                if state.sim.quit_requested {
                    el.exit();
                    return;
                }
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let dx = (position.x - state.mouse_pos.x) as f32;
                let dy = (position.y - state.mouse_pos.y) as f32;
                state.mouse_pos = position;
                if !consumed {
                    if state.rmb_down { state.camera.orbit(dx, dy); }
                    if state.mmb_down { state.camera.pan(dx, dy); }
                }
            }
            WindowEvent::MouseInput { state: bstate, button, .. } => {
                if !consumed {
                    let pressed = bstate == ElementState::Pressed;
                    match button {
                        MouseButton::Right  => state.rmb_down = pressed,
                        MouseButton::Middle => state.mmb_down = pressed,
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !consumed {
                    let s = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p)   => (p.y as f32) / 40.0,
                    };
                    let factor = if s > 0.0 { 0.9 } else { 1.1 };
                    state.camera.zoom(factor);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// State construction.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // retained for the optional WebGPU path; web build uses WebGL2
pub(crate) const WEBGPU_UNAVAILABLE_MSG: &str =
    "⚠️ WebGPU is not available in this view.<br><br>\
     Open this page in a normal <b>Chrome / Edge 113+</b> tab \
     (not the editor's embedded preview / Simple Browser), \
     or view the lightweight \
     <a href=\"./impression/\" style=\"color:#7fd1ff\">artist's impression</a>.";

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // retained for the optional WebGPU path; web build uses WebGL2
pub(crate) fn set_boot_status(html: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("boot-status") {
            el.set_inner_html(html);
        }
    }
}

/// Returns `true` only when the page exposes a usable `navigator.gpu`.
/// Embedded webviews (VS Code Simple Browser, Live Preview, etc.) do not, so
/// we can bail out gracefully before ever touching wgpu.
#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // retained for the optional WebGPU path; web build uses WebGL2
pub(crate) fn webgpu_available() -> bool {
    use wasm_bindgen::JsValue;
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(w.navigator().as_ref(), &JsValue::from_str("gpu")).ok())
        .map(|gpu| !gpu.is_undefined() && !gpu.is_null())
        .unwrap_or(false)
}

impl State {
    /// Build the GPU state. Returns `None` (after posting a friendly notice to
    /// the page) when no WebGPU adapter is available — this happens in embedded
    /// webviews that expose `navigator.gpu` but cannot back it with a real GPU.
    async fn new(window: Arc<Window>) -> Option<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");

        let adapter = match instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await {
            Some(a) => a,
            None => {
                #[cfg(target_arch = "wasm32")]
                {
                    log::warn!("no WebGPU adapter — staying on the fallback notice");
                    set_boot_status(WEBGPU_UNAVAILABLE_MSG);
                    return None;
                }
                #[cfg(not(target_arch = "wasm32"))]
                panic!("no suitable GPU adapter — WebGPU unavailable");
            }
        };

        log::info!(
            "GPU adapter: {} ({:?}, backend {:?})",
            adapter.get_info().name,
            adapter.get_info().device_type,
            adapter.get_info().backend,
        );

        // Request raised limits matching this RTX-class adapter.
        let adapter_limits = adapter.limits();
        let limits = wgpu::Limits {
            max_buffer_size:                  adapter_limits.max_buffer_size,
            max_storage_buffer_binding_size:  adapter_limits.max_storage_buffer_binding_size,
            max_compute_invocations_per_workgroup: adapter_limits.max_compute_invocations_per_workgroup,
            max_compute_workgroup_size_x:     adapter_limits.max_compute_workgroup_size_x,
            max_compute_workgroups_per_dimension: adapter_limits.max_compute_workgroups_per_dimension,
            ..wgpu::Limits::default()
        };

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("hopf-device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ).await.expect("device");

        // Surface config
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage:        wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width:        size.width.max(1),
            height:       size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode:   caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_view = make_depth(&device, config.width, config.height);

        // ----------------------------------------------------------------
        // Uniforms / shared bindings
        // ----------------------------------------------------------------
        let params = Params {
            time: 0.0, scale: 1.5, step_len: 0.04, speed: 1.0,
            field_mode: 0, steps_per_line: 192, seed_count: 1024,
            dt: 1.0 / 60.0, bbox_radius: 4.0,
            preset: 0, flow_phase: 0.0, _pad0: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera = OrbitCamera::new(config.width as f32 / config.height as f32);
        let camera_uniform = camera.uniform();
        let camera_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&camera_uniform),
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

        // ----------------------------------------------------------------
        // Streamline compute pipeline
        // ----------------------------------------------------------------
        let streamline_src = concat_wgsl(include_str!("../shaders/common.wgsl"),
                                         include_str!("../shaders/streamlines.wgsl"));
        let streamline_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("streamlines.wgsl"),
            source: wgpu::ShaderSource::Wgsl(streamline_src.into()),
        });

        let streamline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("streamline-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {  // params
                    binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                },
                wgpu::BindGroupLayoutEntry {  // seeds
                    binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                },
                wgpu::BindGroupLayoutEntry {  // verts (RW)
                    binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                },
            ],
        });
        let streamline_pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("streamline-pll"),
            bind_group_layouts: &[&streamline_bgl],
            push_constant_ranges: &[],
        });
        let streamline_compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("streamline-compute"),
            layout: Some(&streamline_pll),
            module: &streamline_module,
            entry_point: "cs_trace",
            compilation_options: Default::default(),
            cache: None,
        });

        // ----------------------------------------------------------------
        // Particle compute pipeline
        // ----------------------------------------------------------------
        let particle_src = concat_wgsl(include_str!("../shaders/common.wgsl"),
                                       include_str!("../shaders/particles.wgsl"));
        let particle_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(particle_src.into()),
        });
        let particle_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                },
            ],
        });
        let particle_pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle-pll"),
            bind_group_layouts: &[&particle_bgl],
            push_constant_ranges: &[],
        });
        let particle_compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particle-compute"),
            layout: Some(&particle_pll),
            module: &particle_module,
            entry_point: "cs_advect",
            compilation_options: Default::default(),
            cache: None,
        });

        // ----------------------------------------------------------------
        // Render pipelines
        // ----------------------------------------------------------------
        let line_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render_line.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/render_line.wgsl").into()),
        });
        let point_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render_point.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/render_point.wgsl").into()),
        });

        let render_pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("render-pll"),
            bind_group_layouts: &[&camera_bgl],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0,  shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32,   offset: 12, shader_location: 1 },
            ],
        };
        let particle_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Particle>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0,  shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32,   offset: 12, shader_location: 1 },
            ],
        };

        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation:  wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        });

        let streamline_render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("streamline-render"),
            layout: Some(&render_pll),
            vertex: wgpu::VertexState {
                module: &line_module, entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[vertex_layout.clone()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &line_module, entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format, blend, write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let particle_render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particle-render"),
            layout: Some(&render_pll),
            vertex: wgpu::VertexState {
                module: &point_module, entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[particle_layout.clone()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &point_module, entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format, blend, write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // ----------------------------------------------------------------
        // Allocate buffers
        // ----------------------------------------------------------------
        let sim = SimSettings::default();
        let seed_count     = sim.seed_count_request;
        let steps_per_line = sim.steps_per_line_request;
        let particle_count = sim.particle_count_request;

        let seeds_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("seeds"),
            size: (seed_count as u64) * std::mem::size_of::<[f32; 4]>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Initialize seeds (CPU-side, sphere-sampled).
        let seed_data = generate_seeds(seed_count, sim.scale);
        queue.write_buffer(&seeds_buf, 0, bytemuck::cast_slice(&seed_data));

        let index_data = build_line_indices(seed_count, steps_per_line);
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("line-indices"),
            contents: bytemuck::cast_slice(&index_data),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_count = index_data.len() as u32;

        // Per-channel render bind groups carry camera + their color uniform.
        // ----------------------------------------------------------------
        // Build the 3 channels
        // ----------------------------------------------------------------
        let channel_e = make_channel(
            &device, &params_buf, &seeds_buf, &camera_buf, &streamline_bgl, &particle_bgl, &camera_bgl,
            seed_count, steps_per_line, particle_count,
            0, [0.20, 0.45, 1.00, 1.0],    // deep cobalt  — Electric
        );
        let channel_b = make_channel(
            &device, &params_buf, &seeds_buf, &camera_buf, &streamline_bgl, &particle_bgl, &camera_bgl,
            seed_count, steps_per_line, particle_count,
            1, [0.10, 1.00, 0.50, 1.0],    // radiant emerald — Magnetic
        );
        let channel_s = make_channel(
            &device, &params_buf, &seeds_buf, &camera_buf, &streamline_bgl, &particle_bgl, &camera_bgl,
            seed_count, steps_per_line, particle_count,
            2, [1.00, 0.70, 0.10, 1.0],    // amber — Poynting
        );

        // Initialize particle positions for each channel.
        let init_particles = generate_particles(particle_count, sim.scale);
        queue.write_buffer(&channel_e.particles, 0, bytemuck::cast_slice(&init_particles));
        queue.write_buffer(&channel_b.particles, 0, bytemuck::cast_slice(&init_particles));
        queue.write_buffer(&channel_s.particles, 0, bytemuck::cast_slice(&init_particles));

        // ----------------------------------------------------------------
        // FDTD subsystem.  Uses rayon to seed donut + mirror mask on the
        // CPU (real multi-core work) then runs leapfrog Maxwell entirely
        // on the GPU.  Particles advect along the live Poynting vector.
        // ----------------------------------------------------------------
        // The +x flight axis is 3× the square cross-section. Native uses a fat
        // grid; the browser (tighter GPU memory limits) uses a lighter one.
        #[cfg(not(target_arch = "wasm32"))]
        let (cross_n, fdtd_particles) = (160u32, 900_000u32);  // 480×160×160 ≈ 12.3M cells — feed the GPU
        #[cfg(target_arch = "wasm32")]
        let (cross_n, fdtd_particles) = (64u32, 220_000u32);   // 192×64×64
        let fdtd = crate::fdtd::FdtdState::new(
            &device, &queue,
            cross_n,
            /* world_extent  */ 2.5,   // half-side of the narrow cross-section
            fdtd_particles,
        );

        let fdtd_render_color = RenderParams {
            color: [0.55, 1.00, 0.75, 1.0],        // mint, biased emerald
            flow_phase: 0.0,
            steps_per_line: 256,
            _pad0: 0, _pad1: 0,
        };
        let fdtd_color_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fdtd-render-color"),
            contents: bytemuck::bytes_of(&fdtd_render_color),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fdtd_render_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fdtd-render-bg"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: fdtd_color_buf.as_entire_binding() },
            ],
        });

        let mirror_render_color = RenderParams {
            color: [1.00, 0.35, 0.45, 1.0],        // pink-red wireframe
            flow_phase: 0.0,
            steps_per_line: 1,
            _pad0: 0, _pad1: 0,
        };
        let fdtd_mirror_color_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fdtd-mirror-color"),
            contents: bytemuck::bytes_of(&mirror_render_color),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fdtd_mirror_render_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fdtd-mirror-render-bg"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: fdtd_mirror_color_buf.as_entire_binding() },
            ],
        });

        // egui
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
            &device, config.format, Some(wgpu::TextureFormat::Depth32Float), 1, false,
        );

        Some(Self {
            window, surface, device, queue, config, depth_view,
            sim,
            seed_count, steps_per_line, particle_count,
            params_buf, camera_buf, camera_bgl,
            streamline_compute, streamline_render, streamline_bgl,
            seeds_buf, index_buf, index_count,
            particle_compute, particle_render, particle_bgl,
            channel_e, channel_b, channel_s,
            fdtd,
            fdtd_color_buf, fdtd_mirror_color_buf,
            fdtd_render_bind, fdtd_mirror_render_bind,
            camera, mouse_pos: PhysicalPosition::new(0.0, 0.0),
            rmb_down: false, mmb_down: false,
            egui_ctx, egui_winit, egui_renderer,
            last_tick: Instant::now(),
        })
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = make_depth(&self.device, size.width, size.height);
        self.camera.aspect = size.width as f32 / size.height as f32;
    }

    fn update(&mut self) {
        // dt
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        self.sim.last_frame_ms = dt * 1000.0;

        // Fit camera to the scene when requested from the transport bar.
        if self.sim.fit_requested {
            self.camera.fit();
            self.sim.fit_requested = false;
        }

        // Analytic time advances while running, or for a single frame when
        // the user clicks Step. `sim_speed` is the master transport multiplier.
        if self.sim.playing || self.sim.step_once {
            self.sim.time += dt * self.sim.time_scale * self.sim.sim_speed;
        }

        // Auto-pulse: keep firing fresh FDTD donuts on a steady rhythm so the
        // scene is never static. The interval scales with sim_speed so the
        // master speed knob also speeds up the pulse train.
        if self.sim.auto_pulse
            && self.sim.render_mode == RenderMode::Fdtd
            && (self.sim.playing || self.sim.step_once)
        {
            self.sim.pulse_clock += dt * self.sim.sim_speed.max(0.0);
            let interval = self.sim.pulse_interval.max(0.1);
            if self.sim.pulse_clock >= interval {
                self.sim.pulse_clock = 0.0;
                self.sim.fdtd_reseed_requested = true;
            }
        }

        // Demo auto-cycle: switch preset every ~7 s so the viewer sees the
        // whole photon–donut–hopfion family on startup.  (Only meaningful
        // for the analytic render modes — FDTD has its own world.)
        if self.sim.demo_cycle && self.sim.render_mode != RenderMode::Fdtd {
            self.sim.demo_clock += dt;
            if self.sim.demo_clock >= 7.0 {
                self.sim.demo_clock = 0.0;
                let cur = self.sim.preset;
                let order = [
                    Preset::Hopfion, Preset::PhotonHopfion, Preset::Donut,
                    Preset::CpPhoton, Preset::Trefoil, Preset::PlanePhoton, Preset::StaCrunch,
                ];
                let i = order.iter().position(|p| *p == cur).unwrap_or(0);
                let next = order[(i + 1) % order.len()];
                self.sim.preset = next;
                self.sim.preset_dirty = true;
            }
        }

        // Apply preset defaults when preset_dirty is set (manual or auto).
        if self.sim.preset_dirty {
            let d = self.sim.preset.defaults();
            self.sim.scale          = d.scale;
            self.sim.step_len       = d.step_len;
            self.sim.particle_speed = d.particle_speed;
            self.sim.time_scale     = d.time_scale;
            if self.sim.seed_count_request != d.seeds
                || self.sim.steps_per_line_request != d.steps {
                self.sim.seed_count_request = d.seeds;
                self.sim.steps_per_line_request = d.steps;
                self.sim.topology_dirty = true;
            }
            if self.sim.particle_count_request != d.particles {
                self.sim.particle_count_request = d.particles;
                self.sim.particles_dirty = true;
            }
            self.sim.preset_dirty = false;
        }

        // Advance flow phase regardless of playing state so the wireframe
        // *always* visibly streams.
        let two_pi = std::f32::consts::TAU;
        let mut fp = self.sim.last_flow_phase + dt * 1.4;
        if fp > two_pi * 1024.0 { fp -= two_pi * 1024.0; }
        self.sim.last_flow_phase = fp;

        // Rebuild buffers / indices if topology changed.
        if self.sim.topology_dirty {
            self.rebuild_streamline_buffers();
            self.sim.topology_dirty = false;
        }
        if self.sim.particles_dirty {
            self.rebuild_particle_buffers();
            self.sim.particles_dirty = false;
        }

        // Refresh camera uniform.
        let cam_u = self.camera.uniform();
        self.queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&cam_u));

        // Refresh each channel's render uniform (color + flow phase + N).
        for ch in [&self.channel_e, &self.channel_b, &self.channel_s] {
            let rp = RenderParams {
                color: ch.color,
                flow_phase: self.sim.last_flow_phase,
                steps_per_line: self.steps_per_line,
                _pad0: 0, _pad1: 0,
            };
            self.queue.write_buffer(&ch.color_buf, 0, bytemuck::bytes_of(&rp));
        }
    }

    fn write_params(&self, field_mode_id: u32) {
        let p = Params {
            time:           self.sim.time,
            scale:          self.sim.scale,
            step_len:       self.sim.step_len,
            speed:          self.sim.particle_speed,
            field_mode:     field_mode_id,
            steps_per_line: self.steps_per_line,
            seed_count:     self.seed_count,
            dt:             (self.sim.last_frame_ms / 1000.0).max(1.0 / 240.0).min(1.0 / 30.0),
            bbox_radius:    4.0,
            preset:         self.sim.preset.shader_id(),
            flow_phase:     self.sim.last_flow_phase,
            _pad0: 0,
        };
        self.queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&p));
    }

    fn active_channels(&self) -> Vec<&Channel> {
        match self.sim.field_mode {
            FieldMode::E        => vec![&self.channel_e],
            FieldMode::B        => vec![&self.channel_b],
            FieldMode::Both     => vec![&self.channel_e, &self.channel_b],
            FieldMode::Poynting => vec![&self.channel_s],
        }
    }

    fn render(&mut self) {
        // ----- egui frame -----
        let raw_input = self.egui_winit.take_egui_input(&self.window);
        let mut sim = std::mem::take(&mut self.sim);
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            ui::draw_ui(ctx, &mut sim);
        });
        self.sim = sim;
        self.egui_winit.handle_platform_output(&self.window, full_output.platform_output);

        // Handle "save preset JSON" requests from the UI.
        if self.sim.export_requested {
            self.sim.export_requested = false;
            match self.export_preset_json() {
                Ok(path) => {
                    log::info!("Preset JSON written to {}", path);
                    self.sim.last_export_path = Some(path);
                }
                Err(e) => log::error!("Preset export failed: {e}"),
            }
        }

        let pixels_per_point = self.egui_ctx.pixels_per_point();
        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Active channel IDs and refs.
        let active_ids: Vec<u32> = match self.sim.field_mode {
            FieldMode::E        => vec![0],
            FieldMode::B        => vec![1],
            FieldMode::Both     => vec![0, 1],
            FieldMode::Poynting => vec![2],
        };

        // ----- FDTD compute step (only when this mode is active) -----
        if self.sim.render_mode == RenderMode::Fdtd {
            // Position the mirror wall from the gap slider (live wireframe move;
            // the reflecting mask refreshes on the next reseed/pulse).
            let wex = self.fdtd.world_ext_x;
            self.fdtd.set_mirror_gap(&self.device, self.sim.fdtd_mirror_gap.clamp(0.0, 1.4) * wex);

            if self.sim.fdtd_reseed_requested {
                self.sim.fdtd_reseed_requested = false;
                self.fdtd.mirror_enabled = self.sim.fdtd_mirror_enabled;
                // Apply live seed-shape sliders before regenerating the field.
                let we = self.fdtd.world_extent;
                self.fdtd.seed_radius_world = self.sim.fdtd_seed_radius_frac.clamp(0.02, 0.6) * we;
                self.fdtd.seed_width_world  = self.sim.fdtd_seed_width_frac.clamp(0.02, 0.4)  * we;
                self.fdtd.seed_amp          = self.sim.fdtd_seed_amp.max(0.0);
                self.fdtd.reseed(&self.device, &self.queue);
                log::info!("FDTD reseeded: grid={}x{}x{}, mirror={}, R={:.2}, W={:.2}, A={:.2}",
                    self.fdtd.nx, self.fdtd.ny, self.fdtd.nz, self.fdtd.mirror_enabled,
                    self.fdtd.seed_radius_world, self.fdtd.seed_width_world, self.fdtd.seed_amp);
            }
            self.fdtd.mirror_enabled = self.sim.fdtd_mirror_enabled;
            self.fdtd.write_params(&self.queue,
                self.sim.fdtd_dt, self.sim.fdtd_sigma,
                self.sim.fdtd_pml_strength, /* pml_layers */ 8);
            let we = self.fdtd.world_extent;
            self.fdtd.write_adv_params(&self.queue,
                self.sim.fdtd_step,
                self.sim.fdtd_drift_scale,
                4, /* particle substeps per advect call */
                self.sim.fdtd_max_age,
                self.sim.fdtd_density_gate,
                self.sim.fdtd_respawn_scale,
                self.sim.fdtd_respawn_x * we,
            );

            // Live brightness / color: re-write the existing render uniform.
            let rp = RenderParams {
                color: [
                    self.sim.fdtd_color[0],
                    self.sim.fdtd_color[1],
                    self.sim.fdtd_color[2],
                    self.sim.fdtd_brightness.max(0.0),
                ],
                flow_phase: 0.0,
                steps_per_line: 256,
                _pad0: 0, _pad1: 0,
            };
            self.queue.write_buffer(&self.fdtd_color_buf, 0, bytemuck::bytes_of(&rp));

            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fdtd-cmd"),
            });
            // Transport gate: advance the Maxwell field only while running, or
            // for a single frame on Step. `sim_speed` scales the substeps/frame
            // (the effective FDTD playback rate); Step always does one full frame.
            let run = self.sim.playing || self.sim.step_once;
            if run {
                let n = if self.sim.step_once {
                    self.sim.fdtd_substeps
                } else {
                    ((self.sim.fdtd_substeps as f32) * self.sim.sim_speed)
                        .round()
                        .clamp(1.0, 256.0) as u32
                };
                self.fdtd.step(&mut enc, n);
                self.fdtd.dispatch_advect(&mut enc);
            }
            self.queue.submit(std::iter::once(enc.finish()));
            self.sim.fdtd_total_steps = self.fdtd.time_step;
        }

        // Consume a single-step request once per rendered frame.
        self.sim.step_once = false;

        // ----- compute dispatches (each submitted separately so the
        //       queue.write_buffer for params is serialized correctly) -----
        if self.sim.render_mode != RenderMode::Fdtd {
            let active: Vec<&Channel> = self.active_channels();
            for (fid, ch) in active_ids.iter().zip(active.iter()) {
                self.write_params(*fid);

                let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compute-cmd"),
                });
                {
                    let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("compute"),
                        timestamp_writes: None,
                    });
                    match self.sim.render_mode {
                        RenderMode::Lines => {
                            cp.set_pipeline(&self.streamline_compute);
                            cp.set_bind_group(0, &ch.streamline_bind, &[]);
                            let groups = (self.seed_count + 63) / 64;
                            cp.dispatch_workgroups(groups, 1, 1);
                        }
                        RenderMode::Particles => {
                            cp.set_pipeline(&self.particle_compute);
                            cp.set_bind_group(0, &ch.particle_bind, &[]);
                            let groups = (self.particle_count + 63) / 64;
                            cp.dispatch_workgroups(groups, 1, 1);
                        }
                        RenderMode::Fdtd => { /* handled above */ }
                    }
                }
                self.queue.submit(std::iter::once(encoder.finish()));
            }
        }

        // ----- render -----
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render-cmd"),
        });

        // egui texture upload + buffer prep (needs its own command encoder).
        for (id, image) in &full_output.textures_delta.set {
            self.egui_renderer.update_texture(&self.device, &self.queue, *id, image);
        }
        self.egui_renderer.update_buffers(
            &self.device, &self.queue, &mut encoder, &paint_jobs, &screen,
        );

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.012, g: 0.012, b: 0.022, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if self.sim.render_mode == RenderMode::Fdtd {
                // Draw the live FDTD particle field.
                rp.set_pipeline(&self.particle_render);
                rp.set_bind_group(0, &self.fdtd_render_bind, &[]);
                rp.set_vertex_buffer(0, self.fdtd.particle_buffer().slice(..));
                rp.draw(0..self.fdtd.particle_count, 0..1);

                // Mirror wireframe (using the line render pipeline).
                if self.sim.fdtd_show_mirror {
                    rp.set_pipeline(&self.streamline_render);
                    rp.set_bind_group(0, &self.fdtd_mirror_render_bind, &[]);
                    rp.set_vertex_buffer(0, self.fdtd.mirror_verts.slice(..));
                    rp.set_index_buffer(self.fdtd.mirror_index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..self.fdtd.mirror_index_count, 0, 0..1);
                }
            } else {
                for ch in self.active_channels() {
                    match self.sim.render_mode {
                        RenderMode::Lines => {
                            rp.set_pipeline(&self.streamline_render);
                            rp.set_bind_group(0, &ch.line_render_bind, &[]);
                            rp.set_vertex_buffer(0, ch.verts.slice(..));
                            rp.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                            rp.draw_indexed(0..self.index_count, 0, 0..1);
                        }
                        RenderMode::Particles => {
                            rp.set_pipeline(&self.particle_render);
                            rp.set_bind_group(0, &ch.particle_render_bind, &[]);
                            rp.set_vertex_buffer(0, ch.particles.slice(..));
                            rp.draw(0..self.particle_count, 0..1);
                        }
                        RenderMode::Fdtd => { /* handled above */ }
                    }
                }
            }

            self.egui_renderer.render(&mut rp.forget_lifetime(), &paint_jobs, &screen);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        frame.present();
    }

    fn rebuild_streamline_buffers(&mut self) {
        self.seed_count     = self.sim.seed_count_request;
        self.steps_per_line = self.sim.steps_per_line_request;

        // Seeds
        let seeds_size = (self.seed_count as u64) * std::mem::size_of::<[f32; 4]>() as u64;
        self.seeds_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("seeds"),
            size: seeds_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let seed_data = generate_seeds(self.seed_count, self.sim.scale);
        self.queue.write_buffer(&self.seeds_buf, 0, bytemuck::cast_slice(&seed_data));

        // Verts (per-channel)
        let verts_size = (self.seed_count as u64) * (self.steps_per_line as u64)
            * std::mem::size_of::<Vertex>() as u64;
        for ch in [&mut self.channel_e, &mut self.channel_b, &mut self.channel_s] {
            ch.verts = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("streamline-verts"),
                size: verts_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ch.streamline_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("streamline-bg"),
                layout: &self.streamline_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.seeds_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: ch.verts.as_entire_binding() },
                ],
            });
        }

        // Indices
        let idx = build_line_indices(self.seed_count, self.steps_per_line);
        self.index_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("line-indices"),
            contents: bytemuck::cast_slice(&idx),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });
        self.index_count = idx.len() as u32;
    }

    fn rebuild_particle_buffers(&mut self) {
        self.particle_count = self.sim.particle_count_request;
        let size = (self.particle_count as u64) * std::mem::size_of::<Particle>() as u64;
        let init = generate_particles(self.particle_count, self.sim.scale);

        for ch in [&mut self.channel_e, &mut self.channel_b, &mut self.channel_s] {
            ch.particles = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particles"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&ch.particles, 0, bytemuck::cast_slice(&init));
            ch.particle_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particle-bg"),
                layout: &self.particle_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: ch.particles.as_entire_binding() },
                ],
            });
        }
    }

    /// Serialize current params + UI settings to a JSON file in CWD.
    /// Returns the absolute path of the file that was written.
    fn export_preset_json(&self) -> std::io::Result<String> {
        let s = &self.sim;
        let field = match s.field_mode {
            FieldMode::E => "E", FieldMode::B => "B",
            FieldMode::Both => "Both", FieldMode::Poynting => "Poynting",
        };
        let rmode = match s.render_mode {
            RenderMode::Lines => "Lines",
            RenderMode::Particles => "Particles",
            RenderMode::Fdtd => "Fdtd",
        };
        let json = format!(
"{{
  \"preset\":              \"{preset}\",
  \"shader_id\":           {shader_id},
  \"time\":                {time},
  \"time_scale\":          {time_scale},
  \"scale\":               {scale},
  \"field_mode\":          \"{field}\",
  \"render_mode\":         \"{rmode}\",
  \"seed_count\":          {seeds},
  \"steps_per_line\":      {steps},
  \"step_len\":            {step_len},
  \"particle_count\":      {parts},
  \"particle_speed\":      {pspeed},
  \"demo_cycle\":          {demo},
  \"colors\": {{
    \"E\": [{er}, {eg}, {eb_}],
    \"B\": [{br}, {bg}, {bb}],
    \"S\": [{sr}, {sg}, {sb}]
  }}
}}\n",
            preset    = format!("{:?}", s.preset),
            shader_id = s.preset.shader_id(),
            time      = s.time,
            time_scale= s.time_scale,
            scale     = s.scale,
            seeds     = s.seed_count_request,
            steps     = s.steps_per_line_request,
            step_len  = s.step_len,
            parts     = s.particle_count_request,
            pspeed    = s.particle_speed,
            demo      = s.demo_cycle,
            er = self.channel_e.color[0], eg = self.channel_e.color[1], eb_ = self.channel_e.color[2],
            br = self.channel_b.color[0], bg = self.channel_b.color[1], bb  = self.channel_b.color[2],
            sr = self.channel_s.color[0], sg = self.channel_s.color[1], sb  = self.channel_s.color[2],
        );

        let ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let fname = format!("preset-{:?}-{}.json", s.preset, ts);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = std::env::current_dir()?.join(&fname);
            std::fs::write(&path, json)?;
            Ok(path.to_string_lossy().into_owned())
        }
        #[cfg(target_arch = "wasm32")]
        {
            log::info!("preset export (web, not written to disk):\n{json}");
            Ok(fname)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
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
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn concat_wgsl(a: &str, b: &str) -> String {
    let mut s = String::with_capacity(a.len() + b.len() + 1);
    s.push_str(a);
    s.push('\n');
    s.push_str(b);
    s
}

fn build_line_indices(seed_count: u32, steps_per_line: u32) -> Vec<u32> {
    let segments_per_line = steps_per_line.saturating_sub(1);
    let total = (seed_count as usize) * (segments_per_line as usize) * 2;
    let mut out = Vec::with_capacity(total);
    for line in 0..seed_count {
        let base = line * steps_per_line;
        for s in 0..segments_per_line {
            out.push(base + s);
            out.push(base + s + 1);
        }
    }
    out
}

fn generate_seeds(n: u32, scale: f32) -> Vec<[f32; 4]> {
    // Uniform sampling inside the unit ball, then scale.  Hopf field lines
    // close on themselves so any seed inside the structure yields a useful
    // loop.
    let mut out = Vec::with_capacity(n as usize);
    let mut state: u32 = 0xC0DEC0DE;
    for _ in 0..n {
        loop {
            let x = lcg(&mut state) * 2.0 - 1.0;
            let y = lcg(&mut state) * 2.0 - 1.0;
            let z = lcg(&mut state) * 2.0 - 1.0;
            if x*x + y*y + z*z <= 1.0 {
                out.push([x * scale * 1.5, y * scale * 1.5, z * scale * 1.5, 0.0]);
                break;
            }
        }
    }
    out
}

fn generate_particles(n: u32, scale: f32) -> Vec<Particle> {
    let mut out = Vec::with_capacity(n as usize);
    let mut state: u32 = 0xBADD1E;
    for i in 0..n {
        let x = (lcg(&mut state) - 0.5) * 4.0 * scale;
        let y = (lcg(&mut state) - 0.5) * 4.0 * scale;
        let z = (lcg(&mut state) - 0.5) * 4.0 * scale;
        out.push(Particle { pos: [x, y, z], age: (i as f32 * 0.0137) % 1.0 * 4.0 });
    }
    out
}

fn lcg(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 8) & 0x00FFFFFF) as f32 / 16_777_216.0
}

fn make_channel(
    device:           &wgpu::Device,
    params_buf:       &wgpu::Buffer,
    seeds_buf:        &wgpu::Buffer,
    camera_buf:       &wgpu::Buffer,
    streamline_bgl:   &wgpu::BindGroupLayout,
    particle_bgl:     &wgpu::BindGroupLayout,
    camera_bgl:       &wgpu::BindGroupLayout,
    seed_count:       u32,
    steps_per_line:   u32,
    particle_count:   u32,
    field_mode_id:    u32,
    color:            [f32; 4],
) -> Channel {
    // streamline verts
    let verts_size = (seed_count as u64) * (steps_per_line as u64)
        * std::mem::size_of::<Vertex>() as u64;
    let verts = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamline-verts"),
        size: verts_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let streamline_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("streamline-bg"),
        layout: streamline_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: seeds_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: verts.as_entire_binding() },
        ],
    });

    // particles
    let particle_size = (particle_count as u64) * std::mem::size_of::<Particle>() as u64;
    let particles = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles"),
        size: particle_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let particle_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("particle-bg"),
        layout: particle_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: particles.as_entire_binding() },
        ],
    });

    // render color uniform
    let rp = RenderParams {
        color,
        flow_phase: 0.0,
        steps_per_line,
        _pad0: 0, _pad1: 0,
    };
    let color_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("render-params"),
        contents: bytemuck::bytes_of(&rp),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    let line_render_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("line-render-bg"),
        layout: camera_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: color_buf.as_entire_binding() },
        ],
    });
    let particle_render_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("particle-render-bg"),
        layout: camera_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: color_buf.as_entire_binding() },
        ],
    });

    Channel {
        field_mode_id, color,
        verts, streamline_bind,
        particles, particle_bind,
        color_buf,
        line_render_bind, particle_render_bind,
    }
}


