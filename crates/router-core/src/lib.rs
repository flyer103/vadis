//! 领域模型与纯函数核心（DESIGN §2/§12.1）。
//!
//! 硬约束：不依赖任何 HTTP / 协议 crate；金额全程整数 NanoUsd（ADR-006 裁定口径，
//! 决定路径与 trace 中不出现浮点）。

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

pub mod error;
