//! 两份 README 的口径对账（第 239 轮：文档以中文为准；第 240 轮：中文那份就是首页）。
//!
//! 钉的是"两份各说各话"这一族——本仓真被它烧过：英文版早已写着
//! "undo / redo restore the full multi-cursor state"，而代码当时做不到
//! （P326 就是被这句话逮出来的）。`README.md`（中文）是权威文档、逐轮跟着代码补，
//! `README.en.md` 是发版点同步的摘要；两边的**结构**必须齐步，否则英文会悄悄
//! 多出中文没有的承诺（或反过来英文落后到没人发现）。

/// 读仓库根下的那份文件（`CARGO_MANIFEST_DIR` 是 `crates/app`）。
fn readme(name: &str) -> String {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(name);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("读 {} 失败：{e}（用例跑在仓库根之外？）", full.display()))
}

fn count_prefixes(text: &str, prefix: &str) -> usize {
    text.lines().filter(|l| l.starts_with(prefix)).count()
}

/// 取文件里**第一个**引用块（`>` 开头的连续几行）拼成一段——口径公告就钉在
/// 首屏那块，不是"全文某处出现过这个词"。这一刀是探针逼出来的：整文件判断时，
/// 把公告里的 `authoritative` 改成 `primary` 用例照样绿，因为 300 行之后另有一句
/// `authoritative` 顶了上去（＝两条判据互相顶班的假绿）。
fn first_blockquote(text: &str) -> String {
    let mut out = String::new();
    let mut started = false;
    for line in text.lines() {
        match line.strip_prefix('>') {
            Some(rest) => {
                started = true;
                out.push_str(rest.trim());
                out.push(' ');
            }
            None if started => break,
            None => {}
        }
    }
    out
}

/// 两份文件都各自声明"谁是权威"——规矩要写在文档里，不能只活在台账里。
/// 首页换语言是**改文件名**，最容易留下的不是错话而是死链，所以顺手钉住
/// 两边都不再提那个已被改走的老文件名。
#[test]
fn both_readmes_declare_which_one_is_authoritative() {
    let zh = readme("README.md");
    let en = readme("README.en.md");
    let zh_note = first_blockquote(&zh);
    let en_note = first_blockquote(&en);
    assert!(
        zh_note.contains("权威文档") && zh_note.contains("以本文为准"),
        "中文 README 的首屏公告要声明自己是权威版本、随每次提交更新"
    );
    assert!(
        en_note.to_lowercase().contains("authoritative") && en_note.contains("](README.md)"),
        "英文 README 的首屏公告要把权威指回中文首页那份"
    );
    // `to_lowercase()` 只罩英文单词那半：文件名大小写敏感，整段 down 之后
    // `](README.md)` 恒不匹配（本轮就这样红在了一条错判据上，看着像探针咬中了）。
    assert!(
        !zh.contains("README.zh.md") && !en.contains("README.zh.md"),
        "两份 README 里还留着改走前的老文件名，那是死链"
    );
}

/// 结构齐步：小节数与功能条目数都得一致。英文单独加一节（＝中文漏更）
/// 或中文加了英文没跟，两边都会红在这一条上。
#[test]
fn the_two_readmes_stay_structurally_in_step() {
    let zh = readme("README.md");
    let en = readme("README.en.md");
    assert_eq!(
        count_prefixes(&en, "## "),
        count_prefixes(&zh, "## "),
        "两份 README 的小节数分叉了：英文 {e}／中文 {c}",
        e = count_prefixes(&en, "## "),
        c = count_prefixes(&zh, "## "),
    );
    assert_eq!(
        count_prefixes(&en, "- **"),
        count_prefixes(&zh, "- **"),
        "两份 README 的功能条目数分叉了：英文 {e}／中文 {c}",
        e = count_prefixes(&en, "- **"),
        c = count_prefixes(&zh, "- **"),
    );
    // 反空转自证：读到的必须是真文档。别拿字节长度当"谁更全"的尺子——
    // 同样的内容中文占的字节反而少（本机实测：英文 41 695 B／中文 36 069 B），
    // 所以这里只断"两边都不是一页空壳"。
    assert!(
        count_prefixes(&zh, "## ") > 10 && count_prefixes(&en, "## ") > 10,
        "小节数小得可疑：读到的是不是同一份文件？"
    );
    assert!(
        count_prefixes(&zh, "- **") > 10 && count_prefixes(&en, "- **") > 10,
        "功能条目数小得可疑：英文 {e}／中文 {c}",
        e = count_prefixes(&en, "- **"),
        c = count_prefixes(&zh, "- **"),
    );
}
