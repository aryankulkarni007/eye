//! compiler pipeline benchmarks. uses criterion for statistical analysis.
//! run with `cargo bench`.
//!
//! each benchmark measures a stage (or fused stage pair) on a representative
//! `.eye` program.

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use ast::AstNode;
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
            let mut hir = lower_source_file(file, &lexed.interner);
            let _ = typeck::check_file(&mut hir);
        });
    });

    group.bench_function("complex", |b| {
        b.iter(|| {
            let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
            let lexed = Lexer::new(&source).tokenize();
            let parse = parser::parse(&lexed.tokens, &source);
            let file = ast::SourceFile::cast(parse.green).unwrap();
            let mut hir = lower_source_file(file, &lexed.interner);
            let _ = typeck::check_file(&mut hir);
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
            let mut hir = lower_source_file(file, &lexed.interner);
            let _ = effect::infer_file(&mut hir);
        });
    });

    group.bench_function("complex", |b| {
        b.iter(|| {
            let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
            let lexed = Lexer::new(&source).tokenize();
            let parse = parser::parse(&lexed.tokens, &source);
            let file = ast::SourceFile::cast(parse.green).unwrap();
            let mut hir = lower_source_file(file, &lexed.interner);
            let _ = effect::infer_file(&mut hir);
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
        let mut hir = lower_source_file(file, &lexed.interner);
        let typeck = typeck::check_file(&mut hir);
        (hir, typeck)
    };

    group.bench_function("first_fn", |b| {
        let fn_id = *hir.items.functions.values().next().unwrap();
        let body_id = hir.functions[fn_id].body.unwrap();
        let body = &hir.bodies[body_id];
        b.iter(|| {
            let _ = lower_function(
                black_box(&hir),
                &hir.types,
                black_box(body),
                black_box(&typeck[&fn_id]),
                black_box(hir.functions[fn_id].params.len()),
                black_box(hir.functions[fn_id].ret),
            );
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
        let mut hir = lower_source_file(file, &lexed.interner);
        let typeck = typeck::check_file(&mut hir);
        let seed = typeck::expr_type_seed(&typeck);
        let mirs = mir::lower_all(&hir, &typeck);
        (hir, mirs, seed)
    };

    group.bench_function("complex", |b| {
        b.iter(|| {
            let _ = codegen::core::gen_mir(
                black_box(&hir),
                black_box(&mirs),
                black_box(&seed),
            );
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
            let mut hir = lower_source_file(file, &lexed.interner);
            let (typeck, _effects, _diags) = effect::infer_file(&mut hir);
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
            let mut hir = lower_source_file(file, &lexed.interner);
            let (typeck, _effects, _diags) = effect::infer_file(&mut hir);
            let seed = typeck::expr_type_seed(&typeck);
            let _ = codegen::core::gen_mir(&hir, &mir::lower_all(&hir, &typeck), &seed);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    lex,
    parse,
    hir_lower,
    typeck,
    effect,
    mir_lower,
    codegen,
    full_pipeline,
);
criterion_main!(benches);
