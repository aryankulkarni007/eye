use diagnostics::Diagnostic;
use hir::core::HIR;

/// print the HIR as a readable summary -- counts, names, types -- not full debug.
pub fn dump_hir(hir: &HIR) {
    println!("  structs: {} struct(s)", hir.structs.len());
    for (id, s) in hir.structs.iter() {
        println!("    Struct({:?}): {}", id, s.name);
        for &fid in &s.fields {
            let f = &hir.fields[fid];
            println!("      field {}: {:?}", f.name, f.ty);
        }
    }

    println!("  enums: {} enum(s)", hir.enums.len());
    for (id, e) in hir.enums.iter() {
        println!("    Enum({:?}): {}", id, e.name);
        for v in &e.variants {
            println!("      | {}", v.name);
        }
    }

    println!("  fields: {} field(s)", hir.fields.len());
    for (id, f) in hir.fields.iter() {
        println!("    Field({:?}): {} -> {:?}", id, f.name, f.ty);
    }

    println!("  functions: {} fn(s)", hir.functions.len());
    for (id, f) in hir.functions.iter() {
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{} {:?}", p.name, p.ty))
            .collect();
        println!(
            "    Fn({:?}): {}({}) -> {:?}{}",
            id,
            f.name,
            params.join(", "),
            f.ret,
            if f.is_extern { " [extern]" } else { "" }
        );
    }

    println!("  items:");
    for (name, fid) in &hir.items.functions {
        println!("    {name} -> fn({:?})", fid);
    }
    for (name, sid) in &hir.items.structs {
        println!("    {name} -> struct({:?})", sid);
    }
    for (name, eid) in &hir.items.enums {
        println!("    {name} -> enum({:?})", eid);
    }

    println!("  bodies: {} body(s)", hir.bodies.len());
    for (bid, body) in hir.bodies.iter() {
        println!("    Body({:?}):", bid);
        println!("      locals: {}", body.locals.len());
        for (lid, local) in body.locals.iter() {
            println!(
                "        Local({:?}): {}: {:?} {}",
                lid,
                local.name,
                local.ty,
                if local.mutable { "mut" } else { "" }
            );
        }
        println!("      pats: {}", body.pats.len());
        println!("      exprs: {}", body.exprs.len());
        // expression types are no longer stamped on the HIR body (S2C C5);
        // they live in the typeck results. the HIR dump shows structure only.
        for (eid, expr) in body.exprs.iter() {
            println!("        Expr({:?}): {}", eid, variant_name(expr));
        }
        println!("      stmts: {}", body.stmts.len());
        println!("      blocks: {}", body.blocks.len());
    }

    println!("  diagnostics: {}", hir.diagnostics.len());
    for (span, err) in hir.diagnostics.entries() {
        println!("    [{}] {} (span {span:?})", err.code(), err);
    }
}

fn variant_name(e: &hir::core::Expr) -> &'static str {
    use hir::core::Expr::*;
    match e {
        Missing => "Missing",
        Literal(_) => "Literal",
        Path(_) => "Path",
        Binary { .. } => "Binary",
        Unary { .. } => "Unary",
        Call { .. } => "Call",
        ArrayLit(_) => "ArrayLit",
        ArrayRepeat { .. } => "ArrayRepeat",
        Index { .. } => "Index",
        StructLit { .. } => "StructLit",
        Field { .. } => "Field",
        Assign { .. } => "Assign",
        If { .. } => "If",
        Loop { .. } => "Loop",
        Break => "Break",
        Continue => "Continue",
        Return(_) => "Return",
        Ref { .. } => "Ref",
        Deref { .. } => "Deref",
        Cast { .. } => "Cast",
        Match { .. } => "Match",
        SizeOf(_) => "SizeOf",
        Len(_) => "Len",
        Block(_) => "Block",
    }
}

/// print the HIR as full debug representation.
pub fn dump_hir_raw(hir: &HIR) {
    println!("{:#?}", hir);
}
