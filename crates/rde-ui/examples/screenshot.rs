//! Headless-скриншот окна RDE: рендерится виртуально (wgpu offscreen), без видимых
//! окон и без X11/Wayland — «второй рабочий стол» не нужен.
//!
//! `cargo run -p rde-ui --example screenshot -- /path/out.png`

use rde_core::demo;
use rde_ui::RdeApp;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/rde-concept.png".to_owned());
    let state = RdeApp::new(demo());

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_pixels_per_point(1.5)
        .with_max_steps(100_000)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut RdeApp| {
                app.ui(ui);
            },
            state,
        );
    harness.input_mut().max_texture_side = Some(8192);
    for _ in 0..12 {
        harness.step();
    }

    match harness.render() {
        Ok(img) => {
            img.save(&out).expect("save png");
            println!("{out}");
        }
        Err(e) => {
            eprintln!("render failed: {e}");
            std::process::exit(1);
        }
    }
}
