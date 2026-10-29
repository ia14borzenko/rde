//! Хост окна редактора: запуск отдельного окна браузера (Chrome app-mode / Chromium / Edge) на
//! loopback-странице моста и управление его жизненным циклом (P6, ADR-0022 §Решение 1).
//!
//! Почему отдельное окно браузера, а не встроенный webview: на целевом стенде есть рантайм
//! WebKitGTK, но нет dev-заголовков (`webkit2gtk-4.1.pc`) и `sudo` — `wry` не собирается; браузер
//! уже установлен, а MathLive (MIT) упакован в ассеты. Окно открывается в режиме приложения
//! (`--app=…`) — без адресной строки и вкладок, как отдельное приложение (MathType-класс UX).
//!
//! Деградация: если браузера нет — `EditorError::NoBrowser` (панель «Формула» остаётся с
//! LaTeX-полем); если браузер не умеет `--app` — fallback `xdg-open`/`open`/`start` в обычной
//! вкладке (окно тогда закрывает пользователь; RDE получает apply/cancel по мосту).
//!
//! Все параметры окна (тема, зум) обновляются через [`Bridge::update_state`] — страница
//! подхватывает их при следующем `/api/state`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};


use crate::protocol::{EditorState, EditorTheme, clamp_zoom};
use crate::server::Bridge;

/// Ошибка открытия окна редактора (все варианты — с человекочитаемым текстом для панели).
#[derive(Debug)]
pub enum EditorError {
    /// Не найден браузер (Chrome/Chromium/Edge) и нет fallback-браузера по умолчанию.
    NoBrowser(String),
    /// Мост не поднялся (порт/поток/токен).
    Bridge(String),
    /// Не удалось запустить процесс браузера.
    Spawn(String),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBrowser(message) | Self::Bridge(message) | Self::Spawn(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EditorError {}

/// Параметры открытия окна (LaTeX и оформление берутся из текущей панели формулы RDE).
#[derive(Clone, Debug)]
pub struct EditorConfig {
    /// Явный заголовок формулы (шапка окна; пусто — «Новая формула»).
    pub title: String,
    /// Имя проекта (шапка окна; необязательно).
    pub project: String,
    /// LaTeX-источник (source of truth), из него строится визуальная формула.
    pub latex: String,
    /// `true` — display, `false` — inline.
    pub display: bool,
    /// Тема окна (следует теме RDE).
    pub theme: EditorTheme,
    /// Зум 100–200 % (по умолчанию — сохранённый в настройках).
    pub zoom: Option<u32>,
    /// Переопределение браузера (диагностика/тесты; `none` — запретить запуск).
    pub browser_override: Option<String>,
    /// Переопределение файла настроек (тесты).
    pub prefs_path: Option<PathBuf>,
    /// Переопределение профиля браузера (тесты).
    pub profile_dir: Option<PathBuf>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            title: String::new(),
            project: String::new(),
            latex: String::new(),
            display: true,
            theme: EditorTheme::Dark,
            zoom: None,
            browser_override: None,
            prefs_path: None,
            profile_dir: None,
        }
    }
}

/// Сохранённые настройки окна (`~/.config/rde/formula-editor.toml`): зум переживает закрытие.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditorPrefs {
    /// Зум 100–200 %.
    #[serde(default = "default_zoom")]
    pub zoom: u32,
}

fn default_zoom() -> u32 {
    100
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self { zoom: default_zoom() }
    }
}

/// Запуск браузера: программа, аргументы и режим (app-окно или внешняя вкладка).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserLaunch {
    /// Программа (имя в `PATH` или путь).
    pub program: String,
    /// Аргументы командной строки.
    pub args: Vec<String>,
    /// `true` — браузер открывается как внешний (fallback), процесс не отслеживается.
    pub external: bool,
}

/// Открытое окно редактора: мост + процесс браузера (+ сохранённые настройки).
pub struct EditorWindow {
    bridge: Bridge,
    child: Option<Child>,
    external: bool,
    closed: bool,
    prefs_path: PathBuf,
    prefs: EditorPrefs,
}

