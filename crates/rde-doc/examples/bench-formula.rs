//! Замер компиляции формулы (OMML+SVG+PNG): «до/после» смены math-шрифта.
//! `cargo run -p rde-doc --example bench-formula` (STIX Two Math) и
//! `RDE_MATH_FONT=dejavu cargo run ...` (прежний substitute-шрифт).
fn main() {
    let latex = r"\sigma=\sqrt{\frac{1}{N-1}\sum_{i=1}^{N}(x_i-\bar{x})^2}";
    let first = std::time::Instant::now();
    let _ = rde_doc::compile_formula(latex, true).expect("compile");
    let cold = first.elapsed();
    let runs = 20;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        let _ = rde_doc::compile_formula(latex, true).expect("compile");
    }
    let avg = start.elapsed() / runs;
    println!("math_font={} cold={cold:?} avg={avg:?} ({runs} runs)", rde_doc::math_font::resolved_math_family());
}
