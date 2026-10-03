//! 两份 README 的口径对账（第 239 轮：文档以中文为准）。
//!
//! 钉的是"两份各说各话"这一族——本仓真被它烧过：README 英文版早已写着
//! "undo / redo restore the full multi-cursor state"，而代码当时做不到
//! （P326 就是被这句话逮出来的）。中文那份是权威文档、逐轮跟着代码补，
//! 英文那份是发版点同步的摘要；两边的**结构**必须齐步，否则英文会悄悄
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

/// 两份文件都各自声明"谁是权威"——规矩要写在文档里，不能只活在台账里。
#[test]
fn both_readmes_declare_which_one_is_authoritative() {
    let zh = readme("README.zh.md");
    let en = readme("README.md");
    assert!(
        zh.contains("权威文档") && zh.contains("以本文为准"),
        "中文 README 要声明自己是权威版本、随每次提交更新"
    );
    assert!(
        en.contains("README.zh.md") && en.to_lowercase().contains("authoritative"),
        "英文 README 要把权威指回中文那份"
    );
}

/// 结构齐步：小节数与功能条目数都得一致。英文单独加一节（＝中文漏更）
/// 或中文加了英文没跟，两边都会红在这一条上。
#[test]
fn the_two_readmes_stay_structurally_in_step() {
    let zh = readme("README.zh.md");
    let en = readme("README.md");
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
