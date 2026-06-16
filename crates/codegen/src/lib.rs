// the house doc style keeps list-item continuation lines flush with the
// marker; clippy's `doc_lazy_continuation` (newly on by default) wants them
// indented. allowed crate-wide to keep the existing style consistent.
#![allow(clippy::doc_lazy_continuation)]

pub mod core;
