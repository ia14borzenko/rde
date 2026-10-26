//! Рантайм-модель проекта RDE (M2.1): загрузка каталога проекта в дерево объектов,
//! вычисление фактов/статусов и журнал загрузки.
//!
//! Источник истины — файлы проекта; модель всегда пересоздаётся из них (как требует ТЗ §8.4).

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{
    self, ArtifactManifest, DocumentManifest, ManifestKind, NoteManifest, ProjectManifest,
    RepositoryManifest, SourceManifest,
};

/// Тип объекта дерева проекта.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectKind {
    Document,
    Artifact,
    Constants,
    Source,
    Note,
    Repository,
}

impl ObjectKind {
    pub fn label_ru(self) -> &'static str {
        match self {
            ObjectKind::Document => "Документы",
            ObjectKind::Artifact => "Артефакты",
            ObjectKind::Constants => "Константы",
            ObjectKind::Source => "Источники",
            ObjectKind::Note => "Заметки",
            ObjectKind::Repository => "Репозитории",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            ObjectKind::Document => "Д",
            ObjectKind::Artifact => "А",
            ObjectKind::Constants => "π",
            ObjectKind::Source => "И",
            ObjectKind::Note => "З",
            ObjectKind::Repository => "R",
        }
    }
}

/// Свежесть/целостность объекта. В M2.1 вычисляется из фактов на диске
/// (наличие файлов/выходов); полноценные `act_key`/`out_hash` — M2.3.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    UpToDate,
    Stale,
    Failed,
    External,
}

impl Freshness {
    pub fn label_ru(self) -> &'static str {
        match self {
            Freshness::UpToDate => "актуален",
            Freshness::Stale => "устарел",
            Freshness::Failed => "ошибка",
            Freshness::External => "внешний",
        }
    }
}

/// Типизированные данные объекта (для DAG, панелей источника и т.п.).
#[derive(Clone, Debug)]
pub enum ObjectData {
    Document(DocumentManifest),
    Artifact(ArtifactManifest),
    Source(SourceManifest),
    Note(NoteManifest),
    Repository(RepositoryManifest),
}

/// Объект дерева проекта.
#[derive(Clone, Debug)]
pub struct Object {
    pub id: String,
    pub title: String,
    pub kind: ObjectKind,
    pub freshness: Freshness,
    /// Короткая фактическая подпись для дерева («compute · 1 выход», «md · 9 строк»).
    pub detail: String,
    /// Каталог объекта.
    pub dir: PathBuf,
    pub data: ObjectData,
}

/// Группа объектов одного типа (порядок групп фиксирован).
#[derive(Clone, Debug)]
pub struct Group {
    pub kind: ObjectKind,
    pub title: &'static str,
    /// Индексы в `Project::objects`.
    pub objects: Vec<usize>,
}

/// Уровень строки журнала.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Ok,
    Warn,
}

/// Строка журнала проекта.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: &'static str,
    pub level: Level,
    pub text: String,
}

/// Некритичная проблема загрузки (файл/объект пропущен или подозрителен).
#[derive(Clone, Debug)]
pub struct Issue {
    pub path: PathBuf,
    pub message: String,
}

/// Загруженный проект.
#[derive(Clone, Debug)]
pub struct Project {
    pub manifest: ProjectManifest,
    pub root: PathBuf,
    pub name: String,
    pub branch: String,
    pub updated: String,
    pub objects: Vec<Object>,
    pub groups: Vec<Group>,
    pub log: Vec<LogLine>,
    pub issues: Vec<Issue>,
}

impl Project {
    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn stale_count(&self) -> usize {
        self.objects
            .iter()
            .filter(|o| matches!(o.freshness, Freshness::Stale | Freshness::Failed))
            .count()
    }

    pub fn figures(&self) -> usize {
        self.artifacts()
            .flat_map(|a| a.outputs.iter())
            .filter(|o| o.ends_with(".png") || o.ends_with(".svg") || o.ends_with(".jpg"))
            .count()
    }

    pub fn document(&self) -> Option<&DocumentManifest> {
        self.objects.iter().find_map(|o| match &o.data {
            ObjectData::Document(d) => Some(d),
            _ => None,
        })
    }

