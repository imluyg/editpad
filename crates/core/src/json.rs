//! JSON 校验与格式化（P22 第二批，core 纯逻辑）。
//!
//! 手写单遍扫描器：一遍完成「严格语法校验 + 规范化重排」，不引入
//! serde_json（构建环境无法拉取新依赖），也不构建中间 AST——字符串/
//! 数字/字面量原样搬运，结构字符按缩进规范重排，内存峰值 ≈ 输出文本。
//!
//! 错误以「行 : 列」报告，均按 Unicode 字符计——与本编辑器光标列
//! 口径一致（CJK/emoji 各算 1 个标量），可直接用于状态栏提示与跳转。

/// JSON 语法错误：1 起的行号与字符列 + 可读描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "第 {} 行 {} 列: {}", self.line, self.col, self.message)
    }
}

type JsonResult<T> = Result<T, JsonError>;

/// 嵌套深度上限：`value → object/array → value` 是互递归，超深输入曾可
/// 击穿线程栈——栈溢出是进程 abort（非 unwind），上层 catch_unwind 兜底
/// 对它无效。512 层与主流编辑器口径一致，正常数据远达不到。
const MAX_NEST_DEPTH: usize = 512;

/// 校验整段 JSON 文本；合法返回 Ok(())。
pub fn validate_json(text: &str) -> Result<(), JsonError> {
    let mut s = Scanner::new(text);
    let mut sink = String::new();
    s.value(&mut sink, 0, false)?;
    s.skip_ws();
    if let Some(c) = s.peek() {
        return Err(s.error_here(format!("JSON 结束后有多余内容（{c:?}）")));
    }
    Ok(())
}

/// 校验并输出 2 空格缩进的规范化 JSON。
///
/// 值本身原样保留（字符串转义序列、数字书写不做改写），只统一结构
/// 与空白；空对象/空数组保持单行 `{}` / `[]`。
pub fn format_json(text: &str) -> JsonResult<String> {
    let mut out = String::with_capacity(text.len() * 2);
    let mut s = Scanner::new(text);
    s.skip_ws();
    s.value(&mut out, 0, true)?;
    s.skip_ws();
    if let Some(c) = s.peek() {
        return Err(s.error_here(format!("JSON 结束后有多余内容（{c:?}）")));
    }
    Ok(out)
}

// ---------- 扫描器 ----------

struct Scanner {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Scanner {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// 以「当前待读字符」的位置报错（即指向问题字符本身）。
    fn error_here(&self, message: impl Into<String>) -> JsonError {
        JsonError {
            line: self.line,
            col: self.col,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
            self.bump();
        }
    }

    fn expect(&mut self, expected: char) -> JsonResult<()> {
        match self.peek() {
            Some(c) if c == expected => {
                self.bump();
                Ok(())
            }
            other => Err(JsonError {
                line: self.line,
                col: self.col,
                message: format!("期望 {expected:?}，实际 {}", show(other)),
            }),
        }
    }

    fn newline_indent(&self, depth: usize, out: &mut String) {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }

    /// 解析一个 JSON 值：校验语法并把规范化形式写入 `out`。
    ///
    /// * `depth`：当前嵌套深度（缩进级数）；
    /// * `formatted`：是否处于格式化模式（纯校验传 false，不产出）。
    fn value(&mut self, out: &mut String, depth: usize, formatted: bool) -> JsonResult<()> {
        // 深度封顶（见 MAX_NEST_DEPTH）：递归进入下一层容器前拦截
        if depth >= MAX_NEST_DEPTH {
            return Err(self.error_here(format!("嵌套过深（超过 {MAX_NEST_DEPTH} 层）")));
        }
        self.skip_ws();
        match self.peek() {
            None => Err(self.error_here("意外结束：缺少 JSON 值")),
            Some('{') => self.object(out, depth, formatted),
            Some('[') => self.array(out, depth, formatted),
            Some('"') => self.string_verbatim(out),
            Some('t') => self.literal("true", out),
            Some('f') => self.literal("false", out),
            Some('n') => self.literal("null", out),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(out),
            Some(other) => Err(self.error_here(format!("意外的字符 {other:?}"))),
        }
    }

