//! RDE DAG execution (M2.3): ключи активации, ledger состояний и топологическое обновление артефактов.
//!
//! Модель (tz-02 §7):
//! - `act_key = H(kind | in_hash | script_hash? | params_hash | env_hash)` — ключ активации;
//! - `in_hash` строится из `hashed`-групп входов (файлы/выходы других артефактов/источники);
//! - `out_hash` — Merkle-дерево выходов, блобы складываются в CAS;
//! - ledger — committed-транзакции `.rde/updates/<txid>.json` с payload `kind="dag-update"`;
//!   ledger всегда восстанавливается из журнала, отдельного состояния в БД нет.
//!
//! Статусы артефактов вычисляются из фактов: сравнение текущего `act_key` с записанным,
//! наличие выходов в каталоге артефакта и их хешей с записанным `out_hash`
//! (ручная правка выхода = устарел). Data/constants не запускаются и считаются актуальными.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rde_core::{ArtifactManifest, InputSpec, Object, ObjectData, Project};
use rde_hash::{act_key, env_hash, group_hash, in_hash, params_hash, script_hash, sha256, tree_hash, GroupMember, Hash, InGroup, TreeEntry};
use rde_run::{run, InputFile, RunOutcome, RunSpec, RunStatus};
use rde_store::{Store, TxRecord, TxStatus};
use serde::{Deserialize, Serialize};

/// Платформенная строка для `env_hash`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Запись выхода в ledger.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputRecord {
    pub rel: String,
    pub blob: String,
    pub mode: u32,
    pub size: u64,
}

/// Состояние артефакта после успешного обновления (хранится в payload транзакции).
///
/// Компоненты ключа хранятся отдельно, чтобы UI показывал причину устаревания по §7.3 п.2
/// («изменились входы/скрипт/параметры/окружение»), а не одно общее сообщение.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactState {
    pub artifact_id: String,
    pub act_key: String,
    #[serde(default)]
    pub in_hash: Option<String>,
    #[serde(default)]
    pub script_hash: Option<String>,
    #[serde(default)]
    pub params_hash: Option<String>,
    #[serde(default)]
    pub env_hash: Option<String>,
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub out_hash: Option<String>,
    #[serde(default)]
    pub outputs: Vec<OutputRecord>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub updated_unix: u64,
}

/// Вычисленные компоненты ключа активации.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub artifact_id: String,
    pub act_key: Hash,
    pub in_hash: Hash,
    pub script_hash: Option<Hash>,
    pub params_hash: Hash,
    pub env_hash: Hash,
    pub schema_version: String,
}

/// Причина устаревания по компонентам `act_key`/выходов (tz-02 §7.3 п.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StaleReason {
    /// Изменились hashed-входы (`in_hash`).
    InputsChanged,
    /// Изменился скрипт (`script_hash`).
    ScriptChanged,
    /// Изменились параметры (`params_hash`).
    ParamsChanged,
    /// Изменилось окружение (`env_hash`).
    EnvChanged,
    /// Изменилась версия схемы манифеста (`schema_version`).
    SchemaChanged,
    /// Запись состояния сделана старой версией RDE — компоненты неизвестны.
    StateIncomplete,
    /// Объявленные выходы отсутствуют (удалены/потеряны).
    OutputsMissing,
    /// Выходы изменены вручную (`out_hash` не совпадает).
    OutputsModified,
}

impl StaleReason {
    pub fn label_ru(self) -> &'static str {
        match self {
            StaleReason::InputsChanged => "изменились входы",
            StaleReason::ScriptChanged => "изменился скрипт",
            StaleReason::ParamsChanged => "изменились параметры",
            StaleReason::EnvChanged => "изменилось окружение",
            StaleReason::SchemaChanged => "изменилась версия схемы",
            StaleReason::StateIncomplete => "состояние записано старой версией RDE",
            StaleReason::OutputsMissing => "потеряны/отсутствуют выходы",
            StaleReason::OutputsModified => "выходы изменены вручную",
        }
    }
}

/// Живой статус артефакта (считается из хешей и файлов, а не эвристик).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveStatus {
    Fresh,
    /// Устарел; список причин по компонентам (может быть несколько).
    Stale(Vec<StaleReason>),
    NeverRun,
    NotComputable(String),
}

impl LiveStatus {
    pub fn label_ru(&self) -> &str {
        match self {
            LiveStatus::Fresh => "актуален",
            LiveStatus::Stale(_) => "устарел",
            LiveStatus::NeverRun => "не запускался",
            LiveStatus::NotComputable(_) => "ошибка",
        }
    }