    pub fn document_object(&self) -> Option<&Object> {
        self.objects.iter().find(|o| matches!(o.data, ObjectData::Document(_)))
    }

    /// Путь к рабочему файлу документа (для редактора/предпросмотра).
    pub fn document_path(&self) -> Option<PathBuf> {
        self.document_object().map(|o| {
            let file = match &o.data {
                ObjectData::Document(d) => d.file.clone(),
                _ => String::new(),
            };
            o.dir.join(file)
        })
    }

    pub fn artifacts(&self) -> impl Iterator<Item = &ArtifactManifest> {
        self.objects.iter().filter_map(|o| match &o.data {
            ObjectData::Artifact(a) => Some(a),
            _ => None,
        })
    }

    pub fn artifact_object(&self, id: &str) -> Option<&Object> {
        self.objects
            .iter()
            .find(|o| o.id == id && matches!(o.data, ObjectData::Artifact(_)))
    }

    pub fn sources(&self) -> impl Iterator<Item = &Object> {
        self.objects
            .iter()
            .filter(|o| matches!(o.data, ObjectData::Source(_)))
    }

    /// Рёбра DAG: [(источник, зависимый)] только по `hashed`-группам артефактов.
    /// `ref`-группы связаны, но не входят в топосортировку/каскад (tz-02 §7.3 п.1/п.4).
    pub fn dag_edges(&self) -> Vec<(String, String)> {
        let mut edges = Vec::new();
        for object in &self.objects {
            let ObjectData::Artifact(a) = &object.data else { continue };
            for group in a.groups.iter().filter(|group| group.is_hashed()) {
                for input in &group.inputs {
                    if let manifest::InputSpec::Artifact { artifact, .. } = input {
                        if self.artifact_object(artifact).is_some() {
                            edges.push((artifact.clone(), object.id.clone()));
                        }
                    }
                }
            }
        }
        edges
    }

    /// Цикл в `hashed`-графе артефактов: цепочка id `[A, B, ..., A]` или `None`.
    ///
    /// Цикл — ошибка проекта (tz-02 §7.3 п.3): обнаружение выполняется при загрузке и при
    /// сохранении манифеста; здесь — общая функция для загрузчика и DAG-слоя.
    pub fn dag_cycle(&self) -> Option<Vec<String>> {
        let ids: Vec<String> = self
            .objects
            .iter()
            .filter(|object| matches!(object.data, ObjectData::Artifact(_)))
            .map(|object| object.id.clone())
            .collect();
        let edges: Vec<(String, String)> = self
            .dag_edges()
            .into_iter()
            .filter(|(from, to)| ids.contains(from) && ids.contains(to))
            .collect();
        find_cycle(&ids, &edges).map(|chain| chain.into_iter().map(|index| ids[index].clone()).collect())
    }

    /// Названия артефактов в цепочке цикла (для сообщения пользователю).
    pub fn dag_cycle_titles(&self) -> Option<Vec<String>> {
        let chain = self.dag_cycle()?;
        Some(
            chain
                .into_iter()
                .map(|id| self.object(&id).map(|object| object.title.clone()).unwrap_or(id))
                .collect(),
        )
    }
}

/// Найти цикл в ориентированном графе (DFS-раскраска). Возвращает цепочку индексов вида
/// `[a, b, a]`, где `a` — замыкающий узел. Порядок обхода детерминирован (индексы по порядку).
fn find_cycle(nodes: &[String], edges: &[(String, String)]) -> Option<Vec<usize>> {
    let index: std::collections::HashMap<&str, usize> =
        nodes.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (from, to) in edges {
        let (Some(&from), Some(&to)) = (index.get(from.as_str()), index.get(to.as_str())) else { continue };
        adjacency[from].push(to);
    }
    for list in &mut adjacency {
        list.sort_unstable();
        list.dedup();
    }
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        New,
        Visiting,
        Done,
    }
    fn visit(
        node: usize,
        adjacency: &[Vec<usize>],
        state: &mut [State],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[node] = State::Visiting;
        stack.push(node);
        for &next in &adjacency[node] {
            match state[next] {
                State::New => {
                    if let Some(chain) = visit(next, adjacency, state, stack) {
                        return Some(chain);
                    }
                }
                State::Visiting => {
                    // Цикл: от первого вхождения `next` в стек до текущей вершины + замыкание.
                    let start = stack.iter().position(|&n| n == next).unwrap_or(0);
                    let mut chain = stack[start..].to_vec();
                    chain.push(next);
                    return Some(chain);
                }
                State::Done => {}
            }
        }
        stack.pop();
        state[node] = State::Done;
        None
    }
    let mut state = vec![State::New; nodes.len()];
    for node in 0..nodes.len() {
        if state[node] == State::New {
            let mut stack = Vec::new();
            if let Some(chain) = visit(node, &adjacency, &mut state, &mut stack) {
                return Some(chain);
            }
        }
    }
    None
}