impl EditorWindow {
    /// Открыть окно: поднять мост, найти браузер, запустить app-окно. Ошибки — без паники:
    /// панель покажет причину и останется с LaTeX-полем.
    pub fn open(config: EditorConfig) -> Result<Self, EditorError> {
        let prefs_path = config.prefs_path.clone().unwrap_or_else(default_prefs_path);
        let mut prefs = load_prefs(&prefs_path);
        let zoom = clamp_zoom(config.zoom.unwrap_or(prefs.zoom));
        prefs.zoom = zoom;
        let state = EditorState {
            title: config.title.clone(),
            latex: config.latex.clone(),
            display: config.display,
            theme: config.theme,
            zoom,
            project: config.project.clone(),
        };
        let bridge = Bridge::start(state).map_err(EditorError::Bridge)?;
        let profile_dir = config.profile_dir.clone().unwrap_or_else(default_profile_dir);
        let page_url = bridge.page_url();
        let launch = detect_launch(&page_url, &profile_dir, config.browser_override.as_deref()).ok_or_else(|| {
            EditorError::NoBrowser(
                "не найден браузер (Chrome/Chromium/Edge) — окно редактора не открыть; \
                 LaTeX можно править в поле панели «Формула»"
                    .to_owned(),
            )
        })?;
        if let Err(error) = std::fs::create_dir_all(&profile_dir) {
            return Err(EditorError::Spawn(format!("профиль браузера {}: {error}", profile_dir.display())));
        }
        let child = if launch.external {
            // Fallback: обычная вкладка браузера по умолчанию. Процесс-лаунчер сразу выходит —
            // окно живёт отдельно; закрытие определяем только по событиям моста.
            let mut command = Command::new(&launch.program);
            command.args(&launch.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            command.spawn().map_err(|error| EditorError::Spawn(format!("{}: {error}", launch.program)))?;
            None
        } else {
            let mut command = Command::new(&launch.program);
            command
                .args(&launch.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                // Chrome может писать шумные ошибки в stderr — не засоряем журнал RDE.
                .env("CHROME_LOG_FILE", "");
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // Своя группа процессов: при закрытии окна гасим всё дерево Chrome одним сигналом.
                command.process_group(0);
            }
            let child = command
                .spawn()
                .map_err(|error| EditorError::Spawn(format!("браузер {}: {error}", launch.program)))?;
            Some(child)
        };
        let _ = save_prefs(&prefs_path, &prefs);
        Ok(Self { bridge, child, external: launch.external, closed: false, prefs_path, prefs })
    }

    /// Мост (для диагностики/тестов).
    #[must_use]
    pub fn bridge(&self) -> &Bridge {
        &self.bridge
    }

    /// Открыто ли окно (событий/процесса ещё нет).
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.closed
    }

    /// Внешний fallback-режим (браузер по умолчанию: окно закрывает пользователь).
    #[must_use]
    pub fn is_external(&self) -> bool {
        self.external
    }

    /// Забрать накопленные события страницы (Applied/Cancelled/Zoom) и обнаружить закрытие окна.
    ///
    /// Вызывается из GUI-кадра; не блокирует. Если браузер закрыт без «Применить»/«Отмена» —
    /// возвращается [`crate::protocol::EditorEvent::Cancelled`] (изменений нет).
    pub fn poll(&mut self) -> Vec<crate::protocol::EditorEvent> {
        let mut events = Vec::new();
        if self.closed {
            return events;
        }
        for event in self.bridge.drain() {
            if let crate::protocol::EditorEvent::Zoom { zoom } = &event {
                self.prefs.zoom = clamp_zoom(*zoom);
                let _ = save_prefs(&self.prefs_path, &self.prefs);
            }
            events.push(event);
        }
        let terminal = events
            .iter()
            .any(|event| matches!(event, crate::protocol::EditorEvent::Applied { .. } | crate::protocol::EditorEvent::Cancelled));
        if !terminal && let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // Окно закрыто пользователем без кнопок — это «Отмена».
                    self.closed = true;
                    self.bridge.shutdown();
                    events.push(crate::protocol::EditorEvent::Cancelled);
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
        events
    }

    /// Обновить тему окна (страница подхватит через `/api/state`).
    pub fn set_theme(&self, theme: EditorTheme) {
        self.bridge.update_state(|state| state.theme = theme);
    }

    /// Обновить зум окна (страница подхватит через `/api/state`).
    pub fn set_zoom(&mut self, zoom: u32) {
        let zoom = clamp_zoom(zoom);
        self.prefs.zoom = zoom;
        self.bridge.update_state(|state| state.zoom = zoom);
        let _ = save_prefs(&self.prefs_path, &self.prefs);
    }

    /// Текущий зум (для панели/настроек).
    #[must_use]
    pub fn zoom(&self) -> u32 {
        self.prefs.zoom
    }

