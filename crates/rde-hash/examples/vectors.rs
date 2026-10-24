//! Печать golden-векторов (для фиксации формата хеширования в тестах).
use std::collections::BTreeMap;

use rde_hash::{act_key, env_hash, group_hash, in_hash, params_hash, script_hash, InGroup};
use serde_json::json;

fn main() {
    let in_h = in_hash(&[InGroup { name: "g".into(), mode: "hashed".into(), grp_hash: group_hash("g", "hashed", &[]) }]);
    let params = params_hash(&json!({"window": 128, "method": "welch"})).unwrap();
    let env = env_hash("python", "linux-x86_64", None, &BTreeMap::new());
    let script = script_hash("python", "script.py", b"print(1)");
    let key = act_key("compute", in_h, Some(script), params, env);
    println!("in={}", in_h.hex());
    println!("params={}", params.hex());
    println!("env={}", env.hex());
    println!("script={}", script.hex());
    println!("act={}", key.hex());
}
