use std::error::Error;

mod app;
mod camera;
mod fdtd;
mod ui;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("hopf_sta_viz=info,wgpu_core=warn,wgpu_hal=warn"),
    )
    .init();

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = app::App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}
