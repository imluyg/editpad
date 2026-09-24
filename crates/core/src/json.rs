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
///
/// **O-14：纯校验零产出**——`formatted=false` 下扫描器一次都不调用输出汇
/// （见 `run` 与 `Scanner::out`），也不再物化 `Vec<char>` 全文拷贝。
/// 旧实现虽名为「只校验」，实际把整份规范化结果照旧推进一个临时 `String`，
/// 加上 `text.chars().collect::<Vec<char>>()`（4 字节/字符），64MB JSON
/// 校验一次的峰值约 320MB；而校验在格式化前与状态栏提示里被反复触发。
pub fn validate_json(text: &str) -> Result<(), JsonError> {
    run(text, false, |_| {})
}

/// 校验并输出 2 空格缩进的规范化 JSON。
///
/// 值本身原样保留（字符串转义序列、数字书写不做改写），只统一结构
/// 与空白；空对象/空数组保持单行 `{}` / `[]`。
pub fn format_json(text: &str) -> JsonResult<String> {
    let mut out = String::with_capacity(text.len() * 2);
    run(text, true, |s| out.push_str(s))?;
    Ok(out)
}

/// 两种模式共用的扫描收口：解析一个顶层值 + 尾部多余内容检查。
///
/// `formatted` 决定「是否产出」，`emit` 是输出汇（格式化模式由
/// [`format_json`] 接进 `String`；校验模式传空动作）。错误行列由同一条
/// 游标推进，两种模式逐字段一致（用例
/// `error_positions_are_char_based_and_identical_across_both_modes` 钉住）。
fn run<W: FnMut(&str)>(text: &str, formatted: bool, emit: W) -> Result<(), JsonError> {
    let mut s = Scanner::new(text, formatted, emit);
    s.value(0)?;
    s.skip_ws();
    if let Some(c) = s.peek() {
        return Err(s.error_here(format!("JSON 结束后有多余内容（{c:?}）")));
    }
    Ok(())
}

// ---------- 扫描器 ----------

/// 手写递归下降扫描器。
///
/// **游标形态（O-14）**：`text` + 字节偏移 `pos`（恒落在字符边界），
/// 不再是 `Vec<char>` 全文拷贝；行/列仍按 **Unicode 字符**推进（`bump`
/// 每吃一个字符 col +1，`\n` 换行），错误定位与改前逐字段一致。
/// 输出只能经 [`Scanner::out`] / [`Scanner::out_char`]，它们在
/// `formatted=false` 时直接返回——校验模式因此零写入、零分配。
struct Scanner<'a, W: FnMut(&str)> {
    text: &'a str,
    pos: usize,
    line: usize,
    col: usize,
    formatted: bool,
    emit: W,
}

impl<'a, W: FnMut(&str)> Scanner<'a, W> {
    fn new(text: &'a str, formatted: bool, emit: W) -> Self {
        Self { text, pos: 0, line: 1, col: 1, formatted, emit }
    }

    /// 以「当前待读字符」的位置报错（即指向问题字符本身）。
    fn error_here(&self, message: impl Into<String>) -> JsonError {
        JsonError {
            line: self.line,
            col: self.col,
            message: message.into(),
        }
    }

    /// 输出汇：只有格式化模式写入（校验模式连一次调用都不发生）。
    fn out(&mut self, s: &str) {
        if self.formatted {
            (self.emit)(s);
        }
    }

