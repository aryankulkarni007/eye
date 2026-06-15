use hir::core::{FnId, HIR};
use mir::core::{MirBody, MirStmt, RValue};
use rustc_hash::FxHashMap;
use std::fmt::Write as _;

/// print every function's MIR body as a readable summary (counts, not full
/// field listings). `mirs` is the pre-lowered MIR map (the database's
/// `mir_map` query result), shared with c generation so the dump never
/// re-lowers a body.
pub fn dump_mir(hir: &HIR, mirs: &FxHashMap<FnId, MirBody>) {
    let fn_names: rustc_hash::FxHashMap<_, _> = hir
        .items
        .functions
        .iter()
        .map(|(name, &fid)| (fid, name.clone()))
        .collect();

    for (fn_id, _) in hir.functions.iter() {
        let Some(mir) = mirs.get(&fn_id) else {
            continue;
        };
        let name = fn_names.get(&fn_id).map(|s| s.as_str()).unwrap_or("<anon>");
        print_mir_summary(name, mir);
    }
}

/// print every function's MIR body as the full debug representation.
pub fn dump_mir_raw(hir: &HIR, mirs: &FxHashMap<FnId, MirBody>) {
    let fn_names: rustc_hash::FxHashMap<_, _> = hir
        .items
        .functions
        .iter()
        .map(|(name, &fid)| (fid, name.clone()))
        .collect();

    for (fn_id, _) in hir.functions.iter() {
        let Some(mir) = mirs.get(&fn_id) else {
            continue;
        };
        let name = fn_names.get(&fn_id).map(|s| s.as_str()).unwrap_or("<anon>");
        println!("fn {}:", name);
        println!("{:#?}", mir);
        println!();
    }
}

fn print_mir_summary(fn_name: &str, mir: &MirBody) {
    let mut out = String::new();
    let _ = writeln!(out, "fn {}:", fn_name);
    let _ = writeln!(out, "  locals: {}", mir.locals.len());
    let _ = writeln!(out, "  params: {}", mir.params.len());
    let (total, mut counts) = count_stmts(&mir.body.stmts);
    let _ = write!(out, "  stmts: {}", total);
    if !counts.is_empty() {
        counts.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        for (kind, n) in &counts {
            let _ = write!(out, ", {}: {}", kind, n);
        }
    }
    let _ = writeln!(out);
    println!("{}", out);
}

fn count_stmts(stmts: &[MirStmt]) -> (usize, Vec<(&'static str, usize)>) {
    use std::collections::BTreeMap;
    let mut total = 0usize;
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    for stmt in stmts {
        total += 1;
        let (label, nested) = match stmt {
            MirStmt::Let { .. } => ("Let", None),
            MirStmt::Assign { .. } => ("Assign", None),
            MirStmt::Eval(RValue::Println { .. }) => ("Println", None),
            MirStmt::Eval(_) => ("Eval", None),
            MirStmt::If {
                then_block,
                else_block,
                ..
            } => {
                let (tn, tc) = count_stmts(&then_block.stmts);
                let (en, ec) = else_block
                    .as_ref()
                    .map_or((0, vec![]), |b| count_stmts(&b.stmts));
                let mut combined = tc;
                for (k, v) in ec {
                    combined.push((k, v));
                }
                ("If", Some((tn + en, combined)))
            }
            MirStmt::Loop { body } => {
                let (n, c) = count_stmts(&body.stmts);
                ("Loop", Some((n, c)))
            }
            MirStmt::Switch { arms, default, .. } => {
                let mut sub_total = 0usize;
                let mut sub_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
                for arm in arms {
                    let (n, c) = count_stmts(&arm.body.stmts);
                    sub_total += n;
                    for (k, v) in c {
                        *sub_counts.entry(k).or_default() += v;
                    }
                }
                if let Some(def) = default {
                    let (n, c) = count_stmts(&def.stmts);
                    sub_total += n;
                    for (k, v) in c {
                        *sub_counts.entry(k).or_default() += v;
                    }
                }
                let sub_vec: Vec<_> = sub_counts.into_iter().collect();
                ("Switch", Some((sub_total, sub_vec)))
            }
            MirStmt::Break => ("Break", None),
            MirStmt::Continue => ("Continue", None),
            MirStmt::Return(_) => ("Return", None),
        };
        *kinds.entry(label).or_default() += 1;
        if let Some((_, child_counts)) = nested {
            for (k, v) in child_counts {
                *kinds.entry(k).or_default() += v;
            }
        }
    }
    let as_vec: Vec<_> = kinds.into_iter().collect();
    (total, as_vec)
}