    /// Обновить заголовок/LaTeX в открытом окне (при смене формулы без переоткрытия).
    pub fn update_content(&self, title: &str, latex: &str, display: bool) {
        self.bridge.update_state(|state| {
            state.title = title.to_owned();
            state.latex = latex.to_owned();
            state.display = display;
        });
    }

    /// Закрыть окно и освободить ресурсы (идемпотентно; вызывается из `Drop`).
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.kill_child();
        self.bridge.shutdown();
    }

    fn kill_child(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        // Сначала вежливо: SIGKILL через стандартный API (Chrome завершает дочерние процессы сам).
        let _ = child.kill();
        let _ = child.wait();
        #[cfg(unix)]
        {
            // Добить группу процессов (Chrome spawns zygote/renderer/GPU children).
            use std::process::Command as ProcessCommand;
            let _ = ProcessCommand::new("kill").arg("-KILL").arg(format!("-{}", child.id())).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Есть ли графическая сессия (для авто-открытия окна по клику в тексте и диагностики).
#[must_use]
pub fn display_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        set("DISPLAY") || set("WAYLAND_DISPLAY")
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

// ── настройки и пути ────────────────────────────────────────────────────────

/// Файл настроек окна: `$XDG_CONFIG_HOME/rde/formula-editor.toml` или `~/.config/rde/…`.
#[must_use]
pub fn default_prefs_path() -> PathBuf {
    config_root().join("formula-editor.toml")
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rde")
}

/// Профиль браузера окна редактора (`$XDG_CACHE_HOME/rde/formula-editor`): Chrome хранит в нём
/// кэш/геометрию окна; общий профиль пользователя не трогаем.
#[must_use]
pub fn default_profile_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("rde")
        .join("formula-editor-profile")
}

/// Прочитать настройки (нет файла/ошибка — значения по умолчанию; окно не должно падать).
#[must_use]
pub fn load_prefs(path: &Path) -> EditorPrefs {
    let Ok(text) = std::fs::read_to_string(path) else { return EditorPrefs::default() };
    let mut prefs: EditorPrefs = toml::from_str(&text).unwrap_or_default();
    prefs.zoom = clamp_zoom(prefs.zoom);
    prefs
}

/// Записать настройки (ошибки игнорируются: настройки не критичны для работы).
pub fn save_prefs(path: &Path, prefs: &EditorPrefs) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = format!("# Окно визуального редактора формул RDE (P6, ADR-0022)\nzoom = {}\n", clamp_zoom(prefs.zoom));
    std::fs::write(path, text)
}

// ── поиск/запуск браузера ───────────────────────────────────────────────────

/// Кандидаты app-режима по платформам (порядок — приоритет).
const LINUX_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "brave-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "vivaldi",
    "opera",
];
const WINDOWS_CANDIDATES: &[&str] = &["msedge.exe", "chrome.exe", "brave.exe", "chromium.exe"];
const MACOS_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];

/// Найти браузер и собрать команду запуска (по переменным окружения; `override` — приоритетно).
#[must_use]
pub fn detect_launch(page_url: &str, profile_dir: &Path, override_name: Option<&str>) -> Option<BrowserLaunch> {
    let override_name = override_name.map(str::trim).filter(|name| !name.is_empty());
    if override_name.is_some_and(|name| name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("off")) {
        return None;
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    detect_launch_in(override_name, &path_env, page_url, profile_dir, current_platform(), std::env::var("HOME").ok())
}

/// Платформа для выбора кандидатов (вынесено ради тестируемости).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Linux,
    Windows,
    MacOs,
}

pub(crate) fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

/// Чистая проверяемая версия [`detect_launch`]: поиск программы по `path_value` по платформе.
pub(crate) fn detect_launch_in(
    override_name: Option<&str>,
    path_value: &str,
    page_url: &str,
    profile_dir: &Path,
    platform: Platform,
    home: Option<String>,
) -> Option<BrowserLaunch> {
    // 1) Явное переопределение: путь или имя в PATH.
    if let Some(name) = override_name {
        let program = find_program(name, path_value, home.as_deref())?;
        return Some(app_launch(program, page_url, profile_dir, platform));
    }
    // 2) Штатные браузеры платформы, затем 3) fallback «браузер по умолчанию».
    let candidates: &[&str] = match platform {
        Platform::Linux => LINUX_CANDIDATES,
        Platform::Windows => WINDOWS_CANDIDATES,
        Platform::MacOs => MACOS_CANDIDATES,
    };
    for candidate in candidates {
        if let Some(program) = find_program(candidate, path_value, home.as_deref()) {
            return Some(app_launch(program, page_url, profile_dir, platform));
        }
    }
    external_launch(page_url, platform, path_value, home.as_deref())
}