/// Топологический порядок (Кан): зависимость раньше зависимого. Узлы из циклов (если они есть —
/// загрузчик уже сообщил о них) добавляются в конце в порядке id, чтобы дерево всё равно строилось.
fn topological_order(nodes: &[String], edges: &[(String, String)]) -> Vec<usize> {
    let index: std::collections::HashMap<&str, usize> =
        nodes.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let mut indegree = vec![0usize; nodes.len()];
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (from, to) in edges {
        let (Some(&from), Some(&to)) = (index.get(from.as_str()), index.get(to.as_str())) else { continue };
        adjacency[from].push(to);
        indegree[to] += 1;
    }
    for list in &mut adjacency {
        list.sort_unstable();
        list.dedup();
    }
    let mut queue: Vec<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, degree)| **degree == 0)
        .map(|(i, _)| i)
        .collect();
    queue.sort_unstable();
    let mut order = Vec::with_capacity(nodes.len());
    let mut cursor = 0;
    while cursor < queue.len() {
        let node = queue[cursor];
        cursor += 1;
        order.push(node);
        for &next in &adjacency[node] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push(next);
            }
        }
    }
    if order.len() != nodes.len() {
        for (i, degree) in indegree.iter().enumerate() {
            if *degree > 0 && !order.contains(&i) {
                order.push(i);
            }
        }
    }
    order
}

/// Загрузить проект из каталога. Фатальные ошибки `rde.toml` — `Err`; проблемы объектов —
/// `Project::issues` (дерево всё равно строится).
pub fn load_project(root: &Path) -> Result<Project, String> {
    let project_path = root.join(ManifestKind::Project.file_name());
    let project_manifest = read_manifest::<ProjectManifest>(&project_path, ManifestKind::Project)
        .map_err(|e| format!("{}: {e}", project_path.display()))?;
    let mut ctx = LoadCtx {
        root,
        objects: Vec::new(),
        issues: Vec::new(),
        artifacts_by_id: Vec::new(),
        seen_ids: std::collections::BTreeSet::new(),
    };

    // Документы, источники, заметки, репозитории.
    for kind in [
        ManifestKind::Document,
        ManifestKind::Source,
        ManifestKind::Note,
        ManifestKind::Repository,
    ] {
        ctx.load_kind(kind);
    }
    // Артефакты — с топологической сортировкой по зависимостям.
    ctx.load_artifacts();

    let mut project = build_project(project_manifest, root.to_path_buf(), ctx);
    validate_constant_values(&mut project);
    project.log.insert(
        0,
        LogLine {
            time: "только что",
            level: Level::Ok,
            text: format!(
                "Проект «{}» загружен: {} объектов{}",
                project.name,
                project.object_count(),
                if project.issues.is_empty() {
                    String::new()
                } else {
                    format!(", предупреждений: {}", project.issues.len())
                }
            ),
        },
    );
    for issue in &project.issues {
        project.log.push(LogLine {
            time: "только что",
            level: Level::Warn,
            text: format!("{}: {}", issue.path.display(), issue.message),
        });
    }
    for object in &project.objects {
        if object.freshness == Freshness::Stale {
            project.log.push(LogLine {
                time: "только что",
                level: Level::Warn,
                text: format!("{}: {}", object.title, object.detail),
            });
        }
    }
    Ok(project)
}