    fn object(&mut self, out: &mut String, depth: usize, formatted: bool) -> JsonResult<()> {
        self.expect('{')?;
        out.push('{');
        let inner = depth + 1;

        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            out.push('}');
            return Ok(());
        }
        // 非空对象：首个成员换行到内层缩进
        if formatted {
            self.newline_indent(inner, out);
        }
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err(match self.peek() {
                    Some(c) => self.error_here(format!("对象键必须是字符串，实际 {c:?}")),
                    None => self.error_here("意外结束：对象缺少结尾 '}'"),
                });
            }
            self.string_verbatim(out)?;
            self.skip_ws();
            self.expect(':')?;
            if formatted {
                out.push_str(": ");
            } else {
                out.push(':');
            }
            self.value(out, inner, formatted)?;

            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    if formatted {
                        out.push(',');
                        self.newline_indent(inner, out);
                    }
                }
                Some('}') => {
                    self.bump();
                    if formatted {
                        self.newline_indent(depth, out);
                    }
                    out.push('}');
                    return Ok(());
                }
                Some(c) => {
                    return Err(self.error_here(format!(
                        "对象成员之间期望 ',' 或 '}}'，实际 {c:?}"
                    )))
                }
                None => return Err(self.error_here("意外结束：对象缺少结尾 '}'")),
            }
        }
    }

    fn array(&mut self, out: &mut String, depth: usize, formatted: bool) -> JsonResult<()> {
        self.expect('[')?;
        out.push('[');
        let inner = depth + 1;

        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            out.push(']');
            return Ok(());
        }
        // 非空数组：首个元素换行到内层缩进
        if formatted {
            self.newline_indent(inner, out);
        }
        loop {
            self.value(out, inner, formatted)?;

            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    if formatted {
                        out.push(',');
                        self.newline_indent(inner, out);
                    }
                }
                Some(']') => {
                    self.bump();
                    if formatted {
                        self.newline_indent(depth, out);
                    }
                    out.push(']');
                    return Ok(());
                }
                Some(c) => {
                    return Err(
                        self.error_here(format!("数组元素之间期望 ',' 或 ']'，实际 {c:?}"))
                    )
                }
                None => return Err(self.error_here("意外结束：数组缺少结尾 ']'")),
            }
        }
    }

    /// 字符串：转义序列严格校验（含代理对配对），原文整体搬运。
    fn string_verbatim(&mut self, out: &mut String) -> JsonResult<()> {
        self.expect('"')?;
        out.push('"');
        loop {
            // 先看后吃：错误定位指向问题字符本身
            let start = (self.line, self.col);
            let c = match self.bump() {
                Some(c) => c,
                None => {
                    return Err(JsonError {
                        line: self.line,
                        col: self.col,
                        message: "意外结束：字符串缺少结尾 '\"'".to_owned(),
                    })
                }
            };
            match c {
                '"' => {
                    out.push('"');
                    return Ok(());
                }
                '\\' => {
                    out.push('\\');
                    let esc_start = (self.line, self.col);
                    let esc = match self.bump() {
                        Some(e) => e,
                        None => {
                            return Err(JsonError {
                                line: self.line,
                                col: self.col,
                                message: "意外结束：转义序列不完整".to_owned(),
                            })
                        }
                    };
                    match esc {
                        '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => out.push(esc),
                        'u' => {
                            out.push('u');
                            let hi = self.hex4(out)?;
                            if (0xD800..=0xDBFF).contains(&hi) {
                                // 高代理必须紧跟 \uDC00..\uDFFF 低代理
                                if self.peek() == Some('\\') && self.peek_at(1) == Some('u') {
                                    self.bump();
                                    self.bump();
                                    out.push_str("\\u");
                                    let lo = self.hex4(out)?;
                                    if !(0xDC00..=0xDFFF).contains(&lo) {
                                        return Err(JsonError {
                                            line: esc_start.0,
                                            col: esc_start.1,
                                            message: format!(
                                                "代理对不配对：\\u{hi:04X} 后应跟低代理"
                                            ),
                                        });
                                    }
                                } else {
                                    return Err(JsonError {
                                        line: esc_start.0,
                                        col: esc_start.1,
                                        message: "代理对不配对：高代理后应跟 \\u 低代理"
                                            .to_owned(),
                                    });
                                }
                            } else if (0xDC00..=0xDFFF).contains(&hi) {
                                return Err(JsonError {
                                    line: start.0,
                                    col: start.1,
                                    message: "孤立的低代理项".to_owned(),
                                });
                            }
                        }
                        other => {
                            return Err(JsonError {
                                line: esc_start.0,
                                col: esc_start.1,
                                message: format!("非法转义字符 \\{other}"),
                            })
                        }
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(JsonError {
                        line: start.0,
                        col: start.1,
                        message: format!("字符串内含未转义的控制字符 U+{:04X}", c as u32),
                    })
                }
                other => out.push(other),
            }
        }
    }

    /// 读 4 位十六进制（\uXXXX），字符回写进输出。
    fn hex4(&mut self, out: &mut String) -> JsonResult<u32> {
        let mut value = 0u32;
        for _ in 0..4 {
            let start = (self.line, self.col);
            let c = match self.bump() {
                Some(c) => c,
                None => {
                    return Err(JsonError {
                        line: self.line,
                        col: self.col,
                        message: "意外结束：\\u 转义不完整".to_owned(),
                    })
                }
            };
            match c.to_digit(16) {
                Some(digit) => {
                    value = value * 16 + digit;
                    out.push(c);
                }
                None => {
                    return Err(JsonError {
                        line: start.0,
                        col: start.1,
                        message: format!("\\u 转义中出现非十六进制字符 {c}"),
                    })
                }
            }
        }
        Ok(value)
    }

    /// 数字：RFC 8259 文法（禁前导零，小数/指数可选但一旦出现须完整）。
    fn number(&mut self, out: &mut String) -> JsonResult<()> {
        if self.peek() == Some('-') {
            self.bump();
            out.push('-');
        }
        match self.peek() {
            Some('0') => {
                self.bump();
                out.push('0');
                if matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    return Err(self.error_here("数字不得有前导零"));
                }
            }
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    if let Some(d) = self.bump() {
                        out.push(d);
                    }
                }
            }
            other => {
                return Err(
                    self.error_here(format!("数字缺少整数部分，实际 {}", show(other)))
                )
            }
        }
        if self.peek() == Some('.') {
            self.bump();
            out.push('.');
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error_here("小数点后缺少数字"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                if let Some(d) = self.bump() {
                    out.push(d);
                }
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.bump();
            out.push('e');
            if matches!(self.peek(), Some('+') | Some('-')) {
                if let Some(sign) = self.bump() {
                    out.push(sign);
                }
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error_here("指数符号后缺少数字"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                if let Some(d) = self.bump() {
                    out.push(d);
                }
            }
        }
        Ok(())
    }

    /// true/false/null 字面量。
    fn literal(&mut self, word: &str, out: &mut String) -> JsonResult<()> {
        for expected in word.chars() {
            match self.bump() {
                Some(c) if c == expected => out.push(c),
                Some(c) => {
                    let _ = (self.line, self.col);
                    return Err(self.error_here(format!(
                        "非法字面量：期望 {word:?}，遇到 {c:?}"
                    )));
                }
                None => return Err(self.error_here("意外结束：字面量不完整")),
            }
        }
        Ok(())
    }
}

