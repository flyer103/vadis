//! 字节保真原语 `RawBody`（DESIGN §12.3.1，ADR-007「span 保真转发」，AGENTS 硬约束 1/2）。
//!
//! 唯一允许的改写是删除顶层 router 自有字段；其余字节逐字节保留。
//! 实现是**单遍 span 扫描器**（跟踪字符串/转义/括号深度），只定位待删成员的
//! 字节区间后做区间剔除——**禁止 parse → reserialize 往返**（那是字节边界最常见
//! 的破法）。`serde_json` 仅用作原始值片段的**校验器**（true/false/null/number），
//! 绝不用它产出任何出站字节。
//!
//! 容器偏差：DESIGN 草图画的是 `RawBody(Bytes)`，但 `bytes` crate 不在
//! `router-core` 的依赖白名单（§12.1：仅 serde/serde_json），故取 `Vec<u8>`。
//! 出站零拷贝（`Bytes::from(vec)`）在 R2 的 proxy 层完成，语义不受影响。

/// 顶层 router 自有字段白名单（spec §2 / DESIGN §12.3.1）。
///
/// 这是**唯一的**删除清单：`router_meta` 回显 + 路由提示。
/// 新增 router 自有键必须改这里——不允许在调用点散落第二份清单。
/// 注意：客户端请求里合法的未知字段（CONF-11）**不在**此列，永不删除。
pub const ROUTER_OWNED_TOP_LEVEL_KEYS: &[&str] = &["router_meta"];

/// `remove_top_level_keys` 的失败语义（三种失败可被调用方区分处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEditError {
    /// body 为空或仅空白（无可扫描 JSON）。
    EmptyBody,
    /// 顶层不是 JSON 对象（数组/字符串/数字/true/false/null 一律拒绝）。
    NotTopLevelObject { first_byte: u8 },
    /// JSON 结构非法：未闭合字符串/括号、非法转义、尾随内容、空值等。
    Malformed { offset: usize },
}

/// 客户端原始字节：唯一权威。不实现 `DerefMut`/`AsMut`，不暴露任何可变视图，
/// 编译期挡住"顺手改一改"（DESIGN §12.3.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBody(Vec<u8>);

impl RawBody {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// 唯一允许的改写：删除顶层白名单键，其余字节逐字节保留。
    ///
    /// - `keys` 为空或全部未命中 → 恒等（返回逐字节相同的副本）；
    /// - 幂等：连续两次调用 == 一次调用；
    /// - 纯函数：只依赖 (内容, keys)，不依赖时钟/RNG/轮次（AGENTS 硬约束 2）。
    pub fn remove_top_level_keys(&self, keys: &[&str]) -> Result<RawBody, RawEditError> {
        let b = &self.0;
        let members = scan_top_level_members(b)?;

        // 待删成员的扩展区间：除了成员自身 span，还要吞掉一个相邻分隔逗号，
        // 使剩余字节仍是合法 JSON。区间合并保证共享同一逗号的连续删除不重叠。
        let mut dels: Vec<(usize, usize)> = Vec::new();
        for (idx, m) in members.iter().enumerate() {
            let decoded_key = decode_json_string(b, m.key_start, m.key_end)?;
            if !keys.iter().any(|k| *k == decoded_key) {
                continue;
            }
            let (mut s, mut e) = (m.key_start, m.val_end);
            if idx > 0 {
                // 吞掉前导逗号（含成员与逗号之间的空白）。
                let mut j = s;
                while j > 0 && is_ws(b[j - 1]) {
                    j -= 1;
                }
                if j > 0 && b[j - 1] == b',' {
                    s = j - 1;
                }
            } else if idx + 1 < members.len() {
                // 首成员被删且还有后继：吞掉尾随逗号（含值与逗号之间的空白）。
                let mut j = e;
                while j < b.len() && is_ws(b[j]) {
                    j += 1;
                }
                if j < b.len() && b[j] == b',' {
                    e = j + 1;
                }
            }
            dels.push((s, e));
        }

        if dels.is_empty() {
            return Ok(self.clone());
        }
        dels.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(dels.len());
        for (s, e) in dels {
            match merged.last_mut() {
                Some((_, le)) if s <= *le => {
                    // 重叠或相接（共享逗号）→ 取并集。
                    *le = (*le).max(e);
                }
                _ => merged.push((s, e)),
            }
        }

        let mut out = Vec::with_capacity(b.len());
        let mut pos = 0usize;
        for (s, e) in &merged {
            out.extend_from_slice(&b[pos..*s]);
            pos = *e;
        }
        out.extend_from_slice(&b[pos..]);
        Ok(RawBody(out))
    }
}