struct LoadCtx<'a> {
    root: &'a Path,
    objects: Vec<Object>,
    issues: Vec<Issue>,
    artifacts_by_id: Vec<String>,
    seen_ids: std::collections::BTreeSet<String>,
}

impl LoadCtx<'_> {
    fn objects_root(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn issue(&mut self, path: &Path, message: impl Into<String>) {
        self.issues.push(Issue { path: path.to_path_buf(), message: message.into() });
    }

    fn check_id(&mut self, path: &Path, expected_dir: &Path, id: &str) {
        if ulid::Ulid::from_string(id).is_err() {
            self.issue(path, format!("идентификатор `{id}` не ULID"));
        }
        if expected_dir.file_name().and_then(|n| n.to_str()) != Some(id) {
            self.issue(path, format!("id `{id}` не совпадает с именем каталога"));
        }
        if !self.seen_ids.insert(id.to_owned()) {
            self.issue(path, format!("повторяющийся id `{id}`"));
        }
    }

    fn load_kind(&mut self, kind: ManifestKind) {
        let dir = self.objects_root().join(kind.objects_dir());
        let Ok(entries) = fs::read_dir(&dir) else { return };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for object_dir in dirs {
            let path = object_dir.join(kind.file_name());
            if !path.is_file() {
                self.issue(&object_dir, format!("нет манифеста {}", kind.file_name()));
                continue;
            }
            match kind {
                ManifestKind::Document => self.load_document(&object_dir, &path),
                ManifestKind::Source => self.load_source(&object_dir, &path),
                ManifestKind::Note => self.load_note(&object_dir, &path),
                ManifestKind::Repository => self.load_repository(&object_dir, &path),
                _ => {}
            }
        }
    }

    fn push_object(&mut self, object: Object) {
        self.objects.push(object);
    }

    fn load_document(&mut self, dir: &Path, path: &Path) {
        let m = match read_manifest::<DocumentManifest>(path, ManifestKind::Document) {
            Ok(m) => m,
            Err(e) => {
                self.issue(path, e);
                return;
            }
        };
        self.check_id(path, dir, &m.id);
        let file = dir.join(&m.file);
        let (freshness, detail) = if file.is_file() {
            (Freshness::UpToDate, format!("{} · {}", ext(&m.file), human_size(file_size(&file))))
        } else {
            (Freshness::Failed, format!("файл не найден: {}", m.file))
        };
        self.push_object(Object {
            id: m.id.clone(),
            title: m.title.clone(),
            kind: ObjectKind::Document,
            freshness,
            detail,
            dir: dir.to_path_buf(),
            data: ObjectData::Document(m),
        });
    }

    fn load_source(&mut self, dir: &Path, path: &Path) {
        let m = match read_manifest::<SourceManifest>(path, ManifestKind::Source) {
            Ok(m) => m,
            Err(e) => {
                self.issue(path, e);
                return;
            }
        };
        self.check_id(path, dir, &m.id);
        let (freshness, detail) = match m.kind.as_str() {
            "file" => {
                let f = m.file.as_deref().map(|f| dir.join(f));
                match f {
                    Some(f) if f.is_file() => {
                        (Freshness::UpToDate, format!("file · {}", human_size(file_size(&f))))
                    }
                    _ => (Freshness::Stale, format!("файл недоступен: {}", m.file.clone().unwrap_or_default())),
                }
            }
            "url" => (Freshness::External, format!("url · {}", m.url.clone().unwrap_or_default())),
            other => (Freshness::External, format!("{other} · внешний")),
        };
        self.push_object(Object {
            id: m.id.clone(),
            title: m.title.clone(),
            kind: ObjectKind::Source,
            freshness,
            detail,
            dir: dir.to_path_buf(),
            data: ObjectData::Source(m),
        });
    }

    fn load_note(&mut self, dir: &Path, path: &Path) {
        let m = match read_manifest::<NoteManifest>(path, ManifestKind::Note) {
            Ok(m) => m,
            Err(e) => {
                self.issue(path, e);
                return;
            }
        };
        self.check_id(path, dir, &m.id);
        let file = dir.join(&m.file);
        let (freshness, detail) = if file.is_file() {
            let lines = fs::read_to_string(&file).map(|t| t.lines().count()).unwrap_or(0);
            (Freshness::UpToDate, format!("md · {lines} строк"))
        } else {
            (Freshness::Failed, format!("файл не найден: {}", m.file))
        };
        self.push_object(Object {
            id: m.id.clone(),
            title: m.title.clone(),
            kind: ObjectKind::Note,
            freshness,
            detail,
            dir: dir.to_path_buf(),
            data: ObjectData::Note(m),
        });
    }

    fn load_repository(&mut self, dir: &Path, path: &Path) {
        let m = match read_manifest::<RepositoryManifest>(path, ManifestKind::Repository) {
            Ok(m) => m,
            Err(e) => {
                self.issue(path, e);
                return;
            }
        };
        self.check_id(path, dir, &m.id);
        let (freshness, detail) = if m.kind == "external" {
            (Freshness::External, format!("внешний · {}", m.path))
        } else {
            (Freshness::UpToDate, format!("{} · {}", m.kind, m.path))
        };
        self.push_object(Object {
            id: m.id.clone(),
            title: m.title.clone(),
            kind: ObjectKind::Repository,
            freshness,
            detail,
            dir: dir.to_path_buf(),
            data: ObjectData::Repository(m),
        });
    }

    fn load_artifacts(&mut self) {
        let dir = self.objects_root().join(ManifestKind::Artifact.objects_dir());
        let Ok(entries) = fs::read_dir(&dir) else { return };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
        dirs.sort();
        let mut loaded: Vec<(ArtifactManifest, PathBuf)> = Vec::new();
        for object_dir in dirs {
            let path = object_dir.join(ManifestKind::Artifact.file_name());
            if !path.is_file() {
                self.issue(&object_dir, "нет манифеста artifact.toml");
                continue;
            }
            match read_manifest::<ArtifactManifest>(&path, ManifestKind::Artifact) {
                Ok(m) => {
                    self.check_id(&path, &object_dir, &m.id);
                    loaded.push((m, object_dir));
                }
                Err(e) => {
                    self.issue(&path, e);
                }
            }
        }
        // Топологический порядок по hashed-рёбрам (ref-рёбра исключены, tz-02 §7.3 п.4);
        // цикл — issue с цепочкой, порядок для участников цикла остаётся условным.
        let ids: Vec<String> = loaded.iter().map(|(m, _)| m.id.clone()).collect();
        let edges: Vec<(String, String)> = {
            let mut edges = Vec::new();
            for (m, _) in &loaded {
                for group in m.groups.iter().filter(|group| group.is_hashed()) {
                    for input in &group.inputs {
                        if let manifest::InputSpec::Artifact { artifact, .. } = input {
                            if ids.iter().any(|id| id == artifact) {
                                edges.push((artifact.clone(), m.id.clone()));
                            }
                        }
                    }
                }
            }
            edges
        };
        if let Some(chain) = find_cycle(&ids, &edges) {
            let titles: Vec<String> = chain
                .iter()
                .map(|&index| loaded[index].0.title.clone())
                .collect();
            self.issue(&dir, format!("цикл в hashed-зависимостях: {}", titles.join(" → ")));
        }
        let order = topological_order(&ids, &edges);
        for i in order {
            let (m, object_dir) = loaded[i].clone();
            let object = self.artifact_object(m, &object_dir);
            self.artifacts_by_id.push(object.id.clone());
            self.push_object(object);
        }
    }

    fn artifact_object(&mut self, m: ArtifactManifest, dir: &Path) -> Object {
        let (kind, freshness, detail) = match m.kind.as_str() {
            "constants" => {
                let n = m.values.len();
                (ObjectKind::Constants, Freshness::UpToDate, format!("значений: {n}"))
            }
            "data" => {
                let (ok, why) = inputs_present(&m, dir, &self.objects);
                let n = m.groups.iter().map(|g| g.inputs.len()).sum::<usize>();
                if ok {
                    (ObjectKind::Artifact, Freshness::UpToDate, format!("data · входов: {n}"))
                } else {
                    (ObjectKind::Artifact, Freshness::Stale, why)
                }
            }
            _ => {
                let out_dir = dir.join("out");
                let produced = has_any_file(&out_dir);
                let (inputs_ok, why) = inputs_present(&m, dir, &self.objects);
                if !inputs_ok {
                    (ObjectKind::Artifact, Freshness::Stale, why)
                } else if !m.outputs.is_empty() && !produced {
                    (
                        ObjectKind::Artifact,
                        Freshness::Stale,
                        format!("выходы не сгенерированы ({})", m.outputs.len()),
                    )
                } else {
                    (
                        ObjectKind::Artifact,
                        Freshness::UpToDate,
                        format!("compute · выходов: {}", m.outputs.len()),
                    )
                }
            }
        };
        Object {
            id: m.id.clone(),
            title: m.title.clone(),
            kind,
            freshness,
            detail,
            dir: dir.to_path_buf(),
            data: ObjectData::Artifact(m),
        }
    }
}

