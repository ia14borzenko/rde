//! Интеграционные тесты локального моста окна редактора (P6, ADR-0022): реальный TCP на
//! `127.0.0.1`, без браузера и без GUI. Проверяют контракт страницы и безопасность.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use rde_formula_editor::{Bridge, EditorEvent, EditorState, EditorTheme};

/// Сырой HTTP/1.0-запрос к мосту: возвращает (статус, заголовки, тело).
fn request(addr: SocketAddr, method: &str, path: &str, host: &str, body: Option<&str>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("подключение к мосту");
    let body = body.unwrap_or("");
    let mut head = format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n");
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("запрос");
    stream.write_all(body.as_bytes()).expect("тело запроса");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("ответ");
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n").expect("заголовки ответа");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .expect("статус");
    let headers = lines
        .filter_map(|line| line.split_once(':').map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned())))
        .collect();
    (status, headers, body)
}

fn state_with(latex: &str) -> EditorState {
    EditorState { title: "σ".to_owned(), latex: latex.to_owned(), display: true, theme: EditorTheme::Dark, zoom: 100, project: "demo".to_owned() }
}

#[test]
fn page_assets_and_csp_are_served_from_loopback() {
    let bridge = Bridge::start(state_with("\\sigma=\\frac{F}{A}")).expect("мост");
    let host = bridge.addr().to_string();
    let (status, headers, body) = request(bridge.addr(), "GET", "/", &host, None);
    assert_eq!(status, 200);
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("<math-field"), "страница MathLive: {html:.200}");
    assert!(html.contains("./mathlive/mathlive.min.js"));
    let csp = headers.iter().find(|(name, _)| name == "content-security-policy").map(|(_, value)| value.clone());
    let csp = csp.expect("CSP-заголовок");
    assert!(csp.contains("default-src 'self'"), "CSP: {csp}");
    assert!(csp.contains("connect-src 'self'"), "CSP: {csp}");

    // Крупный ассет и шрифт отдаются целиком с правильным MIME.
    for (path, mime, min_len) in [
        ("/mathlive/mathlive.min.js", "text/javascript", 800_000),
        ("/mathlive/mathlive-static.css", "text/css", 5_000),
        ("/mathlive/fonts/KaTeX_Main-Regular.woff2", "font/woff2", 20_000),
        ("/editor/editor.js", "text/javascript", 5_000),
    ] {
        let (status, headers, body) = request(bridge.addr(), "GET", path, &host, None);
        assert_eq!(status, 200, "{path}");
        assert!(body.len() >= min_len, "{path}: {} байт", body.len());
        let content_type = headers.iter().find(|(name, _)| name == "content-type").map(|(_, value)| value.clone()).unwrap_or_default();
        assert!(content_type.starts_with(mime), "{path}: Content-Type {content_type}");
    }
    // Неизвестный ассет — 404, а не пустой 200.
    let (status, _, _) = request(bridge.addr(), "GET", "/mathlive/nope.js", &host, None);
    assert_eq!(status, 404);
}

#[test]
fn state_requires_token_and_foreign_host_is_rejected() {
    let bridge = Bridge::start(state_with("x^2")).expect("мост");
    let host = bridge.addr().to_string();
    let token = bridge.token().to_owned();
    let (status, _, _) = request(bridge.addr(), "GET", "/api/state", &host, None);
    assert_eq!(status, 401, "без токена — 401");
    let (status, _, _) = request(bridge.addr(), "GET", "/api/state?t=deadbeef", &host, None);
    assert_eq!(status, 401, "чужой токен — 401");
    let (status, _, body) = request(bridge.addr(), "GET", &format!("/api/state?t={token}"), &host, None);
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON состояния");
    assert_eq!(json["data"]["latex"], "x^2");
    assert_eq!(json["data"]["display"], true);
    assert_eq!(json["data"]["theme"], "dark");
    // Anti-DNS-rebinding: чужой Host отклоняется даже с токеном.
    let evil_host = "evil.example.com";
    let (status, _, _) = request(bridge.addr(), "GET", &format!("/api/state?t={token}"), evil_host, None);
    assert_eq!(status, 403);
    // Страница тоже не отдаётся чужому Host.
    let (status, _, _) = request(bridge.addr(), "GET", "/", evil_host, None);
    assert_eq!(status, 403);
}