    /// 单字符输出（字符边界由 `pos` 保证，取原始片段零拷贝）。
    fn out_char(&mut self, c: char) {
        if self.formatted {
            let len = c.len_utf8();
            (self.emit)(&self.text[self.pos - len..self.pos]);
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.text[self.pos..].chars().nth(ahead)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
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

    fn newline_indent(&mut self, depth: usize) {
        self.out("\n");
        for _ in 0..depth {
            self.out("  ");
        }
    }

    /// 解析一个 JSON 值：校验语法并把规范化形式写入输出汇。
    ///
    /// 是否产出由 `self.formatted` 决定（纯校验传 false，不产出）。
    fn value(&mut self, depth: usize) -> JsonResult<()> {
        // 深度封顶（见 MAX_NEST_DEPTH）：递归进入下一层容器前拦截
        if depth >= MAX_NEST_DEPTH {
            return Err(self.error_here(format!("嵌套过深（超过 {MAX_NEST_DEPTH} 层）")));
        }
        self.skip_ws();
        match self.peek() {
            None => Err(self.error_here("意外结束：缺少 JSON 值")),
            Some('{') => self.object(depth),
            Some('[') => self.array(depth),
            Some('"') => self.string_verbatim(),
            Some('t') => self.literal("true"),
            Some('f') => self.literal("false"),
            Some('n') => self.literal("null"),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(other) => Err(self.error_here(format!("意外的字符 {other:?}"))),
        }
    }

    fn object(&mut self, depth: usize) -> JsonResult<()> {
        self.expect('{')?;
        self.out("{");
        let inner = depth + 1;

        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            self.out("}");
            return Ok(());
        }
        // 非空对象：首个成员换行到内层缩进
        self.newline_indent(inner);
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err(match self.peek() {
                    Some(c) => self.error_here(format!("对象键必须是字符串，实际 {c:?}")),
                    None => self.error_here("意外结束：对象缺少结尾 '}'"),
                });
            }
            self.string_verbatim()?;
            self.skip_ws();
            self.expect(':')?;
            self.out(": ");
            self.value(inner)?;

            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.out(",");
                    self.newline_indent(inner);
                }
                Some('}') => {
                    self.bump();
                    self.newline_indent(depth);
                    self.out("}");
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

    fn array(&mut self, depth: usize) -> JsonResult<()> {
        self.expect('[')?;
        self.out("[");
        let inner = depth + 1;

        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            self.out("]");
            return Ok(());
        }
        // 非空数组：首个元素换行到内层缩进
        self.newline_indent(inner);
        loop {
            self.value(inner)?;

            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.out(",");
                    self.newline_indent(inner);
                }
                Some(']') => {
                    self.bump();
                    self.newline_indent(depth);
                    self.out("]");
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
    fn string_verbatim(&mut self) -> JsonResult<()> {
        self.expect('"')?;
        self.out("\"");
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
                    self.out("\"");
                    return Ok(());
                }
                '\\' => {
                    self.out("\\");
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
                        '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => self.out_char(esc),
                        'u' => {
                            self.out("u");
                            let hi = self.hex4()?;
                            if (0xD800..=0xDBFF).contains(&hi) {
                                // 高代理必须紧跟 \uDC00..\uDFFF 低代理
                                if self.peek() == Some('\\') && self.peek_at(1) == Some('u') {
                                    self.bump();
                                    self.bump();
                                    self.out("\\u");
                                    let lo = self.hex4()?;
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
                other => self.out_char(other),
            }
        }
    }

    /// 读 4 位十六进制（\uXXXX），字符回写进输出。
    fn hex4(&mut self) -> JsonResult<u32> {
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
                    self.out_char(c);
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
    fn number(&mut self) -> JsonResult<()> {
        if self.peek() == Some('-') {
            self.bump();
            self.out("-");
        }
        match self.peek() {
            Some('0') => {
                self.bump();
                self.out("0");
                if matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    return Err(self.error_here("数字不得有前导零"));
                }
            }
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    if let Some(d) = self.bump() {
                        self.out_char(d);
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
            self.out(".");
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error_here("小数点后缺少数字"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                if let Some(d) = self.bump() {
                    self.out_char(d);
                }
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.bump();
            self.out("e");
            if matches!(self.peek(), Some('+') | Some('-')) {
                if let Some(sign) = self.bump() {
                    self.out_char(sign);
                }
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error_here("指数符号后缺少数字"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                if let Some(d) = self.bump() {
                    self.out_char(d);
                }
            }
        }
        Ok(())
    }

    /// true/false/null 字面量。
    fn literal(&mut self, word: &str) -> JsonResult<()> {
        for expected in word.chars() {
            match self.bump() {
                Some(c) if c == expected => self.out_char(c),
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

    /// O-14 的行列口径穷尽表：`formatted=false`（validate_json）与
    /// `formatted=true`（format_json）必须报出**完全相同**的 JsonError
    /// （行、列、消息三者），且列一律按 Unicode 字符计——CJK 每字 3 字节、
    /// emoji 每字 4 字节，都不许把列号顶到字节刻度上。
    /// 表内期望值取自**改前实现实测**（先对旧实现跑通本用例，再动扫描器）。
    #[test]
    fn error_positions_are_char_based_and_identical_across_both_modes() {
        let cases: &[(&str, &str, usize, usize)] = &[
            // 纯 ASCII：q 前 13 个字符
            (r#"{"k": "abcde\q"}"#, "非法转义", 1, 14),
            // 中文（3 字节/字符）在错误之前：字符刻度 12，字节刻度会是 20
            (r#"{"k": "中文中\q"}"#, "非法转义", 1, 12),
            // 代理对**转义**（源码全是 ASCII）：整对吃掉 12 个字符后
            // 才到 `\q`，列 21——钉住「转义序列按字面字符计数、不折叠」
            (r#"{"k": "\ud83d\ude80\q"}"#, "非法转义", 1, 21),
            // 正文里的真 emoji：一个字符只推一列
            ("{\"k\": \"🚀🚀\\q\"}", "非法转义", 1, 11),
            // 第二行、含 CJK 与缩进：列从行首重算
            ("{\n  \"中文键\": \"值\\q\"\n}", "非法转义", 2, 13),
            // 尾随逗号：'}' 落在第 3 行第 1 列（此处按「期望键」报错）
            ("{\n  \"a\": 1,\n}", "对象键必须是字符串", 3, 1),
            // 缺冒号：指向其后的数字
            ("{\"a\" 1}", "期望 ':'", 1, 6),
            // 数字前导零
            ("[01]", "前导零", 1, 3),
            // 中文字符串未闭合
            ("{\"k\": \"未闭合}", "缺少结尾", 1, 12),
            // 顶层之后的多余内容：CJK 字符串之后的 `1`，字符列 5（字节刻度会是 7）
            ("\"中\" 123", "多余内容", 1, 5),
            // 意外的字符：错误指向 CJK 字符本身（第 2 个字符）
            ("[中, 試]", "意外的字符", 1, 2),
        ];
        for (text, note, line, col) in cases {
            let e_v = validate_json(text).expect_err("用例前提：非法输入");
            let e_f = format_json(text).expect_err("两种模式同判非法");
            assert_eq!(
                e_v, e_f,
                "{note}: 校验与格式化两种模式报出的错误必须逐字段相等"
            );
            assert!(
                e_v.message.contains(note),
                "{note}: 消息不符，实际 {}",
                e_v.message
            );
            assert_eq!((e_v.line, e_v.col), (*line, *col), "{note} @ {text:?}");
        }

        // 嵌套过深：512 层开括号后、第 513 个 `[` 处报错（列 = 已消费字符数 + 1）
        let deep = "[".repeat(600);
        let e_v = validate_json(&deep).expect_err("用例前提：超深非法");
        let e_f = format_json(&deep).expect_err("两种模式同判非法");
        assert_eq!(e_v, e_f, "深度错误的两种模式逐字段相等");
        assert!(e_v.message.contains("嵌套过深"), "{}", e_v.message);
        assert_eq!((e_v.line, e_v.col), (1, 513), "深度错误定位在第 513 个 '['");
    }

    #[test]
    fn validate_mode_emits_nothing_and_format_mode_emits_the_document() {
        // O-14 的内存主张用**可观测行为**钉：校验模式一次输出汇都不调用
        // （改前即使 formatted=false，object()/string_verbatim() 也照旧往
        // 一个临时 String 里 push 整份规范化结果）。不用耗时/内存采样断言。
        let samples = [
            "null",
            "-1.5E-3",
            r#""字符串 \u4e2d \n 转义""#,
            r#"{"a":[1,2,{"b":null},"x"],"c":{"d":true}}"#,
            "{\n  \"k\": [],\n  \"空\": {}\n}",
            // 前导空白：改前 format_json 先 skip_ws 再 value，改后 value 自己
            // skip_ws（两条路径同一函数），产物必须一字不变
            "  \n\t{\"a\": 1}",
        ];
        for text in samples {
            // 校验模式：sink 零调用、零字节
            let mut v_calls = 0usize;
            let mut v_bytes = 0usize;
            {
                let mut sink = |s: &str| {
                    v_calls += 1;
                    v_bytes += s.len();
                };
                run(text, false, &mut sink).unwrap_or_else(|e| panic!("{text}: {e}"));
            }
            assert_eq!(
                (v_calls, v_bytes),
                (0, 0),
                "formatted=false 不得产生任何输出：{text:?}"
            );

            // 格式化模式：sink 收到的字节拼接 == format_json 的返回值
            // （证明「输出汇抽象」没有改写产物字节序列）
            let mut got = String::new();
            let mut calls = 0usize;
            {
                let mut sink = |s: &str| {
                    calls += 1;
                    got.push_str(s);
                };
                run(text, true, &mut sink).unwrap_or_else(|e| panic!("{text}: {e}"));
            }
            assert!(calls > 0, "formatted=true 必须有输出");
            assert_eq!(got, format_json(text).unwrap(), "sink 拼接 == format_json：{text:?}");
        }
        // 绝对值钉（防「两种模式一起漂」）：前导空白吃掉、缩进仍按 2 空格
        assert_eq!(format_json("  \n\t{\"a\": 1}").unwrap(), "{\n  \"a\": 1\n}");
    }
}
