//! Замер открытия проекта на 10 000 объектов (NFR-PERF-01): `cargo run -p rde-core --example perf-10k -- [out.json]`
//!
//! Генерирует временный проект (заметки/документы/источники/репозитории) и измеряет
//! `load_project`; пишет JSON-отчёт. Бюджет: открытие 10 000 объектов < 2 с.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn ulid_like(seed: u64) -> String {
    // Детерминированные ULID-подобные id (26 символов Crockford base32).
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut out = String::with_capacity(26);
    for _ in 0..26 {
        out.push(ALPHABET[(value % 32) as usize] as char);
        value = value.rotate_left(5).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    out
}

fn write_object(root: &Path, kind: &str, id: &str, manifest: &str, files: &[(&str, &str)]) {
    let dir = root.join("objects").join(kind).join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(match kind {
        "documents" => "document.toml",
        "sources" => "source.toml",
        "notes" => "note.toml",
        "repos" => "repo.toml",
        _ => "manifest.toml",
    }), manifest)
    .unwrap();
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
}

fn main() {
    let out = std::env::args().nth(1).map(PathBuf::from);
    let root = std::env::temp_dir().join(format!("rde-perf-10k-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("objects")).unwrap();
    fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Perf 10k\"\n",
    )
    .unwrap();
    for kind in ["artifacts", "documents", "sources", "notes", "repos"] {
        fs::create_dir_all(root.join("objects").join(kind)).unwrap();
    }
    let started = Instant::now();
    let mut seed = 1u64;
    // 6000 заметок + 1500 документов + 1500 источников + 1000 репозиториев = 10 000.
    for _ in 0..6000 {
        let id = ulid_like(seed);
        seed += 1;
        write_object(
            &root,
            "notes",
            &id,
            &format!("schema = \"rde/note@1\"\nid = \"{id}\"\ntitle = \"Заметка {seed}\"\nfile = \"note.md\"\n"),
            &[("note.md", "# заметка\n")],
        );
    }
    for _ in 0..1500 {
        let id = ulid_like(seed);
        seed += 1;
        write_object(
            &root,
            "documents",
            &id,
            &format!("schema = \"rde/document@1\"\nid = \"{id}\"\ntitle = \"Документ {seed}\"\ntemplate = \"free\"\nfile = \"doc.docx\"\n"),
            &[("doc.docx", "")],
        );
    }
    for _ in 0..1500 {
        let id = ulid_like(seed);
        seed += 1;
        write_object(
            &root,
            "sources",
            &id,
            &format!("schema = \"rde/source@1\"\nid = \"{id}\"\ntitle = \"Источник {seed}\"\nkind = \"file\"\nfile = \"source.md\"\n"),
            &[("source.md", "# источник\n")],
        );
    }
    for _ in 0..1000 {
        let id = ulid_like(seed);
        seed += 1;
        write_object(
            &root,
            "repos",
            &id,
            &format!("schema = \"rde/repo@1\"\nid = \"{id}\"\ntitle = \"Репозиторий {seed}\"\nkind = \"internal\"\npath = \"files\"\n"),
            &[],
        );
    }
    let generation_ms = started.elapsed().as_millis() as u64;
    let load_started = Instant::now();
    let project = rde_core::load_project(&root).expect("проект 10k загружается");
    let load_ms = load_started.elapsed().as_millis() as u64;
    let issues = project.issues.len();
    println!(
        "10k: объектов {}, загрузка {} мс (бюджет < 2000 мс), генерация {} мс, issues {}",
        project.object_count(),
        load_ms,
        generation_ms,
        issues
    );
    if let Some(out) = out {
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let report = serde_json::json!({
            "objects": project.object_count(),
            "load_ms": load_ms,
            "generation_ms": generation_ms,
            "budget_ms": 2000,
            "passed": load_ms < 2000,
            "issues": issues,
            "generated_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
            let _ = fs::write(&out, bytes);
        }
        println!("отчёт: {}", out.display());
    }
    let _ = fs::remove_dir_all(&root);
    if load_ms >= 2000 {
        std::process::exit(2);
    }
}