    /// Человекочитаемая причина (для UI/логов); у `Stale` — причины через ` · `.
    pub fn reason(&self) -> Option<String> {
        match self {
            LiveStatus::Stale(reasons) => Some(reasons.iter().map(|reason| reason.label_ru()).collect::<Vec<_>>().join(" · ")),
            LiveStatus::NotComputable(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    /// Структурированные причины устаревания (для панели/инструментов).
    pub fn stale_reasons(&self) -> Option<&[StaleReason]> {
        match self {
            LiveStatus::Stale(reasons) => Some(reasons),
            _ => None,
        }
    }
}

/// Статус артефакта для UI/CLI.
#[derive(Clone, Debug)]
pub struct ArtifactStatus {
    pub artifact_id: String,
    pub title: String,
    pub kind: String,
    pub status: LiveStatus,
    pub act_key: Option<String>,
    pub out_hash: Option<String>,
    pub updated_unix: Option<u64>,
    pub outputs: usize,
    pub run_id: Option<String>,
}

/// Прочитать ledger из committed-транзакций (порядок — по времени начала).
pub fn ledger(store: &Store) -> Result<HashMap<String, ArtifactState>, String> {
    let mut txs: Vec<TxRecord> = store.list_txs().map_err(|e| e.to_string())?;
    txs.retain(|tx| {
        tx.status == TxStatus::Committed
            && tx
                .data
                .as_ref()
                .and_then(|data| data.get("kind"))
                .and_then(|kind| kind.as_str())
                == Some("dag-update")
    });
    txs.sort_by(|a, b| a.started_unix.cmp(&b.started_unix).then_with(|| a.id.cmp(&b.id)));
    let mut ledger = HashMap::new();
    for tx in txs {
        let Some(data) = tx.data else { continue };
        let Some(items) = data.get("artifacts").and_then(|value| value.as_array()) else { continue };
        for item in items {
            if let Ok(state) = serde_json::from_value::<ArtifactState>(item.clone()) {
                ledger.insert(state.artifact_id.clone(), state);
            }
        }
    }
    Ok(ledger)
}

fn missing_hash(what: &str) -> Hash {
    sha256(format!("rde:missing:{what}"))
}

fn hash_file(path: &Path) -> Option<Hash> {
    fs::read(path).ok().map(|bytes| sha256(&bytes))
}

fn object<'a>(project: &'a Project, id: &str) -> Result<&'a Object, String> {
    project.object(id).ok_or_else(|| format!("объект {id} не найден"))
}

fn baseline_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("input.bin")
        .to_owned()
}

/// Хеш входа-ссылки (артефакт/заметка/источник/константы). Файлы обрабатываются в [`activation`].
///
/// Для входа-артефакта по §7.2 берётся `out_hash` зависимости из ledger; если ledger пуст
/// (data-артефакт, который не запускается) — хеш фактического файла или Merkle-дерево
/// совпавших файлов glob-селектора.
fn input_hash(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    input: &InputSpec,
) -> Result<(String, Hash), String> {
    match input {
        InputSpec::Artifact { artifact, selector } => {
            let dep = object(project, artifact)?;
            if selector.trim().is_empty() {
                return Err(format!("у входа `{artifact}` не задан selector"));
            }
            if selector.starts_with("out/") {
                if let Some(state) = ledger.get(artifact) {
                    if let Some(out_hash) = &state.out_hash {
                        if let Ok(hash) = Hash::from_hex(out_hash) {
                            return Ok((artifact.clone(), hash));
                        }
                    }
                }
            }
            // Отсутствующий вход не делает артефакт «невычислимым»: он получает sentinel-хеш,
            // а «устарел/не запускался» определяется статусом (см. §7.3 п.2).
            let Ok(matches) = rde_core::expand_selector(&dep.dir, selector) else {
                return Ok((artifact.clone(), missing_hash(&format!("{artifact}/{selector}"))));
            };
            if matches.len() == 1 {
                let bytes = fs::read(&matches[0].path).map_err(|e| format!("прочитать вход {}: {e}", matches[0].rel))?;
                return Ok((artifact.clone(), sha256(&bytes)));
            }
            // Glob-селектор: идентичность набора файлов — Merkle-дерево (детерминированно).
            let mut entries = Vec::with_capacity(matches.len());
            for matched in &matches {
                let bytes = fs::read(&matched.path).map_err(|e| format!("прочитать вход {}: {e}", matched.rel))?;
                entries.push(TreeEntry::new(matched.rel.clone(), 0o644, sha256(&bytes)));
            }
            Ok((artifact.clone(), tree_hash(&entries)))
        }
        InputSpec::File { file } => Ok((format!("file:{file}"), missing_hash(&format!("file:{file}")))),
        InputSpec::Note { note } => {
            let note_object = object(project, note)?;
            if let ObjectData::Note(manifest) = &note_object.data {
                let path = note_object.dir.join(&manifest.file);
                Ok((format!("note:{note}"), hash_file(&path).unwrap_or_else(|| missing_hash(&format!("note:{note}")))))
            } else {
                Err(format!("{note} — не заметка"))
            }
        }
        InputSpec::Source { source } => {
            let source_object = object(project, source)?;
            if let ObjectData::Source(manifest) = &source_object.data {
                let hash = match &manifest.file {
                    Some(file) => hash_file(&source_object.dir.join(file)),
                    None => manifest.url.as_deref().map(|url| sha256(url.as_bytes())),
                };
                Ok((format!("source:{source}"), hash.unwrap_or_else(|| missing_hash(&format!("source:{source}")))))
            } else {
                Err(format!("{source} — не источник"))
            }
        }
        InputSpec::Constants { constants, selector } => {
            let target = object(project, constants)?;
            let ObjectData::Artifact(manifest) = &target.data else {
                return Err(format!("{constants} — не набор констант"));
            };
            if manifest.kind != "constants" {
                return Err(format!("{constants} — не набор констант (type={})", manifest.kind));
            }
            let value = constants_value(manifest, selector)?;
            let json = rde_hash::canonical_json(&value)?;
            Ok((format!("constants:{constants}"), sha256(json.as_bytes())))
        }
    }
}

