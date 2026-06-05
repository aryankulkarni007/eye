//! Compiler pipeline benchmarks. Uses Criterion for statistical analysis.
//! Run with `cargo bench`.
//!
//! Each benchmark measures the full pipeline (lex → parse → HIR → MIR → codegen)
//! on a representative `.eye` program.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use ast::AstNode;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};
use mir::lower::lower_function;

/// A moderate-sized program exercising all parser and lowering paths.
const COMPLEX_PROGRAM: &str = include_str!("../eyesrc/programs/raytracer.eye");

/// A minimal program for baseline overhead measurement.
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

    let minimal_parse = {
        let source = SourceText::new(MINIMAL_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        parser::parse(&lexed.tokens, &source)
    };
    let complex_parse = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        parser::parse(&lexed.tokens, &source)
    };

    group.bench_function("minimal", |b| {
        let file = ast::SourceFile::cast(minimal_parse.green.clone()).unwrap();
        b.iter(|| {
            let _ = lower_source_file(black_box(file.clone()));
        });
    });

    group.bench_function("complex", |b| {
        let file = ast::SourceFile::cast(complex_parse.green.clone()).unwrap();
        b.iter(|| {
            let _ = lower_source_file(black_box(file.clone()));
        });
    });

    group.finish();
}

fn mir_lower(c: &mut Criterion) {
    let mut group = c.benchmark_group("mir-lower");
    group.sample_size(30);

    let hir = {
        let source = SourceText::new(COMPLEX_PROGRAM.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = ast::SourceFile::cast(parse.green).unwrap();
        lower_source_file(file)
    };

    group.bench_function("first_fn", |b| {
        let fn_id = *hir.items.functions.values().next().unwrap();
        let body_id = hir.functions[fn_id].body.unwrap();
        let body = &hir.bodies[body_id];
        b.iter(|| {
            let _ = lower_function(
                black_box(&hir),
                black_box(body),
                black_box(hir.functions[fn_id].params.len()),
                black_box(hir.functions[fn_id].ret),
            );
        });
    });

    group.finish();
}

fn full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full-pipeline");
    group.sample_size(20);

    group.bench_function("minimal", |b| {
        b.iter(|| {
            let source = SourceText::new(black_box(MINIMAL_PROGRAM).to_string());
            let lexed = Lexer::new(&source).tokenize();
            let parse = parser::parse(&lexed.tokens, &source);
            let file = ast::SourceFile::cast(parse.green).unwrap();
            let hir = lower_source_file(file);
            let _ = codegen::core::gen_mir(&hir);
        });
    });

    group.bench_function("complex", |b| {
        b.iter(|| {
            let source = SourceText::new(black_box(COMPLEX_PROGRAM).to_string());
            let lexed = Lexer::new(&source).tokenize();
            let parse = parser::parse(&lexed.tokens, &source);
            let file = ast::SourceFile::cast(parse.green).unwrap();
            let hir = lower_source_file(file);
            let _ = codegen::core::gen_mir(&hir);
        });
    });

    group.finish();
}

criterion_group!(benches, lex, parse, hir_lower, mir_lower, full_pipeline);
criterion_main!(benches);