/// Проверить расширенные значения констант (R-ART-09, ADR-0005 P1): URI `source`, цель,
/// формат отображения, обозначение. Проблемы — `issues` загрузки (не фатальны), значения с
/// недоступным источником разрешаются позже (`rde-dag`: кэш/ошибка статуса).
fn validate_constant_values(project: &mut Project) {
    let mut issues: Vec<(PathBuf, String)> = Vec::new();
    for object in &project.objects {
        let ObjectData::Artifact(manifest) = &object.data else { continue };
        if manifest.kind != "constants" {
            continue;
        }
        for (key, value) in &manifest.values {
            if !crate::value_format::is_known_format(&value.format) {
                issues.push((object.dir.clone(), format!("константа `{key}`: неизвестный format `{}`", value.format)));
            }
            if value.symbol.chars().any(char::is_whitespace) {
                issues.push((object.dir.clone(), format!("константа `{key}`: symbol содержит пробелы")));
            }
            if value.subscript.chars().any(char::is_whitespace) {
                issues.push((object.dir.clone(), format!("константа `{key}`: subscript содержит пробелы")));
            }
            match value.source_parsed() {
                Err(error) => issues.push((object.dir.clone(), format!("константа `{key}`: {error}"))),
                Ok(None) => {}
                Ok(Some(source)) => {
                    let target = project.object(&source.artifact_id);
                    match target {
                        Some(target) if matches!(target.data, ObjectData::Artifact(_)) => {}
                        Some(_) => issues.push((
                            object.dir.clone(),
                            format!("константа `{key}`: источник `{}` — не артефакт", source.artifact_id),
                        )),
                        None => issues.push((
                            object.dir.clone(),
                            format!("константа `{key}`: артефакт-источник `{}` не найден", source.artifact_id),
                        )),
                    }
                }
            }
        }
    }
    project.issues.extend(issues.into_iter().map(|(path, message)| Issue { path, message }));
}