/// JSON-значение констант: весь набор или одна величина (`selector`).
fn constants_value(manifest: &rde_core::ArtifactManifest, selector: &str) -> Result<serde_json::Value, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        serde_json::to_value(&manifest.values).map_err(|e| format!("константы: {e}"))
    } else {
        let value = manifest
            .values
            .get(selector)
            .ok_or_else(|| format!("константа `{selector}` не найдена"))?;
        serde_json::to_value(value).map_err(|e| format!("константа `{selector}`: {e}"))
    }
}

/// Дополнительные файлы скрипта (`script_files`): путь → хеш блоба; отсутствующий файл
/// получает sentinel-хеш (изменение manifest-файла меняет `script_hash`, §7.2).
fn script_extra_files(artifact_dir: &Path, files: &[String]) -> Vec<(String, Hash)> {
    files
        .iter()
        .map(|rel| {
            let hash = fs::read(artifact_dir.join(rel))
                .map(|bytes| sha256(&bytes))
                .unwrap_or_else(|_| missing_hash(&format!("script:{rel}")));
            (rel.clone(), hash)
        })
        .collect()
}

/// Вычислить `act_key` артефакта (compute). Data/constants не активируются.
pub fn activation(
    project: &Project,
    artifact: &ArtifactManifest,
    ledger: &HashMap<String, ArtifactState>,
) -> Result<Activation, String> {
    let artifact_object = object(project, &artifact.id)?;
    let artifact_dir = artifact_object.dir.clone();
    if artifact.kind != "compute" {
        return Err(format!("артефакт {} не compute (type={})", artifact.id, artifact.kind));
    }
    let runtime = artifact
        .runtime
        .as_ref()
        .ok_or_else(|| format!("у compute-артефакта {} нет runtime", artifact.id))?;

    let mut groups = Vec::new();
    for group in artifact.groups.iter().filter(|group| group.is_hashed()) {
        let mut members = Vec::new();
        for input in &group.inputs {
            let (target, hash) = match input {
                InputSpec::File { file } => {
                    let path = artifact_dir.join(file);
                    (format!("file:{file}"), hash_file(&path).unwrap_or_else(|| missing_hash(&format!("{}/{}", artifact.id, file))))
                }
                other => input_hash(project, ledger, other)?,
            };
            let selector = match input {
                InputSpec::Artifact { selector, .. } => selector.clone(),
                InputSpec::File { file } => file.clone(),
                InputSpec::Note { note } => note.clone(),
                InputSpec::Source { source } => source.clone(),
                InputSpec::Constants { selector, .. } => selector.clone(),
            };
            members.push(GroupMember { target, out: hash, selector });
        }
        groups.push(InGroup { name: group.name.clone(), mode: "hashed".into(), grp_hash: group_hash(&group.name, "hashed", &members) });
    }
    let in_hash = in_hash(&groups);

    let script_path = artifact_dir.join(&runtime.entry);
    let script_bytes = fs::read(&script_path).unwrap_or_default();
    let extra_files = script_extra_files(&artifact_dir, &artifact.script_files);
    let script = Some(script_hash(&runtime.kind, &runtime.entry, &script_bytes, &extra_files));
    let params = params_hash(&artifact.params)?;
    let env = env_hash(&runtime.kind, &platform(), None, &BTreeMap::new());
    let key = act_key(&artifact.kind, &artifact.schema, in_hash, script, params, env);
    Ok(Activation {
        artifact_id: artifact.id.clone(),
        act_key: key,
        in_hash,
        script_hash: script,
        params_hash: params,
        env_hash: env,
        schema_version: artifact.schema.clone(),
    })
}

