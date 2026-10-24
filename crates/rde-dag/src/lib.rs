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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactState {
    pub artifact_id: String,
    pub act_key: String,
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
}

/// Живой статус артефакта (считается из хешей и файлов, а не эвристик).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveStatus {
    Fresh,
    Stale(String),
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

    pub fn reason(&self) -> Option<&str> {
        match self {
            LiveStatus::Stale(reason) | LiveStatus::NotComputable(reason) => Some(reason),
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

/// Хеш входа-ссылки (артефакт/заметка/источник). Файлы обрабатываются в [`activation`].
fn input_hash(
    project: &Project,
    ledger: &HashMap<String, ArtifactState>,
    input: &InputSpec,
) -> Result<(String, Hash), String> {
    match input {
        InputSpec::Artifact { artifact, selector } => {
            let dep = object(project, artifact)?;
            if selector.is_empty() {
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
            let path = dep.dir.join(selector);
            match hash_file(&path) {
                Some(hash) => Ok((artifact.clone(), hash)),
                None => Ok((artifact.clone(), missing_hash(&format!("{artifact}/{selector}")))),
            }
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
    }
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
    for group in artifact.groups.iter().filter(|group| group.mode == "hashed") {
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
            };
            members.push(GroupMember { target, out: hash, selector });
        }
        groups.push(InGroup { name: group.name.clone(), mode: "hashed".into(), grp_hash: group_hash(&group.name, "hashed", &members) });
    }
    let in_hash = in_hash(&groups);

    let script_path = artifact_dir.join(&runtime.entry);
    let script = Some(script_hash(&runtime.kind, &runtime.entry, &fs::read(&script_path).unwrap_or_default()));
    let params = params_hash(&artifact.params)?;
    let env = env_hash(&runtime.kind, &platform(), None, &BTreeMap::new());
    let key = act_key(&artifact.kind, in_hash, script, params, env);
    Ok(Activation { artifact_id: artifact.id.clone(), act_key: key, in_hash, script_hash: script, params_hash: params, env_hash: env })
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

fn is_fresh(artifact_dir: &Path, state: Option<&ArtifactState>, act_key: Hash) -> Result<(), String> {
    let Some(state) = state else {
        return Err("не запускался".to_owned());
    };
    if state.act_key != act_key.hex() {
        return Err("изменились входы, скрипт или параметры".to_owned());
    }
    let Some(expected) = state.out_hash.as_deref().and_then(|hex| Hash::from_hex(hex).ok()) else {
        return Err("выходы не записаны".to_owned());
    };
    let Some(actual) = actual_out_hash(artifact_dir, state) else {
        return Err("выходы отсутствуют (удалены)".to_owned());
    };
    if actual != expected {
        return Err("выход изменён вручную".to_owned());
    }
    Ok(())
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
                    match is_fresh(&artifact_object.dir, state, activation.act_key) {
                        Ok(()) => (LiveStatus::Fresh, Some(activation.act_key.hex()), out_hash, updated, run_id),
                        Err(reason) if reason == "не запускался" => (LiveStatus::NeverRun, Some(activation.act_key.hex()), None, None, None),
                        Err(reason) => (LiveStatus::Stale(reason), Some(activation.act_key.hex()), out_hash, updated, run_id),
                    }
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
fn dependents_closure(project: &Project, targets: &[String]) -> HashSet<String> {
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
    /// Ограничить обновление целями + транзитивно зависимыми (None — все устаревшие).
    pub targets: Option<Vec<String>>,
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

/// Построить `RunSpec` артефакта: входы hashed-групп → `inputs/<basename>`, объявленные выходы.
fn build_spec(project: &Project, artifact: &ArtifactManifest) -> Result<RunSpec, String> {
    let artifact_object = object(project, &artifact.id)?;
    let runtime = artifact.runtime.as_ref().ok_or_else(|| "нет runtime".to_owned())?;
    let mut inputs = Vec::new();
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    for group in artifact.groups.iter().filter(|group| group.mode == "hashed") {
        for input in &group.inputs {
            let (source, rel_name) = match input {
                InputSpec::File { file } => (artifact_object.dir.join(file), file.clone()),
                InputSpec::Artifact { artifact: dep_id, selector } => {
                    if selector.is_empty() {
                        return Err(format!("у входа `{dep_id}` пустой selector"));
                    }
                    let dep = object(project, dep_id)?;
                    (dep.dir.join(selector), selector.clone())
                }
                InputSpec::Note { note } => {
                    let note_object = object(project, note)?;
                    if let ObjectData::Note(manifest) = &note_object.data {
                        (note_object.dir.join(&manifest.file), manifest.file.clone())
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
                        (source_object.dir.join(&file), file)
                    } else {
                        return Err(format!("{source} — не источник"));
                    }
                }
            };
            let mut name = baseline_name(&rel_name);
            let mut counter = 1;
            while used_names.contains(&name) {
                name = format!("{counter}-{}", baseline_name(&rel_name));
                counter += 1;
            }
            used_names.insert(name.clone());
            inputs.push(InputFile::new(source, format!("inputs/{name}")));
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

fn state_from_outcome(artifact: &ArtifactManifest, outcome: &RunOutcome, act_key: Hash, entries: &[TreeEntry]) -> ArtifactState {
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
        act_key: act_key.hex(),
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
    act_key: Hash,
    entries: &[TreeEntry],
) -> Result<(String, ArtifactState), String> {
    let state = state_from_outcome(artifact, outcome, act_key, entries);
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
    let allowed: Option<HashSet<String>> = options.targets.as_ref().map(|targets| dependents_closure(project, targets));
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
        if is_fresh(&artifact_object.dir, state, activation.act_key).is_ok() {
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

        match commit_update(store, artifact, &outcome, activation.act_key, &entries) {
            Ok((tx, state)) => {
                ledger.insert(artifact.id.clone(), state.clone());
                report.tx_ids.push(tx);
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
