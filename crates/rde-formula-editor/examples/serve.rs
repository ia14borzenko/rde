//! Live-проверка моста окна редактора без GUI RDE (для скриншотов и e2e-скрипта).
//!
//! Запускает `Bridge` со встроенными ассетами, печатает URL страницы редактора, затем ждёт
//! событий страницы и выходит после Apply/Cancel. Используется `scripts/verify-formula-editor.sh`
//! (headless Chrome + CDP) и для ручных скриншотов:
//!
//! ```bash
//! cargo run -p rde-formula-editor --example serve
//! # URL=http://127.0.0.1:PORT/?t=...
//! RDE_EDITOR_THEME=light RDE_EDITOR_LATEX='\int_0^1 x^2 dx' cargo run -p rde-formula-editor --example serve
//! ```

use rde_formula_editor::{Bridge, EditorEvent, EditorState, EditorTheme};

fn main() {
    let latex = std::env::var("RDE_EDITOR_LATEX").unwrap_or_else(|_| "\\sigma=\\frac{F}{A}".to_owned());
    let title = std::env::var("RDE_EDITOR_TITLE").unwrap_or_else(|_| "Средняя квадратичная σ".to_owned());
    let project = std::env::var("RDE_EDITOR_PROJECT").unwrap_or_else(|_| "demo-project".to_owned());
    let display = std::env::var("RDE_EDITOR_DISPLAY").map(|value| value != "0").unwrap_or(true);
    let theme = if std::env::var("RDE_EDITOR_THEME").is_ok_and(|value| value.eq_ignore_ascii_case("light")) {
        EditorTheme::Light
    } else {
        EditorTheme::Dark
    };
    let state = EditorState { title, latex, display, theme, zoom: 100, project };
    let bridge = match Bridge::start(state) {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("мост не поднялся: {error}");
            std::process::exit(2);
        }
    };
    // Единственная строка, которую парсит скрипт: `URL=<адрес страницы с токеном>`.
    println!("URL={}", bridge.page_url());
    println!("READY");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    loop {
        for event in bridge.drain() {
            match &event {
                EditorEvent::Applied { latex } => {
                    println!("EVENT=applied");
                    println!("LATEX={latex}");
                    return;
                }
                EditorEvent::Cancelled => {
                    println!("EVENT=cancelled");
                    return;
                }
                EditorEvent::Zoom { zoom } => println!("EVENT=zoom zoom={zoom}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}