/// Текущий `out_hash` по фактическим файлам в каталоге артефакта (None — если выходов нет).
fn actual_out_hash(artifact_dir: &Path, state: &ArtifactState) -> Option<Hash> {
    if state.outputs.is_empty() {
        return None;
    }
    let mut entries = Vec::with_capacity(state.outputs.len());
    for output in &state.outputs {
        let bytes = fs::read(artifact_dir.join(&output.rel)).ok()?;
        entries.push(TreeEntry::new(output.rel.clone(), output.mode, sha256(&bytes)));
    }
    Some(tree_hash(&entries))
}

/// Проверить свежесть артефакта по компонентам (§7.3 п.2). Возвращает причины устаревания;
/// пустой список — актуален.
fn stale_reasons(artifact_dir: &Path, state: &ArtifactState, activation: &Activation) -> Vec<StaleReason> {
    let mut reasons = Vec::new();
    if state.act_key == activation.act_key.hex() {
        // Ключ совпал — входы/скрипт/параметры/окружение не менялись.
    } else {
        let mut component_known = false;
        if let Some(recorded) = &state.in_hash {
            component_known = true;
            if *recorded != activation.in_hash.hex() {
                reasons.push(StaleReason::InputsChanged);
            }
        }
        if let Some(recorded) = &state.script_hash {
            component_known = true;
            if Some(recorded.clone()) != activation.script_hash.map(|hash| hash.hex()) {
                reasons.push(StaleReason::ScriptChanged);
            }
        }
        if let Some(recorded) = &state.params_hash {
            component_known = true;
            if *recorded != activation.params_hash.hex() {
                reasons.push(StaleReason::ParamsChanged);
            }
        }
        if let Some(recorded) = &state.env_hash {
            component_known = true;
            if *recorded != activation.env_hash.hex() {
                reasons.push(StaleReason::EnvChanged);
            }
        }
        if let Some(recorded) = &state.schema_version {
            component_known = true;
            if *recorded != activation.schema_version {
                reasons.push(StaleReason::SchemaChanged);
            }
        }
        if !component_known {
            reasons.push(StaleReason::StateIncomplete);
        } else if reasons.is_empty() {
            // Компоненты совпали, а act_key — нет: состояние записано другой версией формулы.
            reasons.push(StaleReason::StateIncomplete);
        }
    }
    // Целостность выходов: наличие и совпадение out_hash (ручная правка = предупреждение).
    match state.out_hash.as_deref().and_then(|hex| Hash::from_hex(hex).ok()) {
        None => reasons.push(StaleReason::OutputsMissing),
        Some(expected) => match actual_out_hash(artifact_dir, state) {
            None => reasons.push(StaleReason::OutputsMissing),
            Some(actual) if actual != expected => reasons.push(StaleReason::OutputsModified),
            Some(_) => {}
        },
    }
    reasons
}

/// Оценить все артефакты проекта (compute — по хешам, data/constants — актуальны).
pub fn evaluate(project: &Project, store: &Store) -> Result<BTreeMap<String, ArtifactStatus>, String> {
    let ledger = ledger(store)?;
    let mut result = BTreeMap::new();
    for artifact_object in project.objects.iter().filter(|o| matches!(o.data, ObjectData::Artifact(_))) {
        let ObjectData::Artifact(artifact) = &artifact_object.data else { continue };
        let (status, act_key, out_hash, updated, run_id) = if artifact.kind != "compute" {
            (LiveStatus::Fresh, None, None, None, None)
        } else {
            match activation(project, artifact, &ledger) {
                Err(error) => (LiveStatus::NotComputable(error), None, None, None, None),
                Ok(activation) => {
                    let state = ledger.get(&artifact.id);
                    let updated = state.map(|state| state.updated_unix);
                    let out_hash = state.and_then(|state| state.out_hash.clone());
                    let run_id = state.and_then(|state| state.run_id.clone());
                    let status = match state {
                        None => LiveStatus::NeverRun,
                        Some(state) => {
                            let reasons = stale_reasons(&artifact_object.dir, state, &activation);
                            if reasons.is_empty() {
                                LiveStatus::Fresh
                            } else {
                                LiveStatus::Stale(reasons)
                            }
                        }
                    };
                    (status, Some(activation.act_key.hex()), out_hash, updated, run_id)
                }
            }
        };
        result.insert(
            artifact.id.clone(),
            ArtifactStatus {
                artifact_id: artifact.id.clone(),
                title: artifact_object.title.clone(),
                kind: artifact.kind.clone(),
                status,
                act_key,
                out_hash,
                updated_unix: updated,
                outputs: artifact.outputs.len(),
                run_id,
            },
        );
    }
    Ok(result)
}