/// Проверить наличие `hashed`-входов артефакта (ref-группы не влияют на свежесть, §7.3 п.1).
/// Селекторы раскрываются как путь или glob (см. [`crate::selector`]).
fn inputs_present(m: &ArtifactManifest, dir: &Path, objects: &[Object]) -> (bool, String) {
    let find = |id: &str| objects.iter().find(|object| object.id == id);
    for group in m.groups.iter().filter(|group| group.is_hashed()) {
        for input in &group.inputs {
            match input {
                manifest::InputSpec::File { file } => {
                    if !dir.join(file).is_file() {
                        return (false, format!("вход не найден: {file}"));
                    }
                }
                manifest::InputSpec::Artifact { artifact, selector } => {
                    let Some(target) = find(artifact) else {
                        return (false, format!("зависимый артефакт не найден: {artifact}"));
                    };
                    if selector.trim().is_empty() {
                        return (false, format!("у входа `{artifact}` не задан selector"));
                    }
                    if let Err(error) = crate::selector::expand_selector(&target.dir, selector) {
                        return (false, format!("вход {artifact}/{selector}: {error}"));
                    }
                }
                manifest::InputSpec::Note { note } => {
                    let Some(target) = find(note) else {
                        return (false, format!("заметка не найдена: {note}"));
                    };
                    let file = match &target.data {
                        ObjectData::Note(manifest) => target.dir.join(&manifest.file),
                        _ => return (false, format!("{note} — не заметка")),
                    };
                    if !file.is_file() {
                        return (false, format!("файл заметки не найден: {note}"));
                    }
                }
                manifest::InputSpec::Source { source } => {
                    let Some(target) = find(source) else {
                        return (false, format!("источник не найден: {source}"));
                    };
                    match &target.data {
                        ObjectData::Source(manifest) => {
                            if let Some(file) = &manifest.file {
                                if !target.dir.join(file).is_file() {
                                    return (false, format!("файл источника не найден: {source}"));
                                }
                            }
                        }
                        _ => return (false, format!("{source} — не источник")),
                    }
                }
                manifest::InputSpec::Constants { constants, selector } => {
                    let Some(target) = find(constants) else {
                        return (false, format!("набор констант не найден: {constants}"));
                    };
                    match &target.data {
                        ObjectData::Artifact(manifest) if manifest.kind == "constants" => {
                            let selector = selector.trim();
                            if !selector.is_empty() && !manifest.values.contains_key(selector) {
                                return (false, format!("константа не найдена: {constants}/{selector}"));
                            }
                        }
                        _ => return (false, format!("{constants} — не набор констант")),
                    }
                }
            }
        }
    }
    (true, String::new())
}

