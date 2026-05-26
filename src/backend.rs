use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    thread,
};

use codegen::core::CGen;
use hir::core::HIR;

pub fn emit_and_compile(input_path: &Path, hir: &HIR) -> anyhow::Result<()> {
    println!("generating c code...");
    let generator = CGen::new(hir);
    let mut generated_c = generator.gen_all();

    println!("formatting c code...");
    generated_c = format_with_clang_format(generated_c);

    let c_output_path = input_path.with_extension("c");
    let binary_path = input_path.with_extension("");
    let mut c_file = File::create(&c_output_path)?;
    c_file.write_all(generated_c.as_bytes())?;
    println!("c source written to {}", c_output_path.display());

    println!("invoking c compiler...");
    let compile_status = Command::new("clang")
        .arg(&c_output_path)
        .arg("-o")
        .arg(&binary_path)
        .arg("-O2")
        .status();

    match compile_status {
        Ok(status) if status.success() => {
            println!("build successful: run `{}`", binary_path.display());
        }
        Ok(status) => {
            eprintln!("\nbackend compilation failed: {}", status);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\nFailed to launch C compiler (is clang installed?): {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Pipe `source` through `clang-format`, returning the formatted text or the
/// original input on any failure. Drains stdin from a dedicated writer thread
/// so the call cannot deadlock when both pipes fill.
fn format_with_clang_format(source: String) -> String {
    let mut child = match Command::new("clang-format")
        .arg("--fallback-style=LLVM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            println!("  (Note: clang-format missing from system; writing raw C layout)");
            return source;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            eprintln!("  (clang-format stdin unavailable; using raw layout)");
            return source;
        }
    };

    let input_bytes = source.clone().into_bytes();
    let writer = thread::spawn(move || stdin.write_all(&input_bytes));

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            let _ = writer.join();
            eprintln!("  (clang-format wait failed: {}; using raw layout)", e);
            return source;
        }
    };

    if let Ok(Err(e)) = writer.join() {
        eprintln!(
            "  (clang-format stdin write failed: {}; using raw layout)",
            e
        );
        return source;
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "  (clang-format exited {}; using raw layout)",
            output.status
        );
        if !stderr.trim().is_empty() {
            eprintln!("  clang-format stderr: {}", stderr.trim());
        }
        return source;
    }

    match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "  (clang-format produced non-UTF-8 output: {}; using raw layout)",
                e
            );
            source
        }
    }
}
