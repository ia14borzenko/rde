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
//! `params_hash` использует канонический JSON по RFC 8785 (JCS): ключи объектов сортируются
//! по UTF-16 code units, строки экранируются по ECMAScript, числа печатаются по
//! ECMAScript `Number::toString` (кратчайшее round-trip представление, экспонента при
//! |x| ≥ 1e21 или < 1e-6); `NaN`/`Infinity` запрещены. Это делает `params_hash` и хеши
//! констант воспроизводимыми между платформами и версиями RDE.

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

/// Каноническое JSON-представление по RFC 8785 (JCS): детерминированная сериализация,
/// пригодная для хеширования. `NaN`/`Infinity` — ошибка (JSON их не допускает).
pub fn canonical_json(value: &serde_json::Value) -> Result<String, String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &serde_json::Value, out: &mut String) -> Result<(), String> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => out.push_str(&jcs_number(n)?),
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
            // JCS: ключи сортируются по возрастанию UTF-16 code units (RFC 8785 §3.2.3).
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| cmp_utf16(a, b));
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

/// Сравнение строк по UTF-16 code units (порядок ключей JCS).
fn cmp_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// JCS-сериализация числа: целые — как есть, дробные — по ECMAScript `Number::toString`.
fn jcs_number(number: &serde_json::Number) -> Result<String, String> {
    if let Some(value) = number.as_i64() {
        return Ok(value.to_string());
    }
    if let Some(value) = number.as_u64() {
        return Ok(value.to_string());
    }
    let value = number.as_f64().ok_or_else(|| format!("не число: {number}"))?;
    jcs_f64(value)
}

/// ECMAScript `Number::toString` (RFC 8785 §3.2.2.3) для конечных f64.
///
/// Использует кратчайшее round-trip представление Rust (`{:e}`), затем переформатирует его
/// по правилам ES6: десятичная запись при 1e-6 ≤ |x| < 1e21, иначе экспоненциальная.
pub fn jcs_f64(value: f64) -> Result<String, String> {
    if !value.is_finite() {
        return Err(format!("JCS не допускает {value} (NaN/Infinity)"));
    }
    if value == 0.0 {
        return Ok("0".to_owned());
    }
    let negative = value < 0.0;
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci
        .split_once('e')
        .ok_or_else(|| format!("не удалось разобрать экспоненциальную запись `{sci}`"))?;
    let exponent: i32 = exponent.parse().map_err(|e| format!("экспонента `{sci}`: {e}"))?;
    let mut digits: String = mantissa.chars().filter(|ch| *ch != '.').collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let k = digits.len() as i32;
    let n = exponent + 1; // положение десятичной точки: value = 0.digits × 10^n
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let exp = n - 1;
        if exp >= 0 {
            out.push('+');
        }
        out.push_str(&exp.to_string());
    }
    Ok(out)
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