fn show(c: Option<char>) -> String {
    match c {
        Some(c) => format!("{c:?}"),
        None => "输入结束".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_of(text: &str) -> JsonError {
        validate_json(text).expect_err("应校验失败")
    }

    #[test]
    fn deep_nesting_reports_error_instead_of_stack_overflow() {
        // 回归：value/object/array 互递归曾无深度上限，10 万层 `[` 直接
        // 击穿线程栈（进程 abort，catch_unwind 兜不住）。现应报错而非崩溃。
        let err = err_of(&"[".repeat(100_000));
        assert!(err.message.contains("嵌套过深"), "实际：{err}");

        // 上限之内（顶层值 depth 0，最深层容器 depth 511）照常合法
        let legal = format!("{}{}", "[".repeat(511), "]".repeat(511));
        assert!(validate_json(&legal).is_ok());
        let formatted = format_json(&legal).unwrap();
        assert!(validate_json(&formatted).is_ok(), "格式化输出须仍合法");

        // 格式化路径同样受限（同一 value 入口）
        let fmt_err = format_json(&"[".repeat(100_000)).unwrap_err();
        assert!(fmt_err.message.contains("嵌套过深"));
        // 对象与数组混合嵌套同受封顶
        let mixed = "{\"a\":".repeat(100_000);
        assert!(validate_json(&mixed).unwrap_err().message.contains("嵌套过深"));
    }

    #[test]
    fn valid_documents_pass_and_format_is_stable() {
        let samples = [
            r#"null"#,
            r#"true"#,
            r#"-0.5e+10"#,
            r#""字符串 \u4e2d \n 转义""#,
            r#""合法代理对 \ud83d\ude80""#,
            "[]",
            "{}",
            r#"{"a":[1,2,{"b":null},"x"],"c":{"d":true,"e":-1.5E-3}}"#,
        ];
        for sample in samples {
            validate_json(sample).unwrap_or_else(|e| panic!("{sample}: {e}"));
            let once = format_json(sample).unwrap();
            let twice = format_json(&once).unwrap();
            assert_eq!(once, twice, "格式化必须幂等：{sample}");
        }
    }

    #[test]
    fn format_produces_expected_layout() {
        let pretty =
            format_json(r#"{"k":[1,{"inner":false},[]],"s":"v"}"#).unwrap();
        let expected = "{\n  \"k\": [\n    1,\n    {\n      \"inner\": false\n    },\n    []\n  ],\n  \"s\": \"v\"\n}";
        assert_eq!(pretty, expected);

        // 空容器保持单行
        assert_eq!(format_json("{}").unwrap(), "{}");
        assert_eq!(format_json("[ ]").unwrap(), "[]");
    }

    #[test]
    fn invalid_inputs_report_line_col() {
        // 尾随逗号：'}' 在第 3 行第 1 列
        let e = err_of("{\n  \"a\": 1,\n}");
        assert_eq!((e.line, e.col), (3, 1));

        // 缺冒号：指向数字 '1' 所在列
        let e = err_of("{\"a\" 1}");
        assert_eq!(e.line, 1);
        assert_eq!(e.col, 6, "应指向缺失冒号处之后的数字");
        assert!(e.message.contains("':'"), "{}", e.message);

        // 数字前导零
        let e = err_of("[01]");
        assert_eq!(e.col, 3, "前导零后的第二个 0");
        assert!(e.message.contains("前导零"), "{}", e.message);

        // 非法转义：指向 q
        let e = err_of("\"a\\qb\"");
        assert_eq!(e.col, 4);
        assert!(e.message.contains("非法转义"), "{}", e.message);

        // 未闭合字符串
        let e = err_of("{\"k\": \"未闭合}");
        assert!(e.message.contains("缺少结尾"), "{}", e.message);
        assert_eq!(e.line, 1);

        // 孤立低代理
        let e = err_of("\"\\udc00\"");
        assert!(e.message.contains("低代理"), "{}", e.message);

        // 高代理未跟低代理
        let e = err_of("\"\\ud83d\"");
        assert!(e.message.contains("代理对"), "{}", e.message);

        // 顶层之后的多余内容
        let e = err_of("123 abc");
        assert!(e.message.contains("多余内容"), "{}", e.message);
    }

    #[test]
    fn surrogate_pairs_accepted_and_error_cols_are_char_based() {
        // 合法代理对（🚀 的 UTF-16 编码）
        validate_json("\"\\ud83d\\ude80\"").unwrap();

        // 错误列按字符计而非字节：'q' 在第 14 个字符处
        let e = err_of("{\"k\": \"abcde\\q\"}");
        assert_eq!(e.line, 1);
        assert_eq!(e.col, 14, "列按 Unicode 字符计，指向非法转义的 q");
    }
}
