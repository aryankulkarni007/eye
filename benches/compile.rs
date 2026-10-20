//! compiler pipeline benchmarks. uses criterion for statistical analysis.
//! run with `cargo bench`.
//!
//! each benchmark measures a stage (or fused stage pair) on a representative
//! `.eye` program.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;
use std::hint::black_box;
use std::{env, fs};

use ast::AstNode;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};
use mir::lower::lower_function;

/// a moderate-sized program exercising all parser and lowering paths.
const COMPLEX_PROGRAM: &str = include_str!("../eyesrc/programs/raytracer.eye");

/// a minimal program for baseline overhead measurement.
const MINIMAL_PROGRAM: &str = "\
main() {
    let int32 x = 42;
    println(\"{}\", x);
}
";

fn lex(c: &mut Criterion) {
    let mut group = c.benchmark_group("lex");
    group.sample_size(30);

    group.bench_function("minimal", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(MINIMAL_PROGRAM).to_string());
                  let _ = Lexer::new(&source).tokenize();
              });
         });

    group.bench_function("complex", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
                  let _ = Lexer::new(&source).tokenize();
              });
         });

    group.finish();
}

fn parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    group.sample_size(30);

    let minimal_lexed = {
        let source = SourceText::new(MINIMAL_PROGRAM.to_string());
        Lexer::new(&source).tokenize()
    };
    let complex_lexed = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        Lexer::new(&source).tokenize()
    };

    group.bench_function("minimal", |b| {
             let source = SourceText::new(MINIMAL_PROGRAM.to_string());
             b.iter(|| {
                  let _ = parser::parse(black_box(&minimal_lexed.tokens), black_box(&source));
              });
         });

    group.bench_function("complex", |b| {
             let source = SourceText::new(COMPLEX_PROGRAM.to_string());
             b.iter(|| {
                  let _ = parser::parse(black_box(&complex_lexed.tokens), black_box(&source));
              });
         });

    group.finish();
}

fn hir_lower(c: &mut Criterion) {
    let mut group = c.benchmark_group("hir-lower");
    group.sample_size(30);

    let (minimal_parse, minimal_interner) = {
        let source = SourceText::new(MINIMAL_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        (parser::parse(&lexed.tokens, &source), lexed.interner)
    };
    let (complex_parse, complex_interner) = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        (parser::parse(&lexed.tokens, &source), lexed.interner)
    };

    group.bench_function("minimal", |b| {
             let file = ast::SourceFile::cast(minimal_parse.green.clone()).unwrap();
             b.iter(|| {
                  let _ = lower_source_file(black_box(file.clone()), &minimal_interner);
              });
         });

    group.bench_function("complex", |b| {
             let file = ast::SourceFile::cast(complex_parse.green.clone()).unwrap();
             b.iter(|| {
                  let _ = lower_source_file(black_box(file.clone()), &complex_interner);
              });
         });

    group.finish();
}

/// type checking (fused with HIR lower since HIR is not Clone).
/// isolate typeck cost by subtracting `hir-lower` from this measurement.
fn typeck(c: &mut Criterion) {
    let mut group = c.benchmark_group("typeck");
    group.sample_size(30);

    group.bench_function("minimal", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(MINIMAL_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let _ = typeck::check_file(&hir);
              });
         });

    group.bench_function("complex", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let _ = typeck::check_file(&hir);
              });
         });

    group.finish();
}

/// fused type-checking + effect inference through `effect::infer_file`
/// (the production path used in the database's `lowered_file` query).
/// includes HIR lower (not Clone); subtract `hir-lower` for the effect-inference
/// cost on top of type checking.
fn effect(c: &mut Criterion) {
    let mut group = c.benchmark_group("effect");
    group.sample_size(30);

    group.bench_function("minimal", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(MINIMAL_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let _ = effect::infer_file(&hir);
              });
         });

    group.bench_function("complex", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let _ = effect::infer_file(&hir);
              });
         });

    group.finish();
}

fn mir_lower(c: &mut Criterion) {
    let mut group = c.benchmark_group("mir-lower");
    group.sample_size(30);

    let (hir, typeck) = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = ast::SourceFile::cast(parse.green).unwrap();
        let hir = lower_source_file(file, &lexed.interner);
        let typeck = typeck::check_file(&hir);
        (hir, typeck)
    };

    group.bench_function("first_fn", |b| {
             let fn_id = *hir.items.functions.values().next().unwrap();
             let body_id = hir.functions[fn_id].body.unwrap();
             let body = &hir.bodies[body_id];
             b.iter(|| {
                  let _ = lower_function(black_box(&hir),
                                         &hir.types,
                                         black_box(body),
                                         black_box(&typeck[&fn_id]),
                                         black_box(hir.functions[fn_id].params.len()),
                                         black_box(hir.functions[fn_id].ret));
              });
         });

    group.bench_function("all_fns", |b| {
             b.iter(|| {
                  let _ = mir::lower_all(black_box(&hir), black_box(&typeck));
              });
         });

    group.finish();
}

