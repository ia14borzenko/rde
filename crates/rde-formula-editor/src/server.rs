//! Локальный HTTP-мост «страница редактора ↔ RDE» (P6, ADR-0022 §3.1).
//!
//! Свой минимальный HTTP/1.0-сервер на `std` (без async/новых зависимостей — по правилам
//! прототипа): слушает **только** `127.0.0.1` на случайном порту, отдаёт встроенные ассеты и
//! принимает JSON-события страницы. Канал событий — `std::sync::mpsc`: RDE опрашивает очередь
//! из GUI-кадра (`Bridge::drain`), сервер ничего не знает про UI.
//!
//! Безопасность (ТЗ §1.4):
//! - bind только loopback; `Host` обязан быть `127.0.0.1[:port]`/`localhost[:port]` (anti-DNS-rebinding);
//! - API-маршруты требуют одноразовый токен `t` (CSPRNG, сравнение за постоянное время);
//! - тело запроса ≤ 512 КиБ, заголовки ≤ 32 КиБ, таймауты чтения/записи;
//! - страница отдаётся со строгим CSP (`default-src 'self'`): скрипты/шрифты/стили — только из
//!   бандла, никакой сети; LaTeX — данные, из страницы не исполняется никакой код RDE.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::assets;
use crate::protocol::{ApiReply, ApplyRequest, EditorEvent, EditorState, PrefsRequest, clamp_zoom};

/// Максимальный размер тела запроса (LaTeX-источники — килобайты, запас большой).
const MAX_BODY: usize = 512 * 1024;
/// Максимальный размер заголовков запроса.
const MAX_HEAD: usize = 32 * 1024;
/// Таймаут чтения/записи соединения (локальные запросы мгновенные; защита от зависших клиентов).
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// Политика безопасности страницы: только собственные ассеты, без сети и внешних шрифтов.
const PAGE_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                        font-src 'self'; img-src 'self' data:; connect-src 'self'; \
                        base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Общее состояние моста между серверным потоком и GUI-потоком.
struct Shared {
    /// Текущее состояние (LaTeX/тема/зум/заголовок), его читает страница.
    state: Mutex<EditorState>,
    /// Очередь событий страницы для RDE (`mpsc::Sender` не Sync — под мьютексом).
    events: Mutex<Sender<EditorEvent>>,
    /// Одноразовый токен API (hex, 32 символа).
    token: String,
}

/// Мост к странице редактора: сервер + очередь событий. Живёт столько же, сколько окно.
pub struct Bridge {
    addr: SocketAddr,
    token: String,
    shared: Arc<Shared>,
    receiver: Receiver<EditorEvent>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Запустить мост с начальным состоянием; порт выбирает ОС (только loopback).
    pub fn start(state: EditorState) -> Result<Self, String> {
        let token = random_token().map_err(|error| format!("токен моста: {error}"))?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| format!("порт моста: {error}"))?;
        let addr = listener.local_addr().map_err(|error| format!("адрес моста: {error}"))?;
        let (sender, receiver) = channel();
        let shared = Arc::new(Shared { state: Mutex::new(state), events: Mutex::new(sender), token: token.clone() });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("rde-formula-editor-bridge".to_owned())
            .spawn(move || serve(listener, &thread_shared, &thread_stop))
            .map_err(|error| format!("поток моста: {error}"))?;
        Ok(Self { addr, token, shared, receiver, stop, thread: Some(thread) })
    }

    /// Адрес моста (`127.0.0.1:порт`).
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Токен API (страница получает его в query страницы `?t=`).
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// URL страницы редактора для запуска браузера (`http://127.0.0.1:порт/?t=…`).
    #[must_use]
    pub fn page_url(&self) -> String {
        format!("http://{}/?t={}", self.addr, self.token)
    }

    /// Базовый URL (без токена) — для диагностики/тестов.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Текущее состояние окна.
    #[must_use]
    pub fn state(&self) -> EditorState {
        self.shared.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    /// Обновить состояние (тема/зум/заголовок) — страница увидит при следующем `/api/state`.
    pub fn update_state(&self, update: impl FnOnce(&mut EditorState)) {
        if let Ok(mut state) = self.shared.state.lock() {
            update(&mut state);
        }
    }

    /// Забрать накопившиеся события страницы (не блокирует).
    pub fn drain(&self) -> Vec<EditorEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Остановить сервер и дождаться потока (идемпотентно; вызывается и из `Drop`).
    pub fn shutdown(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // Разбудить `accept` соединением-пустышкой: после него поток увидит stop-флаг.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Одноразовый токен API: 16 байт CSPRNG в hex (32 символа).
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| format!("CSPRNG недоступен: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Сравнение токенов за постоянное время (не даём измерить префикс по времени ответа).
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ── серверный цикл ───────────────────────────────────────────────────────────

fn serve(listener: TcpListener, shared: &Arc<Shared>, stop: &Arc<AtomicBool>) {
    for connection in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(mut stream) = connection else { continue };
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let response = match read_request(&mut stream) {
            Ok(request) => route(&request, shared),
            Err(error) => error_response(error),
        };
        let _ = write_response(&mut stream, &response);
    }
}

enum RequestError {
    BadRequest(String),
    TooLarge,
}

fn error_response(error: RequestError) -> HttpResponse {
    match error {
        RequestError::BadRequest(message) => json_response(400, &ApiReply::<()>::fail(format!("некорректный запрос: {message}"))),
        RequestError::TooLarge => json_response(413, &ApiReply::<()>::fail("запрос слишком большой")),
    }
}

// ── HTTP-разбор/ответ ────────────────────────────────────────────────────────

struct HttpRequest {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    host: String,
    body: Vec<u8>,
}

impl HttpRequest {
    fn query_value(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, RequestError> {
    let mut buffer = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            break position;
        }
        if buffer.len() > MAX_HEAD {
            return Err(RequestError::TooLarge);
        }
        let read = stream.read(&mut chunk).map_err(|error| RequestError::BadRequest(error.to_string()))?;
        if read == 0 {
            return Err(RequestError::BadRequest("соединение закрыто до заголовков".to_owned()));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or_else(|| RequestError::BadRequest("пустая строка запроса".to_owned()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| RequestError::BadRequest("нет метода".to_owned()))?.to_ascii_uppercase();
    let target = parts.next().ok_or_else(|| RequestError::BadRequest("нет пути".to_owned()))?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), parse_query(query)),
        None => (target.to_owned(), Vec::new()),
    };
    let mut host = String::new();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => host = value.to_owned(),
            "content-length" => {
                content_length = value.parse().map_err(|_| RequestError::BadRequest("content-length".to_owned()))?
            }
            _ => {}
        }
    }
    if content_length > MAX_BODY {
        return Err(RequestError::TooLarge);
    }
    let body_start = head_end + 4;
    let mut body = buffer.get(body_start..).map(<[u8]>::to_vec).unwrap_or_default();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).map_err(|error| RequestError::BadRequest(error.to_string()))?;
        if read == 0 {
            return Err(RequestError::BadRequest("тело запроса не дочитано".to_owned()));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(HttpRequest { method, path, query, host, body })
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(part), String::new()),
        })
        .collect()
}

