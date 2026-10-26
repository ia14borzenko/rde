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
use rde_run::{run, InputFile, NetworkPolicy, RunOutcome, RunSpec, RunStatus};
use rde_store::{Store, TxRecord, TxStatus};
use serde::{Deserialize, Serialize};

/// Платформенная строка для `env_hash` (`RDE_PLATFORM` может переопределить).
pub fn platform() -> String {
    rde_env::platform()
}

/// Разрешённые окружения артефактов проекта (P3): ключ — `(runtime.kind, runtime.env)`.
#[derive(Clone, Debug, Default)]
pub struct EnvContext {
    locks: BTreeMap<(String, String), Result<rde_env::RuntimeLock, String>>,
}

impl EnvContext {
    /// Разрешить окружения для всех runtime, встречающихся у compute-артефактов.
    pub fn resolve(project: &Project) -> Self {
        let mut context = Self::default();
        for artifact in project.artifacts().filter(|artifact| artifact.kind == "compute") {
            let Some(runtime) = &artifact.runtime else { continue };
            let key = (runtime.kind.clone(), runtime.env.clone());
            context
                .locks
                .entry(key)
                .or_insert_with_key(|(kind, mode)| rde_env::resolve(&project.root, kind, mode));
        }
        context
    }

    /// Разрешённое окружение артефакта.
    pub fn lock(&self, runtime: &rde_core::RuntimeManifest) -> Result<&rde_env::RuntimeLock, String> {
        match self.locks.get(&(runtime.kind.clone(), runtime.env.clone())) {
            Some(Ok(lock)) => Ok(lock),
            Some(Err(error)) => Err(format!("окружение {}: {error}", runtime.kind)),
            None => Err(format!("окружение для runtime `{}` не разрешено", runtime.kind)),
        }
    }
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

/// Разрешённый hashed-вход артефакта — для provenance (`resolvedDependencies`, R-ART-08).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedInput {
    pub group: String,
    pub mode: String,
    /// Цель: id артефакта, `file:<rel>`, `note:<id>`, `source:<id>`, `constants:<id>`.
    pub target: String,
    /// URI вида `rde://artifact/<id>@<out_hash>` (или `rde://file/...@<sha256>`).
    pub uri: String,
    pub digest: String,
    pub selector: String,
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
    /// Digest разрешённого окружения (вход `env_hash`, R-RUN-07).
    pub env_lock_digest: String,
    pub schema_version: String,
    /// Разрешённые hashed-входы (для provenance).
    pub resolved: Vec<ResolvedInput>,
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
    /// Digest окружения, при котором актуален артефакт.
    pub env_lock_digest: Option<String>,
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
            // Откат к состоянию «ещё не запускался»: состояние артефакта удаляется из ledger.
            if item.get("removed").and_then(|value| value.as_bool()) == Some(true) {
                if let Some(artifact_id) = item.get("artifact_id").and_then(|value| value.as_str()) {
                    ledger.remove(artifact_id);
                }
                continue;
            }
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
            let (_json, hash) = resolve_constants(project, ledger, manifest, selector)?;
            Ok((format!("constants:{constants}"), hash))
        }
    }
}

/// Разрешённая константа/величина (ADR-0005, P1): значение с учётом `source`, обозначение,
/// формат и хеш, участвующий в `in_hash` зависимых артефактов.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConstant {
    pub constants_id: String,
    pub key: String,
    /// Разрешённое значение (для `source` — из выхода артефакта, иначе литерал `value`).
    pub value: f64,
    pub unit: String,
    pub display: String,
    pub style: String,
    pub symbol: String,
    pub subscript: String,
    pub format: String,
    /// Текст для документа (R-DOC-23): `display` / `symbol+subscript` / «значение + NBSP + единица».
    pub text: String,
    /// Rich-стиль docx-вставки (`italic`/`italic-subscript`).
    pub run_style: String,
    /// Digest источника (`out_hash` артефакта или sha256 файла) — только при `source`.
    pub source_digest: Option<String>,
    /// Хеш константы (совместим со старой схемой для литералов без новых полей).
    pub hash: Hash,
}

impl ResolvedConstant {
    /// JSON-представление для материализации входа-константы в скрипт (`inputs/constants.json`).
    pub fn json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("value".to_owned(), serde_json::json!(self.value));
        map.insert("unit".to_owned(), serde_json::json!(self.unit));
        map.insert("display".to_owned(), serde_json::json!(self.display));
        map.insert("style".to_owned(), serde_json::json!(self.style));
        if !self.symbol.is_empty() {
            map.insert("symbol".to_owned(), serde_json::json!(self.symbol));
        }
        if !self.subscript.is_empty() {
            map.insert("subscript".to_owned(), serde_json::json!(self.subscript));
        }
        if !self.format.is_empty() {
            map.insert("format".to_owned(), serde_json::json!(self.format));
        }
        if let Some(digest) = &self.source_digest {
            map.insert("source_digest".to_owned(), serde_json::json!(digest));
        }
        serde_json::Value::Object(map)
    }
}

/// Найти артефакт-набор констант по id.
fn constants_artifact<'a>(project: &'a Project, id: &str) -> Result<&'a ArtifactManifest, String> {
    let target = object(project, id)?;
    let ObjectData::Artifact(manifest) = &target.data else {
        return Err(format!("{id} — не артефакт"));
    };
    if manifest.kind != "constants" {
        return Err(format!("{id} не является объектом констант (type = {})", manifest.kind));
    }
    Ok(manifest)
}

