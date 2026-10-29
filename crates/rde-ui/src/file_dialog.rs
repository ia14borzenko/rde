//! Системный диалог выбора файла/папки («Проводник…») — ADR-0017.
//!
//! Зачем: поля путей в диалогах RDE нельзя заставлять пользователя набирать руками.
//! Кнопка «Проводник…» открывает **системное** окно выбора, а ручной ввод остаётся рабочим.
//!
//! Backend — внешние CLI (`kdialog` в KDE, `zenity` в GTK/прочих окружениях): ноль новых
//! crate-зависимостей, внешний вид — как в ОС, поведение тестируется подменой backend'а (DI).
//! Альтернативы (`rfd`/xdg-desktop-portal, своё окно на egui) разобраны в ADR-0017.
//!
//! Ключевые свойства:
//! - **UI не блокируется**: диалог крутится в отдельном потоке, результат забирается в кадре
//!   `RdeApp::ui` через [`FileDialogService::poll`]; по завершении поток просит перерисовку.
//! - **graceful degradation**: нет графической сессии / нет `kdialog`/`zenity` / CLI упал →
//!   `Err(…)`, кнопка не «съедает» ввод; RdeApp пишет в журнал «введите путь вручную», поле
//!   остаётся редактируемым.
//! - отмена пользователем — `Ok(None)` (не ошибка, поле не меняется).

use std::path::PathBuf;
use std::sync::mpsc;

/// Подсказка для журнала/диалога при деградации: что делать пользователю.
pub const MANUAL_HINT: &str = "введите путь вручную";

/// Человекочитаемая ошибка деградации backend'а (используется как есть в журнале и в диалоге).
pub fn degradation(message: impl std::fmt::Display) -> String {
    format!("диалог выбора недоступен: {message} — {MANUAL_HINT}")
}

/// Фильтр файлов: человекочитаемое имя + расширения без точки (`docx`, `pdf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl FileFilter {
    /// `FileFilter::new("Документы Word", &["docx"])`.
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        let extensions = extensions
            .iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect();
        Self { name: name.into(), extensions }
    }

    /// Глобы масок: `["*.docx"]`.
    pub fn masks(&self) -> Vec<String> {
        self.extensions.iter().map(|ext| format!("*.{ext}")).collect()
    }
}

/// Что просим у системы.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// Выбрать существующий файл.
    File { filters: Vec<FileFilter> },
    /// Выбрать существующий каталог.
    Folder,
    /// Выбрать путь сохранения (файл может ещё не существовать).
    Save { default_name: String, filters: Vec<FileFilter> },
}

impl PickKind {
    /// Файл с фильтром по расширениям.
    pub fn file(name: &str, extensions: &[&str]) -> Self {
        PickKind::File { filters: vec![FileFilter::new(name, extensions)] }
    }

    /// Любой файл (без фильтра).
    pub fn any_file() -> Self {
        PickKind::File { filters: Vec::new() }
    }
}

/// Запрос к backend'у: что показать и с какого каталога начать.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub title: String,
    pub start_dir: Option<PathBuf>,
    pub kind: PickKind,
}

impl PickRequest {
    pub fn new(title: impl Into<String>, kind: PickKind) -> Self {
        Self { title: title.into(), start_dir: None, kind }
    }

    pub fn with_start(mut self, start: impl Into<PathBuf>) -> Self {
        let start = start.into();
        if !start.as_os_str().is_empty() {
            self.start_dir = Some(start);
        }
        self
    }

    /// Каталог, с которого начинается диалог: стартовый путь → HOME → текущий каталог.
    fn effective_start(&self) -> PathBuf {
        if let Some(start) = &self.start_dir {
            if start.is_dir() {
                return start.clone();
            }
            if let Some(parent) = start.parent()
                && parent.is_dir()
            {
                return parent.to_path_buf();
            }
        }
        std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_dir()).unwrap_or_else(|| PathBuf::from("."))
    }
}

