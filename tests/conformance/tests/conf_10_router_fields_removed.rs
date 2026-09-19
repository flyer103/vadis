//! CONF-10：删 router 自有字段后其余字节与客户端逐字节相同；`router_meta` 回显不进上游。
//!
//! 字节级断言已落地为 **真实执行** 的用例（下方三个），驱动
//! `router_core::RawBody::remove_top_level_keys`（ADR-007 单遍 span 扫描；
//! DESIGN §12.3.1）。完整语义的其余断言（router_meta 回显不进上游 = 代理链路层面
//! 的检查，依赖 R2 的转发路径）仍挂 `#[ignore]`，届时本文件在此追加。

#![forbid(unsafe_code)]

use router_core::{RawBody, ROUTER_OWNED_TOP_LEVEL_KEYS};

/// 主体：模拟真实客户端请求（含转义、多字节 UTF-8、嵌套结构、尾随换行），
/// 删除白名单键后，除被删成员及其分隔逗号外**逐字节相同**。
#[tokio::test]
async fn conf_10_byte_exact_after_router_field_removal() {
    // 尾部 \n 刻意保留在字面量里：尾随换行必须原样透传。
    let input = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"router_meta\": {\"echo\": true, \"nested\": [{\"k\": \"v\"}]},\
        \"stream\": true\
    }\n";
    let raw = RawBody::new(input.as_bytes().to_vec());

    let out = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");

    // 期望 = 同一输入去掉 router_meta 成员 + 其前导逗号，其余逐字节不动。
    let expected = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"stream\": true\
    }\n";
    assert_eq!(out.as_bytes(), expected.as_bytes());

    // 语义交叉校验：router_meta 不在，其余键俱在。
    let v: serde_json::Value = serde_json::from_slice(out.as_bytes()).unwrap();
    assert!(v.get("router_meta").is_none());
    assert_eq!(v["model"], "provider/model");
    assert_eq!(v["stream"], true);
}

/// 键不存在时是严格 no-op：输出与输入**逐字节相同**（含全部空白）。
#[tokio::test]
async fn conf_10_noop_when_no_router_fields_present() {
    let input = "{\n  \"model\": \"m\",\n  \"n\": [1, 2, {\"deep\": \"},\"}]\n}\r\n";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let out = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");
    assert_eq!(out.as_bytes(), input.as_bytes());
}

/// 幂等：连续两次删除 == 一次删除（AGENTS 硬约束 2 的内容确定性推论）。
#[tokio::test]
async fn conf_10_removal_is_idempotent() {
    let input = "{\"a\":1,\"router_meta\":{\"b\":[2,{\"c\":\"},\"}],\"d\":null},\"e\":true}";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let once = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    let twice = once
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    assert_eq!(once.as_bytes(), twice.as_bytes());
    assert_eq!(once.as_bytes(), b"{\"a\":1,\"e\":true}");
}

/// 代理链路级断言（router_meta 回显由响应侧注入、绝不进入上游请求）依赖 R2 的
/// 转发路径；字节级删除语义已由上面三个真实用例覆盖。
#[ignore = "CONF-10（链路级）：依赖 R2 转发路径——router_meta 回显由 proxy 响应侧注入"]
#[tokio::test]
async fn conf_10_router_meta_echo_never_reaches_upstream() {
    unimplemented!("proxy forwarding path lands in Round 2");
}
