//! Property-тесты DAG (S4, tz-02 §7.3 п.3/п.4): случайные графы, топопорядок, циклы, каскад.
//!
//! Без внешних зависимостей: детерминированный xorshift64*, 40–100 итераций на свойство.
//! Граф строится на диске как настоящий проект RDE (source of truth — файлы), чтобы проверять
//! ту же загрузку/валидацию, что и в приложении.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rde_core::load_project;
use rde_dag::{dependents_closure, topological_compute};

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
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-dag-prop-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Записать проект из `nodes` compute-артефактов и рёбер `dep → dependent`.
fn write_graph(root: &Path, nodes: &[String], hashed: &[(usize, usize)], ref_edges: &[(usize, usize)]) {
    std::fs::create_dir_all(root.join("objects/artifacts")).unwrap();
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"prop\"\n",
    )
    .unwrap();
    for (index, id) in nodes.iter().enumerate() {
        let mut manifest = format!(
            "schema = \"rde/artifact@1\"\nid = \"{id}\"\ntitle = \"Узел {index}\"\ntype = \"compute\"\n\
             outputs = [\"out/x.txt\"]\n[runtime]\nkind = \"python\"\nenv = \"project\"\nentry = \"script.py\"\n"
        );
        let mut hashed_inputs = Vec::new();
        for &(from, to) in hashed {
            if to == index {
                hashed_inputs.push(format!("{{ artifact = \"{}\", selector = \"out/x.txt\" }}", nodes[from]));
            }
        }
        if !hashed_inputs.is_empty() {
            manifest.push_str(&format!(
                "\n[[groups]]\nname = \"hashed\"\nmode = \"hashed\"\ninputs = [\n  {},\n]\n",
                hashed_inputs.join(",\n  ")
            ));
        }
        let mut ref_inputs = Vec::new();
        for &(from, to) in ref_edges {
            if to == index {
                ref_inputs.push(format!("{{ artifact = \"{}\", selector = \"out/x.txt\" }}", nodes[from]));
            }
        }
        if !ref_inputs.is_empty() {
            manifest.push_str(&format!(
                "\n[[groups]]\nname = \"ref\"\nmode = \"ref\"\ninputs = [\n  {},\n]\n",
                ref_inputs.join(",\n  ")
            ));
        }
        let dir = root.join("objects/artifacts").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("artifact.toml"), manifest).unwrap();
        std::fs::write(dir.join("script.py"), "print('x')\n").unwrap();
    }
}

/// Случайный ациклический hashed-граф: рёбра только «раньше → позже» в случайной перестановке.
fn random_dag(rng: &mut Rng, count: usize) -> (Vec<usize>, Vec<(usize, usize)>) {
    let mut order: Vec<usize> = (0..count).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let mut edges = Vec::new();
    for i in 0..count {
        for j in (i + 1)..count {
            if rng.chance(30) {
                edges.push((order[i], order[j]));
            }
        }
    }
    (order, edges)
}

fn positions(order: &[String]) -> HashMap<String, usize> {
    order.iter().enumerate().map(|(i, id)| (id.clone(), i)).collect()
}

#[test]
fn property_topological_order_respects_hashed_edges_only() {
    let mut rng = Rng::new(0xD06_01);
    for iteration in 0..40 {
        let count = 2 + rng.below(7) as usize;
        let nodes: Vec<String> = (0..count).map(|_| ulid::Ulid::generate().to_string()).collect();
        let (_, hashed) = random_dag(&mut rng, count);
        // ref-рёбра — произвольные, могут образовывать визуальные циклы.
        let mut ref_edges = Vec::new();
        for i in 0..count {
            for j in 0..count {
                if i != j && rng.chance(15) {
                    ref_edges.push((i, j));
                }
            }
        }
        let root = tmp(&format!("topo-{iteration}"));
        write_graph(&root, &nodes, &hashed, &ref_edges);
        let project = load_project(&root).unwrap();
        assert!(project.issues.is_empty(), "итерация {iteration}: {:?}", project.issues);

        // dag_edges — ровно hashed-рёбра, без ref.
        let expected: HashSet<(String, String)> = hashed
            .iter()
            .map(|&(from, to)| (nodes[from].clone(), nodes[to].clone()))
            .collect();
        let actual: HashSet<(String, String)> = project.dag_edges().into_iter().collect();
        assert_eq!(actual, expected, "итерация {iteration}: ref-рёбра не должны попадать в DAG");

        let order = topological_compute(&project).unwrap();
        assert_eq!(order.len(), count, "итерация {iteration}: все compute-узлы в порядке");
        let pos = positions(&order);
        for &(from, to) in &hashed {
            assert!(
                pos[&nodes[from]] < pos[&nodes[to]],
                "итерация {iteration}: зависимость {} должна быть раньше {}",
                nodes[from],
                nodes[to]
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn property_cycles_are_rejected() {
    let mut rng = Rng::new(0xD06_02);
    for iteration in 0..30 {
        let count = 3 + rng.below(5) as usize;
        let nodes: Vec<String> = (0..count).map(|_| ulid::Ulid::generate().to_string()).collect();
        // Гарантированный цикл 0→1→…→count-1→0 (hashed), плюс случайные рёбра.
        let mut hashed: Vec<(usize, usize)> = (0..count).map(|i| (i, (i + 1) % count)).collect();
        for i in 0..count {
            for j in 0..count {
                if i != j && rng.chance(10) && !hashed.contains(&(i, j)) {
                    hashed.push((i, j));
                }
            }
        }
        let root = tmp(&format!("cycle-{iteration}"));
        write_graph(&root, &nodes, &hashed, &[]);
        let project = load_project(&root).unwrap();
        assert!(
            project.issues.iter().any(|issue| issue.message.contains("цикл")),
            "итерация {iteration}: загрузка должна сообщить о цикле: {:?}",
            project.issues
        );
        assert!(topological_compute(&project).is_err(), "итерация {iteration}: цикл — ошибка обновления");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn property_dependents_closure_matches_bfs() {
    let mut rng = Rng::new(0xD06_03);
    for iteration in 0..40 {
        let count = 2 + rng.below(8) as usize;
        let nodes: Vec<String> = (0..count).map(|_| ulid::Ulid::generate().to_string()).collect();
        let (_, hashed) = random_dag(&mut rng, count);
        let root = tmp(&format!("closure-{iteration}"));
        write_graph(&root, &nodes, &hashed, &[]);
        let project = load_project(&root).unwrap();

        let target_count = 1 + rng.below(2) as usize;
        let targets: Vec<String> = (0..target_count)
            .map(|_| nodes[rng.below(count as u64) as usize].clone())
            .collect();
        let closure = dependents_closure(&project, &targets);

        // Независимая BFS по hashed-рёбрам.
        let mut expected: BTreeSet<String> = targets.iter().cloned().collect();
        let mut queue: Vec<String> = targets.clone();
        while let Some(current) = queue.pop() {
            for (from, to) in project.dag_edges() {
                if from == current && expected.insert(to.clone()) {
                    queue.push(to);
                }
            }
        }
        let actual: BTreeSet<String> = closure.into_iter().collect();
        assert_eq!(actual, expected, "итерация {iteration}: каскад должен совпадать с BFS");
        let _ = std::fs::remove_dir_all(&root);
    }
}
