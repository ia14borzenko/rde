//! RDE content hashing (M2.2): каноническое представление, Merkle-деревья и формулы `act_key`.
//!
//! Реализация нормативной модели `tz-02` §7.2:
//! - `tree_hash` / `out_hash` — Merkle-дерево версии файлов (путь, режим, блоб);
//! - `group_hash` → `in_hash` — входы артефакта (группы `hashed`; входы `ref` в `in_hash` не входят);
//! - `script_hash`, `params_hash`, `env_hash`, `act_key`.
//!
//! Инженерная деталь (дополнение к §7.2): все компоненты пишутся в хеш **length-prefixed**
//! (u64 LE длина + байты), а строки внутри — через канонический JSON/UTF-8. Это исключает
//! неоднозначность склейки `a|b` vs `a|bc`; доменный префикс (`rde:*:1`) входит в хеш первым.
//!
//! `params_hash` использует канонический JSON: ключи объектов сортируются, массивы сохраняют
//! порядок, числа сериализуются `serde_json` (для используемых в манифестах типов это
//! детерминированно; полный RFC 8785/JCS — при появлении дробных чисел с неоднозначным
//! представлением, см. M2.3).

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

pub const HASH_LEN: usize = 32;

/// SHA-256 значение (сырые байты; hex — [`Hash::hex`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash([u8; HASH_LEN]);

impl Hash {
    pub fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(HASH_LEN * 2);
        for byte in self.0 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// Восстановить хеш из 64-символьной hex-строки (для чтения сохранённых деревьев).
    pub fn from_hex(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.len() != HASH_LEN * 2 {
            return Err(format!("hex-хеш должен быть {} символов, получено {}", HASH_LEN * 2, text.len()));
        }
        let mut bytes = [0u8; HASH_LEN];
        for (i, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
            let pair = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            bytes[i] = u8::from_str_radix(pair, 16).map_err(|e| format!("hex `{pair}`: {e}"))?;
        }
        Ok(Self(bytes))
    }

    /// Первые `n` hex-символов (для подписей/логов; не для сравнения).
    pub fn short(&self, n: usize) -> String {
        self.hex().chars().take(n).collect()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short(12))
    }
}

/// Сырой SHA-256 содержимого (хеш блоба CAS).
pub fn sha256(data: impl AsRef<[u8]>) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(data.as_ref());
    Hash(hasher.finalize().into())
}

/// Доменно-разделённый писатель: всё length-prefixed, домен первым.
struct Canonical {
    hasher: Sha256,
}

impl Canonical {
    fn new(domain: &str) -> Self {
        let mut this = Self { hasher: Sha256::new() };
        this.write_bytes(domain.as_bytes());
        this
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.hasher.update((bytes.len() as u64).to_le_bytes());
        self.hasher.update(bytes);
    }

    fn str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.hasher.update(value.to_le_bytes());
    }

    fn hash(&mut self, value: Hash) {
        self.write_bytes(value.as_bytes());
    }

    fn optional_hash(&mut self, value: Option<Hash>) {
        match value {
            Some(hash) => {
                self.hasher.update([1u8]);
                self.hash(hash);
            }
            None => self.hasher.update([0u8]),
        }
    }

    fn finish(self) -> Hash {
        Hash(self.hasher.finalize().into())
    }
}

/// Каноническое JSON-представление: ключи объектов сортируются, массивы — по порядку.
pub fn canonical_json(value: &serde_json::Value) -> Result<String, String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &serde_json::Value, out: &mut String) -> Result<(), String> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => out.push_str(&n.to_string()),
        serde_json::Value::String(s) => {
            out.push_str(&serde_json::to_string(s).map_err(|e| e.to_string())?);
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).map_err(|e| e.to_string())?);
                out.push(':');
                write_canonical(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Элемент Merkle-дерева версии файлов: относительный путь, режим, хеш содержимого.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub mode: u32,
    pub blob: Hash,
}

impl TreeEntry {
    pub fn new(path: impl Into<String>, mode: u32, blob: Hash) -> Self {
        Self { path: path.into(), mode, blob }
    }
}

/// `tree_hash` — Merkle-корень версии файлов (порядок входа не важен: сортировка по пути).
pub fn tree_hash(entries: &[TreeEntry]) -> Hash {
    let mut sorted: Vec<&TreeEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut canon = Canonical::new("rde:tree:1");
    canon.u32(sorted.len() as u32);
    for entry in sorted {
        canon.str(&entry.path);
        canon.u32(entry.mode);
        canon.hash(entry.blob);
    }
    canon.finish()
}

/// `out_hash` — хеш набора выходов артефакта (то же Merkle-дерево; отдельная функция для читаемости).
pub fn out_hash(entries: &[TreeEntry]) -> Hash {
    tree_hash(entries)
}

