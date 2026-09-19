//! Conformance host crate. Cases are numbered CONF-01…CONF-19 per DESIGN
//! §12.8, living in `tests/conf_<NN>_<slug>.rs`; unimplemented paths all
//! carry `#[ignore = "CONF-NN: depends on <item>"]`.

#![forbid(unsafe_code)]