/// Топологический порядок compute-артефактов (зависимость → зависимый). Ошибка при цикле.
pub fn topological_compute(project: &Project) -> Result<Vec<String>, String> {
    let compute: Vec<String> = project
        .artifacts()
        .filter(|artifact| artifact.kind == "compute")
        .map(|artifact| artifact.id.clone())
        .collect();
    let index: HashMap<&str, usize> = compute.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let mut indegree = vec![0usize; compute.len()];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); compute.len()];
    for (from, to) in project.dag_edges() {
        let (Some(&from), Some(&to)) = (index.get(from.as_str()), index.get(to.as_str())) else { continue };
        edges[from].push(to);
        indegree[to] += 1;
    }
    let mut queue: Vec<usize> = indegree.iter().enumerate().filter(|(_, d)| **d == 0).map(|(i, _)| i).collect();
    queue.sort_unstable();
    let mut result = Vec::with_capacity(compute.len());
    let mut cursor = 0;
    while cursor < queue.len() {
        let node = queue[cursor];
        cursor += 1;
        result.push(compute[node].clone());
        for &next in &edges[node] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push(next);
            }
        }
    }
    if result.len() != compute.len() {
        return Err("в графе артефактов есть цикл".to_owned());
    }
    Ok(result)
}

/// Транзитивное замыкание зависимых для набора целей (чтобы «обновить связанные»).
pub fn dependents_closure(project: &Project, targets: &[String]) -> HashSet<String> {
    let mut closure: HashSet<String> = targets.iter().cloned().collect();
    let edges = project.dag_edges();
    let mut changed = true;
    while changed {
        changed = false;
        for (from, to) in &edges {
            if closure.contains(from) && !closure.contains(to) {
                closure.insert(to.clone());
                changed = true;
            }
        }
    }
    closure
}

#[derive(Clone, Debug, Default)]
pub struct UpdateOptions {
    /// Ограничить обновление целями (None — все устаревшие).
    pub targets: Option<Vec<String>>,
    /// Для `targets`: включить транзитивно зависимые артефакты («обновить связанные»).
    /// `false` — обновить только сами цели («обновить артефакт»).
    pub include_dependents: bool,
    /// Принудительно пересчитать явно указанные цели, даже если они актуальны.
    pub force: bool,
    /// Продолжать после ошибки отдельного артефакта.
    pub continue_on_error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    Skipped,
    Ran,
    Failed,
}

#[derive(Clone, Debug)]
pub struct UpdatedArtifact {
    pub artifact_id: String,
    pub title: String,
    pub action: UpdateAction,
    pub message: String,
    pub run_id: Option<String>,
    pub act_key: Option<String>,
    pub out_hash: Option<String>,
    pub log_path: Option<PathBuf>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct UpdateReport {
    pub artifacts: Vec<UpdatedArtifact>,
    pub errors: Vec<String>,
    pub tx_ids: Vec<String>,
}

impl UpdateReport {
    pub fn ran(&self) -> usize {
        self.artifacts.iter().filter(|a| a.action == UpdateAction::Ran).count()
    }

    pub fn skipped(&self) -> usize {
        self.artifacts.iter().filter(|a| a.action == UpdateAction::Skipped).count()
    }