/// Вход группы `hashed`: цель (id артефакта/путь), хеш выхода/блоба, селектор.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupMember {
    pub target: String,
    pub out: Hash,
    pub selector: String,
}

/// `group_hash` — хеш группы входов `hashed` (сортировка по (target, out, selector)).
pub fn group_hash(name: &str, mode: &str, members: &[GroupMember]) -> Hash {
    let mut sorted: Vec<&GroupMember> = members.iter().collect();
    sorted.sort_by(|a, b| {
        a.target
            .cmp(&b.target)
            .then_with(|| a.out.as_bytes().cmp(b.out.as_bytes()))
            .then_with(|| a.selector.cmp(&b.selector))
    });
    let mut canon = Canonical::new("rde:grp:1");
    canon.str(name);
    canon.str(mode);
    canon.u32(sorted.len() as u32);
    for member in sorted {
        canon.str(&member.target);
        canon.hash(member.out);
        canon.str(&member.selector);
    }
    canon.finish()
}

/// Группа `in_hash`: имя, режим, хеш группы. `ref`-группы в `in_hash` не входят (см. §7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InGroup {
    pub name: String,
    pub mode: String,
    pub grp_hash: Hash,
}

/// `in_hash` — агрегат `hashed`-групп артефакта (порядок групп не важен).
pub fn in_hash(groups: &[InGroup]) -> Hash {
    let mut sorted: Vec<&InGroup> = groups.iter().filter(|g| g.mode == "hashed").collect();
    sorted.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.mode.cmp(&b.mode))
            .then_with(|| a.grp_hash.as_bytes().cmp(b.grp_hash.as_bytes()))
    });
    let mut canon = Canonical::new("rde:in:1");
    canon.u32(sorted.len() as u32);
    for group in sorted {
        canon.str(&group.name);
        canon.str(&group.mode);
        canon.hash(group.grp_hash);
    }
    canon.finish()
}

/// `script_hash` — исполняемый файл артефакта + runtime.
pub fn script_hash(runtime: &str, entry: &str, bytes: &[u8]) -> Hash {
    let mut canon = Canonical::new("rde:script:1");
    canon.str(runtime);
    canon.str(entry);
    canon.write_bytes(bytes);
    canon.finish()
}

/// `params_hash` — хеш параметров артефакта (канонический JSON).
pub fn params_hash(params: &serde_json::Value) -> Result<Hash, String> {
    let json = canonical_json(params)?;
    let mut canon = Canonical::new("rde:params:1");
    canon.str(&json);
    Ok(canon.finish())
}

/// `env_hash` — хеш окружения: runtime, платформа, lock-файл и явные переменные.
pub fn env_hash(runtime: &str, platform: &str, lockfile: Option<Hash>, vars: &BTreeMap<String, String>) -> Hash {
    let mut canon = Canonical::new("rde:env:1");
    canon.str(runtime);
    canon.str(platform);
    canon.optional_hash(lockfile);
    canon.u32(vars.len() as u32);
    for (key, value) in vars {
        canon.str(key);
        canon.str(value);
    }
    canon.finish()
}

