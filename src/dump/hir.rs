use hir::core::HIR;

/// Dump the full HIR to stderr for debugging.
pub fn dump_hir(hir: &HIR) {
    eprintln!("===== HIR DUMP =====");
    eprintln!("--- Structs ({}) ---", hir.structs.len());
    for (id, s) in hir.structs.iter() {
        eprintln!("  Struct({:?}): name={}, fields={:?}", id, s.name, s.fields);
    }
    eprintln!("--- Enums ({}) ---", hir.enums.len());
    for (id, e) in hir.enums.iter() {
        eprintln!(
            "  Enum({:?}): name={}, variants={:?}",
            id, e.name, e.variants
        );
    }
    eprintln!("--- Fields ({}) ---", hir.fields.len());
    for (id, f) in hir.fields.iter() {
        eprintln!("  Field({:?}): name={}, ty={:?}", id, f.name, f.ty);
    }
    eprintln!("--- Functions ({}) ---", hir.functions.len());
    for (id, f) in hir.functions.iter() {
        eprintln!(
            "  Fn({:?}): name={}, params={:?}, ret={:?}, body={:?}",
            id, f.name, f.params, f.ret, f.body
        );
    }
    eprintln!("--- ItemScope ---");
    eprintln!("  functions: {:?}", hir.items.functions);
    eprintln!("  structs:   {:?}", hir.items.structs);
    eprintln!("  enums:     {:?}", hir.items.enums);
    eprintln!("--- Bodies ---");
    for (id, body) in hir.bodies.iter() {
        eprintln!("  Body({:?}):", id);
        eprintln!("    locals ({})", body.locals.len());
        for (lid, local) in body.locals.iter() {
            eprintln!(
                "      Local({:?}): name='{}', ty={:?}, mutable={}",
                lid, local.name, local.ty, local.mutable
            );
        }
        eprintln!("    pats ({})", body.pats.len());
        for (pid, pat) in body.pats.iter() {
            eprintln!("      Pat({:?}): {:?}", pid, pat);
        }
        eprintln!("    exprs ({})", body.exprs.len());
        for (eid, expr) in body.exprs.iter() {
            let ty = body.expr_types.get(eid);
            eprintln!("      Expr({:?}): {:?}", eid, expr);
            if let Some(t) = ty {
                eprintln!("        type: {:?}", t);
            }
        }
        eprintln!("    stmts ({})", body.stmts.len());
        for (sid, stmt) in body.stmts.iter() {
            eprintln!("      Stmt({:?}): {:?}", sid, stmt);
        }
        eprintln!("    blocks ({})", body.blocks.len());
        for (bid, block) in body.blocks.iter() {
            eprintln!(
                "      Block({:?}): stmts={:?}, tail={:?}",
                bid, block.stmts, block.tail
            );
        }
        eprintln!("    body.block: {:?}", body.block);
        eprintln!("    body.tail:  {:?}", body.tail);
    }
    eprintln!("--- Diagnostics ({}) ---", hir.diagnostics.len());
    for d in &hir.diagnostics {
        eprintln!("  {}: {:?}", d.msg, d.ptr);
    }
}