/// Расширенная ли константа (есть `source` или новые поля обозначения/формата). Для литеральных
/// констант без новых полей хеш остаётся прежним — совместимость `act_key` (ADR-0005 §1.3).
fn constant_is_extended(value: &rde_core::ConstantValue) -> bool {
    value.source.is_some() || !value.symbol.is_empty() || !value.subscript.is_empty() || !value.format.is_empty()
}

/// Значение из `source` (выход артефакта) и digest источника (закреплённый `out_hash`,
/// текущий `out_hash` из ledger или sha256 файла).
fn source_value_and_digest(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    value: &rde_core::ConstantValue,
) -> Result<(f64, String), String> {
    let Some(source) = value.source_parsed()? else {
        return Err("source не задан".to_owned());
    };
    let target = object(project, &source.artifact_id)?;
    let path = target.dir.join(&source.file);
    let bytes = fs::read(&path).map_err(|e| format!("прочитать источник значения {}: {e}", path.display()))?;
    let digest = match &source.out_hash {
        Some(pinned) if !pinned.is_empty() => pinned.clone(),
        _ => match ledger.get(&source.artifact_id).and_then(|state| state.out_hash.clone()) {
            Some(current) => current,
            None => sha256(&bytes).hex(),
        },
    };
    let numeric = match &source.pointer {
        Some(pointer) => {
            let json: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("{} не JSON: {e}", path.display()))?;
            match json.pointer(pointer) {
                Some(serde_json::Value::Number(number)) => {
                    number.as_f64().ok_or_else(|| format!("JSON pointer `{pointer}` — не число"))?
                }
                Some(serde_json::Value::String(text)) => text
                    .trim()
                    .parse::<f64>()
                    .map_err(|e| format!("JSON pointer `{pointer}` не число: {e}"))?,
                Some(other) => return Err(format!("JSON pointer `{pointer}` дал не число: {other}")),
                None => return Err(format!("JSON pointer `{pointer}` не найден в {}", source.file)),
            }
        }
        None => String::from_utf8_lossy(&bytes)
            .trim()
            .parse::<f64>()
            .map_err(|e| format!("{} не число: {e}", source.file))?,
    };
    Ok((numeric, digest))
}

/// JSON одной константы с разрешённым значением (для материализации/хеша).
fn constant_json(value: &rde_core::ConstantValue, resolved: f64, digest: Option<&str>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("value".to_owned(), serde_json::json!(resolved));
    map.insert("unit".to_owned(), serde_json::json!(value.unit));
    map.insert("display".to_owned(), serde_json::json!(value.display));
    map.insert("style".to_owned(), serde_json::json!(value.style));
    if !value.symbol.is_empty() {
        map.insert("symbol".to_owned(), serde_json::json!(value.symbol));
    }
    if !value.subscript.is_empty() {
        map.insert("subscript".to_owned(), serde_json::json!(value.subscript));
    }
    if !value.format.is_empty() {
        map.insert("format".to_owned(), serde_json::json!(value.format));
    }
    if let Some(source) = &value.source {
        map.insert("source".to_owned(), serde_json::json!(source));
    }
    if let Some(digest) = digest {
        map.insert("source_digest".to_owned(), serde_json::json!(digest));
    }
    serde_json::Value::Object(map)
}

/// Значение константы для `in_hash`: `source` разрешается, если доступен; ошибка источника не
/// делает артефакт «невычислимым» (как missing_hash у входов, §7.3 п.2) — используется кэш `value`
/// и sentinel-digest, детерминированный по URI источника.
fn source_or_cached(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    value: &rde_core::ConstantValue,
) -> (f64, Option<String>) {
    match &value.source {
        None => (value.value, None),
        Some(uri) => match source_value_and_digest(project, ledger, value) {
            Ok((resolved, digest)) => (resolved, Some(digest)),
            Err(_error) => {
                let sentinel = sha256(format!("rde:missing:constant-source:{uri}")).hex();
                (value.value, Some(format!("missing:{sentinel}")))
            }
        },
    }
}

/// Разрешить набор констант (все — при пустом `selector`, одну — по имени):
/// JSON для материализации входа + хеш для `in_hash`. Литеральные константы без новых полей
/// хешируются прежней схемой (`sha256(JCS(ConstantValue))`), расширенные — `rde:const:1`.
fn resolve_constants(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    manifest: &rde_core::ArtifactManifest,
    selector: &str,
) -> Result<(serde_json::Value, Hash), String> {
    let selector = selector.trim();
    if selector.is_empty() {
        let mut map = serde_json::Map::new();
        let mut extended = false;
        for (key, value) in &manifest.values {
            if constant_is_extended(value) {
                extended = true;
                let (resolved, digest) = source_or_cached(project, ledger, value);
                map.insert(key.clone(), constant_json(value, resolved, digest.as_deref()));
            } else {
                map.insert(
                    key.clone(),
                    serde_json::to_value(value).map_err(|e| format!("константа `{key}`: {e}"))?,
                );
            }
        }
        let json = serde_json::Value::Object(map);
        let canonical = rde_hash::canonical_json(&json)?;
        let hash = if extended {
            rde_hash::constants_hash(&canonical)
        } else {
            sha256(canonical.as_bytes())
        };
        Ok((json, hash))
    } else {
        let value = manifest
            .values
            .get(selector)
            .ok_or_else(|| format!("константа `{selector}` не найдена"))?;
        if constant_is_extended(value) {
            let (resolved, digest) = source_or_cached(project, ledger, value);
            let json = constant_json(value, resolved, digest.as_deref());
            let canonical = rde_hash::canonical_json(&json)?;
            Ok((json, rde_hash::constants_hash(&canonical)))
        } else {
            let json = serde_json::to_value(value).map_err(|e| format!("константа `{selector}`: {e}"))?;
            let canonical = rde_hash::canonical_json(&json)?;
            Ok((json, sha256(canonical.as_bytes())))
        }
    }
}