/// `act_key` — ключ активации артефакта. `None`-компоненты (например, `ref`-группы) кодируются флагом.
pub fn act_key(kind: &str, in_hash: Hash, script_hash: Option<Hash>, params_hash: Hash, env_hash: Hash) -> Hash {
    let mut canon = Canonical::new("rde:act:1");
    canon.str(kind);
    canon.hash(in_hash);
    canon.optional_hash(script_hash);
    canon.hash(params_hash);
    canon.hash(env_hash);
    canon.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree(entries: &[(&str, u32, &[u8])]) -> Vec<TreeEntry> {
        entries
            .iter()
            .map(|(path, mode, bytes)| TreeEntry::new(*path, *mode, sha256(bytes)))
            .collect()
    }

    #[test]
    fn canonical_json_sorts_keys_and_keeps_arrays() {
        let a = canonical_json(&json!({"b": 1, "a": {"z": [3, 1], "y": "ы"}})).unwrap();
        let b = canonical_json(&json!({"a": {"y": "ы", "z": [3, 1]}, "b": 1})).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":{"y":"ы","z":[3,1]},"b":1}"#);
    }

    #[test]
    fn tree_hash_is_order_independent_and_sensitive() {
        let one = tree(&[("out/a.png", 0o644, b"a"), ("out/b.svg", 0o644, b"b")]);
        let two = tree(&[("out/b.svg", 0o644, b"b"), ("out/a.png", 0o644, b"a")]);
        assert_eq!(tree_hash(&one), tree_hash(&two));
        let mut changed = one.clone();
        changed[0].blob = sha256(b"other");
        assert_ne!(tree_hash(&one), tree_hash(&changed));
        let empty = tree_hash(&[]);
        assert_ne!(empty, tree_hash(&one));
    }

    #[test]
    fn group_and_in_hash_are_order_independent() {
        let hash_a = sha256(b"a");
        let hash_b = sha256(b"b");
        let m1 = GroupMember { target: "art1".into(), out: hash_a, selector: "out/a.csv".into() };
        let m2 = GroupMember { target: "art2".into(), out: hash_b, selector: "out/b.csv".into() };
        let g1 = group_hash("g", "hashed", &[m1.clone(), m2.clone()]);
        let g2 = group_hash("g", "hashed", &[m2.clone(), m1.clone()]);
        assert_eq!(g1, g2);

        let i1 = in_hash(&[
            InGroup { name: "b".into(), mode: "hashed".into(), grp_hash: hash_b },
            InGroup { name: "a".into(), mode: "hashed".into(), grp_hash: hash_a },
        ]);
        let i2 = in_hash(&[
            InGroup { name: "a".into(), mode: "hashed".into(), grp_hash: hash_a },
            InGroup { name: "b".into(), mode: "hashed".into(), grp_hash: hash_b },
        ]);
        assert_eq!(i1, i2);
        // ref-группы не влияют на in_hash.
        let i_with_ref = in_hash(&[
            InGroup { name: "a".into(), mode: "hashed".into(), grp_hash: hash_a },
            InGroup { name: "b".into(), mode: "hashed".into(), grp_hash: hash_b },
            InGroup { name: "ref".into(), mode: "ref".into(), grp_hash: sha256(b"ref") },
        ]);
        assert_eq!(i1, i_with_ref);
        // порядок членов группы не влияет на group_hash, порядок групп — на in_hash по его составу.
        assert_ne!(g1, group_hash("g", "hashed", &[m1]));
    }

    #[test]
    fn act_key_is_sensitive_to_every_component() {
        let in_h = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: sha256(b"g") }]);
        let script = Some(script_hash("python", "script.py", b"print(1)"));
        let params = params_hash(&json!({"window": 128, "method": "welch"})).unwrap();
        let env = env_hash("python", "linux-x86_64", Some(sha256(b"lock")), &BTreeMap::new());
        let base = act_key("compute", in_h, script, params, env);

        let other_in = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: sha256(b"g2") }]);
        assert_ne!(base, act_key("compute", other_in, script, params, env));
        assert_ne!(base, act_key("data", in_h, script, params, env));
        assert_ne!(base, act_key("compute", in_h, None, params, env));
        let other_params = params_hash(&json!({"window": 256, "method": "welch"})).unwrap();
        assert_ne!(base, act_key("compute", in_h, script, other_params, env));
        let other_env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
        assert_ne!(base, act_key("compute", in_h, script, params, other_env));
        // params_hash чувствителен к порядку ключей? нет — канонизация делает его нечувствительным.
        let params2 = params_hash(&json!({"method": "welch", "window": 128})).unwrap();
        assert_eq!(params, params2);
    }

    #[test]
    fn hex_and_short_are_stable() {
        let h = sha256(b"rde");
        assert_eq!(h.hex().len(), 64);
        assert_eq!(h.short(8).len(), 8);
        assert_eq!(h, sha256(b"rde"));
        assert_ne!(h, sha256(b"rde2"));
    }

    #[test]
    fn golden_vectors_are_stable() {
        // Векторы фиксируют формат хеширования: любое изменение схемы должно осознанно менять эти строки.
        let in_h = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: group_hash("g", "hashed", &[]) }]);
        let params = params_hash(&json!({"window": 128, "method": "welch"})).unwrap();
        let env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
        let script = script_hash("python", "script.py", b"print(1)");
        let key = act_key("compute", in_h, Some(script), params, env);
        assert_eq!(in_h.hex(), "9c6f4bedc2c1de53b61711cc59ce8665fbddce4abdc00ad144a1ba2f5e69fea4");
        assert_eq!(params.hex(), "7de5e854d5c238b16b7fd6437605e367b63c4e958d5c7000e552911ee44c6523");
        assert_eq!(env.hex(), "d8560e3c36d5e08bba444207382e2f2cd2f94a4d27168406d8db843338a9a9ab");
        assert_eq!(script.hex(), "92ac5907bcf8189dcbfde7341cdc8edc5bd1deb3c2e9eeb29c705c975b9db61d");
        assert_eq!(key.hex(), "2cdf3a6a0d04223b525ae225e9f8271606e1cf9b594aa31a0fc29a6ba5c62494");
    }
}