/// 顶层对象成员的字节 span（不含分隔逗号；`key_*` 含两侧引号）。
struct MemberSpan {
    key_start: usize,
    key_end: usize,
    val_end: usize,
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i
}

/// 校验 `b[at]` 起的转义序列（`\"` `\\` `\uXXXX` 等 RFC 8259 合法形态）。
/// `at` 指向反斜杠本身。
fn check_escape(b: &[u8], at: usize) -> Result<(), RawEditError> {
    let next = *b
        .get(at + 1)
        .ok_or(RawEditError::Malformed { offset: at })?;
    match next {
        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => Ok(()),
        b'u' => {
            for k in 2..6 {
                let h = *b
                    .get(at + k)
                    .ok_or(RawEditError::Malformed { offset: at })?;
                if !h.is_ascii_hexdigit() {
                    return Err(RawEditError::Malformed { offset: at });
                }
            }
            Ok(())
        }
        _ => Err(RawEditError::Malformed { offset: at }),
    }
}

/// 从开引号 `i` 扫到闭引号之后；返回闭引号后一位下标。
/// 严格校验转义；对原始控制字符宽松（不影响 span 判定）。多字节 UTF-8 无需特判：
/// >= 0x80 的字节与所有结构 ASCII 字节（`" \` `{}[],:`）不重合。
fn scan_json_string(b: &[u8], i: usize) -> Result<usize, RawEditError> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => {
                check_escape(b, j)?;
                j += 2;
            }
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    Err(RawEditError::Malformed { offset: i })
}

/// 把 JSON 字符串字面量（含引号的区间）解码成 `String` 用于键比对。
/// 键匹配必须走解码后语义相等：`"router_\u006deta"` 与 `"router_meta"` 是同一个键。
fn decode_json_string(b: &[u8], start: usize, end: usize) -> Result<String, RawEditError> {
    let mut out = String::with_capacity(end - start);
    let mut j = start + 1;
    while j < end - 1 {
        match b[j] {
            b'\\' => {
                check_escape(b, j)?;
                match b[j + 1] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let cp = u32::from_str_radix(
                            std::str::from_utf8(&b[j + 2..j + 6])
                                .map_err(|_| RawEditError::Malformed { offset: j })?,
                            16,
                        )
                        .map_err(|_| RawEditError::Malformed { offset: j })?;
                        let ch = char::from_u32(cp).ok_or(RawEditError::Malformed { offset: j })?;
                        out.push(ch);
                        j += 4;
                    }
                    _ => unreachable!("check_escape 已过滤"),
                }
                j += 2;
            }
            c => {
                // 多字节 UTF-8 原样入缓冲；非法 UTF-8 → Malformed。
                let step = utf8_step(c);
                let s = std::str::from_utf8(&b[j..j + step])
                    .map_err(|_| RawEditError::Malformed { offset: j })?;
                out.push_str(s);
                j += step;
            }
        }
    }
    Ok(out)
}

/// 由首字节确定 UTF-8 序列长度；ASCII（含结构字节）恒为 1。
#[inline]
fn utf8_step(c: u8) -> usize {
    match c {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        // 非法首字节（0x80..=0xBF / >= 0xF8）：按 1 处理，随后的 from_utf8 会报错。
        _ => 1,
    }
}

