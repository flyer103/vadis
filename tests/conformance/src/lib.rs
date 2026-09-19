//! conformance 用例宿主 crate。用例按 DESIGN §12.8 的 CONF-01…CONF-19 编号，
//! 位于 `tests/conf_<NN>_<slug>.rs`；未实现路径一律 `#[ignore = "CONF-NN: 依赖 <实现项>"]`。

#![forbid(unsafe_code)]