/// Разрешить константу/величину для UI/документа (ADR-0005, P1): значение (с `source`),
/// обозначение, единица, формат, текст и хеш.
pub fn resolve_constant(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    constants_id: &str,
    key: &str,
) -> Result<ResolvedConstant, String> {
    let manifest = constants_artifact(project, constants_id)?;
    let value = manifest
        .values
        .get(key)
        .ok_or_else(|| format!("константа `{key}` не найдена в {}", manifest.title))?;
    let (resolved, source_digest) = match &value.source {
        Some(_) => {
            let (resolved, digest) = source_value_and_digest(project, ledger, value)?;
            (resolved, Some(digest))
        }
        None => (value.value, None),
    };
    let canonical = rde_hash::canonical_json(&constant_json(value, resolved, source_digest.as_deref()))?;
    let hash = if constant_is_extended(value) {
        rde_hash::constants_hash(&canonical)
    } else {
        sha256(canonical.as_bytes())
    };
    Ok(ResolvedConstant {
        constants_id: constants_id.to_owned(),
        key: key.to_owned(),
        value: resolved,
        unit: value.unit.clone(),
        display: value.display.clone(),
        style: value.style.clone(),
        symbol: value.symbol.clone(),
        subscript: value.subscript.clone(),
        format: value.format.clone(),
        text: rde_core::value_format::constant_text(
            resolved,
            &value.unit,
            &value.display,
            &value.symbol,
            &value.subscript,
            &value.format,
        ),
        run_style: rde_core::value_format::constant_run_style(&value.style, &value.symbol, &value.subscript),
        source_digest,
        hash,
    })
}

/// Разрешить значение набора констант для материализации входа скрипта: `selector` пуст —
/// весь набор (`inputs/constants.json`), иначе одна константа.
pub fn resolved_constants_json(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    manifest: &rde_core::ArtifactManifest,
    selector: &str,
) -> Result<serde_json::Value, String> {
    resolve_constants(project, ledger, manifest, selector).map(|(json, _hash)| json)
}

