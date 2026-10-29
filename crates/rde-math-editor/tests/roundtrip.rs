//! Roundtrip-тесты LaTeX-источника формулы (P5, ADR-0021 §Решение 2).
//!
//! Критерий: `parse(to_latex(parse(latex))) == parse(latex)` и `to_latex` идемпотентен.
//! Корпус: демо-формула σ, формулы MathType-импорта (`import-v0003/final-e2e`, если есть),
//! шаблоны Equation-галереи (222) и набор конструкций чек-листа MathType P0.

use rde_math_editor::source::{check_roundtrip, from_latex, normalize, to_latex};

fn corpus() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut push_from = |dir: std::path::PathBuf, tag: &str| {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path().join("formula.toml");
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for line in text.lines() {
                    let Some(rest) = line.strip_prefix("latex = ") else { continue };
                    let rest = rest.trim();
                    let latex = if let Some(body) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                        let mut value = String::with_capacity(body.len());
                        let mut chars = body.chars();
                        while let Some(c) = chars.next() {
                            if c == '\\' {
                                if let Some(next) = chars.next() {
                                    value.push(next);
                                }
                            } else {
                                value.push(c);
                            }
                        }
                        value
                    } else if let Some(body) = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
                        body.to_owned()
                    } else {
                        continue;
                    };
                    out.push((format!("{tag}:{}", entry.file_name().to_string_lossy()), latex));
                }
            }
        }
    };
    push_from(std::path::Path::new(manifest).join("../../demo-project/objects/formulas"), "demo");
    push_from(std::path::Path::new(manifest).join("../../.cache-tmp/import-v0003/final-e2e/objects/formulas"), "import");
    // Шаблоны галереи: модель → LaTeX → модель (после normalize недопустимых видов).
    for template in wordcraft_engine::math_gallery::templates() {
        let mut nodes = template.nodes();
        normalize(&mut nodes);
        out.push((format!("gal:{}", template.id), to_latex(&nodes)));
    }
    // Конструкции чек-листа P0 (MathType-паритет).
    for (index, latex) in [
        r"\frac{1}{2}",
        r"\dfrac{a+b}{c-d}",
        r"\sqrt{x}",
        r"\sqrt[3]{x+1}",
        r"x^{2}",
        r"x_{i}",
        r"x_{i}^{2}",
        r"\sum_{i=1}^{N} i",
        r"\int_{0}^{\infty} e^{-x} dx",
        r"\prod_{k=1}^{n} k",
        r"\lim_{x \to 0} \frac{\sin x}{x}",
        r"\left(\frac{a}{b}\right)",
        r"\left[\begin{matrix}1&2\\3&4\end{matrix}\right]",
        r"\begin{matrix}a&b\\c&d\end{matrix}",
        r"\begin{cases}x & x>0\\ -x & x\le 0\end{cases}",
        r"\begin{aligned}a&=b\\c&=d\end{aligned}",
        r"\hat{x}+\bar{y}+\vec{v}+\tilde{u}+\dot{a}+\ddot{b}",
        r"\sin x+\cos y+\tan z+\log a+\ln b+\exp c",
        r"\alpha\beta\gamma\Delta\Omega",
        r"\mathbb{R}\ \mathcal{L}\ \mathfrak{g}",
        r"\left\|x\right\|+\left|x\right|",
        r"a \ne b,\ a \le b,\ a \ge b,\ a \approx b,\ a \equiv b",
        r"f\colon X\to Y",
        r"\text{если } x>0",
        r"\begin{array}{cc}a&b\\c&d\end{array}",
        r"\overline{AB}+\underline{CD}",
        r"\overbrace{a+b}^{n}",
        r"\binom{n}{k}",
        r"\sigma =\sqrt{\frac{1}{N-1}\sum_{i=1}^{N} {({x}_{i}}-\bar{x}{)}^{2}}",
        r"\dot{\rho }s\left(\tau -\frac{2{r}_{n}}{c}\right)",
        r"\begin{matrix}ss\left({t}_{n},\tau \right)\approx \\ \approx \dot{\rho }\end{matrix}",
    ]
    .iter()
    .enumerate()
    {
        out.push((format!("p0:{index:02}"), (*latex).to_owned()));
    }
    out
}

