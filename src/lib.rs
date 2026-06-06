// src/lib.rs
//
// Crate root shared by the native binary and the WebAssembly build.
//
//   * Native  → `run_native()` (called from `main.rs`) sets up env_logger and
//     blocks on the winit event loop.
//   * Browser → `start()` is the `#[wasm_bindgen(start)]` entry; it installs a
//     panic hook + console logger and spawns the same event loop on the page.
//
// Either way the heavy lifting (Spacetime-Algebra FDTD on the GPU) is identical;
// only the device request (blocking vs async) and the CPU seeding (rayon vs
// serial) differ by target — see `app.rs` and `fdtd.rs`.

mod app;
mod camera;
mod fdtd;
mod ui;

use app::{App, UserEvent};
use winit::event_loop::{ControlFlow, EventLoop};

/// Build the winit event loop and drive the application. Shared by both targets.
pub fn run() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let app = App::new(&event_loop);

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut app = app;
        event_loop.run_app(&mut app).expect("event loop error");
    }

    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(app);
    }
}

/// Native entry point (invoked from `main.rs`).
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("hopf_sta_viz=info,wgpu_core=warn,wgpu_hal=warn"),
    )
    .init();
    run();
    Ok(())
}

/// Browser entry point. Called automatically when the wasm module is loaded.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("hopf-sta-viz: starting WebGPU build");

    // Embedded webviews (VS Code Simple Browser / Live Preview, etc.) do not
    // expose `navigator.gpu`. Detect that up front and show a friendly message
    // instead of spinning up wgpu just to panic on the missing adapter.
    if !app::webgpu_available() {
        log::warn!("navigator.gpu is unavailable — not starting the GPU solver");
        app::set_boot_status(app::WEBGPU_UNAVAILABLE_MSG);
        return;
    }

    run();
}