    pub fn failed(&self) -> usize {
        self.artifacts.iter().filter(|a| a.action == UpdateAction::Failed).count()
    }
}

/// Построить `RunSpec` артефакта: входы hashed-групп материализуются в `inputs/...`.
///
/// - файл проекта → `inputs/<basename>`;
/// - выход другого артефакта: selector-файл → `inputs/<basename>`, glob → все совпавшие файлы;
/// - константы → сгенерированный `inputs/constants.json` / `inputs/constant-<имя>.json`;
/// - заметка/источник → `inputs/<basename>`.
fn build_spec(project: &Project, artifact: &ArtifactManifest) -> Result<RunSpec, String> {
    let artifact_object = object(project, &artifact.id)?;
    let runtime = artifact.runtime.as_ref().ok_or_else(|| "нет runtime".to_owned())?;
    let mut inputs = Vec::new();
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    for (_, input) in artifact.hashed_inputs() {
        match input {
            InputSpec::File { file } => {
                let name = unique_input_name(&mut used_names, baseline_name(file));
                inputs.push(InputFile::new(artifact_object.dir.join(file), format!("inputs/{name}")));
            }
            InputSpec::Artifact { artifact: dep_id, selector } => {
                if selector.trim().is_empty() {
                    return Err(format!("у входа `{dep_id}` пустой selector"));
                }
                let dep = object(project, dep_id)?;
                let matches = rde_core::expand_selector(&dep.dir, selector)?;
                for matched in matches {
                    let name = unique_input_name(&mut used_names, baseline_name(&matched.rel));
                    inputs.push(InputFile::new(matched.path, format!("inputs/{name}")));
                }
            }
            InputSpec::Note { note } => {
                let note_object = object(project, note)?;
                if let ObjectData::Note(manifest) = &note_object.data {
                    let name = unique_input_name(&mut used_names, baseline_name(&manifest.file));
                    inputs.push(InputFile::new(note_object.dir.join(&manifest.file), format!("inputs/{name}")));
                } else {
                    return Err(format!("{note} — не заметка"));
                }
            }
            InputSpec::Source { source } => {
                let source_object = object(project, source)?;
                if let ObjectData::Source(manifest) = &source_object.data {
                    let file = manifest.file.clone().unwrap_or_else(|| manifest.url.clone().unwrap_or_default());
                    if file.is_empty() {
                        return Err(format!("источник {source} без файла/URL — нельзя подать в скрипт"));
                    }
                    let name = unique_input_name(&mut used_names, baseline_name(&file));
                    inputs.push(InputFile::new(source_object.dir.join(&file), format!("inputs/{name}")));
                } else {
                    return Err(format!("{source} — не источник"));
                }
            }
            InputSpec::Constants { constants, selector } => {
                let target = object(project, constants)?;
                let ObjectData::Artifact(manifest) = &target.data else {
                    return Err(format!("{constants} — не набор констант"));
                };
                if manifest.kind != "constants" {
                    return Err(format!("{constants} — не набор констант (type={})", manifest.kind));
                }
                let value = constants_value(manifest, selector)?;
                let bytes = serde_json::to_vec_pretty(&value).map_err(|e| format!("константы: {e}"))?;
                let selector = selector.trim();
                let name = if selector.is_empty() {
                    unique_input_name(&mut used_names, "constants.json".to_owned())
                } else {
                    unique_input_name(&mut used_names, format!("constant-{}.json", sanitize_name(selector)))
                };
                inputs.push(InputFile::from_bytes(format!("inputs/{name}"), bytes));
            }
        }
    }
    if artifact.outputs.is_empty() {
        return Err("у compute-артефакта нет объявленных выходов".to_owned());
    }
    Ok(RunSpec {
        artifact_id: artifact.id.clone(),
        runtime: runtime.kind.clone(),
        entry: runtime.entry.clone(),
        artifact_dir: artifact_object.dir.clone(),
        inputs,
        outputs: artifact.outputs.clone(),
        timeout: artifact.timeout_s.map(Duration::from_secs),
        env: BTreeMap::new(),
    })
}

/// Уникальное имя входа в `inputs/` (коллизии базовых имён получают числовой префикс).
fn unique_input_name(used: &mut BTreeSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let mut counter = 1;
    loop {
        let candidate = format!("{counter}-{name}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

/// Безопасное имя файла для селектора константы (`R_н` → `R_н`).
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' { ch } else { '_' })
        .collect()
}

fn state_from_outcome(artifact: &ArtifactManifest, outcome: &RunOutcome, activation: &Activation, entries: &[TreeEntry]) -> ArtifactState {
    let mut outputs = Vec::with_capacity(entries.len());
    for entry in entries {
        outputs.push(OutputRecord {
            rel: entry.path.clone(),
            blob: entry.blob.hex(),
            mode: entry.mode,
            size: outcome
                .outputs
                .iter()
                .find(|output| output.rel == entry.path)
                .map(|output| output.size)
                .unwrap_or(0),
        });
    }
    ArtifactState {
        artifact_id: artifact.id.clone(),
        act_key: activation.act_key.hex(),
        in_hash: Some(activation.in_hash.hex()),
        script_hash: activation.script_hash.map(|hash| hash.hex()),
        params_hash: Some(activation.params_hash.hex()),
        env_hash: Some(activation.env_hash.hex()),
        schema_version: Some(activation.schema_version.clone()),
        out_hash: Some(tree_hash(entries).hex()),
        outputs,
        run_id: Some(outcome.run_id.clone()),
        updated_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

fn commit_update(
    store: &mut Store,
    artifact: &ArtifactManifest,
    outcome: &RunOutcome,
    activation: &Activation,
    entries: &[TreeEntry],
) -> Result<(String, ArtifactState), String> {
    let state = state_from_outcome(artifact, outcome, activation, entries);
    let payload = serde_json::json!({ "kind": "dag-update", "artifacts": [state] });
    let tx = store
        .begin_tx_with(&format!("dag update: {}", artifact.title), Some(payload))
        .map_err(|e| e.to_string())?;
    if let Err(error) = store.put_tree(entries).map_err(|e| e.to_string()) {
        let _ = store.fail_tx(&tx, &error);
        return Err(error);
    }
    match store.commit_tx(&tx) {
        Ok(()) => Ok((tx, state)),
        Err(error) => {
            let message = error.to_string();
            let _ = store.fail_tx(&tx, &message);
            Err(message)
        }
    }
}

/// Обновить устаревшие артефакты в топологическом порядке.
///
/// Каждый успешный запуск фиксируется отдельной транзакцией (CAS: блобы + дерево; ledger — payload
/// транзакции). При ошибке ledger не меняется; уже успешно обновлённые артефакты остаются в новом
/// состоянии (прогрессивное обновление, см. §L плана).
pub fn update(project: &Project, store: &mut Store, options: &UpdateOptions) -> Result<UpdateReport, String> {
    let mut ledger = ledger(store)?;
    let order = topological_compute(project)?;
    let allowed: Option<HashSet<String>> = options.targets.as_ref().map(|targets| {
        if options.include_dependents {
            dependents_closure(project, targets)
        } else {
            targets.iter().cloned().collect()
        }
    });
    let mut report = UpdateReport::default();
    let artifacts: HashMap<&str, &ArtifactManifest> = project.artifacts().map(|a| (a.id.as_str(), a)).collect();

    for artifact_id in order {
        if let Some(allowed) = &allowed {
            if !allowed.contains(&artifact_id) {
                continue;
            }
        }
        let Some(artifact) = artifacts.get(artifact_id.as_str()).copied() else { continue };
        let artifact_object = object(project, &artifact.id)?;
        let activation = match activation(project, artifact, &ledger) {
            Ok(activation) => activation,
            Err(error) => {
                report.errors.push(format!("{}: {error}", artifact.title));
                report.artifacts.push(UpdatedArtifact {
                    artifact_id: artifact.id.clone(),
                    title: artifact.title.clone(),
                    action: UpdateAction::Failed,
                    message: error.clone(),
                    run_id: None,
                    act_key: None,
                    out_hash: None,
                    log_path: None,
                    duration_ms: 0,
                });
                if !options.continue_on_error {
                    return Ok(report);
                }
                continue;
            }
        };
        let state = ledger.get(&artifact.id);
        let forced = options.force && options.targets.as_ref().is_some_and(|targets| targets.contains(&artifact.id));
        if !forced && state.is_some_and(|state| stale_reasons(&artifact_object.dir, state, &activation).is_empty()) {
            report.artifacts.push(UpdatedArtifact {
                artifact_id: artifact.id.clone(),
                title: artifact.title.clone(),
                action: UpdateAction::Skipped,
                message: "актуален".to_owned(),
                run_id: state.and_then(|s| s.run_id.clone()),
                act_key: Some(activation.act_key.hex()),
                out_hash: state.and_then(|s| s.out_hash.clone()),
                log_path: None,
                duration_ms: 0,
            });
            continue;
        }

        let spec = match build_spec(project, artifact) {
            Ok(spec) => spec,
            Err(error) => {
                report.errors.push(format!("{}: {error}", artifact.title));
                report.artifacts.push(UpdatedArtifact {
                    artifact_id: artifact.id.clone(),
                    title: artifact.title.clone(),
                    action: UpdateAction::Failed,
                    message: error.clone(),
                    run_id: None,
                    act_key: Some(activation.act_key.hex()),
                    out_hash: None,
                    log_path: None,
                    duration_ms: 0,
                });
                if !options.continue_on_error {
                    return Ok(report);
                }
                continue;
            }
        };
        let outcome = match run(&project.root, &spec, None) {
            Ok(outcome) => outcome,
            Err(error) => {
                report.errors.push(format!("{}: {error}", artifact.title));
                report.artifacts.push(UpdatedArtifact {
                    artifact_id: artifact.id.clone(),
                    title: artifact.title.clone(),
                    action: UpdateAction::Failed,
                    message: error.clone(),
                    run_id: None,
                    act_key: Some(activation.act_key.hex()),
                    out_hash: None,
                    log_path: None,
                    duration_ms: 0,
                });
                if !options.continue_on_error {
                    return Ok(report);
                }
                continue;
            }
        };
        if outcome.status != RunStatus::Ok {
            let message = outcome.error.clone().unwrap_or_else(|| format!("код возврата {:?}", outcome.exit_code));
            report.errors.push(format!("{}: {message}", artifact.title));
            report.artifacts.push(UpdatedArtifact {
                artifact_id: artifact.id.clone(),
                title: artifact.title.clone(),
                action: UpdateAction::Failed,
                message,
                run_id: Some(outcome.run_id.clone()),
                act_key: Some(activation.act_key.hex()),
                out_hash: None,
                log_path: Some(outcome.stderr_log.clone()),
                duration_ms: outcome.duration_ms,
            });
            if !options.continue_on_error {
                return Ok(report);
            }
            continue;
        }

        // Складываем выходы в CAS, копируем обратно в каталог артефакта, фиксируем транзакцию.
        let mut entries = Vec::with_capacity(outcome.outputs.len());
        let mut write_error = None;
        for output in &outcome.outputs {
            match fs::read(&output.path) {
                Ok(bytes) => {
                    let blob = store.put_blob(&bytes).map_err(|e| e.to_string())?;
                    let dest = artifact_object.dir.join(&output.rel);
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if let Err(error) = fs::copy(&output.path, &dest) {
                        write_error = Some(format!("скопировать выход {}: {error}", output.rel));
                        break;
                    }
                    entries.push(TreeEntry::new(output.rel.clone(), 0o644, blob));
                }
                Err(error) => {
                    write_error = Some(format!("прочитать выход {}: {error}", output.rel));
                    break;
                }
            }
        }
        if let Some(error) = write_error {
            report.errors.push(format!("{}: {error}", artifact.title));
            report.artifacts.push(UpdatedArtifact {
                artifact_id: artifact.id.clone(),
                title: artifact.title.clone(),
                action: UpdateAction::Failed,
                message: error.clone(),
                run_id: Some(outcome.run_id.clone()),
                act_key: Some(activation.act_key.hex()),
                out_hash: None,
                log_path: Some(outcome.stderr_log.clone()),
                duration_ms: outcome.duration_ms,
            });
            if !options.continue_on_error {
                return Ok(report);
            }
            continue;
        }

        match commit_update(store, artifact, &outcome, &activation, &entries) {
            Ok((tx, state)) => {
                ledger.insert(artifact.id.clone(), state.clone());
                report.tx_ids.push(tx);
                // R-VCS-02/§7.3 п.10: out_hash — идентичность выходов для git-диффа.
                if let Some(out_hash) = &state.out_hash {
                    if let Err(error) = rde_core::record_artifact_out_hash(&artifact_object.dir, out_hash) {
                        report.errors.push(format!("{}: out_hash не записан в artifact.toml: {error}", artifact.title));
                    }
                }
                report.artifacts.push(UpdatedArtifact {
                    artifact_id: artifact.id.clone(),
                    title: artifact.title.clone(),
                    action: UpdateAction::Ran,
                    message: format!("обновлён за {} мс", outcome.duration_ms),
                    run_id: Some(outcome.run_id.clone()),
                    act_key: Some(activation.act_key.hex()),
                    out_hash: state.out_hash.clone(),
                    log_path: Some(outcome.stdout_log.clone()),
                    duration_ms: outcome.duration_ms,
                });
            }
            Err(error) => {
                report.errors.push(format!("{}: {error}", artifact.title));
                report.artifacts.push(UpdatedArtifact {
                    artifact_id: artifact.id.clone(),
                    title: artifact.title.clone(),
                    action: UpdateAction::Failed,
                    message: error.clone(),
                    run_id: Some(outcome.run_id.clone()),
                    act_key: Some(activation.act_key.hex()),
                    out_hash: None,
                    log_path: Some(outcome.stdout_log.clone()),
                    duration_ms: outcome.duration_ms,
                });
                if !options.continue_on_error {
                    return Ok(report);
                }
            }
        }
    }
    Ok(report)
}

/// Удобный хелпер: идентификаторы устаревших compute-артефактов в топологическом порядке.
pub fn stale_artifacts(project: &Project, store: &Store) -> Result<Vec<String>, String> {
    let statuses = evaluate(project, store)?;
    let order = topological_compute(project)?;
    Ok(order
        .into_iter()
        .filter(|id| matches!(statuses.get(id).map(|s| &s.status), Some(LiveStatus::Stale(_) | LiveStatus::NeverRun)))
        .collect())
}