/// Собрать команду app-окна (`--app=` + изолированный профиль + запрет сети/первого запуска).
fn app_launch(program: String, page_url: &str, profile_dir: &Path, platform: Platform) -> BrowserLaunch {
    let mut args = vec![
        format!("--app={page_url}"),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--disable-extensions".to_owned(),
        // Рантайм офлайновый: гасим фоновые сетевые службы браузера (sync/обновления/телеметрия).
        "--disable-background-networking".to_owned(),
        "--disable-component-update".to_owned(),
        "--disable-sync".to_owned(),
        "--disable-default-apps".to_owned(),
        "--disable-client-side-phishing-detection".to_owned(),
        "--no-service-autorun".to_owned(),
        "--disable-features=Translate,MediaRouter".to_owned(),
        "--window-size=1000,640".to_owned(),
    ];
    if platform == Platform::Linux {
        // WM_CLASS: отдельная иконка/группировка окна в панели задач (X11/Wayland).
        args.push("--class=RdeFormulaEditor".to_owned());
    }
    BrowserLaunch { program, args, external: false }
}

/// Fallback: браузер по умолчанию (Android-подобный минимализм — окно закрывает пользователь).
fn external_launch(page_url: &str, platform: Platform, path_value: &str, home: Option<&str>) -> Option<BrowserLaunch> {
    let (name, args): (&str, Vec<String>) = match platform {
        Platform::Linux => ("xdg-open", vec![page_url.to_owned()]),
        Platform::MacOs => ("open", vec![page_url.to_owned()]),
        Platform::Windows => ("cmd", vec!["/C".to_owned(), "start".to_owned(), String::new(), page_url.to_owned()]),
    };
    find_program(name, path_value, home).map(|program| BrowserLaunch { program, args, external: true })
}

