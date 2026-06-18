//! value emission: `gen_rvalue` / `gen_operand` / `gen_place`, the place-type
//! recovery they drive (`place_type` / `index_access` / `place_is_pointer_like`
//! / `field_type`), the `println` intrinsic rendering, and literals. these turn
//! a MIR value into its c text; no control flow lives here.

use hir::core::{Literal, TypeInterner, TypeKind, TypeRef};
use mir::core::{MirBody, Operand, Place, RValue, Type};
use std::fmt::Write as _;

use super::super::arrays::array_wrapper_name;
use super::super::types::{CType, spec_for_type};
use super::{MirGen, c_fn_name, local_name, write_c_char_literal};

enum IndexAccess {
    ArrayValue,
    ArrayPointer,
    Direct,
}

/// whether `ty` is a string value - a reference to a `uint8` array
/// (`&[uint8; N]`). such a value prints with `%s` over its byte array, not as a
/// pointer address.
fn is_byte_string(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Ref(inner)
            if matches!(
                types.lookup(*inner),
                TypeKind::Array { elem, .. } if matches!(types.lookup(*elem), TypeKind::Path(n) if n == "uint8")
            )
    )
}

impl<'a> MirGen<'a> {
    pub(crate) fn gen_rvalue(&mut self, mir: &MirBody, rv: &RValue) {
        match rv {
            RValue::Use(op) => self.gen_operand(mir, op),
            RValue::Binary(op, a, b) => {
                self.output.push('(');
                self.gen_operand(mir, a);
                self.w(format_args!(" {} ", op));
                self.gen_operand(mir, b);
                self.output.push(')');
            }
            RValue::Unary(op, operand) => {
                self.w(format_args!("{}", op));
                self.gen_operand(mir, operand);
            }
            RValue::Cast(operand, ty) => {
                let types = &self.hir.types;
                self.output
                    .write_fmt(format_args!("({})", CType::new(*ty, types)))
                    .expect("writing to String cannot fail");
                self.gen_operand(mir, operand);
            }
            // `sizeof(T)`: the c backend is the layout authority.
            RValue::SizeOf(ty) => {
                let types = &self.hir.types;
                self.output
                    .write_fmt(format_args!("sizeof({})", CType::new(*ty, types)))
                    .expect("writing to String cannot fail");
            }
            RValue::Ref(place) => {
                self.output.push('&');
                self.gen_place(mir, place);
            }
            RValue::Deref(operand) => {
                self.output.push_str("(*");
                self.gen_operand(mir, operand);
                self.output.push(')');
            }
            RValue::Println { args } => self.gen_println(mir, args),
            RValue::Variant(v) => {
                let label = &self.hir.enums[v.enum_id].variants[v.idx as usize].name;
                self.w(format_args!("{}", label));
            }
            RValue::Call { func, args } => {
                let name = c_fn_name(&self.hir.functions[*func]);
                self.w(format_args!("{}(", name));
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, a);
                }
                self.output.push(')');
            }
            // an indirect call: call through the function-pointer operand. c
            // calls through a pointer value with ordinary call syntax.
            RValue::CallIndirect { callee, args } => {
                self.gen_operand(mir, callee);
                self.output.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, a);
                }
                self.output.push(')');
            }
            // a function used as a value: its bare c name, which decays to a
            // function pointer in value context.
            RValue::Func(func) => {
                let name = c_fn_name(&self.hir.functions[*func]);
                self.w(format_args!("{}", name));
            }
            // a value array is a compound literal of its wrapper struct:
            // `(__eye_arr_T_N){{ a, b, c }}` - the outer brace is the struct,
            // the inner initializes its `data[N]`. the type is carried on the
            // node, so unlike the HIR path there is no type-recovery fallback:
            // the emitter trusts the lowering (R2 / I2).
            RValue::ArrayLit { ty, elems } => {
                let types = &self.hir.types;
                let TypeKind::Array { elem, len } = types.lookup(*ty) else {
                    unreachable!("ArrayLit rvalue carries a non-array type: {ty:?}");
                };
                self.output
                    .write_fmt(format_args!(
                        "({}){{{{ ",
                        array_wrapper_name(*elem, *len, types)
                    ))
                    .expect("writing to String cannot fail");
                for (i, el) in elems.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, el);
                }
                self.output.push_str(" }}");
            }
            // `[value; count]`: emit the wrapper with `count` copies of the
            // (already evaluated-once) operand. a future native backend can lower
            // this to a fill loop / memset instead.
            RValue::ArrayRepeat { ty, value, count } => {
                let types = &self.hir.types;
                let TypeKind::Array { elem, len } = types.lookup(*ty) else {
                    unreachable!("ArrayRepeat rvalue carries a non-array type: {ty:?}");
                };
                self.output
                    .write_fmt(format_args!(
                        "({}){{{{ ",
                        array_wrapper_name(*elem, *len, types)
                    ))
                    .expect("writing to String cannot fail");
                for i in 0..*count {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, value);
                }
                self.output.push_str(" }}");
            }
            RValue::StructLit { ty, fields } => {
                let types = &self.hir.types;
                self.output
                    .write_fmt(format_args!("({}){{ ", CType::new(*ty, types)))
                    .expect("writing to String cannot fail");
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.w(format_args!(".{} = ", name));
                    self.gen_operand(mir, value);
                }
                self.output.push_str(" }");
            }
        }
    }

    pub(crate) fn gen_operand(&mut self, mir: &MirBody, op: &Operand) {
        match op {
            Operand::Const(lit) => self.gen_literal(lit),
            Operand::Copy(place) => self.gen_place(mir, place),
        }
    }

    pub(crate) fn gen_place(&mut self, mir: &MirBody, place: &Place) {
        match place {
            Place::Local(id) => {
                self.output.push_str(local_name(&self.local_names, *id));
            }
            // a global is referenced by its bare c symbol name.
            Place::Global(name) => self.output.push_str(name),
            // arrays are wrapper structs, so indexing reaches through `data`. a
            // value array uses `.data[i]`; a reference or pointer to an array
            // uses `->data[i]`; a raw pointer indexes directly. the decision is
            // driven by the base place's type (`place_type`).
            Place::Index(base, index) => {
                self.gen_place(mir, base);
                match self.index_access(mir, base) {
                    IndexAccess::ArrayValue => self.output.push_str(".data["),
                    IndexAccess::ArrayPointer => self.output.push_str("->data["),
                    IndexAccess::Direct => self.output.push('['),
                }
                self.gen_operand(mir, index);
                self.output.push(']');
            }
            // a field through a reference or pointer auto-derefs to `->`; a
            // field on a value uses `.`.
            Place::Field(base, name) => {
                self.gen_place(mir, base);
                if self.place_is_pointer_like(mir, base) {
                    self.w(format_args!("->{}", name));
                } else {
                    self.w(format_args!(".{}", name));
                }
            }
            Place::Deref(base) => {
                self.output.push_str("(*");
                self.gen_place(mir, base);
                self.output.push(')');
            }
        }
    }

    pub(crate) fn gen_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(v) => self.output.push_str(itoa::Buffer::new().format(*v)),
            Literal::Float(s) => self.output.push_str(s.as_str()),
            // a string literal is `&[uint8; N]`: a wrapper pointer into its
            // file-scope byte static. the static is `uint8_t[]`, layout-identical
            // to the wrapper's first member, so the cast lets `s->data[i]`/`len`
            // work. the print intrinsic emits the byte form (`%s`) separately.
            Literal::String(s) => {
                let n = hir::core::decode_string_literal(s).len() as u64;
                let types = &self.hir.types;
                let uint8_ty = types.uint8_ty();
                let wrapper = array_wrapper_name(uint8_ty, n, types);
                self.w(format_args!("({}*)__eye_str{}", wrapper, self.string_id(s)));
            }
            Literal::Bool(b) => self.output.push_str(if *b { "true" } else { "false" }),
            Literal::Char(c) => write_c_char_literal(*c, &mut self.output),
        }
    }

    /// the `println` intrinsic, lowered to `printf` with a per-argument specifier
    /// substituted for each `{}` and a trailing `\n` appended automatically.
    fn gen_println(&mut self, mir: &MirBody, args: &[Operand]) {
        self.output.push_str("printf(");
        let Some((fmt, values)) = args.split_first() else {
            self.output.push(')');
            return;
        };

        let Operand::Const(Literal::String(s)) = fmt else {
            // non-literal format string: forward operands unchanged.
            self.gen_operand(mir, fmt);
            for v in values {
                self.output.push_str(", ");
                self.gen_operand(mir, v);
            }
            self.output.push(')');
            return;
        };

        let mut value_iter = values.iter();
        let mut rendered = String::with_capacity(s.len() + values.len() * 2);
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'{') {
                // `{{` escapes a literal `{` (and `}}` a literal `}` below);
                // the HIR arity scan skips them with the same rule.
                chars.next();
                rendered.push('{');
            } else if c == '{' && chars.peek() == Some(&'}') {
                // placeholder/argument counts are equal here: HIR rejects a
                // mismatch for any literal format string (U5,
                // printlnaritymismatch) with the same `{}` scan. the `%d`
                // fallback below is defensive only.
                chars.next();
                let spec = value_iter
                    .next()
                    .map(|op| self.operand_spec(mir, op))
                    .unwrap_or("%d");
                rendered.push_str(spec);
            } else if c == '}' && chars.peek() == Some(&'}') {
                chars.next();
                rendered.push('}');
            } else if c == '%' {
                rendered.push_str("%%");
            } else {
                rendered.push(c);
            }
        }
        self.w(format_args!("\"{}\\n\"", rendered));
        for v in values {
            self.output.push_str(", ");
            self.gen_println_value(mir, v);
        }
        self.output.push(')');
    }

    /// emit a `println` value argument. a string (`&[uint8; N]`) prints with `%s`,
    /// which needs a `char*` byte pointer, not the wrapper pointer (`%p`): a
    /// string literal is its raw c string (NUL-terminated by c); a string place
    /// dereferences to its byte array (`->data`, NUL-terminated by the static).
    /// every other value emits normally.
    fn gen_println_value(&mut self, mir: &MirBody, op: &Operand) {
        match op {
            Operand::Const(Literal::String(s)) => self.w(format_args!("\"{}\"", s)),
            Operand::Copy(place) => {
                let (is_str, is_pointer) = {
                    let types = &self.hir.types;
                    let ty = self.place_type(mir, place);
                    // everything `spec_for_type` formats as `%p` except `ptr`
                    // itself, which is already `void*` and needs no cast.
                    (
                        is_byte_string(ty, types),
                        matches!(
                            types.lookup(ty),
                            TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::Fn { .. }
                        ),
                    )
                };
                if is_str {
                    self.gen_place(mir, place);
                    self.output.push_str("->data");
                } else if is_pointer {
                    // `%p` requires a `void*` argument; any other pointer type
                    // passed through varargs is formally undefined.
                    self.output.push_str("(void*)");
                    self.gen_operand(mir, op);
                } else {
                    self.gen_operand(mir, op);
                }
            }
            _ => self.gen_operand(mir, op),
        }
    }

    fn operand_spec(&mut self, mir: &MirBody, op: &Operand) -> &'static str {
        match op {
            Operand::Const(Literal::Int(_)) => "%d",
            Operand::Const(Literal::Float(_)) => "%f",
            Operand::Const(Literal::String(_)) => "%s",
            Operand::Const(Literal::Bool(_)) => "%d",
            Operand::Const(Literal::Char(_)) => "%c",
            Operand::Copy(place) => {
                let types = &self.hir.types;
                spec_for_type(self.place_type(mir, place), types)
            }
        }
    }

    /// recover the type of a place from `MirLocal.ty` plus the HIR struct/union
    /// definitions. total (REDESIGN I2): it always returns a [`Type`], never
    /// rejects. a projection whose type cannot be resolved (only reachable on a
    /// malformed input the front end would already have diagnosed) falls back to
    /// [`TypeRef::Error`], which the callers (the `.`/`->` and `.data[]`
    /// decisions, the printf specifier) handle without panicking.
    // A2: memoized. the cache is checked on entry and populated
    // on each return. repeated calls (index_access, place_is_pointer_like,
    // specifier resolution) for the same place are o(1) after the first walk.
    fn place_type(&mut self, mir: &MirBody, place: &Place) -> Type {
        if let Some(&ty) = self.place_types.get(place) {
            return ty;
        }
        let ty = match place {
            Place::Local(id) => mir.locals[*id].ty,
            // a global's type comes from its HIR declaration.
            Place::Global(name) => match self.hir.items.globals.get(name) {
                Some(&id) => self.hir.globals[id].ty,
                None => self.error_ty,
            },
            Place::Field(base, name) => self.field_type(mir, base, name),
            // `a[i]` has the element type of an array base, or the pointee of a
            // reference/raw-pointer base.
            Place::Index(base, _) => {
                let base_ty = self.place_type(mir, base);
                let types = &self.hir.types;
                match types.lookup(base_ty) {
                    TypeKind::Array { elem, .. } => *elem,
                    TypeKind::Ref(inner) | TypeKind::Ptr(inner) => match types.lookup(*inner) {
                        TypeKind::Array { elem, .. } => *elem,
                        _ => *inner,
                    },
                    _ => base_ty,
                }
            }
            // `*p` has the pointee type.
            Place::Deref(base) => {
                let base_ty = self.place_type(mir, base);
                let types = &self.hir.types;
                match types.lookup(base_ty) {
                    TypeKind::Ref(inner) | TypeKind::Ptr(inner) => *inner,
                    _ => base_ty,
                }
            }
        };
        self.place_types.insert(place.clone(), ty);
        ty
    }

    fn index_access(&mut self, mir: &MirBody, place: &Place) -> IndexAccess {
        let ty = self.place_type(mir, place);
        let types = &self.hir.types;
        match types.lookup(ty) {
            TypeKind::Array { .. } => IndexAccess::ArrayValue,
            TypeKind::Ref(inner) | TypeKind::Ptr(inner)
                if matches!(types.lookup(*inner), TypeKind::Array { .. }) =>
            {
                IndexAccess::ArrayPointer
            }
            _ => IndexAccess::Direct,
        }
    }

    fn place_is_pointer_like(&mut self, mir: &MirBody, place: &Place) -> bool {
        let ty = self.place_type(mir, place);
        let types = &self.hir.types;
        matches!(types.lookup(ty), TypeKind::Ref(_) | TypeKind::Ptr(_))
    }

    /// the declared type of field `name` on the (possibly reference/pointer)
    /// struct or union that `base` resolves to. structs and unions share the
    /// field arena, so a union member resolves the same way.
    fn field_type(&mut self, mir: &MirBody, base: &Place, name: &hir::core::Text) -> Type {
        let base_ty = self.place_type(mir, base);
        let types = &self.hir.types;
        let struct_name = match types.lookup(base_ty) {
            TypeKind::Path(n) => n,
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => match types.lookup(*inner) {
                TypeKind::Path(n) => n,
                _ => return self.error_ty,
            },
            _ => return self.error_ty,
        };
        let field_id = self
            .hir
            .items
            .structs
            .get(struct_name)
            .and_then(|&id| self.hir.structs[id].field_index.get(name).copied())
            .or_else(|| {
                self.hir
                    .items
                    .unions
                    .get(struct_name)
                    .and_then(|&id| self.hir.unions[id].field_index.get(name).copied())
            });
        match field_id {
            Some(id) => self.hir.fields[id].ty,
            None => self.error_ty,
        }
    }
}