/// Минимальный percent-decode (значения токена/зума — ASCII; для пути ассетов достаточно).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        if bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

struct HttpResponse {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self { status, content_type: content_type.to_owned(), headers: Vec::new(), body }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

fn json_response(status: u16, reply: &ApiReply<impl serde::Serialize>) -> HttpResponse {
    let body = serde_json::to_vec(reply).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"json\"}".to_vec());
    HttpResponse::new(status, "application/json; charset=utf-8", body).header("Cache-Control", "no-store")
}

fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Unknown",
    };
    let mut head = format!(
        "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

// ── маршрутизация ───────────────────────────────────────────────────────────

fn route(request: &HttpRequest, shared: &Shared) -> HttpResponse {
    // Anti-DNS-rebinding: страница обязана открываться с loopback-именем.
    if !host_is_loopback(&request.host) {
        return json_response(403, &ApiReply::<()>::fail("мост доступен только с loopback"));
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => serve_editor_page(),
        ("GET", "/favicon.ico") => HttpResponse::new(204, "image/x-icon", Vec::new()),
        ("GET", "/health") => json_response(200, &ApiReply::<()>::fail("")), // ok:true, без данных
        ("GET", path) if path.starts_with("/editor/") || path.starts_with("/mathlive/") => serve_asset(path),
        ("GET", "/api/state") => authorized(request, shared, || {
            let state = shared.state.lock().map(|state| state.clone()).unwrap_or_default();
            json_response(200, &ApiReply::ok(state))
        }),
        ("GET", "/api/ping") => authorized(request, shared, || json_response(200, &ApiReply::<()>::done())),
        ("POST", "/api/apply") => authorized(request, shared, || {
            let request_body = match serde_json::from_slice::<ApplyRequest>(&request.body) {
                Ok(body) => body,
                Err(error) => return json_response(400, &ApiReply::<()>::fail(format!("тело apply: {error}"))),
            };
            let latex = request_body.latex;
            if latex.trim().is_empty() {
                return json_response(400, &ApiReply::<()>::fail("пустой LaTeX"));
            }
            if latex.len() > MAX_BODY {
                return json_response(413, &ApiReply::<()>::fail("LaTeX слишком длинный"));
            }
            if let Ok(events) = shared.events.lock() {
                let _ = events.send(EditorEvent::Applied { latex });
            }
            // Ответ странице: RDE закроет окно; страница показывает «применено».
            json_response(200, &ApiReply::<()>::done())
        }),
        ("POST", "/api/cancel") => authorized(request, shared, || {
            if let Ok(events) = shared.events.lock() {
                let _ = events.send(EditorEvent::Cancelled);
            }
            json_response(200, &ApiReply::<()>::done())
        }),
        ("POST", "/api/prefs") => authorized(request, shared, || {
            let Ok(body) = serde_json::from_slice::<PrefsRequest>(&request.body) else {
                return json_response(400, &ApiReply::<()>::fail("тело prefs"));
            };
            let zoom = clamp_zoom(body.zoom);
            if let Ok(mut state) = shared.state.lock() {
                state.zoom = zoom;
            }
            if let Ok(events) = shared.events.lock() {
                let _ = events.send(EditorEvent::Zoom { zoom });
            }
            json_response(200, &ApiReply::<()>::done())
        }),
        // API-путь с неверным методом — 405, прочие пути — 404.
        (_, path) if path.starts_with("/api/") => json_response(405, &ApiReply::<()>::fail("метод не поддержан")),
        _ => json_response(404, &ApiReply::<()>::fail("не найдено")),
    }
}

/// `Host` должен быть loopback-именем (`127.0.0.1[:port]`, `localhost[:port]`, `[::1][:port]`).
fn host_is_loopback(host: &str) -> bool {
    let host = host.trim();
    let host = if let Some(rest) = host.strip_prefix('[') {
        // IPv6 в скобках: [::1]:port
        match rest.split_once(']') {
            Some((address, _)) => format!("[{address}]"),
            None => return false,
        }
    } else {
        host.split(':').next().unwrap_or("").to_ascii_lowercase()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

fn authorized(request: &HttpRequest, shared: &Shared, action: impl FnOnce() -> HttpResponse) -> HttpResponse {
    let token = request.query_value("t").unwrap_or("");
    if token.is_empty() || !constant_time_eq(token.as_bytes(), shared.token.as_bytes()) {
        return json_response(401, &ApiReply::<()>::fail("неверный токен моста"));
    }
    action()
}

fn serve_editor_page() -> HttpResponse {
    let Some(asset) = assets::lookup("/editor/editor.html") else {
        return json_response(500, &ApiReply::<()>::fail("страница редактора не встроена"));
    };
    HttpResponse::new(200, asset.mime, asset.bytes.to_vec())
        .header("Cache-Control", "no-store")
        .header("Content-Security-Policy", PAGE_CSP)
        .header("Referrer-Policy", "no-referrer")
}

fn serve_asset(path: &str) -> HttpResponse {
    let Some(asset) = assets::lookup(path) else {
        return json_response(404, &ApiReply::<()>::fail("ассет не найден"));
    };
    HttpResponse::new(200, asset.mime, asset.bytes.to_vec()).header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_only_equal_tokens() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn host_validation() {
        assert!(host_is_loopback("127.0.0.1:8080"));
        assert!(host_is_loopback("localhost:1"));
        assert!(host_is_loopback("LOCALHOST"));
        assert!(host_is_loopback("[::1]:9000"));
        assert!(!host_is_loopback("evil.example.com"));
        assert!(!host_is_loopback("127.0.0.1.evil.com"));
        assert!(!host_is_loopback(""));
    }

    #[test]
    fn query_and_percent_decoding() {
        let query = parse_query("t=deadbeef&latex=%5Cfrac%7B1%7D%7B2%7D&plus=a+b");
        assert_eq!(query.iter().find(|(key, _)| key == "t").map(|(_, value)| value.as_str()), Some("deadbeef"));
        assert_eq!(
            query.iter().find(|(key, _)| key == "latex").map(|(_, value)| value.as_str()),
            Some("\\frac{1}{2}")
        );
        assert_eq!(query.iter().find(|(key, _)| key == "plus").map(|(_, value)| value.as_str()), Some("a b"));
    }

    #[test]
    fn route_rejects_foreign_host_and_bad_token() {
        let (sender, _receiver) = channel();
        let shared = Shared { state: Mutex::new(EditorState::default()), events: Mutex::new(sender), token: "secret".to_owned() };
        let foreign = HttpRequest {
            method: "GET".to_owned(),
            path: "/api/state".to_owned(),
            query: vec![("t".to_owned(), "secret".to_owned())],
            host: "evil.example.com".to_owned(),
            body: Vec::new(),
        };
        assert_eq!(route(&foreign, &shared).status, 403);
        let unauthorized = HttpRequest {
            method: "GET".to_owned(),
            path: "/api/state".to_owned(),
            query: vec![("t".to_owned(), "wrong".to_owned())],
            host: "127.0.0.1:1".to_owned(),
            body: Vec::new(),
        };
        assert_eq!(route(&unauthorized, &shared).status, 401);
        let page = HttpRequest {
            method: "GET".to_owned(),
            path: "/".to_owned(),
            query: Vec::new(),
            host: "127.0.0.1:1".to_owned(),
            body: Vec::new(),
        };
        let response = route(&page, &shared);
        assert_eq!(response.status, 200);
        assert!(response.headers.iter().any(|(name, value)| name == "Content-Security-Policy" && value.contains("default-src 'self'")));
        assert!(String::from_utf8_lossy(&response.body).contains("<math-field"));
    }
}