/// Хеш констант для `in_hash` зависимого артефакта (ADR-0005 P1): для литеральных констант — прежняя
/// схема `sha256(JCS(ConstantValue))` (совместимость `act_key`), для расширенных (`source`,
/// обозначение, format) — `rde:const:1` с разрешённым значением и digest источника.
pub fn constants_input_hash(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    constants_id: &str,
    selector: &str,
) -> Result<Hash, String> {
    let manifest = constants_artifact(project, constants_id)?;
    resolve_constants(project, ledger, manifest, selector).map(|(_json, hash)| hash)
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
    env: &EnvContext,
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
    let mut resolved = Vec::new();
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
            let uri_target = match input {
                InputSpec::Artifact { artifact, .. } => format!("rde://artifact/{artifact}"),
                InputSpec::File { file } => format!("rde://file/{file}"),
                InputSpec::Note { note } => format!("rde://note/{note}"),
                InputSpec::Source { source } => format!("rde://source/{source}"),
                InputSpec::Constants { constants, .. } => format!("rde://constants/{constants}"),
            };
            resolved.push(ResolvedInput {
                group: group.name.clone(),
                mode: group.mode.clone(),
                target: target.clone(),
                uri: format!("{uri_target}@{}", hash.hex()),
                digest: hash.hex(),
                selector: selector.clone(),
            });
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
    // Окружение (P3): digest разрешённого окружения + объявленные переменные (R-RUN-07).
    let env_lock = env.lock(runtime)?;
    let lock_digest = Hash::from_hex(&env_lock.digest)
        .map_err(|e| format!("digest окружения `{}`: {e}", env_lock.digest))?;
    let env_hash = env_hash(&runtime.kind, &env_lock.platform, Some(lock_digest), &runtime.vars);
    let key = act_key(&artifact.kind, &artifact.schema, in_hash, script, params, env_hash);
    Ok(Activation {
        artifact_id: artifact.id.clone(),
        act_key: key,
        in_hash,
        script_hash: script,
        params_hash: params,
        env_hash,
        env_lock_digest: env_lock.digest.clone(),
        schema_version: artifact.schema.clone(),
        resolved,
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
    let env = EnvContext::resolve(project);
    let mut result = BTreeMap::new();
    for artifact_object in project.objects.iter().filter(|o| matches!(o.data, ObjectData::Artifact(_))) {
        let ObjectData::Artifact(artifact) = &artifact_object.data else { continue };
        let (status, act_key, out_hash, updated, run_id, env_digest) = if artifact.kind != "compute" {
            (LiveStatus::Fresh, None, None, None, None, None)
        } else {
            match activation(project, artifact, &ledger, &env) {
                Err(error) => (LiveStatus::NotComputable(error), None, None, None, None, None),
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
                    (
                        status,
                        Some(activation.act_key.hex()),
                        out_hash,
                        updated,
                        run_id,
                        Some(activation.env_lock_digest.clone()),
                    )
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
                env_lock_digest: env_digest,
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

#[derive(Clone, Default)]
pub struct UpdateOptions {
    /// Ограничить обновление целями (None — все устаревшие).
    pub targets: Option<Vec<String>>,
    /// Для `targets`: включить транзитивно зависимые артефакты («обновить связанные»).
    /// `false` — обновить только сами цели («обновить артефакт»).
    pub include_dependents: bool,
    /// Принудительно пересчитать явно указанные цели, даже если они актуальны.
    pub force: bool,
    /// Одна транзакция на всю пачку обновлений (атомарный commit корней).
    /// `false` — по транзакции на артефакт (гранулярный откат).
    pub batch: bool,
    /// Продолжать после ошибки отдельного артефакта.
    pub continue_on_error: bool,
    /// Отмена обновления из UI: передаётся в rde-run (SIGTERM → grace → SIGKILL).
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl std::fmt::Debug for UpdateOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateOptions")
            .field("targets", &self.targets)
            .field("include_dependents", &self.include_dependents)
            .field("force", &self.force)
            .field("batch", &self.batch)
            .field("continue_on_error", &self.continue_on_error)
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
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
    /// Ошибки, из-за которых артефакт не обновлён (или предупреждения обновления).
    pub errors: Vec<String>,
    /// Информационные заметки (например, обновление `rde.lock`).
    pub notes: Vec<String>,
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
fn build_spec(
    project: &Project,
    artifact: &ArtifactManifest,
    ledger: &HashMap<String, ArtifactState>,
    env: &EnvContext,
    activation: &Activation,
) -> Result<RunSpec, String> {
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
                let value = resolved_constants_json(project, ledger, manifest, selector)?;
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
    // P3: интерпретатор/политика сети/согласие/провенанс из разрешённого окружения и проекта.
    let env_lock = env.lock(runtime)?;
    let interpreter = if env_lock.interpreter.trim().is_empty() {
        None
    } else if env_lock.interpreter.contains('/') {
        // Абсолютный путь: cwd процесса запуска — workdir, а не корень проекта.
        let project_root = project.root.canonicalize().unwrap_or_else(|_| project.root.clone());
        Some(project_root.join(&env_lock.interpreter).display().to_string())
    } else {
        Some(env_lock.interpreter.clone())
    };
    let network_allow = artifact.network_allow.unwrap_or(project.manifest.network_allow);
    Ok(RunSpec {
        artifact_id: artifact.id.clone(),
        runtime: runtime.kind.clone(),
        entry: runtime.entry.clone(),
        artifact_dir: artifact_object.dir.clone(),
        inputs,
        outputs: artifact.outputs.clone(),
        timeout: artifact.timeout_s.map(Duration::from_secs),
        env: runtime.vars.clone(),
        interpreter,
        network: if network_allow { NetworkPolicy::Allow } else { NetworkPolicy::Deny },
        trusted: project.manifest.trusted,
        initiator: "user".to_owned(),
        script_hash: activation.script_hash.map(|hash| hash.hex()),
        env_lock_digest: Some(env_lock.digest.clone()),
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

/// Записать `runs/<run_id>/provenance.json` (R-ART-08): SLSA-подобный statement с subject,
/// externalParameters, resolvedDependencies и runDetails (их пишет rde-run).
fn write_provenance(
    artifact: &ArtifactManifest,
    outcome: &RunOutcome,
    activation: &Activation,
    subject_out_hash: Option<&str>,
    error: Option<&str>,
) -> Result<PathBuf, String> {
    let run_dir = outcome
        .stdout_log
        .parent()
        .ok_or_else(|| format!("у лога {} нет каталога", outcome.stdout_log.display()))?;
    let details: serde_json::Value = fs::read(&outcome.run_details_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    let dependencies: Vec<serde_json::Value> = activation
        .resolved
        .iter()
        .map(|input| {
            serde_json::json!({
                "uri": input.uri,
                "digest": { "sha256": input.digest },
                "group": input.group,
                "selector": input.selector,
                "mode": input.mode,
            })
        })
        .collect();
    let subject: Vec<serde_json::Value> = match subject_out_hash {
        Some(hash) => vec![serde_json::json!({ "name": artifact.id, "digest": { "sha256": hash } })],
        None => Vec::new(),
    };
    let provenance = serde_json::json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": subject,
        "predicateType": "https://rde.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": "rde/dag-update@1",
                "externalParameters": {
                    "artifact": artifact.id,
                    "title": artifact.title,
                    "type": artifact.kind,
                    "schema_version": artifact.schema,
                    "act_key": activation.act_key.hex(),
                    "in_hash": activation.in_hash.hex(),
                    "script_hash": activation.script_hash.map(|hash| hash.hex()),
                    "params_hash": activation.params_hash.hex(),
                    "env_hash": activation.env_hash.hex(),
                    "env_lock_digest": activation.env_lock_digest,
                    "params": artifact.params,
                    "initiator": details.get("initiator").cloned().unwrap_or(serde_json::json!("user")),
                },
                "resolvedDependencies": dependencies,
            },
            "runDetails": details,
            "error": error,
        }
    });
    let path = run_dir.join("provenance.json");
    let bytes = serde_json::to_vec_pretty(&provenance).map_err(|e| format!("json provenance: {e}"))?;
    fs::write(&path, bytes).map_err(|e| format!("записать {}: {e}", path.display()))?;
    Ok(path)
}

/// Имя корня артефакта в store: `artifact/<id>` → tree_hash его выходов.
pub fn artifact_root_name(artifact_id: &str) -> String {
    format!("artifact/{artifact_id}")
}

/// Зафиксировать обновление артефакта транзакцией с переключением корня (`artifact/<id>`).
fn commit_update(
    store: &mut Store,
    artifact: &ArtifactManifest,
    outcome: &RunOutcome,
    activation: &Activation,
    entries: &[TreeEntry],
) -> Result<(String, ArtifactState), String> {
    let state = state_from_outcome(artifact, outcome, activation, entries);
    let payload = serde_json::json!({ "kind": "dag-update", "artifacts": [state] });
    let tree = tree_hash(entries);
    let mut roots = store.current_roots().map_err(|e| e.to_string())?;
    roots.insert(artifact_root_name(&artifact.id), tree);
    let tx = store
        .begin_tx_with_roots(&format!("dag update: {}", artifact.title), Some(payload), roots)
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

/// Зафиксировать разрешённое окружение артефакта в `rde.lock` (R-RUN-02): версии/lock-файлы
/// становятся частью проекта и git-истории; `env_hash` считается из фактического окружения.
fn record_env_lock(project: &Project, artifact: &ArtifactManifest, env: &EnvContext, report: &mut UpdateReport) {
    let Some(runtime) = &artifact.runtime else { return };
    let Ok(lock) = env.lock(runtime) else { return };
    match rde_env::update_entry(&project.root, lock) {
        Ok(true) => report.notes.push(format!("{}: rde.lock обновлён ({} {})", artifact.title, lock.runtime, lock.version)),
        Ok(false) => {}
        Err(error) => report.errors.push(format!("{}: rde.lock не записан: {error}", artifact.title)),
    }
}

/// Успешный запуск, ожидающий commit в режиме одной транзакции на пачку.
struct PendingCommit {
    artifact_id: String,
    title: String,
    state: ArtifactState,
    tree: Hash,
    run_id: String,
    duration_ms: u64,
    stdout_log: PathBuf,
    act_key: String,
    outcome: RunOutcome,
    activation: Activation,
}

/// Зафиксировать пачку обновлений одной транзакцией (§7.3 п.7): все корни переключаются атомарно.
fn commit_batch(store: &mut Store, pending: &[PendingCommit]) -> Result<String, String> {
    let states: Vec<ArtifactState> = pending.iter().map(|item| item.state.clone()).collect();
    let payload = serde_json::json!({ "kind": "dag-update", "batch": true, "artifacts": states });
    let mut roots = store.current_roots().map_err(|e| e.to_string())?;
    for item in pending {
        roots.insert(artifact_root_name(&item.artifact_id), item.tree);
    }
    let tx = store
        .begin_tx_with_roots("dag update: пачка", Some(payload), roots)
        .map_err(|e| e.to_string())?;
    match store.commit_tx(&tx) {
        Ok(()) => Ok(tx),
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
    let env = EnvContext::resolve(project);
    let order = topological_compute(project)?;
    // R-RUN-06: первый запуск скриптов требует явного согласия «проект доверенный».
    if !project.manifest.trusted {
        return Err(
            "запуск скриптов требует согласия: проект не помечен доверенным (trusted = true в rde.toml)"
                .to_owned(),
        );
    }
    let allowed: Option<HashSet<String>> = options.targets.as_ref().map(|targets| {
        if options.include_dependents {
            dependents_closure(project, targets)
        } else {
            targets.iter().cloned().collect()
        }
    });
    let mut report = UpdateReport::default();
    let mut pending: Vec<PendingCommit> = Vec::new();
    let artifacts: HashMap<&str, &ArtifactManifest> = project.artifacts().map(|a| (a.id.as_str(), a)).collect();

    for artifact_id in order {
        if options
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed))
        {
            report.errors.push("обновление отменено пользователем".to_owned());
            break;
        }
        if let Some(allowed) = &allowed {
            if !allowed.contains(&artifact_id) {
                continue;
            }
        }
        let Some(artifact) = artifacts.get(artifact_id.as_str()).copied() else { continue };
        let artifact_object = object(project, &artifact.id)?;
        let activation = match activation(project, artifact, &ledger, &env) {
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
        // force: при явных целях — пересчитать их; при targets = None — пересчитать всё.
        let forced = options.force
            && match &options.targets {
                Some(targets) => targets.contains(&artifact.id),
                None => true,
            };
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

        let spec = match build_spec(project, artifact, &ledger, &env, &activation) {
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
        let outcome = match run(&project.root, &spec, options.cancel.clone()) {
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
            if let Err(error) = write_provenance(artifact, &outcome, &activation, None, Some(&message)) {
                report.errors.push(format!("{}: provenance не записан: {error}", artifact.title));
            }
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

        if options.batch {
            // Пачка: деревья пишем сразу, состояния/корни фиксируем одной транзакцией в конце.
            if let Err(error) = store.put_tree(&entries).map_err(|e| e.to_string()) {
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
            let state = state_from_outcome(artifact, &outcome, &activation, &entries);
            ledger.insert(artifact.id.clone(), state.clone());
            pending.push(PendingCommit {
                artifact_id: artifact.id.clone(),
                title: artifact.title.clone(),
                state,
                tree: tree_hash(&entries),
                run_id: outcome.run_id.clone(),
                duration_ms: outcome.duration_ms,
                stdout_log: outcome.stdout_log.clone(),
                act_key: activation.act_key.hex(),
                outcome: outcome.clone(),
                activation: activation.clone(),
            });
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
                if let Err(error) = write_provenance(artifact, &outcome, &activation, state.out_hash.as_deref(), None) {
                    report.errors.push(format!("{}: provenance не записан: {error}", artifact.title));
                }
                record_env_lock(project, artifact, &env, &mut report);
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
    if options.batch && !pending.is_empty() {
        match commit_batch(store, &pending) {
            Ok(tx) => {
                report.tx_ids.push(tx);
                for item in pending {
                    if let Some(artifact) = artifacts.get(item.artifact_id.as_str()) {
                        record_env_lock(project, artifact, &env, &mut report);
                        if let Err(error) =
                            write_provenance(artifact, &item.outcome, &item.activation, item.state.out_hash.as_deref(), None)
                        {
                            report.errors.push(format!("{}: provenance не записан: {error}", item.title));
                        }
                    }
                    if let Some(out_hash) = &item.state.out_hash {
                        let dir = artifacts
                            .get(item.artifact_id.as_str())
                            .and_then(|artifact| object(project, &artifact.id).ok())
                            .map(|object| object.dir.clone());
                        if let Some(dir) = dir {
                            if let Err(error) = rde_core::record_artifact_out_hash(&dir, out_hash) {
                                report.errors.push(format!("{}: out_hash не записан в artifact.toml: {error}", item.title));
                            }
                        }
                    }
                    report.artifacts.push(UpdatedArtifact {
                        artifact_id: item.artifact_id,
                        title: item.title,
                        action: UpdateAction::Ran,
                        message: format!("обновлён за {} мс", item.duration_ms),
                        run_id: Some(item.run_id),
                        act_key: Some(item.act_key),
                        out_hash: item.state.out_hash.clone(),
                        log_path: Some(item.stdout_log),
                        duration_ms: item.duration_ms,
                    });
                }
            }
            Err(error) => {
                report.errors.push(format!("транзакция пачки не зафиксирована: {error}"));
                for item in pending {
                    if let Some(artifact) = artifacts.get(item.artifact_id.as_str()) {
                        let _ = write_provenance(artifact, &item.outcome, &item.activation, None, Some(&error));
                    }
                    report.artifacts.push(UpdatedArtifact {
                        artifact_id: item.artifact_id,
                        title: item.title,
                        action: UpdateAction::Failed,
                        message: error.clone(),
                        run_id: Some(item.run_id),
                        act_key: Some(item.act_key),
                        out_hash: None,
                        log_path: Some(item.stdout_log),
                        duration_ms: item.duration_ms,
                    });
                }
            }
        }
    }
    Ok(report)
}

/// Состояние ledger до указанной транзакции (для отката: каким был артефакт раньше).
fn ledger_before(store: &Store, before_txid: &str) -> Result<HashMap<String, ArtifactState>, String> {
    let mut txs: Vec<TxRecord> = store.list_txs().map_err(|e| e.to_string())?;
    txs.retain(|tx| tx.status == TxStatus::Committed);
    txs.sort_by(|a, b| a.started_unix.cmp(&b.started_unix).then_with(|| a.id.cmp(&b.id)));
    let mut ledger = HashMap::new();
    for tx in txs {
        // Останавливаемся на целевой транзакции: нужны состояния строго до неё.
        if tx.id == before_txid {
            break;
        }
        let is_dag_update = tx
            .data
            .as_ref()
            .and_then(|data| data.get("kind"))
            .and_then(|kind| kind.as_str())
            == Some("dag-update");
        if !is_dag_update {
            continue;
        }
        let Some(data) = tx.data else { continue };
        let Some(items) = data.get("artifacts").and_then(|value| value.as_array()) else { continue };
        for item in items {
            if item.get("removed").and_then(|value| value.as_bool()) == Some(true) {
                if let Some(artifact_id) = item.get("artifact_id").and_then(|value| value.as_str()) {
                    ledger.remove(artifact_id);
                }
                continue;
            }
            if let Ok(state) = serde_json::from_value::<ArtifactState>(item.clone()) {
                ledger.insert(state.artifact_id.clone(), state);
            }
        }
    }
    Ok(ledger)
}

/// Результат отката обновления артефактов.
#[derive(Clone, Debug, Default)]
pub struct RollbackOutcome {
    /// Транзакция store, переключившая корни.
    pub rollback_tx: String,
    /// Транзакция ledger, вернувшая состояния артефактов.
    pub ledger_tx: String,
    pub rolled_back_tx: String,
    /// Артефакты, у которых восстановлены выходы/состояние.
    pub restored: Vec<String>,
    /// Артефакты, вернувшиеся в состояние «ещё не запускался».
    pub reset: Vec<String>,
}

/// Откатить обновление (последнее или конкретную транзакцию): store переключает корни O(1),
/// dag материализует прежние выходы из CAS в каталоги артефактов и возвращает ledger-состояния.
pub fn rollback(project: &Project, store: &mut Store, txid: Option<&str>) -> Result<Option<RollbackOutcome>, String> {
    let report = match txid {
        Some(txid) => store.rollback(txid).map_err(|e| e.to_string())?,
        None => match store.rollback_latest().map_err(|e| e.to_string())? {
            Some(report) => report,
            None => return Ok(None),
        },
    };
    let rolled_tx = store
        .read_tx(&report.rolled_back_tx)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("транзакция {} не найдена", report.rolled_back_tx))?;
    let history = ledger_before(store, &report.rolled_back_tx)?;
    let mut outcome = RollbackOutcome {
        rollback_tx: report.rollback_tx.clone(),
        ledger_tx: String::new(),
        rolled_back_tx: report.rolled_back_tx.clone(),
        restored: Vec::new(),
        reset: Vec::new(),
    };
    let mut ledger_items: Vec<serde_json::Value> = Vec::new();

    // Меняем только корни артефактов: document/<id> и прочие имена — зона rde-doc (P4).
    let mut names: BTreeSet<String> = report.previous.names().map(str::to_owned).collect();
    names.extend(report.restored.names().map(str::to_owned));
    for name in names {
        let Some(artifact_id) = name.strip_prefix("artifact/") else { continue };
        let Some(artifact_object) = project.artifact_object(artifact_id) else { continue };
        let prev_tree = report.previous.get(&name);
        let restored_tree = report.restored.get(&name);
        if prev_tree == restored_tree {
            continue;
        }
        // Материализуем восстановленное дерево выходов в каталог артефакта.
        let mut restored_paths: BTreeSet<String> = BTreeSet::new();
        if let Some(tree) = restored_tree {
            let entries = store.get_tree(tree).map_err(|e| e.to_string())?.ok_or_else(|| {
                format!("дерево {tree} отката отсутствует в store (вне окна отката?)")
            })?;
            for entry in &entries {
                let bytes = store
                    .get_blob(entry.blob)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("блоб {} отсутствует в CAS", entry.blob))?;
                let dest = artifact_object.dir.join(&entry.path);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("создать каталог выхода: {e}"))?;
                }
                fs::write(&dest, bytes).map_err(|e| format!("записать {}: {e}", dest.display()))?;
                restored_paths.insert(entry.path.clone());
            }
        }
        // Удаляем выходы, которых нет в восстановленной версии (в т.ч. все — при откате
        // к состоянию «ещё не запускался»).
        if let Some(prev_tree) = prev_tree {
            if let Some(prev_entries) = store.get_tree(prev_tree).map_err(|e| e.to_string())? {
                for entry in prev_entries {
                    if !restored_paths.contains(&entry.path) {
                        let _ = fs::remove_file(artifact_object.dir.join(&entry.path));
                    }
                }
            }
        }
        // Ledger: прежнее состояние или «ещё не запускался».
        match history.get(artifact_id) {
            Some(state) => {
                if let Some(out_hash) = &state.out_hash {
                    let _ = rde_core::record_artifact_out_hash(&artifact_object.dir, out_hash);
                }
                ledger_items.push(serde_json::to_value(state).map_err(|e| e.to_string())?);
                outcome.restored.push(artifact_id.to_owned());
            }
            None => {
                let _ = rde_core::clear_artifact_out_hash(&artifact_object.dir);
                ledger_items.push(serde_json::json!({ "removed": true, "artifact_id": artifact_id }));
                outcome.reset.push(artifact_id.to_owned());
            }
        }
    }

    if !ledger_items.is_empty() {
        let payload = serde_json::json!({
            "kind": "dag-update",
            "rollback": true,
            "rolled_back": report.rolled_back_tx,
            "artifacts": ledger_items,
        });
        let ledger_tx = store
            .begin_tx_with(&format!("dag rollback: {}", rolled_tx.note), Some(payload))
            .map_err(|e| e.to_string())?;
        store.commit_tx(&ledger_tx).map_err(|e| e.to_string())?;
        outcome.ledger_tx = ledger_tx;
    }
    Ok(Some(outcome))
}

/// Разобранная цель вставки документа: `rde://artifact/<id>[@<out_hash>]/<selector>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertTarget {
    pub artifact_id: String,
    /// Зафиксированный `out_hash` цели (если указан в URI).
    pub out_hash: Option<String>,
    /// Путь выхода + (для JSON) JSON pointer после `#`.
    pub selector: String,
}

/// Разобрать URI цели вставки (R-DOC-11).
pub fn parse_insert_target(uri: &str) -> Result<InsertTarget, String> {
    let rest = uri
        .strip_prefix("rde://artifact/")
        .ok_or_else(|| format!("URI вставки должен начинаться с rde://artifact/: {uri}"))?;
    let (head, selector) = match rest.split_once('/') {
        Some((head, selector)) => (head, selector.to_owned()),
        None => (rest, String::new()),
    };
    let (artifact_id, out_hash) = match head.split_once('@') {
        Some((id, hash)) => (id.to_owned(), Some(hash.to_owned())),
        None => (head.to_owned(), None),
    };
    if artifact_id.is_empty() || selector.is_empty() {
        return Err(format!("URI вставки неполный (нужен artifact/<id>/<selector>): {uri}"));
    }
    Ok(InsertTarget { artifact_id, out_hash, selector })
}

/// Текущий `out_hash` артефакта из ledger (None — data/constants или ещё не запускался).
fn artifact_out_hash(project: &Project, store: &Store, artifact_id: &str) -> Option<String> {
    let ledger = ledger(store).ok()?;
    let _ = project;
    ledger.get(artifact_id).and_then(|state| state.out_hash.clone())
}

/// Разрешить значение текстовой вставки: читает выход артефакта и применяет JSON pointer
/// (`out/stats.json#/stats/x1/mean`) или берёт текст файла. Возвращает (значение, hash цели).
pub fn resolve_insert_text(project: &Project, store: &Store, uri: &str) -> Result<(String, String), String> {
    let target = parse_insert_target(uri)?;
    let object = project.artifact_object(&target.artifact_id).ok_or_else(|| format!("артефакт {} не найден", target.artifact_id))?;
    let (file_part, pointer) = match target.selector.split_once('#') {
        Some((file, pointer)) => (file.to_owned(), Some(pointer.to_owned())),
        None => (target.selector.clone(), None),
    };
    let path = object.dir.join(&file_part);
    let bytes = fs::read(&path).map_err(|e| format!("прочитать выход {}: {e}", path.display()))?;
    let hash = target
        .out_hash
        .clone()
        .or_else(|| artifact_out_hash(project, store, &target.artifact_id))
        .unwrap_or_else(|| sha256(&bytes).hex());
    let value = match pointer {
        Some(pointer) => {
            let json: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("{} не JSON: {e}", path.display()))?;
            let value = json
                .pointer(&pointer)
                .ok_or_else(|| format!("JSON pointer `{pointer}` не найден в {file_part}"))?;
            match value {
                serde_json::Value::String(text) => text.clone(),
                // Сырое значение: формат отображения выбирает документ (rde_doc::format_value).
                other => other.to_string(),
            }
        }
        None => String::from_utf8_lossy(&bytes).trim_end_matches(['\n', '\r']).to_owned(),
    };
    Ok((value, hash))
}

/// Разрешить рисунок вставки: путь к файлу-выходу артефакта + расширение + hash цели.
pub fn resolve_insert_image(project: &Project, store: &Store, uri: &str) -> Result<(PathBuf, String, String), String> {
    let target = parse_insert_target(uri)?;
    let object = project.artifact_object(&target.artifact_id).ok_or_else(|| format!("артефакт {} не найден", target.artifact_id))?;
    let file_part = target.selector.split('#').next().unwrap_or(&target.selector).to_owned();
    let path = object.dir.join(&file_part);
    let bytes = fs::read(&path).map_err(|e| format!("прочитать рисунок {}: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "svg" | "png" | "jpg" | "jpeg" | "gif") {
        return Err(format!("выход {file_part} не похож на рисунок (svg/png/jpg)"));
    }
    let hash = target
        .out_hash
        .clone()
        .or_else(|| artifact_out_hash(project, store, &target.artifact_id))
        .unwrap_or_else(|| sha256(&bytes).hex());
    Ok((path, ext, hash))
}

/// Разрешить вставку константы (`rde://constants/<object_id>/<key>`, R-ART-09/R-DOC-11):
/// возвращает (текст для документа, подсказка rich-формата, hash значения).
/// Значение с `source` разрешается из выхода артефакта (ADR-0005, P1), digest входит в hash.
pub fn resolve_insert_constant(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    uri: &str,
) -> Result<(String, String, String), String> {
    let rest = uri
        .strip_prefix("rde://constants/")
        .ok_or_else(|| format!("URI константы должен начинаться с rde://constants/: {uri}"))?;
    let (object_id, key) = rest
        .split_once('/')
        .ok_or_else(|| format!("в URI константы нет ключа значения: {uri}"))?;
    let resolved = resolve_constant(project, ledger, object_id, key)?;
    Ok((resolved.text, resolved.run_style, resolved.hash.hex()))
}

/// Разрешить вставку таблицы (`rde://artifact/<id>@<out_hash>/<csv-путь>`): строки + hash данных.
pub fn resolve_insert_table(project: &Project, store: &Store, uri: &str) -> Result<(Vec<Vec<String>>, String), String> {
    let target = parse_insert_target(uri)?;
    let object = project
        .artifact_object(&target.artifact_id)
        .ok_or_else(|| format!("артефакт {} не найден", target.artifact_id))?;
    let file_part = target.selector.split('#').next().unwrap_or(&target.selector).to_owned();
    let path = object.dir.join(&file_part);
    let bytes = fs::read(&path).map_err(|e| format!("прочитать данные таблицы {}: {e}", path.display()))?;
    let hash = target
        .out_hash
        .clone()
        .or_else(|| artifact_out_hash(project, store, &target.artifact_id))
        .unwrap_or_else(|| sha256(&bytes).hex());
    let text = String::from_utf8_lossy(&bytes).to_string();
    Ok((parse_csv(&text), hash))
}

/// Простой CSV-парсер (запятая, кавычки, переносы строк) для таблиц из данных артефактов.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' if field.is_empty() && !quoted => quoted = true,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
            }
            other => field.push(other),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
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