fn build_project(manifest: ProjectManifest, root: PathBuf, ctx: LoadCtx<'_>) -> Project {
    let name = manifest.name.clone();
    let mut groups: Vec<Group> = Vec::new();
    for kind in [
        ObjectKind::Document,
        ObjectKind::Artifact,
        ObjectKind::Constants,
        ObjectKind::Source,
        ObjectKind::Note,
        ObjectKind::Repository,
    ] {
        let objects: Vec<usize> = ctx
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == kind)
            .map(|(i, _)| i)
            .collect();
        if !objects.is_empty() {
            groups.push(Group { kind, title: kind.label_ru(), objects });
        }
    }
    let branch = if root.join(".git").exists() { "main".to_owned() } else { "—".to_owned() };
    Project {
        manifest,
        root,
        name,
        branch,
        updated: "только что".to_owned(),
        objects: ctx.objects,
        groups,
        log: Vec::new(),
        issues: ctx.issues,
    }
}

/// Доступ к полю `schema` манифеста (для проверки версии схемы).
pub trait HasSchemaField {
    fn schema_field(&self) -> &str;
}

impl HasSchemaField for ProjectManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}
impl HasSchemaField for ArtifactManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}
impl HasSchemaField for DocumentManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}
impl HasSchemaField for SourceManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}
impl HasSchemaField for NoteManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}
impl HasSchemaField for RepositoryManifest {
    fn schema_field(&self) -> &str {
        &self.schema
    }
}

fn read_manifest<T>(path: &Path, kind: ManifestKind) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + HasSchemaField,
{
    let text = fs::read_to_string(path).map_err(|e| format!("не читается: {e}"))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("TOML не разобран: {e}"))?;
    let errors = manifest::validate(kind, &value);
    if !errors.is_empty() {
        return Err(format!("не проходит JSON Schema: {}", errors.join("; ")));
    }
    let typed: T = toml::from_str(&text).map_err(|e| format!("TOML не разобран: {e}"))?;
    let schema = typed.schema_field();
    if schema != kind.schema_id() {
        return Err(format!("schema `{schema}` не поддерживается (ожидается `{}`)", kind.schema_id()));
    }
    Ok(typed)
}

fn ext(file: &str) -> &str {
    file.rsplit('.').next().unwrap_or("file")
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} КБ", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} Б")
    }
}

fn has_any_file(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    for e in entries.flatten() {
        let name = e.file_name();
        // Служебные/скрытые файлы (.gitignore-заглушки) не считаются выходом.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_file() || p.is_dir() && has_any_file(&p) {
            return true;
        }
    }
    false
}