/// standalone code generation — MIR → C string emission.
fn codegen(c: &mut Criterion) {
    let mut group = c.benchmark_group("codegen");
    group.sample_size(30);

    let (hir, mirs, seed) = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = ast::SourceFile::cast(parse.green).unwrap();
        let hir = lower_source_file(file, &lexed.interner);
        let typeck = typeck::check_file(&hir);
        let seed = typeck::expr_type_seed(&typeck);
        let mirs = mir::lower_all(&hir, &typeck);
        (hir, mirs, seed)
    };

    group.bench_function("complex", |b| {
             b.iter(|| {
                  let _ =
                      codegen::core::gen_mir(black_box(&hir), black_box(&mirs), black_box(&seed));
              });
         });

    group.finish();
}

/// full pipeline using the production `effect::infer_file` path
/// (lex → parse → HIR → fused typeck+effect → MIR → codegen).
fn full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full-pipeline");
    group.sample_size(20);

    group.bench_function("minimal", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(MINIMAL_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let (typeck, _effects, _diags) = effect::infer_file(&hir);
                  let seed = typeck::expr_type_seed(&typeck);
                  let _ = codegen::core::gen_mir(&hir, &mir::lower_all(&hir, &typeck), &seed);
              });
         });

    group.bench_function("complex", |b| {
             b.iter(|| {
                  let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
                  let lexed = Lexer::new(&source).tokenize();
                  let parse = parser::parse(&lexed.tokens, &source);
                  let file = ast::SourceFile::cast(parse.green).unwrap();
                  let hir = lower_source_file(file, &lexed.interner);
                  let (typeck, _effects, _diags) = effect::infer_file(&hir);
                  let seed = typeck::expr_type_seed(&typeck);
                  let _ = codegen::core::gen_mir(&hir, &mir::lower_all(&hir, &typeck), &seed);
              });
         });

    group.finish();
}

/// one corpus file that the pipeline accepted.
struct CorpusFile {
    name: String,
    src: String,
}

/// every `.eye` file under `eyesrc/`, split by whether the pipeline accepts it.
/// `skipped` carries the reason, so the summary table can list it.
struct Corpus {
    files: Vec<CorpusFile>,
    skipped: Vec<(String, String)>,
}

/// criterion's directory name for a benchmark id: path separators become `_`.
fn criterion_dir(name: &str) -> String {
    name.replace('/', "_")
}

/// discovered once and shared by both corpus groups and the summary table.
/// a file is accepted when lexing, parsing and the checked front half raise no
/// `Error`-severity diagnostic (warnings do not fail a build, as in the driver).
fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
              fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
                  for entry in fs::read_dir(dir).unwrap() {
                      let path = entry.unwrap().path();
                      if path.is_dir() {
                          walk(&path, out);
                      } else if path.extension().is_some_and(|e| e == "eye") {
                          out.push(path);
                      }
                  }
              }
              let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("eyesrc");
              let mut paths = Vec::new();
              walk(&root, &mut paths);
              paths.sort();

              let mut corpus = Corpus { files: Vec::new(),
                                        skipped: Vec::new() };
              for path in paths {
                  let name = path.strip_prefix(&root)
                                 .unwrap()
                                 .to_string_lossy()
                                 .into_owned();
                  let src = fs::read_to_string(&path).unwrap();
                  match compile(&src) {
                      Ok(_) => corpus.files.push(CorpusFile { name, src }),
                      Err(reason) => corpus.skipped.push((name, reason)),
                  }
              }
              corpus
          })
}

/// one full compile (lex -> ... -> C string), the same chain as `full-pipeline`.
/// `Err` names the stage that rejected the source; codegen only runs on a
/// diagnostic-free front half, since it assumes resolved, well-typed HIR.
fn compile(src: &str) -> Result<String, String> {
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    if !lexed.diags.is_empty() {
        return Err("lexer errors".into());
    }
    let parse = parser::parse(&lexed.tokens, &source);
    if !parse.diagnostics.is_empty() {
        return Err("parser errors".into());
    }
    let file = ast::SourceFile::cast(parse.green).unwrap();
    let hir = lower_source_file(file, &lexed.interner);
    let (typeck, _effects, effect_diags) = effect::infer_file(&hir);

    let mut front = hir.diagnostics.clone();
    for results in typeck.values() {
        front.extend(results.diagnostics.clone());
    }
    front.extend(effect_diags);
    let fatal = front.into_diags()
                     .iter()
                     .filter(|d| d.kind.severity() == diagnostics::Severity::Error)
                     .count();
    if fatal > 0 {
        return Err(format!("{fatal} front-half error(s)"));
    }

    let seed = typeck::expr_type_seed(&typeck);
    Ok(codegen::core::gen_mir(&hir,
                              &mir::lower_all(&hir, &typeck),
                              &seed))
}