/// 扫描一个成员值，返回值结束（不含）的下标。对象/数组用括号栈保证配对；
/// 原始值（number/true/false/null）用 serde_json **只做校验**（不做任何产出）。
fn scan_value(b: &[u8], i: usize) -> Result<usize, RawEditError> {
    match b[i] {
        b'"' => scan_json_string(b, i),
        b'{' | b'[' => {
            let mut stack: Vec<u8> = vec![b[i]];
            let mut j = i + 1;
            while j < b.len() {
                let c = b[j];
                if *stack.last().expect("栈非空") == b'"' {
                    // 在字符串内：只认转义与闭引号。
                    match c {
                        b'\\' => {
                            check_escape(b, j)?;
                            j += 2;
                            continue;
                        }
                        b'"' => {
                            stack.pop();
                        }
                        _ => {}
                    }
                } else {
                    match c {
                        b'"' => stack.push(b'"'),
                        b'{' | b'[' => stack.push(c),
                        b'}' | b']' => {
                            let open = stack.pop().expect("栈非空");
                            let expect_close = if open == b'{' { b'}' } else { b']' };
                            if c != expect_close {
                                return Err(RawEditError::Malformed { offset: j });
                            }
                            if stack.is_empty() {
                                return Ok(j + 1);
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            Err(RawEditError::Malformed { offset: i })
        }
        _ => {
            // 原始值：扫到首个定界符（空白/逗号/闭括号/EOF）。
            let mut j = i;
            while j < b.len() && !is_ws(b[j]) && b[j] != b',' && b[j] != b'}' {
                j += 1;
            }
            if j == i {
                return Err(RawEditError::Malformed { offset: i }); // 空值，如 {"a":}
            }
            serde_json::from_slice::<serde_json::Value>(&b[i..j])
                .map_err(|_| RawEditError::Malformed { offset: i })?;
            Ok(j)
        }
    }
}

/// 单遍扫描顶层对象的全部成员 span；同时完成整份 body 的结构校验。
fn scan_top_level_members(b: &[u8]) -> Result<Vec<MemberSpan>, RawEditError> {
    let n = b.len();
    let mut i = skip_ws(b, 0);
    if i >= n {
        return Err(RawEditError::EmptyBody);
    }
    if b[i] != b'{' {
        return Err(RawEditError::NotTopLevelObject { first_byte: b[i] });
    }
    i = skip_ws(b, i + 1);

    let mut members = Vec::new();
    loop {
        if i < n && b[i] == b'}' {
            i += 1;
            break;
        }
        // 成员键必须是字符串。
        if i >= n || b[i] != b'"' {
            return Err(RawEditError::Malformed { offset: i.min(n) });
        }
        let key_start = i;
        i = scan_json_string(b, i)?;
        let key_end = i;
        i = skip_ws(b, i);
        if i >= n || b[i] != b':' {
            return Err(RawEditError::Malformed { offset: i.min(n) });
        }
        i = skip_ws(b, i + 1);
        if i >= n {
            return Err(RawEditError::Malformed { offset: n });
        }
        let val_end = scan_value(b, i)?;
        i = skip_ws(b, val_end);
        members.push(MemberSpan {
            key_start,
            key_end,
            val_end,
        });
        if i < n && b[i] == b',' {
            i = skip_ws(b, i + 1);
            // 尾随逗号（`{"a":1,}`）非法：逗号后必须紧跟下一个成员。
            if i < n && b[i] == b'}' {
                return Err(RawEditError::Malformed { offset: i });
            }
            continue;
        }
        if i < n && b[i] == b'}' {
            i += 1;
            break;
        }
        return Err(RawEditError::Malformed { offset: i.min(n) });
    }
    // 闭括号后只允许尾随空白。
    let tail = skip_ws(b, i);
    if tail != n {
        return Err(RawEditError::Malformed { offset: tail });
    }
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 快捷：删键并断言逐字节等于期望。
    fn assert_remove(input: &str, keys: &[&str], expected: &str) {
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw
            .remove_top_level_keys(keys)
            .unwrap_or_else(|e| panic!("unexpected err: {e:?}"));
        assert_eq!(
            String::from_utf8_lossy(out.as_bytes()),
            expected,
            "input: {input}"
        );
    }

    // 1. 字符串值内的 { } , : 不能误判为结构。
    #[test]
    fn string_value_with_structural_chars() {
        assert_remove(
            r#"{"model":"a{b}c:d,e","router_meta":{}}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"model":"a{b}c:d,e"}"#,
        );
    }

    // 2. 转义引号与反斜杠原样保留。
    #[test]
    fn escaped_quote_and_backslash_preserved() {
        assert_remove(
            r#"{"s":"a\"b\\c\"d","router_meta":1}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"s":"a\"b\\c\"d"}"#,
        );
    }

    // 3. \uXXXX 形式原样保留（不解码重编码）。
    #[test]
    fn unicode_escape_form_preserved() {
        assert_remove(
            r#"{"s":"\u0041\u4e2d\ud83d\ude00","router_meta":null}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"s":"\u0041\u4e2d\ud83d\ude00"}"#,
        );
    }

    // 4. 嵌套对象/数组括号平衡：只删顶层，同名嵌套键不动。
    #[test]
    fn nested_brackets_balance_and_only_top_level_removed() {
        assert_remove(
            "{\"router_meta\":{\"x\":[1,{\"y\":\"},]\"}],\"z\":[[]]},\"a\":1}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":1}"#,
        );
        // 嵌套出现的 router_meta 是客户端字段（CONF-11），不是顶层回显 → 不删。
        assert_remove(
            r#"{"a":{"router_meta":1},"b":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":{"router_meta":1},"b":2}"#,
        );
    }

    // 5. 全值形态逐字节保留：1e-9 / 前导负号 / 大整数 / bool / null / [] / {} / 空串。
    #[test]
    fn all_value_forms_preserved() {
        assert_remove(
            r#"{"n1":1e-9,"n2":-0.5,"n3":123456789012345678901234567890,"t":true,"f":false,"z":null,"arr":[],"obj":{},"es":"","router_meta":0}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"n1":1e-9,"n2":-0.5,"n3":123456789012345678901234567890,"t":true,"f":false,"z":null,"arr":[],"obj":{},"es":""}"#,
        );
    }

    // 6. 键不存在 → no-op 且输出与输入逐字节相同。
    #[test]
    fn missing_key_is_byte_identical_noop() {
        let input = r#"{"a":1,"b":[2,3]}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        assert_eq!(out.as_bytes(), input.as_bytes());
    }

    // 7. keys = [] → 恒等。
    #[test]
    fn empty_key_list_is_identity() {
        let input = r#"{"router_meta":1,"a":2}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw.remove_top_level_keys(&[]).unwrap();
        assert_eq!(out.as_bytes(), input.as_bytes());
    }

    // 8. 幂等：连续两次 == 一次（含逗号吞并边界）。
    #[test]
    fn idempotent_double_remove() {
        let input = "{\n  \"model\": \"m\",\n  \"router_meta\": {\"a\": [1, {\"b\": \"},\"}]},\n  \"x\": 1e-9\n}";
        let raw = RawBody::new(input.as_bytes().to_vec());
        let once = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let twice = once
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        assert_eq!(once.as_bytes(), twice.as_bytes());
    }

    // 9. 顶层非对象 → Err(NotTopLevelObject)（数组/字符串/数字/标量各钉一种）。
    #[test]
    fn top_level_non_object_rejected() {
        for input in ["[1,2]", "\"str\"", "42", "true", "null"] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert!(
                matches!(
                    raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                    Err(RawEditError::NotTopLevelObject { .. })
                ),
                "expected NotTopLevelObject for {input}"
            );
        }
    }

    // 10. 空 body / 仅空白 → Err(EmptyBody)。
    #[test]
    fn empty_or_whitespace_only_rejected() {
        for input in ["", "   ", " \n\r\t "] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert_eq!(
                raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                Err(RawEditError::EmptyBody)
            );
        }
    }

    // 11. 结构非法 → Err(Malformed)：空值/尾随逗号/尾随内容/未闭合/括号错配/非法转义。
    #[test]
    fn malformed_bodies_rejected() {
        for input in [
            r#"{"a":}"#,
            r#"{"a":1,}"#,
            r#"{"a":1} x"#,
            r#"{"a":"b"#,
            r#"{"a":{"b":1}"#,
            r#"{"a":[1}"#,
            r#"{"a":1"#,
            r#"{"a":"b\q"}"#,
            r#"{"a":"\u00zz"}"#,
        ] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert!(
                matches!(
                    raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                    Err(RawEditError::Malformed { .. })
                ),
                "expected Malformed for {input}"
            );
        }
    }

    // 12. 键序与嵌套字段顺序完全不变（多字段 + 删除中段成员）。
    #[test]
    fn key_and_nested_order_unchanged() {
        assert_remove(
            r#"{"z":1,"a":{"deep":[3,2,{"k":"v"}]},"router_meta":0,"m":"x","b":true}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"z":1,"a":{"deep":[3,2,{"k":"v"}]},"m":"x","b":true}"#,
        );
    }

    // 13. 尾随换行 / CRLF / 多字节 UTF-8 值原样保留。
    #[test]
    fn trailing_newline_crlf_and_utf8_preserved() {
        assert_remove(
            "{\n  \"a\": \"中文🚀\",\n  \"router_meta\": 1\n}\r\n",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{\n  \"a\": \"中文🚀\"\n}\r\n",
        );
    }

    // 14. 成员间空白与首成员删除的逗号吞并：输出仍是合法 JSON 且其余字节不变。
    #[test]
    fn whitespace_around_members_and_first_member_removal() {
        assert_remove(
            "{ \"router_meta\":1 , \"a\":2 , \"b\":3 }",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{  \"a\":2 , \"b\":3 }", // 删的是 `, "router_meta":1`：键前空白留下，前导空格保留
        );
        assert_remove(
            "{\"a\":1 ,\n\t\"router_meta\":2 , \"b\":3}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{\"a\":1  , \"b\":3}",
        );
    }

    // 15. 全部成员被删：空对象 / 仅剩空白的花括号。
    #[test]
    fn removing_all_members_yields_empty_object() {
        assert_remove(r#"{"router_meta":1}"#, ROUTER_OWNED_TOP_LEVEL_KEYS, "{}");
        assert_remove(
            r#"{"router_meta":1, "router_meta":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{}",
        );
    }

    // 16. 转义形式的键语义匹配：`"router_\u006deta"` 与 `"router_meta"` 同键。
    #[test]
    fn escaped_key_form_still_matches() {
        assert_remove(
            r#"{"a":1,"router_\u006deta":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":1}"#,
        );
    }

    // 17. `{` 前导空白保留（字节保真包含首字节之前的部分）。
    #[test]
    fn leading_whitespace_preserved() {
        assert_remove(
            "  {\"a\":1,\"router_meta\":2}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "  {\"a\":1}",
        );
    }

    // 18. 删除输出本身是合法 JSON（对删改后的字节做一次解析断言）。
    #[test]
    fn output_is_valid_json() {
        let raw = RawBody::new(
            "{\"router_meta\":{\"x\":[1,{\"y\":\"},]\"}]},\"model\":\"m\",\"n\":1e-09}"
                .as_bytes()
                .to_vec(),
        );
        let out = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(out.as_bytes()).unwrap();
        assert_eq!(v["model"], "m");
        assert!(v.get("router_meta").is_none());
    }
}
