use crate::app::{FieldMode, HelpLog, Preset, Remark, RenderMode, SimSettings};

// ---------------------------------------------------------------------------
// F1 context-help + user-remark logging.
//
// Every interactive control is registered with a stable `object_id` and a
// short doc string via `reg(..)`. Hovering shows the doc as a tooltip;
// pressing F1 while hovering opens a remark popup pinned to that control.
// Remarks (note + object-ID + live value + context) are appended to
// `ui-remarks.json` so they can be inspected to see "what is wrong".
// ---------------------------------------------------------------------------

/// Register a control for F1 help. Call AFTER the widget has been added so its
/// `Response` is available. Shows a hover tooltip and arms F1 to open the
/// remark popup for this `id`.
fn reg(
    ui: &egui::Ui,
    help: &mut HelpLog,
    id: &str,
    doc: &str,
    value: String,
    resp: egui::Response,
) -> egui::Response {
    if resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::F1)) {
        help.open_id    = Some(id.to_string());
        help.open_help  = doc.to_string();
        help.open_value = value;
        help.draft.clear();
    }
    resp.on_hover_text(format!("{doc}\n\n[F1] log a remark  ·  id = {id}"))
}

fn unix_ms() -> u128 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"'  => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Serialize the remark log to `ui-remarks.json` (MCP-style JSON) in the CWD.
fn write_remarks(s: &SimSettings) -> std::io::Result<String> {
    let mut items = String::new();
    for (i, r) in s.help.remarks.iter().enumerate() {
        if i > 0 { items.push_str(",\n"); }
        items.push_str(&format!(
            "    {{ \"object_id\": \"{oid}\", \"value\": \"{val}\", \"note\": \"{note}\", \
\"unix_ms\": {ms}, \"preset\": \"{preset}\", \"render_mode\": \"{rmode}\", \"field_mode\": \"{fmode}\" }}",
            oid    = json_escape(&r.object_id),
            val    = json_escape(&r.value),
            note   = json_escape(&r.note),
            ms     = r.unix_ms,
            preset = json_escape(&r.preset),
            rmode  = json_escape(&r.render_mode),
            fmode  = json_escape(&r.field_mode),
        ));
    }
    let json = format!(
"{{
  \"schema\": \"hopf-sta-viz/ui-remarks@1\",
  \"app\": \"hopf-sta-viz\",
  \"generated_unix_ms\": {now},
  \"count\": {count},
  \"remarks\": [
{items}
  ]
}}
",
        now   = unix_ms(),
        count = s.help.remarks.len(),
        items = items,
    );
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::env::current_dir()?.join("ui-remarks.json");
        std::fs::write(&path, json)?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        log::info!("ui-remarks (web, not written to disk):\n{json}");
        Ok("(web: logged to console)".to_string())
    }
}