/// full pipeline on each corpus file separately. throughput is source bytes/s,
/// so rows of different sizes are comparable.
fn corpus_per_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("corpus-per-file");
    group.sample_size(20);
    for f in &corpus().files {
        group.throughput(Throughput::Bytes(f.src.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(&f.name), &f.src, |b, src| {
                 b.iter(|| black_box(compile(black_box(src))));
             });
    }
    group.finish();
}

/// full pipeline over the whole corpus in one iteration: total source bytes /
/// total time, which weights each file by its size.
fn corpus_total(c: &mut Criterion) {
    let files = &corpus().files;
    let bytes: usize = files.iter().map(|f| f.src.len()).sum();
    let mut group = c.benchmark_group("corpus-total");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(bytes as u64));
    group.bench_function(format!("{}-files", files.len()), |b| {
             b.iter(|| {
                  for f in files {
                      let _ = black_box(compile(black_box(&f.src)));
                  }
              });
         });
    group.finish();
}

/// mean time in ns from criterion's `estimates.json`, read without a JSON
/// dependency: the number after the first `"point_estimate":` following `"mean"`.
/// `None` when the file is missing or predates this run (e.g. a name filter
/// excluded the group), so stale results never reach the table.
fn criterion_mean_ns(name: &str, run_start: SystemTime) -> Option<f64> {
    let target =
        env::var_os("CARGO_TARGET_DIR").map(PathBuf::from)
                                       .unwrap_or_else(|| {
                                           Path::new(env!("CARGO_MANIFEST_DIR")).join("target")
                                       });
    let path = target.join("criterion/corpus-per-file")
                     .join(criterion_dir(name))
                     .join("new/estimates.json");
    if fs::metadata(&path).ok()?.modified().ok()? < run_start {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let after_mean = &text[text.find("\"mean\"")?..];
    let rest = &after_mean[after_mean.find("\"point_estimate\":")? + "\"point_estimate\":".len()..];
    let end =
        rest.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+')))?;
    rest[..end].parse().ok()
}

fn fmt_time(ns: f64) -> String {
    if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else {
        format!("{:.1} us", ns / 1e3)
    }
}

/// per-file table and the size-weighted summary, printed after criterion's own
/// output. the weighted figure is sum(bytes) / sum(mean times): the rate of one
/// big compile of the whole corpus, so large files count in proportion to size.
fn print_summary(run_start: SystemTime) {
    let corpus = corpus();
    let mut rows = Vec::new();
    for f in &corpus.files {
        if let Some(ns) = criterion_mean_ns(&f.name, run_start) {
            rows.push((f, ns));
        }
    }
    if rows.is_empty() {
        return;
    }
    let total_ns: f64 = rows.iter().map(|(_, ns)| ns).sum();
    let total_bytes: usize = rows.iter().map(|(f, _)| f.src.len()).sum();
    let total_lines: usize = rows.iter().map(|(f, _)| f.src.lines().count()).sum();
    let width = rows.iter().map(|(f, _)| f.name.len()).max().unwrap().max(4);
    let mib = |bytes: usize, ns: f64| bytes as f64 / (1024.0 * 1024.0) / (ns / 1e9);

    println!("\ncorpus: full pipeline (lex -> C), mean per file");
    println!("{:<width$}  {:>5}  {:>7}  {:>10}  {:>9}  {:>6}",
             "file", "lines", "bytes", "time", "MiB/s", "share");
    println!("{}", "-".repeat(width + 45));
    for (f, ns) in &rows {
        println!("{:<width$}  {:>5}  {:>7}  {:>10}  {:>9.1}  {:>5.1}%",
                 f.name,
                 f.src.lines().count(),
                 f.src.len(),
                 fmt_time(*ns),
                 mib(f.src.len(), *ns),
                 100.0 * ns / total_ns,);
    }
    println!("{}", "-".repeat(width + 45));
    println!("{:<width$}  {:>5}  {:>7}  {:>10}  {:>9.1}  {:>6}",
             format!("total ({} files)", rows.len()),
             total_lines,
             total_bytes,
             fmt_time(total_ns),
             mib(total_bytes, total_ns),
             "",);
    println!("\nweighted average: {:.1} MiB/s, {:.2} us per line  (size-weighted; unweighted mean per file: {})",
             mib(total_bytes, total_ns),
             total_ns / 1e3 / total_lines as f64,
             fmt_time(total_ns / rows.len() as f64),);
    if !corpus.skipped.is_empty() {
        println!("\nskipped (do not compile):");
        for (name, reason) in &corpus.skipped {
            println!("  {name}: {reason}");
        }
    }
}

criterion_group!(benches,
                 lex,
                 parse,
                 hir_lower,
                 typeck,
                 effect,
                 mir_lower,
                 codegen,
                 full_pipeline,
                 corpus_per_file,
                 corpus_total,);
fn main() {
    let run_start = SystemTime::now();
    benches();
    Criterion::default().configure_from_args().final_summary();
    print_summary(run_start);
}