/// `script_hash` — исполняемый файл артефакта + runtime + дополнительные файлы скрипта.
///
/// Формула §7.2: `H("rde:script:1" | runtime_id | entry_relpath | H(script_bytes) |
/// sorted[extra_script_files])`. Дополнительные файлы кодируются парой (путь, хеш блоба),
/// сортируются по пути; их отсутствие фиксируется sentinel-хешем на стороне DAG.
pub fn script_hash(runtime: &str, entry: &str, bytes: &[u8], extra_files: &[(String, Hash)]) -> Hash {
    let mut canon = Canonical::new("rde:script:1");
    canon.str(runtime);
    canon.str(entry);
    canon.hash(sha256(bytes));
    let mut sorted: Vec<&(String, Hash)> = extra_files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    canon.u32(sorted.len() as u32);
    for (rel, blob) in sorted {
        canon.str(rel);
        canon.hash(*blob);
    }
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

/// `constant_hash` — хеш расширенной константы/величины (ADR-0005, R-ART-09): канонический JSON
/// разрешённого значения (поля обозначения/единицы/формата + разрешённое значение + digest
/// источника). Для **литеральных** констант без новых полей используется прежняя схема
/// `sha256(canonical_json(ConstantValue))` (совместимость act_key) — см. `rde-dag`.
pub fn constants_hash(payload: &str) -> Hash {
    let mut canon = Canonical::new("rde:const:1");
    canon.str(payload);
    canon.finish()
}

/// `act_key` — ключ активации артефакта (§7.2).
///
/// `H("rde:act:1" | type | schema_version | in_hash | script_hash | params_hash | env_hash)`.
/// `script_hash = None` (нет скрипта) кодируется флагом, чтобы не смешиваться с хешем пустого скрипта.
pub fn act_key(
    kind: &str,
    schema_version: &str,
    in_hash: Hash,
    script_hash: Option<Hash>,
    params_hash: Hash,
    env_hash: Hash,
) -> Hash {
    let mut canon = Canonical::new("rde:act:1");
    canon.str(kind);
    canon.str(schema_version);
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

    /// Детерминированный ГПСЧ для property-тестов (xorshift64*), без внешних зависимостей.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn bytes(&mut self, max: usize) -> Vec<u8> {
            let len = self.below(max as u64 + 1) as usize;
            (0..len).map(|_| self.next() as u8).collect()
        }
        fn json_with(&mut self, depth: u32, floats: bool) -> serde_json::Value {
            if floats {
                match self.below(if depth == 0 { 5 } else { 7 }) {
                    0 => json!(null),
                    1 => json!(self.next() % 2 == 0),
                    2 => json!(self.next() as i64 - 1_000_000),
                    3 => self.random_string(),
                    4 => self.random_float(),
                    5 => self.random_array(depth, floats),
                    _ => self.random_object(depth, floats),
                }
            } else {
                match self.below(if depth == 0 { 4 } else { 6 }) {
                    0 => json!(null),
                    1 => json!(self.next() % 2 == 0),
                    2 => json!(self.next() as i64 - 1_000_000),
                    3 => self.random_string(),
                    4 => self.random_array(depth, floats),
                    _ => self.random_object(depth, floats),
                }
            }
        }
        fn random_string(&mut self) -> serde_json::Value {
            let mut text = String::new();
            for _ in 0..self.below(6) {
                text.push(char::from(b'a' + (self.next() % 26) as u8));
            }
            json!(text)
        }
        fn random_float(&mut self) -> serde_json::Value {
            // Краевые для JCS значения: экспоненциальная запись, -0, длинная мантисса.
            let candidates = [
                0.0, -0.0, 1.5, 0.1 + 0.2, 1e21, 1e-7, 1e-6, 123.456, 1e20, -2.5e-10,
                1.2345678901234567e300,
            ];
            json!(candidates[(self.next() % candidates.len() as u64) as usize])
        }
        fn random_array(&mut self, depth: u32, floats: bool) -> serde_json::Value {
            let len = self.below(4) as usize;
            serde_json::Value::Array((0..len).map(|_| self.json_with(depth - 1, floats)).collect())
        }
        fn random_object(&mut self, depth: u32, floats: bool) -> serde_json::Value {
            let len = self.below(4) as usize;
            let mut map = serde_json::Map::new();
            for _ in 0..len {
                let key: String = (0..1 + self.below(4))
                    .map(|_| char::from(b'a' + (self.next() % 26) as u8))
                    .collect();
                map.insert(key, self.json_with(depth - 1, floats));
            }
            serde_json::Value::Object(map)
        }
    }

    #[test]
    fn jcs_numbers_follow_ecmascript() {
        // RFC 8785 §3.2.2.3: те же примеры, что в спецификации ECMAScript Number::toString.
        let cases: [(f64, &str); 16] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (123.456, "123.456"),
            (2.5e-10, "2.5e-10"),
            (1.2345678901234567e300, "1.2345678901234567e+300"),
            (-1e-7, "-1e-7"),
            (1000.0, "1000"),
            (9.999999999999999e20, "999999999999999900000"),
        ];
        for (value, expected) in cases {
            assert_eq!(jcs_f64(value).unwrap(), expected, "f64 {value:?}");
        }
        assert!(jcs_f64(f64::NAN).is_err());
        assert!(jcs_f64(f64::INFINITY).is_err());
    }

    #[test]
    fn canonical_json_sorts_keys_by_utf16_and_keeps_arrays() {
        let a = canonical_json(&json!({"b": 1, "a": {"z": [3, 1], "y": "ы"}})).unwrap();
        let b = canonical_json(&json!({"a": {"y": "ы", "z": [3, 1]}, "b": 1})).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":{"y":"ы","z":[3,1]},"b":1}"#);
        // UTF-16 code units: U+1F600 (суррогаты D83D DE00) < U+FFFF, хотя по code point — наоборот.
        let keys = json!({"\u{FFFF}": 1, "\u{1F600}": 2});
        assert_eq!(canonical_json(&keys).unwrap(), "{\"😀\":2,\"￿\":1}");
        // Управляющие символы экранируются по ECMAScript.
        assert_eq!(canonical_json(&json!("\u{1}\u{1f}\n")).unwrap(), "\"\\u0001\\u001f\\n\"");
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
        assert_ne!(g1, group_hash("g", "hashed", &[m1]));
    }

    #[test]
    fn act_key_is_sensitive_to_every_component() {
        let in_h = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: sha256(b"g") }]);
        let script = Some(script_hash("python", "script.py", b"print(1)", &[]));
        let params = params_hash(&json!({"window": 128, "method": "welch"})).unwrap();
        let env = env_hash("python", "linux-x86_64", Some(sha256(b"lock")), &BTreeMap::new());
        let schema = "rde/artifact@1";
        let base = act_key("compute", schema, in_h, script, params, env);

        let other_in = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: sha256(b"g2") }]);
        assert_ne!(base, act_key("compute", schema, other_in, script, params, env));
        assert_ne!(base, act_key("data", schema, in_h, script, params, env));
        assert_ne!(base, act_key("compute", "rde/artifact@2", in_h, script, params, env));
        assert_ne!(base, act_key("compute", schema, in_h, None, params, env));
        let other_params = params_hash(&json!({"window": 256, "method": "welch"})).unwrap();
        assert_ne!(base, act_key("compute", schema, in_h, script, other_params, env));
        let other_env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
        assert_ne!(base, act_key("compute", schema, in_h, script, params, other_env));
        // params_hash нечувствителен к порядку ключей (канонизация).
        let params2 = params_hash(&json!({"method": "welch", "window": 128})).unwrap();
        assert_eq!(params, params2);
    }

    #[test]
    fn script_hash_includes_extra_files_and_nested_bytes() {
        let base = script_hash("python", "script.py", b"print(1)", &[]);
        let with_extra = script_hash(
            "python",
            "script.py",
            b"print(1)",
            &[("lib/util.py".into(), sha256(b"x = 1")), ("lib/a.py".into(), sha256(b"y = 2"))],
        );
        assert_ne!(base, with_extra);
        // Порядок extra-файлов не важен.
        let reordered = script_hash(
            "python",
            "script.py",
            b"print(1)",
            &[("lib/a.py".into(), sha256(b"y = 2")), ("lib/util.py".into(), sha256(b"x = 1"))],
        );
        assert_eq!(with_extra, reordered);
        // Меняется любой компонент: runtime, entry, байты, extra-файл.
        assert_ne!(base, script_hash("shell", "script.py", b"print(1)", &[]));
        assert_ne!(base, script_hash("python", "main.py", b"print(1)", &[]));
        assert_ne!(base, script_hash("python", "script.py", b"print(2)", &[]));
    }

    #[test]
    fn hex_and_short_are_stable() {
        let h = sha256(b"rde");
        assert_eq!(h.hex().len(), 64);
        assert_eq!(h.short(8).len(), 8);
        assert_eq!(h, sha256(b"rde"));
        assert_ne!(h, sha256(b"rde2"));
        assert_eq!(Hash::from_hex(&h.hex()).unwrap(), h);
        assert!(Hash::from_hex("xyz").is_err());
    }

    // ── property-тесты (детерминированный ГПСЧ, 200 итераций) ────────────────

    #[test]
    fn property_canonical_json_is_idempotent() {
        // Без float: `serde_json` парсит редкие f64 не с корректным округлением (fast path),
        // поэтому идемпотентность проверяем на структуре, а числа — отдельным property-тестом
        // через корректно округляющий std-парсер (`property_jcs_f64_roundtrips`).
        let mut rng = Rng::new(0x5EED_1);
        for _ in 0..200 {
            let value = rng.json_with(3, false);
            let once = canonical_json(&value).expect("канонизация");
            let parsed: serde_json::Value = serde_json::from_str(&once).expect("канонический JSON парсится");
            let twice = canonical_json(&parsed).expect("повторная канонизация");
            assert_eq!(once, twice, "JCS не идемпотентен для {value}");
        }
    }

    #[test]
    fn property_jcs_f64_roundtrips() {
        // Кратчайшее представление должно читаться обратно в тот же f64 (std-парсер корректен)
        // и канонизироваться в ту же строку — это и есть требование ECMAScript Number::toString.
        let mut rng = Rng::new(0x5EED_5);
        for _ in 0..500 {
            let mantissa = rng.next() as f64 / 1e19;
            let exponent = rng.below(40) as i32 - 20;
            let value = mantissa * 10f64.powi(exponent);
            if !value.is_finite() {
                continue;
            }
            let text = jcs_f64(value).expect("конечное число");
            let parsed: f64 = text.parse().expect("std-парсер");
            assert_eq!(parsed, value, "jcs_f64({value:?}) = {text} не round-trip");
            assert_eq!(jcs_f64(parsed).unwrap(), text, "канонизация нестабильна для {value:?}");
        }
        // Краевые значения: поднормали, границы экспоненциальной записи.
        for value in [f64::MIN_POSITIVE, f64::MAX, 1e-6, 1e21, 5e-324, 0.0, -0.0] {
            let text = jcs_f64(value).unwrap();
            let parsed: f64 = text.parse().unwrap();
            if value == 0.0 {
                // JCS: и +0.0, и −0.0 канонизируются в "0" (знак нуля теряется по спецификации).
                assert_eq!(text, "0");
            } else {
                assert_eq!(parsed.to_bits(), value.to_bits(), "{value:?} → {text}");
            }
            assert_eq!(jcs_f64(parsed).unwrap(), text);
        }
    }

    #[test]
    fn property_tree_hash_permutation_invariant() {
        let mut rng = Rng::new(0x5EED_2);
        for _ in 0..100 {
            let count = 1 + rng.below(5) as usize;
            let entries: Vec<TreeEntry> = (0..count)
                .map(|index| TreeEntry::new(format!("out/f{index}.bin"), 0o644, sha256(rng.bytes(8))))
                .collect();
            let mut shuffled = entries.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, rng.below(i as u64 + 1) as usize);
            }
            assert_eq!(tree_hash(&entries), tree_hash(&shuffled), "порядок файлов не должен влиять");
        }
    }

    #[test]
    fn property_act_key_sensitive_to_mutations() {
        let mut rng = Rng::new(0x5EED_3);
        let schema = "rde/artifact@1";
        for _ in 0..100 {
            let in_h = in_hash(&[InGroup {
                name: "g".into(),
                mode: "hashed".into(),
                grp_hash: sha256(rng.bytes(8)),
            }]);
            let params = params_hash(&json!({"k": rng.next() as u32})).unwrap();
            let env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
            let script = Some(script_hash("python", "script.py", &rng.bytes(8), &[]));
            let key = act_key("compute", schema, in_h, script, params, env);
            let other = act_key("compute", schema, in_h, script, params_hash(&json!({"k": 7u32})).unwrap(), env);
            assert_ne!(key, other, "изменение параметров меняет act_key");
            let other = act_key("compute", schema, in_h, None, params, env);
            assert_ne!(key, other, "изменение скрипта меняет act_key");
        }
    }

    #[test]
    fn property_in_hash_ignores_ref_groups() {
        let mut rng = Rng::new(0x5EED_4);
        for _ in 0..100 {
            let count = rng.below(4) as usize;
            let hashed: Vec<InGroup> = (0..count)
                .map(|i| InGroup {
                    name: format!("g{i}"),
                    mode: "hashed".into(),
                    grp_hash: sha256(rng.bytes(4)),
                })
                .collect();
            let base = in_hash(&hashed);
            let mut with_ref = hashed.clone();
            with_ref.push(InGroup {
                name: "ref".into(),
                mode: "ref".into(),
                grp_hash: sha256(rng.bytes(4)),
            });
            assert_eq!(base, in_hash(&with_ref));
        }
    }

    #[test]
    fn golden_vectors_are_stable() {
        // Векторы фиксируют формат хеширования: любое изменение схемы должно осознанно менять эти строки.
        let in_h = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: group_hash("g", "hashed", &[]) }]);
        let params = params_hash(&json!({"window": 128, "method": "welch"})).unwrap();
        let env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
        let script = script_hash("python", "script.py", b"print(1)", &[]);
        let key = act_key("compute", "rde/artifact@1", in_h, Some(script), params, env);
        assert_eq!(in_h.hex(), "9c6f4bedc2c1de53b61711cc59ce8665fbddce4abdc00ad144a1ba2f5e69fea4");
        assert_eq!(params.hex(), "7de5e854d5c238b16b7fd6437605e367b63c4e958d5c7000e552911ee44c6523");
        assert_eq!(env.hex(), "d8560e3c36d5e08bba444207382e2f2cd2f94a4d27168406d8db843338a9a9ab");
        assert_eq!(script.hex(), "ca76cf7fe5778ffaeebcfa62dcddfa5352e686ead44b4fdf9032253f358c17cf");
        assert_eq!(key.hex(), "0edbfde5cb6e6f6ed7e9d4ed580e58f072e41d23f4f7abb37dabb5b196af6d5f");
    }
}