pub fn draw_ui(ctx: &egui::Context, s: &mut SimSettings) {
    egui::Window::new("Hopfion · STA · GPU")
        .default_pos([12.0, 12.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Rañada electromagnetic hopfion")
                .strong());
            ui.label("Faraday bivector  F = E + I·c·B   on the GPU (WGSL).");
            ui.label(egui::RichText::new(
                "Hover any control for help · press F1 to log a remark → ui-remarks.json")
                .small().italics().color(egui::Color32::from_rgb(150, 180, 220)));
            ui.separator();

            // ================================================================
            // SIMULATION TRANSPORT  (the "tape deck" — always at the top)
            // ================================================================
            ui.label(egui::RichText::new("Simulation").strong());
            ui.horizontal(|ui| {
                // Play / Pause toggle.
                let play_label = if s.playing { "⏸ Pause" } else { "▶ Play" };
                let r = ui.add(egui::Button::new(
                    egui::RichText::new(play_label).size(16.0).strong()));
                if r.clicked() { s.playing = !s.playing; }
                reg(ui, &mut s.help, "sim.play",
                    "Run or pause the simulation. In FDTD mode this advances the live Maxwell field; in the analytic modes it advances the field time t. Paused = everything freezes in place.",
                    format!("{}", s.playing), r);

                // Step one frame (only meaningful while paused).
                let r = ui.add_enabled(!s.playing,
                    egui::Button::new(egui::RichText::new("⏭ Step").size(16.0)));
                if r.clicked() { s.step_once = true; }
                reg(ui, &mut s.help, "sim.step",
                    "Advance the simulation by exactly one frame and stay paused. Enabled only while paused — use it to inspect the field step by step.",
                    format!("{}", s.step_once), r);

                // Reseed — the single most important knob, promoted up here.
                let r = ui.add(egui::Button::new(
                    egui::RichText::new("⟳ Pulse now").size(16.0).strong())
                    .fill(egui::Color32::from_rgb(60, 90, 140)));
                if r.clicked() {
                    s.render_mode = RenderMode::Fdtd;
                    s.fdtd_reseed_requested = true;
                    s.playing = true;
                    s.pulse_clock = 0.0;
                }
                reg(ui, &mut s.help, "sim.reseed",
                    "Fire one fresh flying-donut pulse into the FDTD grid right now (switches to FDTD + resumes play). For non-stop motion, leave Auto-pulse on below.",
                    format!("{:?}", s.render_mode), r);

                // Fit camera.
                let r = ui.add(egui::Button::new(
                    egui::RichText::new("⊡ Fit").size(16.0)));
                if r.clicked() { s.fit_requested = true; }
                reg(ui, &mut s.help, "sim.fit",
                    "Reframe the camera to a default 3/4 view centred on the scene — use it whenever you get lost.",
                    format!("{}", s.fit_requested), r);

                // Quit.
                let r = ui.add(egui::Button::new(
                    egui::RichText::new("⏹ Quit").size(16.0))
                    .fill(egui::Color32::from_rgb(120, 50, 50)));
                if r.clicked() { s.quit_requested = true; }
                reg(ui, &mut s.help, "sim.quit",
                    "Close the application immediately.",
                    format!("{}", s.quit_requested), r);
            });

            // Master sim-speed slider.
            let r = ui.add(egui::Slider::new(&mut s.sim_speed, 0.0..=4.0)
                .text("sim speed ×"));
            reg(ui, &mut s.help, "sim.speed",
                "Master simulation speed. Scales how fast the field evolves each frame — FDTD substeps/frame in live mode, analytic time-rate otherwise. 1.0 = normal, 0 = frozen.",
                format!("{:.2}", s.sim_speed), r);

            // Auto-pulse: keep firing donuts so the scene is never static.
            ui.horizontal(|ui| {
                let r = ui.toggle_value(&mut s.auto_pulse, "🔁 Auto-pulse");
                reg(ui, &mut s.help, "sim.auto_pulse",
                    "Continuously launch a fresh FDTD donut on a steady rhythm so there is always something happening. Turn off to fire single pulses manually.",
                    format!("{}", s.auto_pulse), r);
                let r = ui.add_enabled(s.auto_pulse,
                    egui::Slider::new(&mut s.pulse_interval, 0.2..=6.0)
                        .text("pulse every (s)"));
                reg(ui, &mut s.help, "sim.pulse_interval",
                    "Seconds between automatic pulses. Lower = a denser, faster pulse train. Scales with sim speed.",
                    format!("{:.2}", s.pulse_interval), r);
            });

            let (status, col) = if s.playing {
                (format!("● running · {:.2}× speed", s.sim_speed),
                 egui::Color32::from_rgb(120, 220, 140))
            } else {
                ("⏸ paused — press Step to advance one frame".to_string(),
                 egui::Color32::from_rgb(220, 180, 120))
            };
            ui.label(egui::RichText::new(status).small().color(col));
            ui.separator();

            // ---- DEMO + preset selector ------------------------------------
            ui.horizontal(|ui| {
                let demo_label = if s.demo_cycle { "■ DEMO (auto)" } else { "▶ DEMO" };
                let r = ui.add(egui::Button::new(
                    egui::RichText::new(demo_label).size(16.0).strong()));
                if r.clicked() {
                    s.demo_cycle = !s.demo_cycle;
                    s.demo_clock = 0.0;
                }
                reg(ui, &mut s.help, "demo.toggle",
                    "Toggle auto-cycling through every STA preset (~7 s each). Turn off for manual control.",
                    format!("{}", s.demo_cycle), r);
                ui.label(if s.demo_cycle {
                    "cycling presets every ~7 s"
                } else {
                    "manual preset"
                });
            });

            let mut chosen = s.preset;
            let combo = egui::ComboBox::from_label("STA preset")
                .selected_text(chosen.label())
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        ui.selectable_value(&mut chosen, *p, p.label());
                    }
                });
            reg(ui, &mut s.help, "preset.select",
                "Pick which projection of the 4-D Bateman seed to render (hopfion, photon, donut, …).",
                format!("{:?}", s.preset), combo.response);
            if chosen != s.preset {
                s.preset = chosen;
                s.preset_dirty = true;
                s.demo_cycle = false;
            }

            // Export current state as a JSON preset.
            ui.horizontal(|ui| {
                let r = ui.button("💾 Save preset → JSON");
                if r.clicked() {
                    s.export_requested = true;
                }
                reg(ui, &mut s.help, "preset.export",
                    "Write the current parameters to preset-<name>-<ts>.json in the working directory.",
                    format!("{:?}", s.preset), r);
                if let Some(p) = &s.last_export_path {
                    ui.label(egui::RichText::new(format!("→ {}", p))
                        .small().color(egui::Color32::from_rgb(120, 220, 140)));
                }
            });
            ui.separator();

            ui.horizontal(|ui| {
                let r = ui.add(egui::Slider::new(&mut s.time_scale, 0.0..=4.0).text("time-scale (analytic)"));
                reg(ui, &mut s.help, "sim.time_scale",
                    "Base time-rate for the analytic (non-FDTD) modes, before the master 'sim speed' multiplier. Per-preset feel knob.",
                    format!("{:.3}", s.time_scale), r);
            });
            let r = ui.add(egui::Slider::new(&mut s.time, -10.0..=10.0).text("time t"));
            reg(ui, &mut s.help, "sim.time",
                "Absolute analytic time of the field. Drag to scrub the wavepacket forward/backward (pause first for fine control).",
                format!("{:.3}", s.time), r);

            ui.separator();
            ui.label("Field / topology");
            let r = ui.add(egui::Slider::new(&mut s.scale, 0.4..=4.0).text("characteristic radius R"));
            reg(ui, &mut s.help, "field.radius",
                "Characteristic radius R of the hopfion core. Smaller = tighter, more localized knot.",
                format!("{:.3}", s.scale), r);

            ui.horizontal(|ui| {
                ui.label("field:");
                let r = ui.radio_value(&mut s.field_mode, FieldMode::E, "E");
                reg(ui, &mut s.help, "field.mode.E",
                    "Show the electric Hopf field E (cyan).",
                    format!("{:?}", s.field_mode), r);
                let r = ui.radio_value(&mut s.field_mode, FieldMode::B, "B");
                reg(ui, &mut s.help, "field.mode.B",
                    "Show the magnetic Hopf field B (magenta).",
                    format!("{:?}", s.field_mode), r);
                let r = ui.radio_value(&mut s.field_mode, FieldMode::Both, "E & B");
                reg(ui, &mut s.help, "field.mode.Both",
                    "Overlay both E and B fields to see the mutual linking.",
                    format!("{:?}", s.field_mode), r);
                let r = ui.radio_value(&mut s.field_mode, FieldMode::Poynting, "Poynting S");
                reg(ui, &mut s.help, "field.mode.Poynting",
                    "Show the Poynting vector S = E × B (amber) — the energy-flow direction.",
                    format!("{:?}", s.field_mode), r);
            });

            ui.separator();
            ui.label("Render");
            ui.horizontal(|ui| {
                ui.label("mode:");
                let r = ui.radio_value(&mut s.render_mode, RenderMode::Lines, "wireframe streamlines");
                reg(ui, &mut s.help, "render.mode.lines",
                    "Trace field lines as static wireframe streamlines.",
                    format!("{:?}", s.render_mode), r);
                let r = ui.radio_value(&mut s.render_mode, RenderMode::Particles, "flowing particles");
                reg(ui, &mut s.help, "render.mode.particles",
                    "Advect particles along the analytic field for a flowing look.",
                    format!("{:?}", s.render_mode), r);
                let r = ui.radio_value(&mut s.render_mode, RenderMode::Fdtd, "FDTD (live Maxwell)");
                reg(ui, &mut s.help, "render.mode.fdtd",
                    "Run a real Yee-leapfrog Maxwell solver on a 384×128×128 GPU grid (3× longer along the +x flight axis) and advect particles in the live field.",
                    format!("{:?}", s.render_mode), r);
            });

            match s.render_mode {
                RenderMode::Lines => {
                    let mut sc = s.seed_count_request;
                    let r = ui.add(egui::Slider::new(&mut sc, 64..=16_384).text("streamlines"));
                    if r.changed() {
                        s.seed_count_request = sc;
                        s.topology_dirty = true;
                    }
                    reg(ui, &mut s.help, "lines.streamlines",
                        "Number of seeded field lines. Higher = denser wireframe (rebuilds topology).",
                        format!("{}", s.seed_count_request), r);

                    let mut spl = s.steps_per_line_request;
                    let r = ui.add(egui::Slider::new(&mut spl, 16..=1024).text("steps / line"));
                    if r.changed() {
                        s.steps_per_line_request = spl;
                        s.topology_dirty = true;
                    }
                    reg(ui, &mut s.help, "lines.steps",
                        "Integration steps per streamline — longer lines, smoother curves, more cost.",
                        format!("{}", s.steps_per_line_request), r);

                    let r = ui.add(egui::Slider::new(&mut s.step_len, 0.005..=0.2)
                        .text("step length")
                        .logarithmic(true));
                    reg(ui, &mut s.help, "lines.step_len",
                        "World-space distance integrated per step. Smaller = more accurate field tracing.",
                        format!("{:.4}", s.step_len), r);
                }
                RenderMode::Particles => {
                    let mut pc = s.particle_count_request;
                    let r = ui.add(egui::Slider::new(&mut pc, 1024..=1_000_000).logarithmic(true)
                        .text("particles"));
                    if r.changed() {
                        s.particle_count_request = pc;
                        s.particles_dirty = true;
                    }
                    reg(ui, &mut s.help, "particles.count",
                        "Number of advected particles (reallocates GPU buffers when changed).",
                        format!("{}", s.particle_count_request), r);

                    let r = ui.add(egui::Slider::new(&mut s.particle_speed, 0.05..=8.0)
                        .text("particle speed"));
                    reg(ui, &mut s.help, "particles.speed",
                        "How fast particles flow along the field each frame.",
                        format!("{:.3}", s.particle_speed), r);
                }
                RenderMode::Fdtd => {
                    ui.label(egui::RichText::new(
                        "REAL FDTD: Yee leapfrog Maxwell on a 384×128×128 GPU grid."
                    ).strong().color(egui::Color32::from_rgb(120, 220, 140)));
                    ui.label("F = E + I·c·B  ·  dt = 0.45·dx  ·  CFL safe at √3");

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_substeps, 1..=32)
                        .text("substeps / frame"));
                    reg(ui, &mut s.help, "fdtd.substeps",
                        "FDTD time-steps marched per rendered frame. Higher = faster propagation + more GPU load.",
                        format!("{}", s.fdtd_substeps), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_dt, 0.05..=0.55)
                        .text("dt (CFL ≤ 0.577)"));
                    reg(ui, &mut s.help, "fdtd.dt",
                        "Courant time-step as a fraction of dx. Above ~0.577 the leapfrog goes numerically unstable.",
                        format!("{:.3}", s.fdtd_dt), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_sigma, 0.0..=0.05)
                        .text("σ  bulk damping"));
                    reg(ui, &mut s.help, "fdtd.sigma",
                        "Uniform conductivity that bleeds energy from the whole grid (tames runaway resonance).",
                        format!("{:.4}", s.fdtd_sigma), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_pml_strength, 0.0..=0.20)
                        .text("PML strength"));
                    reg(ui, &mut s.help, "fdtd.pml",
                        "Absorbing-boundary strength. Higher swallows outgoing waves but can reflect if too abrupt.",
                        format!("{:.3}", s.fdtd_pml_strength), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_drift_scale, 0.5..=24.0)
                        .text("|S| drift gain"));
                    reg(ui, &mut s.help, "fdtd.drift",
                        "How strongly particles drift along the Poynting vector |E×B|. Visual flow speed.",
                        format!("{:.2}", s.fdtd_drift_scale), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_step, 0.001..=0.05)
                        .text("particle step (world)"));
                    reg(ui, &mut s.help, "fdtd.step",
                        "World-space distance a particle moves per advect substep.",
                        format!("{:.4}", s.fdtd_step), r);

                    ui.horizontal(|ui| {
                        let r = ui.checkbox(&mut s.fdtd_mirror_enabled, "PEC mirror enabled");
                        reg(ui, &mut s.help, "fdtd.mirror_enabled",
                            "Enable a perfect-electric-conductor wall so the donut bounces (mirror collision).",
                            format!("{}", s.fdtd_mirror_enabled), r);
                        let r = ui.checkbox(&mut s.fdtd_show_mirror, "show mirror box");
                        reg(ui, &mut s.help, "fdtd.show_mirror",
                            "Draw the pink wireframe box marking the PEC mirror location.",
                            format!("{}", s.fdtd_show_mirror), r);
                    });

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_mirror_gap, 0.0..=1.4)
                        .text("mirror gap (0 = touching cube)"));
                    reg(ui, &mut s.help, "fdtd.mirror_gap",
                        "Distance of the PEC mirror wall in from the cube's far (+x) face. 0 = wall flush against the cube; increase to pull the wall toward the pulse so the bounce happens sooner and is easy to see. (Reflecting mask updates on the next pulse.)",
                        format!("{:.2}", s.fdtd_mirror_gap), r);

                    let r = ui.button(egui::RichText::new("⟳ RESEED FLYING DONUT")
                        .strong().size(15.0));
                    if r.clicked() {
                        s.fdtd_reseed_requested = true;
                    }
                    reg(ui, &mut s.help, "fdtd.reseed",
                        "Re-inject a fresh toroidal donut pulse using the current Seed-donut-shape settings.",
                        format!("step={}", s.fdtd_total_steps), r);

                    ui.label(format!("FDTD time-step: {}", s.fdtd_total_steps));

                    ui.separator();
                    ui.label(egui::RichText::new("Visual tuning (live)")
                        .strong().color(egui::Color32::from_rgb(255, 215, 120)));

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_brightness, 0.05..=4.0)
                        .text("brightness"));
                    reg(ui, &mut s.help, "fdtd.brightness",
                        "Additive-blend glow multiplier for the dots. Raise if the donut looks too dim.",
                        format!("{:.2}", s.fdtd_brightness), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_density_gate, 0.0..=0.6)
                        .text("density gate (hide weak |S|)")
                        .logarithmic(true));
                    reg(ui, &mut s.help, "fdtd.density_gate",
                        "Hide particles where the energy density |E×B| is below this threshold — clears the empty corners.",
                        format!("{:.4}", s.fdtd_density_gate), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_max_age, 0.2..=16.0)
                        .text("particle lifetime (s)"));
                    reg(ui, &mut s.help, "fdtd.max_age",
                        "Seconds before a particle respawns. Longer = persistent trails; shorter = crisp wavefront.",
                        format!("{:.2}", s.fdtd_max_age), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_respawn_scale, 0.2..=3.0)
                        .text("respawn box scale"));
                    reg(ui, &mut s.help, "fdtd.respawn_scale",
                        "Size of the box particles respawn into. Larger spreads them; smaller concentrates the seed.",
                        format!("{:.2}", s.fdtd_respawn_scale), r);

                    let r = ui.add(egui::Slider::new(&mut s.fdtd_respawn_x, -1.0..=1.0)
                        .text("respawn X offset (× world)"));
                    reg(ui, &mut s.help, "fdtd.respawn_x",
                        "Shift the respawn box along +x/−x (fraction of world extent) to follow the propagating pulse.",
                        format!("{:.3}", s.fdtd_respawn_x), r);

                    ui.horizontal(|ui| {
                        ui.label("dot color RGB:");
                        let r = ui.color_edit_button_rgb(&mut s.fdtd_color);
                        reg(ui, &mut s.help, "fdtd.color",
                            "Base RGB color of the FDTD particles.",
                            format!("[{:.2}, {:.2}, {:.2}]",
                                s.fdtd_color[0], s.fdtd_color[1], s.fdtd_color[2]), r);
                    });

                    ui.collapsing("Seed donut shape (applied on RESEED)", |ui| {
                        let r = ui.add(egui::Slider::new(&mut s.fdtd_seed_radius_frac, 0.02..=0.6)
                            .text("seed radius (× world)"));
                        let r_changed = r.changed();
                        reg(ui, &mut s.help, "fdtd.seed_radius",
                            "Major radius of the injected donut, as a fraction of the world extent.",
                            format!("{:.3}", s.fdtd_seed_radius_frac), r);

                        let r = ui.add(egui::Slider::new(&mut s.fdtd_seed_width_frac, 0.02..=0.4)
                            .text("seed width (× world)"));
                        let w_changed = r.changed();
                        reg(ui, &mut s.help, "fdtd.seed_width",
                            "Tube thickness of the injected donut, as a fraction of the world extent.",
                            format!("{:.3}", s.fdtd_seed_width_frac), r);

                        let r = ui.add(egui::Slider::new(&mut s.fdtd_seed_amp, 0.1..=4.0)
                            .text("seed amplitude"));
                        let a_changed = r.changed();
                        reg(ui, &mut s.help, "fdtd.seed_amp",
                            "Peak field strength of the injected pulse. Higher = brighter, faster-propagating donut.",
                            format!("{:.2}", s.fdtd_seed_amp), r);

                        if r_changed || w_changed || a_changed {
                            ui.label(egui::RichText::new("→ click RESEED above to apply")
                                .small().color(egui::Color32::from_rgb(255, 180, 120)));
                        }
                    });

                    ui.horizontal(|ui| {
                        let r = ui.button("preset: bright");
                        if r.clicked() {
                            s.fdtd_brightness   = 2.5;
                            s.fdtd_density_gate = 0.06;
                            s.fdtd_max_age      = 2.0;
                            s.fdtd_respawn_scale = 1.4;
                        }
                        reg(ui, &mut s.help, "fdtd.preset.bright",
                            "Quick look: bright, gated, short-lived dots concentrated on the donut.",
                            "preset".into(), r);

                        let r = ui.button("preset: trails");
                        if r.clicked() {
                            s.fdtd_brightness   = 1.2;
                            s.fdtd_density_gate = 0.02;
                            s.fdtd_max_age      = 8.0;
                            s.fdtd_respawn_scale = 0.6;
                        }
                        reg(ui, &mut s.help, "fdtd.preset.trails",
                            "Quick look: long-lived particles that leave flowing trails.",
                            "preset".into(), r);

                        let r = ui.button("preset: sparse");
                        if r.clicked() {
                            s.fdtd_brightness   = 1.0;
                            s.fdtd_density_gate = 0.18;
                            s.fdtd_max_age      = 3.0;
                            s.fdtd_respawn_scale = 1.0;
                        }
                        reg(ui, &mut s.help, "fdtd.preset.sparse",
                            "Quick look: heavily gated, few dots — only the strongest field region.",
                            "preset".into(), r);
                    });

                    ui.label(egui::RichText::new(
                        "Tip: substeps≥8 + grid 384×128×128 is what warms the RTX 2070. \
                         Watch the donut propagate +x, hit the pink box, and bounce."
                    ).small().italics());
                }
            }

            ui.separator();
            ui.collapsing("Presets · what you're seeing", |ui| {
                ui.label("All presets are projections of one 4-D Bateman seed.");
                ui.label("• Hopfion         — Rañada link, Hopf index 1.");
                ui.label("• Photon hopfion  — same field, single-photon scale.");
                ui.label("• Flying donut    — Hellwarth–Nouchi toroidal pulse, link=0.");
                ui.label("• Plane photon    — linearly polarized Gaussian wavepacket.");
                ui.label("• CP photon       — circularly polarized, helical streamlines.");
                ui.label("• Trefoil         — Hopf index 2 (Kedia, BBP 2013).");
                ui.label("• STA crunch      — high-density stress test (8 k lines × 512).");
            });

            ui.collapsing("Help / controls", |ui| {
                ui.label("• F1 over a control: log a remark → ui-remarks.json");
                ui.label("• Hover a control  : show its help tooltip");
                ui.label("• Right-mouse drag : orbit");
                ui.label("• Middle-mouse drag: pan");
                ui.label("• Wheel             : zoom");
                ui.label("• Field modes:");
                ui.label("   E         — electric Hopf field (cyan)");
                ui.label("   B         — magnetic Hopf field (magenta)");
                ui.label("   E & B     — both, overlaid");
                ui.label("   Poynting  — S = E × B (amber)");
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!("frame: {:.1} ms ({:.0} fps)",
                    s.last_frame_ms, 1000.0 / s.last_frame_ms.max(0.01)));
                if !s.help.remarks.is_empty() {
                    ui.label(egui::RichText::new(
                        format!("· {} remark(s) logged", s.help.remarks.len()))
                        .small().color(egui::Color32::from_rgb(255, 200, 120)));
                }
            });
        });

    // ---- F1 remark popup (rendered outside the main window) ----------------
    if let Some(id) = s.help.open_id.clone() {
        let mut open = true;
        egui::Window::new(format!("🛈 remark · {id}"))
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&s.help.open_help).strong());
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("object-id : {id}"))
                    .monospace().small());
                ui.label(egui::RichText::new(format!("value now : {}", s.help.open_value))
                    .monospace().small());
                ui.separator();
                ui.label("Your remark — what is wrong / what you want changed:");
                ui.add(egui::TextEdit::multiline(&mut s.help.draft)
                    .desired_rows(3)
                    .desired_width(340.0)
                    .hint_text("e.g. too dim here, or this slider has no visible effect…"));
                ui.horizontal(|ui| {
                    if ui.button("📝 Log remark → JSON").clicked() {
                        let note = s.help.draft.trim().to_string();
                        if !note.is_empty() {
                            let remark = Remark {
                                object_id:   id.clone(),
                                value:       s.help.open_value.clone(),
                                note,
                                unix_ms:     unix_ms(),
                                preset:      format!("{:?}", s.preset),
                                render_mode: format!("{:?}", s.render_mode),
                                field_mode:  format!("{:?}", s.field_mode),
                            };
                            s.help.remarks.push(remark);
                            s.help.draft.clear();
                            s.help.flush_requested = true;
                        }
                        open = false;
                    }
                    if ui.button("Close").clicked() {
                        open = false;
                    }
                });
                if let Some(p) = &s.help.last_log_path {
                    ui.label(egui::RichText::new(
                        format!("logged → {p}  ({} total)", s.help.remarks.len()))
                        .small().color(egui::Color32::from_rgb(120, 220, 140)));
                }
            });
        if !open {
            s.help.open_id = None;
        }
    }

    // ---- Flush the remark log to disk when a new remark was added ----------
    if s.help.flush_requested {
        s.help.flush_requested = false;
        match write_remarks(s) {
            Ok(p)  => s.help.last_log_path = Some(p),
            Err(e) => log::error!("remark log write failed: {e}"),
        }
    }
}