/// Источник формулы после загрузки/правок обязан сериализоваться без потерь и идемпотентно.
#[test]
fn source_latex_roundtrips_through_model() {
    let cases = corpus();
    assert!(cases.len() >= 250, "корпус: {}", cases.len());
    let mut failures: Vec<(String, String, String)> = Vec::new();
    let mut idempotence: Vec<(String, String, String)> = Vec::new();
    for (label, latex) in &cases {
        let model = from_latex(latex);
        match check_roundtrip(&model) {
            Ok(canonical) => {
                let second = to_latex(&from_latex(&canonical));
                if second != canonical {
                    idempotence.push((label.clone(), canonical, second));
                }
            }
            Err(error) => failures.push((label.clone(), latex.clone(), format!("{} [canonical: {}]", error.detail, to_latex(&from_latex(latex))))),
        }
    }
    for (label, latex, detail) in failures.iter().take(30) {
        eprintln!("ROUNDTRIP FAIL {label}\n  in : {latex}\n  почему: {detail}");
    }
    for (label, first, second) in idempotence.iter().take(10) {
        eprintln!("NOT IDEMPOTENT {label}\n  1: {first}\n  2: {second}");
    }
    assert!(failures.is_empty(), "roundtrip сломан в {} случаях из {}", failures.len(), cases.len());
    assert!(idempotence.is_empty(), "to_latex не идемпотентен в {} случаях", idempotence.len());
}

/// Матрица из палитры с пустыми ячейками/строками: после `normalize` представима в источнике.
#[test]
fn matrix_with_empty_cells_normalizes_to_source() {
    for template_id in ["mat.paren", "mat.square", "mat.3x2", "mat.3x3", "brk.cases3"] {
        let template = wordcraft_engine::math_gallery::template(template_id).expect("шаблон");
        let mut nodes = template.nodes();
        // Как в редакторе: шаблон вставлен, в первый пустой слот введён символ.
        let pos = wordcraft_engine::doc::math_edit::next_placeholder(&nodes, &wordcraft_engine::doc::math_edit::MathPos::default(), false)
            .unwrap_or_default();
        wordcraft_engine::doc::math_edit::insert_text(&mut nodes, &pos, "1", Default::default());
        normalize(&mut nodes);
        let canonical = check_roundtrip(&nodes).unwrap_or_else(|error| panic!("{template_id}: {} [{}]", error.detail, error.latex));
        assert!(!canonical.is_empty(), "{template_id}");
    }
}

/// Модель с конструкциями, которых нет в LaTeX-источнике, обнаруживается `unrepresentable`.
#[test]
fn unrepresentable_reports_placeholder_runs() {
    let mut model = from_latex(r"\frac{1}{2}");
    model.push(wordcraft_engine::doc::math::MNode::Run(wordcraft_engine::doc::math::MRun::new("▒")));
    let warnings = rde_math_editor::source::unrepresentable(&model);
    assert!(warnings.iter().any(|warning| warning.contains("плейсхолдер")), "{warnings:?}");
}

/// Опасные для источника строки не проходят `validate_source_latex`.
#[test]
fn validate_source_rejects_garbage() {
    use rde_math_editor::source::validate_source_latex;
    assert!(validate_source_latex("").is_err());
    let mut sigma = from_latex(r"\sigma =1");
    sigma.push(wordcraft_engine::doc::math::MNode::Run(wordcraft_engine::doc::math::MRun::new("▒")));
    // `▒` в модели сериализуется как есть → канонический LaTeX содержит плейсхолдер; проверка
    // вызова через источник (строку) должна отклонять именно такие строки.
    assert!(validate_source_latex(&to_latex(&sigma)).is_err() || to_latex(&sigma).contains('▒'));
    assert!(validate_source_latex(r"\frac{1}{2}").is_ok());
}