/// «Адрес» поля пути в UI: куда вернуть выбор. Диалоги открываются по одному, поэтому этого
/// достаточно, чтобы разложить результат по нужному полю.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathTarget {
    /// «Открыть проект» — каталог с `rde.toml`.
    OpenProject,
    /// «Импорт .docx» — файл `.docx`.
    ImportDocx,
    /// «Импорт ревью» — `.docx` после правок в Word.
    ImportReview,
    /// «Импорт цитат» — `.bib`/`.ris`/`.json`.
    ImportCitations,
    /// «Новый проект» — каталог проекта.
    NewProjectDir,
    /// «Настройки приложения → программа ACP-агента» — исполняемый файл.
    AgentProgram,
    /// Файл источника (прикрепить/заменить), id объекта-источника.
    SourceFile(String),
    /// Путь репозитория (каталог), id объекта-репозитория.
    RepositoryPath(String),
    /// Файл данных артефакта (`inputs/…`).
    ArtifactInputFile,
}

/// Backend системного выбора. Реализации: [`CliPicker`] (kdialog/zenity),
/// [`UnavailablePicker`] (явная деградация) и тестовый фейк (`test_support::FakePicker`).
pub trait PickerBackend: Send + Sync {
    /// `Ok(Some(path))` — выбрано, `Ok(None)` — отменено, `Err` — backend недоступен/упал.
    fn pick(&self, request: &PickRequest) -> Result<Option<PathBuf>, String>;
    /// Короткое имя backend'а для журнала/диагностики.
    fn name(&self) -> &'static str;
}

/// Какое CLI-семейство использовать.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogFlavor {
    KDialog,
    Zenity,
}

/// Backend через внешнюю CLI-утилиту (`kdialog`/`zenity`): аргументы строятся отдельно
/// (тестируется без запуска), запуск — в фоновом потоке сервиса.
#[derive(Clone, Debug)]
pub struct CliPicker {
    pub flavor: DialogFlavor,
    pub program: PathBuf,
}

impl CliPicker {
    pub fn new(flavor: DialogFlavor, program: impl Into<PathBuf>) -> Self {
        Self { flavor, program: program.into() }
    }

    /// Аргументы командной строки (без имени программы). Чистая функция — покрыта тестами.
    pub fn args(&self, request: &PickRequest) -> Vec<String> {
        let start = request.effective_start();
        match self.flavor {
            DialogFlavor::KDialog => {
                let mut args = vec!["--title".to_owned(), request.title.clone()];
                let start_text = match request.kind {
                    PickKind::Save { ref default_name, .. } => {
                        start.join(if default_name.trim().is_empty() { "untitled" } else { default_name.trim() }).display().to_string()
                    }
                    _ => start.display().to_string(),
                };
                let filter = kdialog_filter(&request.kind);
                match request.kind {
                    PickKind::File { .. } => {
                        args.push("--getopenfilename".to_owned());
                        args.push(start_text);
                    }
                    PickKind::Folder => {
                        args.push("--getexistingdirectory".to_owned());
                        args.push(start_text);
                    }
                    PickKind::Save { .. } => {
                        args.push("--getsavefilename".to_owned());
                        args.push(start_text);
                    }
                }
                if let Some(filter) = filter {
                    args.push(filter);
                }
                args
            }
            DialogFlavor::Zenity => {
                let mut args = vec![
                    "--file-selection".to_owned(),
                    format!("--title={}", request.title),
                ];
                match &request.kind {
                    PickKind::Folder => args.push("--directory".to_owned()),
                    PickKind::Save { default_name, .. } => {
                        args.push("--save".to_owned());
                        args.push("--confirm-overwrite".to_owned());
                        let name = if default_name.trim().is_empty() { "untitled" } else { default_name.trim() };
                        args.push(format!("--filename={}", start.join(name).display()));
                    }
                    PickKind::File { filters } => {
                        // Начинаем «внутри» каталога: зенити ждёт путь-файл для --filename,
                        // завершающий «/» открывает каталог, не выбирая файл.
                        args.push(format!("--filename={}/", start.display()));
                        for filter in filters {
                            let masks = filter.masks().join(" ");
                            if !masks.is_empty() {
                                args.push(format!("--file-filter={} | {}", filter.name, masks));
                            }
                        }
                    }
                }
                if matches!(request.kind, PickKind::Folder) {
                    args.push(format!("--filename={}/", start.display()));
                }
                args
            }
        }
    }
}

