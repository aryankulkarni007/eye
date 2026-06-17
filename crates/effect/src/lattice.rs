//! the effect lattice: the [`Atom`] machine-effect kinds, the [`EffectSet`]
//! bitset (join = union, bottom = `pure`), the dense witness-atom index
//! (`atom_index` / `LIVE_ATOMS`), and the annotation-name <-> set mapping
//! (`parse_effect_name` / `describe`).

/// the number of live atoms (io/ffi/state) - the witness-array width.
pub(crate) const LIVE_ATOMS: usize = 3;

/// dense index of a live atom for witness storage (`io`=0, `ffi`=1, `state`=2).
/// reserved atoms have no producer, so no witness.
pub(crate) fn atom_index(atom: Atom) -> Option<usize> {
    match atom {
        Atom::Io => Some(0),
        Atom::Ffi => Some(1),
        Atom::State => Some(2),
        _ => None,
    }
}

/// the effect lattice: a bitset of machine-effect atoms. `pure` is the empty
/// set (the lattice bottom); union is the join. row-ready (EFFECT.md): `atoms`
/// is the live bitset and a future effect-variable tail slot stays dormant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EffectSet {
    atoms: u8,
}

/// one machine-effect atom. live atoms have producers today; reserved atoms
/// hold their bit and start firing when their primitive lands (EFFECT.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Atom {
    /// `print` / `println` - the printf seam.
    Io = 1 << 0,
    /// calling an `extern` fn, or dereferencing a raw pointer (`T*` / `ptr`).
    Ffi = 1 << 1,
    /// reading or writing a `mut` global.
    State = 1 << 2,
    /// reserved (no producer yet): a real heap allocator.
    Alloc = 1 << 3,
    /// reserved: bounds traps (runtime-safety theme).
    Panic = 1 << 4,
    /// reserved: non-termination analysis (gates prime totality).
    Diverge = 1 << 5,
}

impl EffectSet {
    /// the empty set - `pure`, the lattice bottom.
    pub const fn pure() -> Self {
        Self { atoms: 0 }
    }

    /// true when no atom is set (the fn is `pure`).
    pub fn is_pure(self) -> bool {
        self.atoms == 0
    }

    /// true when `atom` is in the set.
    pub fn contains(self, atom: Atom) -> bool {
        self.atoms & atom as u8 != 0
    }

    /// add `atom` to the set.
    pub fn insert(&mut self, atom: Atom) {
        self.atoms |= atom as u8;
    }

    /// the join (union) of two sets - the fixpoint's upward step.
    pub fn union(self, other: Self) -> Self {
        Self {
            atoms: self.atoms | other.atoms,
        }
    }

    /// the raw atom bitset (for fixpoint storage / `Eq` backdating).
    pub fn bits(self) -> u8 {
        self.atoms
    }

    /// the full *live* set (`io | ffi | state`) - the conservative answer for a
    /// call through a fn-pointer value, whose target is unknown at compile time
    /// (EFFECT.md). reserved atoms are excluded: they have no producer, so no
    /// honest verdict could claim them.
    pub const fn live() -> Self {
        Self {
            atoms: Atom::Io as u8 | Atom::Ffi as u8 | Atom::State as u8,
        }
    }
}

/// a declared effect name -> its atom. `pure` is the explicit empty set
/// (`Ok(None)`); an unknown name is `Err` (only the live atoms are valid
/// annotation names - reserved atoms have no producer, EFFECT.md).
pub(crate) fn parse_effect_name(name: &str) -> Result<Option<Atom>, ()> {
    match name {
        "pure" => Ok(None),
        "io" => Ok(Some(Atom::Io)),
        "ffi" => Ok(Some(Atom::Ffi)),
        "state" => Ok(Some(Atom::State)),
        _ => Err(()),
    }
}

/// render an effect set as its annotation spelling: `pure` for the empty set,
/// else the live atoms joined with ` | ` in a fixed order.
pub(crate) fn describe(set: EffectSet) -> String {
    if set.is_pure() {
        return "pure".to_string();
    }
    let mut parts = Vec::new();
    if set.contains(Atom::Io) {
        parts.push("io");
    }
    if set.contains(Atom::Ffi) {
        parts.push("ffi");
    }
    if set.contains(Atom::State) {
        parts.push("state");
    }
    parts.join(" | ")
}