/// Найти программу: абсолютный путь — проверить исполняемость; имя — поиск по `PATH` (+`$HOME`).
fn find_program(name: &str, path_value: &str, home: Option<&str>) -> Option<String> {
    if name.contains('/') || name.contains('\\') {
        return is_executable(Path::new(name)).then(|| name.to_owned());
    }
    for directory in path_value.split(if cfg!(target_os = "windows") { ';' } else { ':' }) {
        if directory.is_empty() {
            continue;
        }
        let candidate = Path::new(directory).join(name);
        if is_executable(&candidate) {
            return Some(candidate.display().to_string());
        }
    }
    let home = home?;
    for suffix in [".local/bin", "bin"] {
        let candidate = Path::new(home).join(suffix).join(name);
        if is_executable(&candidate) {
            return Some(candidate.display().to_string());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).map(|meta| meta.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-formula-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("временный каталог");
        dir
    }

    #[test]
    fn detects_chrome_from_path_and_builds_app_launch() {
        let bin = temp_dir("bin");
        let chrome = bin.join("google-chrome");
        std::fs::write(&chrome, "#!/bin/sh\nexit 0\n").expect("скрипт");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&chrome, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let launch = detect_launch_in(
            None,
            bin.to_str().expect("path"),
            "http://127.0.0.1:1234/?t=abc",
            Path::new("/tmp/profile"),
            Platform::Linux,
            None,
        )
        .expect("браузер найден");
        assert!(!launch.external);
        assert!(launch.program.ends_with("google-chrome"));
        assert!(launch.args.iter().any(|arg| arg == "--app=http://127.0.0.1:1234/?t=abc"));
        assert!(launch.args.iter().any(|arg| arg.starts_with("--user-data-dir=/tmp/profile")));
        assert!(launch.args.iter().any(|arg| arg == "--class=RdeFormulaEditor"));
        let _ = std::fs::remove_dir_all(&bin);
    }

    #[test]
    fn override_wins_and_none_disables() {
        let bin = temp_dir("override");
        let custom = bin.join("my-browser");
        std::fs::write(&custom, "#!/bin/sh\nexit 0\n").expect("скрипт");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&custom, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let launch = detect_launch_in(
            Some("my-browser"),
            bin.to_str().expect("path"),
            "http://127.0.0.1:1/",
            Path::new("/tmp/profile"),
            Platform::Linux,
            None,
        )
        .expect("override");
        assert!(launch.program.ends_with("my-browser"));

        let page = "http://127.0.0.1:1/";
        let disabled = detect_launch_in(Some("off"), bin.to_str().expect("path"), page, Path::new("/tmp/profile"), Platform::Linux, None);
        assert!(disabled.is_none());
        let _ = std::fs::remove_dir_all(&bin);
    }

    #[test]
    fn falls_back_to_default_browser_when_no_chrome() {
        let bin = temp_dir("xdg");
        let xdg = bin.join("xdg-open");
        std::fs::write(&xdg, "#!/bin/sh\nexit 0\n").expect("скрипт");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&xdg, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let launch = detect_launch_in(None, bin.to_str().expect("path"), "http://127.0.0.1:9/", Path::new("/tmp/profile"), Platform::Linux, None)
            .expect("fallback на браузер по умолчанию");
        assert!(launch.external);
        assert!(launch.program.ends_with("xdg-open"));
        assert_eq!(launch.args, vec!["http://127.0.0.1:9/".to_owned()]);
        let _ = std::fs::remove_dir_all(&bin);
    }

    #[test]
    fn platform_fallbacks_use_system_openers() {
        use std::os::unix::fs::PermissionsExt;
        let bin = temp_dir("openers");
        for name in ["xdg-open", "open", "cmd"] {
            let path = bin.join(name);
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("скрипт");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let path = bin.to_str().expect("path");
        let url = "http://127.0.0.1:7/";
        let mac = detect_launch_in(None, path, url, Path::new("/tmp/profile"), Platform::MacOs, None).expect("macOS fallback");
        assert!(mac.external && mac.program.ends_with("open"), "macOS: open: {mac:?}");
        let win = detect_launch_in(None, path, url, Path::new("/tmp/profile"), Platform::Windows, None).expect("Windows fallback");
        assert!(win.external && win.program.ends_with("cmd"), "Windows: cmd: {win:?}");
        assert_eq!(win.args.first().map(String::as_str), Some("/C"));
        assert_eq!(win.args.last().map(String::as_str), Some(url));
        let linux = detect_launch_in(None, path, url, Path::new("/tmp/profile"), Platform::Linux, None).expect("Linux fallback");
        assert!(linux.external && linux.program.ends_with("xdg-open"), "Linux: xdg-open: {linux:?}");
        let _ = std::fs::remove_dir_all(&bin);
    }

    #[test]
    fn prefs_roundtrip_and_clamping() {
        let dir = temp_dir("prefs");
        let path = dir.join("formula-editor.toml");
        assert_eq!(load_prefs(&path), EditorPrefs::default());
        save_prefs(&path, &EditorPrefs { zoom: 150 }).expect("запись");
        assert_eq!(load_prefs(&path), EditorPrefs { zoom: 150 });
        std::fs::write(&path, "zoom = 999\n").expect("запись");
        assert_eq!(load_prefs(&path).zoom, 200, "зум клампится при чтении");
        std::fs::write(&path, "не toml").expect("запись");
        assert_eq!(load_prefs(&path), EditorPrefs::default(), "битые настройки — дефолт");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Жизненный цикл на «фейковом браузере» (shell-скрипт вместо Chrome): окно считается
    /// открытым, а закрытие процесса без Apply трактуется как «Отмена».
    #[cfg(unix)]
    #[test]
    fn window_lifecycle_with_fake_browser() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("window");
        let fake = dir.join("fake-browser");
        std::fs::write(&fake, "#!/bin/sh\nsleep 2\n").expect("скрипт");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let config = EditorConfig {
            latex: "\\sigma=\\frac{F}{A}".to_owned(),
            title: "σ".to_owned(),
            browser_override: Some(fake.display().to_string()),
            prefs_path: Some(dir.join("prefs.toml")),
            profile_dir: Some(dir.join("profile")),
            ..EditorConfig::default()
        };
        let mut window = EditorWindow::open(config).expect("окно открылось");
        assert!(window.is_open());
        assert!(!window.is_external());
        assert_eq!(window.bridge().state().latex, "\\sigma=\\frac{F}{A}");
        // Мост жив: страница отдаётся по loopback (без сети).
        assert!(window.bridge().page_url().starts_with("http://127.0.0.1:"));
        // Процесс-фейк живёт 2 с; принудительно закрываем — событий Applied нет.
        window.close();
        assert!(!window.is_open());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