/// Фильтр kdialog — одна маска с описанием: `Документы Word (*.docx *.doc)`.
fn kdialog_filter(kind: &PickKind) -> Option<String> {
    let filters = match kind {
        PickKind::File { filters } | PickKind::Save { filters, .. } => filters,
        PickKind::Folder => return None,
    };
    if filters.is_empty() {
        return None;
    }
    let masks: Vec<String> = filters.iter().flat_map(FileFilter::masks).collect();
    if masks.is_empty() {
        return None;
    }
    let names: Vec<&str> = filters.iter().map(|filter| filter.name.as_str()).collect();
    Some(format!("{} ({})", names.join(" / "), masks.join(" ")))
}

impl PickerBackend for CliPicker {
    fn pick(&self, request: &PickRequest) -> Result<Option<PathBuf>, String> {
        let output = std::process::Command::new(&self.program)
            .args(self.args(request))
            .output()
            .map_err(|error| degradation(format!("не удалось запустить {}: {error}", self.program.display())))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        match output.status.code() {
            // Успех: путь в stdout (пустая строка = пользователь нажал OK без выбора).
            Some(0) => {
                let path = stdout.trim();
                if path.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(PathBuf::from(path)))
                }
            }
            // Отмена — не ошибка.
            Some(1) => Ok(None),
            code => {
                let detail = stderr.trim();
                let reason = if detail.is_empty() {
                    format!("{} завершился с кодом {:?}", self.program.display(), code)
                } else {
                    format!("{}: {}", self.program.display(), detail)
                };
                Err(degradation(reason))
            }
        }
    }

    fn name(&self) -> &'static str {
        match self.flavor {
            DialogFlavor::KDialog => "kdialog",
            DialogFlavor::Zenity => "zenity",
        }
    }
}

/// Backend, который всегда честно сообщает о деградации (нет графики/CLI).
#[derive(Clone, Debug)]
pub struct UnavailablePicker {
    pub reason: String,
}

impl UnavailablePicker {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl PickerBackend for UnavailablePicker {
    fn pick(&self, _request: &PickRequest) -> Result<Option<PathBuf>, String> {
        Err(degradation(self.reason.clone()))
    }

    fn name(&self) -> &'static str {
        "недоступен"
    }
}

/// Есть ли графическая сессия (переменные XDG/wayland не пусты).
pub fn graphical_session_available() -> bool {
    let has = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    has("DISPLAY") || has("WAYLAND_DISPLAY")
}

/// Подсказка о текущем backend'е для тултипа кнопки (без запуска диалога).
pub fn backend_hint() -> &'static str {
    if !graphical_session_available() {
        return "Нет графической сессии — диалог недоступен, введите путь вручную";
    }
    if find_in_path("kdialog").is_some() || find_in_path("zenity").is_some() {
        "Выбрать файл/каталог в системном диалоге"
    } else {
        "kdialog/zenity не найдены — диалог недоступен, введите путь вручную"
    }
}

/// Найти исполняемый файл в `PATH`.
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata()
                    && meta.permissions().mode() & 0o111 == 0
                {
                    continue;
                }
            }
            return Some(candidate);
        }
    }
    None
}

/// Автовыбор backend'а: kdialog в KDE, иначе zenity, иначе второй из пары; без графики — деградация.
pub fn detect_backend() -> Box<dyn PickerBackend> {
    if !graphical_session_available() {
        return Box::new(UnavailablePicker::new(
            "нет графической сессии (DISPLAY/WAYLAND_DISPLAY не заданы)",
        ));
    }
    let kdialog = find_in_path("kdialog");
    let zenity = find_in_path("zenity");
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|desktop| {
            let desktop = desktop.to_ascii_lowercase();
            desktop.contains("kde") || desktop.contains("plasma")
        })
        .unwrap_or(false);
    match (kdialog, zenity) {
        (Some(kdialog), _) if kde => Box::new(CliPicker::new(DialogFlavor::KDialog, kdialog)),
        (_, Some(zenity)) => Box::new(CliPicker::new(DialogFlavor::Zenity, zenity)),
        (Some(kdialog), None) => Box::new(CliPicker::new(DialogFlavor::KDialog, kdialog)),
        (None, None) => Box::new(UnavailablePicker::new("не найдены kdialog и zenity")),
    }
}