#[test]
fn apply_and_cancel_events_reach_rust() {
    let bridge = Bridge::start(state_with("\\frac{1}{2}")).expect("мост");
    let host = bridge.addr().to_string();
    let token = bridge.token().to_owned();
    let (status, _, _) = request(
        bridge.addr(),
        "POST",
        &format!("/api/apply?t={token}"),
        &host,
        Some(r#"{"latex":"\\int_0^1 x^2 dx = \\frac{1}{3}"}"#),
    );
    assert_eq!(status, 200);
    let events = bridge.drain();
    assert_eq!(events.len(), 1, "события: {events:?}");
    assert_eq!(events[0], EditorEvent::Applied { latex: "\\int_0^1 x^2 dx = \\frac{1}{3}".to_owned() });
    // Пустой LaTeX — 400 и никаких событий.
    let (status, _, _) = request(bridge.addr(), "POST", &format!("/api/apply?t={token}"), &host, Some(r#"{"latex":"   "}"#));
    assert_eq!(status, 400);
    assert!(bridge.drain().is_empty());
    // Отмена — событие Cancelled.
    let (status, _, _) = request(bridge.addr(), "POST", &format!("/api/cancel?t={token}"), &host, Some("{}"));
    assert_eq!(status, 200);
    assert_eq!(bridge.drain(), vec![EditorEvent::Cancelled]);
    // Без токена apply не принимается.
    let (status, _, _) = request(bridge.addr(), "POST", "/api/apply", &host, Some(r#"{"latex":"x"}"#));
    assert_eq!(status, 401);
    assert!(bridge.drain().is_empty());
}

#[test]
fn zoom_pref_updates_state_and_emits_event() {
    let bridge = Bridge::start(state_with("x")).expect("мост");
    let host = bridge.addr().to_string();
    let token = bridge.token().to_owned();
    let (status, _, _) = request(bridge.addr(), "POST", &format!("/api/prefs?t={token}"), &host, Some(r#"{"zoom":170}"#));
    assert_eq!(status, 200);
    assert_eq!(bridge.state().zoom, 170);
    assert_eq!(bridge.drain(), vec![EditorEvent::Zoom { zoom: 170 }]);
    // Клампинг 100–200: 900 → 200.
    let _ = request(bridge.addr(), "POST", &format!("/api/prefs?t={token}"), &host, Some(r#"{"zoom":900}"#));
    assert_eq!(bridge.state().zoom, 200);
    let _ = bridge.drain();
}

#[test]
fn rde_can_push_theme_and_content_to_open_page() {
    let bridge = Bridge::start(state_with("x")).expect("мост");
    bridge.update_state(|state| {
        state.theme = EditorTheme::Light;
        state.latex = "E = mc^2".to_owned();
        state.title = "Масса-энергия".to_owned();
        state.display = false;
    });
    let host = bridge.addr().to_string();
    let token = bridge.token().to_owned();
    let (status, _, body) = request(bridge.addr(), "GET", &format!("/api/state?t={token}"), &host, None);
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(json["data"]["theme"], "light");
    assert_eq!(json["data"]["latex"], "E = mc^2");
    assert_eq!(json["data"]["display"], false);
}

#[test]
fn method_mismatch_and_body_limits() {
    let bridge = Bridge::start(state_with("x")).expect("мост");
    let host = bridge.addr().to_string();
    let token = bridge.token().to_owned();
    let (status, _, _) = request(bridge.addr(), "POST", &format!("/api/state?t={token}"), &host, Some("{}"));
    assert_eq!(status, 405);
    let (status, _, _) = request(bridge.addr(), "GET", "/api/apply", &host, None);
    assert_eq!(status, 405);
    // Битый JSON — 400, мост жив.
    let (status, _, _) = request(bridge.addr(), "POST", &format!("/api/apply?t={token}"), &host, Some("{не json"));
    assert_eq!(status, 400);
    let (status, _, _) = request(bridge.addr(), "GET", &format!("/api/state?t={token}"), &host, None);
    assert_eq!(status, 200, "мост продолжает обслуживать после ошибки клиента");
}

#[test]
fn shutdown_stops_listener_and_is_idempotent() {
    use std::time::Duration;
    let mut bridge = Bridge::start(state_with("x")).expect("мост");
    let addr = bridge.addr();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok(), "мост слушает");
    bridge.shutdown();
    bridge.shutdown(); // повторный вызов — no-op, без паники/зависания
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err(),
        "после shutdown порт не принимает соединения"
    );
}