/// Завершённый запрос backend'а: адрес поля + результат + токен поколения запроса.
///
/// Токен нужен, чтобы «поздний» выбор от запроса, чей диалог уже закрыт/пересоздан, не попал
/// в поле нового диалога (см. [`FileDialogService::abandon`]).
#[derive(Clone, Debug)]
pub struct PickDone {
    pub target: PathTarget,
    pub token: u64,
    pub result: Result<Option<PathBuf>, String>,
}

/// Неблокирующий сервис: запускает выбор в фоновом потоке и отдаёт результаты в кадре UI.
pub struct FileDialogService {
    backend: std::sync::Arc<dyn PickerBackend>,
    tx: mpsc::Sender<PickDone>,
    rx: mpsc::Receiver<PickDone>,
    /// Текущий запрос «в полёте»: адрес поля + токен поколения.
    in_flight: Option<(PathTarget, u64)>,
    /// Счётчик поколений запросов (для отбрасывания устаревших результатов).
    next_token: u64,
}

impl FileDialogService {
    /// Сервис с автоматически выбранным backend'ом (kdialog/zenity/деградация).
    pub fn new() -> Self {
        Self::with_backend(std::sync::Arc::from(detect_backend()))
    }

    /// Сервис с явным backend'ом (DI: тесты, скриншоты, альтернативный редактор).
    pub fn with_backend(backend: std::sync::Arc<dyn PickerBackend>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { backend, tx, rx, in_flight: None, next_token: 0 }
    }

    /// Подменить backend (тесты kittest: клик по кнопке должен вызвать фейк).
    pub fn set_backend(&mut self, backend: std::sync::Arc<dyn PickerBackend>) {
        self.backend = backend;
    }

    /// Имя действующего backend'а (`kdialog`/`zenity`/`недоступен`/`fake`).
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Запрос в полёте (для индикатора на кнопке «Проводник…»).
    pub fn in_flight(&self) -> Option<&PathTarget> {
        self.in_flight.as_ref().map(|(target, _)| target)
    }

    /// Забыть запрос «в полёте» для адреса: его поздний результат будет отброшен по токену.
    ///
    /// Нужно при закрытии диалога во время выбора и при повторном открытии диалога: системное
    /// окно могло остаться открытым, но его выбор уже не должен менять новое поле.
    pub fn abandon(&mut self, target: &PathTarget) {
        if self.in_flight.as_ref().is_some_and(|(current, _)| current == target) {
            self.in_flight = None;
        }
    }

    /// Запустить выбор в фоне. `false` — уже открыт другой диалог (одно окно за раз).
    pub fn spawn(&mut self, ctx: &egui::Context, target: PathTarget, request: PickRequest) -> bool {
        if self.in_flight.is_some() {
            return false;
        }
        let backend = std::sync::Arc::clone(&self.backend);
        let sender = self.tx.clone();
        let ctx = ctx.clone();
        let result_target = target.clone();
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.in_flight = Some((target, token));
        std::thread::spawn(move || {
            let result = backend.pick(&request);
            let _ = sender.send(PickDone { target: result_target, token, result });
            // Разбудить UI: результат нужно разложить по полю в ближайшем кадре.
            ctx.request_repaint();
        });
        true
    }

    /// Забрать готовые результаты (неблокирующе); снимает «в полёте» у совпавшего адреса.
    pub fn poll(&mut self) -> Vec<PickDone> {
        let mut done = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            // Принимаем только результат текущего поколения: поздний ответ отменённого/переоткрытого
            // диалога молча отбрасывается (иначе он подменил бы путь в новом диалоге).
            let current = self
                .in_flight
                .as_ref()
                .is_some_and(|(target, token)| target == &item.target && *token == item.token);
            if !current {
                continue;
            }
            self.in_flight = None;
            done.push(item);
        }
        done
    }
}

impl Default for FileDialogService {
    fn default() -> Self {
        Self::new()
    }
}

/// Тестовые/скриншотные фейки backend'а: без реальных системных окон.
#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::Mutex;

    /// Заранее заданный сценарий ответа backend'а.
    #[derive(Clone, Debug)]
    pub enum ScriptedReply {
        Selected(PathBuf),
        Cancelled,
        Failed(String),
    }

    /// Фейк: возвращает заданные ответы по очереди и запоминает запросы.
    pub struct FakePicker {
        replies: Mutex<std::collections::VecDeque<ScriptedReply>>,
        requests: Mutex<Vec<PickRequest>>,
        delay: std::time::Duration,
    }

    impl FakePicker {
        pub fn new(replies: Vec<ScriptedReply>) -> Self {
            Self { replies: Mutex::new(replies.into()), requests: Mutex::new(Vec::new()), delay: std::time::Duration::ZERO }
        }

        /// Один ответ на все запросы.
        pub fn once(reply: ScriptedReply) -> Self {
            Self::new(vec![reply])
        }

        /// Задержка ответа — проверка «UI не блокируется».
        pub fn with_delay(mut self, delay: std::time::Duration) -> Self {
            self.delay = delay;
            self
        }

        /// Сколько запросов пришло (и каких).
        pub fn requests(&self) -> Vec<PickRequest> {
            self.requests.lock().expect("fake picker").clone()
        }

        pub fn last_request(&self) -> Option<PickRequest> {
            self.requests.lock().expect("fake picker").last().cloned()
        }
    }

    impl PickerBackend for FakePicker {
        fn pick(&self, request: &PickRequest) -> Result<Option<PathBuf>, String> {
            self.requests.lock().expect("fake picker").push(request.clone());
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let reply = self.replies.lock().expect("fake picker").pop_front();
            match reply {
                Some(ScriptedReply::Selected(path)) => Ok(Some(path)),
                Some(ScriptedReply::Cancelled) => Ok(None),
                Some(ScriptedReply::Failed(error)) => Err(error),
                // Сценарий исчерпан — ведём себя как отмена (тест не должен зависеть от порядка).
                None => Ok(None),
            }
        }

        fn name(&self) -> &'static str {
            "fake"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn request(kind: PickKind) -> PickRequest {
        PickRequest { title: "Тестовый выбор".to_owned(), start_dir: Some(PathBuf::from("/tmp")), kind }
    }

    /// kdialog: маска с описанием, стартовый каталог, команда под вид выбора.
    #[test]
    fn kdialog_args_cover_file_folder_and_save() {
        let picker = CliPicker::new(DialogFlavor::KDialog, "/usr/bin/kdialog");
        let file = picker.args(&request(PickKind::file("Документы Word", &["docx", ".DOC"])));
        assert_eq!(
            file,
            vec![
                "--title".to_owned(),
                "Тестовый выбор".to_owned(),
                "--getopenfilename".to_owned(),
                "/tmp".to_owned(),
                "Документы Word (*.docx *.doc)".to_owned(),
            ]
        );
        let folder = picker.args(&request(PickKind::Folder));
        assert_eq!(folder[2], "--getexistingdirectory");
        assert_eq!(folder.len(), 4, "у каталога нет фильтров: {folder:?}");
        let save = picker.args(&request(PickKind::Save {
            default_name: "отчёт.docx".to_owned(),
            filters: vec![FileFilter::new("Word", &["docx"])],
        }));
        assert_eq!(save[2], "--getsavefilename");
        assert_eq!(save[3], "/tmp/отчёт.docx");
        assert_eq!(save[4], "Word (*.docx)");
        // Без фильтров — просто выбор файла.
        let any = picker.args(&request(PickKind::any_file()));
        assert_eq!(any.len(), 4, "{any:?}");
        assert_eq!(picker.name(), "kdialog");
    }

    /// zenity: --directory/--save/--file-filter и стартовый каталог с завершающим «/».
    #[test]
    fn zenity_args_cover_file_folder_and_save() {
        let picker = CliPicker::new(DialogFlavor::Zenity, "/usr/bin/zenity");
        let file = picker.args(&request(PickKind::File {
            filters: vec![FileFilter::new("Цитаты", &["bib", "ris"]), FileFilter::new("Word", &["docx"])],
        }));
        assert_eq!(file[0], "--file-selection");
        assert_eq!(file[1], "--title=Тестовый выбор");
        assert!(file.contains(&"--filename=/tmp/".to_owned()), "{file:?}");
        assert!(file.contains(&"--file-filter=Цитаты | *.bib *.ris".to_owned()), "{file:?}");
        assert!(file.contains(&"--file-filter=Word | *.docx".to_owned()), "{file:?}");
        let folder = picker.args(&request(PickKind::Folder));
        assert!(folder.contains(&"--directory".to_owned()), "{folder:?}");
        assert!(folder.contains(&"--filename=/tmp/".to_owned()), "{folder:?}");
        let save = picker.args(&request(PickKind::Save { default_name: "a.docx".to_owned(), filters: Vec::new() }));
        assert!(save.contains(&"--save".to_owned()), "{save:?}");
        assert!(save.contains(&"--confirm-overwrite".to_owned()), "{save:?}");
        assert!(save.contains(&"--filename=/tmp/a.docx".to_owned()), "{save:?}");
        assert_eq!(picker.name(), "zenity");
    }

    /// Стартовый путь: несуществующий файл → его каталог; пусто → HOME/«.».
    #[test]
    fn effective_start_dir_is_robust() {
        let request = PickRequest { title: "t".to_owned(), start_dir: Some(PathBuf::from("/tmp/нет-такого-файла.docx")), kind: PickKind::Folder };
        assert_eq!(request.effective_start(), PathBuf::from("/tmp"));
        let request = PickRequest { title: "t".to_owned(), start_dir: None, kind: PickKind::Folder };
        let start = request.effective_start();
        assert!(start.is_dir(), "дефолтный старт должен быть каталогом: {start:?}");
    }

    /// Код 0 + stdout → путь; код 1 → отмена; код 2 + stderr → деградация с подсказкой.
    #[test]
    fn cli_exit_codes_map_to_selected_cancelled_and_error() {
        let dir = temp_dir("codes");
        let selected = dir.join("выбор.docx");
        let picker = CliPicker::new(DialogFlavor::Zenity, script(&dir, "select.sh", &format!("printf '%s\\n' '{}'", selected.display())));
        assert_eq!(picker.pick(&request(PickKind::any_file())).unwrap(), Some(selected));
        let cancel = CliPicker::new(DialogFlavor::Zenity, script(&dir, "cancel.sh", "exit 1"));
        assert_eq!(cancel.pick(&request(PickKind::any_file())).unwrap(), None, "отмена — не ошибка");
        let broken = CliPicker::new(DialogFlavor::Zenity, script(&dir, "broken.sh", "echo 'нет дисплея' >&2\nexit 2"));
        let error = broken.pick(&request(PickKind::any_file())).expect_err("ошибка backend'а");
        assert!(error.contains("нет дисплея"), "{error}");
        assert!(error.contains(MANUAL_HINT), "деградация подсказывает ручной ввод: {error}");
        // Несуществующая программа — тоже деградация, а не паника.
        let missing = CliPicker::new(DialogFlavor::KDialog, dir.join("нет-такой-программы"));
        let error = missing.pick(&request(PickKind::Folder)).expect_err("backend отсутствует");
        assert!(error.contains(MANUAL_HINT), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Сервис: spawn не блокирует, результат забирается poll'ом; второй spawn при «в полёте» — false.
    #[test]
    fn service_is_non_blocking_and_serialized() {
        use super::test_support::{FakePicker, ScriptedReply};
        let ctx = egui::Context::default();
        let selected = PathBuf::from("/tmp/выбранный.docx");
        let mut service = FileDialogService::with_backend(Arc::new(
            FakePicker::new(vec![ScriptedReply::Selected(selected.clone())]).with_delay(std::time::Duration::from_millis(120)),
        ));
        assert!(service.spawn(&ctx, PathTarget::ImportDocx, request(PickKind::any_file())));
        assert!(service.in_flight().is_some());
        assert!(!service.spawn(&ctx, PathTarget::OpenProject, request(PickKind::Folder)), "второй диалог не открывается");
        assert!(service.poll().is_empty(), "результата ещё нет — UI свободен");
        let done = wait_for(&mut service);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].target, PathTarget::ImportDocx);
        assert_eq!(done[0].result.as_ref().unwrap(), &Some(selected));
        assert!(service.in_flight().is_none(), "после результата можно открыть следующий диалог");
    }

    /// Фейк-протокол: запрос доходит до backend'а с нужным kind и стартовым каталогом.
    #[test]
    fn service_passes_request_to_backend() {
        use super::test_support::{FakePicker, ScriptedReply};
        let ctx = egui::Context::default();
        let picker = Arc::new(FakePicker::once(ScriptedReply::Cancelled));
        let mut service = FileDialogService::with_backend(Arc::clone(&picker) as Arc<dyn PickerBackend>);
        let mut req = request(PickKind::file("Word", &["docx"]));
        req.title = "Импорт .docx".to_owned();
        req.start_dir = Some(PathBuf::from("/tmp"));
        assert!(service.spawn(&ctx, PathTarget::ImportDocx, req.clone()));
        let done = wait_for(&mut service);
        assert_eq!(done.len(), 1);
        assert!(done[0].result.as_ref().unwrap().is_none(), "отмена");
        let logged = picker.last_request().expect("backend получил запрос");
        assert_eq!(logged, req);
        assert_eq!(service.backend_name(), "fake");
    }

    /// Отменённый (закрытый диалог) запрос: поздний результат отбрасывается, новый запрос
    /// можно открыть сразу (не «залипает» в in_flight) — регресс GUI-02.
    #[test]
    fn abandoned_request_drops_late_result_and_allows_new_spawn() {
        use super::test_support::{FakePicker, ScriptedReply};
        let ctx = egui::Context::default();
        let picker = Arc::new(
            FakePicker::new(vec![
                ScriptedReply::Selected(PathBuf::from("/tmp/поздний.docx")),
                ScriptedReply::Selected(PathBuf::from("/tmp/новый.docx")),
            ])
            .with_delay(std::time::Duration::from_millis(80)),
        );
        let mut service = FileDialogService::with_backend(Arc::clone(&picker) as Arc<dyn PickerBackend>);
        assert!(service.spawn(&ctx, PathTarget::ImportDocx, request(PickKind::any_file())));
        assert!(service.in_flight().is_some());
        // Диалог закрыли, пока системное окно было открыто.
        service.abandon(&PathTarget::ImportDocx);
        assert!(service.in_flight().is_none(), "после abandon выбор не блокирует новый запрос");
        // Дожидаемся позднего результата — он должен быть отброшен.
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(service.poll().is_empty(), "поздний выбор отменённого диалога не применяется");
        // Новый запрос работает и получает уже свой результат.
        assert!(service.spawn(&ctx, PathTarget::ImportDocx, request(PickKind::any_file())));
        let done = wait_for(&mut service);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].result.as_ref().unwrap(), &Some(PathBuf::from("/tmp/новый.docx")));
    }

    /// Без графической сессии backend честно деградирует (в headless-сессии это всегда так).
    #[test]
    fn headless_session_degrades_with_manual_hint() {
        if graphical_session_available() {
            return; // на машине с графикой эту ветку проверяет e2e-сценарий пользователя
        }
        let picker = detect_backend();
        let error = picker
            .pick(&request(PickKind::Folder))
            .expect_err("в headless диалог должен быть недоступен");
        assert!(error.contains("графической сессии"), "{error}");
        assert!(error.contains(MANUAL_HINT), "{error}");
        assert_eq!(picker.name(), "недоступен");
    }

    fn wait_for(service: &mut FileDialogService) -> Vec<PickDone> {
        let started = std::time::Instant::now();
        loop {
            let done = service.poll();
            if !done.is_empty() {
                return done;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "backend не ответил вовремя");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-file-dialog-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Исполняемый shell-скрипт во временном каталоге (фейковый kdialog/zenity).
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        path
    }
}
