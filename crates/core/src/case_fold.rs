//! 大小写折叠的**唯一一份判据**（第 306 轮 M10b＝定案 R-21 改判）。
//!
//! 历史：字面查找只折 ASCII（`eq_ignore_ascii_case`／`fold_byte`），而正则查找走
//! `(?i)`＝Unicode 折叠，于是同一句查询在两种模式下**命中集不同**（实测 16 对分歧，
//! 清单在 `docs/决策记录.md` R-21）。本轮把两侧统一成 **Unicode 简单大小写折叠**，
//! 并且刻意**不在本仓重写那张表**：
//!
//! 折叠类由 [`regex_syntax::hir::ClassUnicode::try_case_fold_simple`] 展开，而那正是
//! fancy-regex 编译 `(?i)` 时走到的同一步（其上游 `hir/translate.rs::case_fold_char`
//! 对每个字面字符做的就是一件事：建单字符类再 `try_case_fold_simple`）。
//! ⇒ 「字面命中的集合」与「正则命中的集合」**由构造相同**，不是靠两份数据互相校对。
//!
//! 数据版本随依赖走：regex-syntax 0.8.11 的表由
//! `ucd-generate case-folding-simple ucd-16.0.0 --chars --all-pairs` 生成（Unicode 16.0.0）。

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// 非 ASCII 字符的类代表缓存。逐字符扫描会问成千上万次，而折叠表是静态的 ⇒
    /// 每个字符只需展开一次。线程本地＝后台扫描线程（查找／FIF）之间不抢锁。
    static CLASS_KEYS: RefCell<HashMap<char, char>> = RefCell::new(HashMap::new());
}

/// `c` 所在简单折叠等价类的**最小码点**＝这个类的规范代表。
///
/// 判等靠代表相等，而不是靠展开两边再比集合：第 306 轮把依赖里那张表逐条核对过
/// （2938 条关系**全部对称**、每个类内部的最小码点**全部一致**），所以"同代表 ⟺ 同类"
/// 成立；这两条性质另有守卫用例钉住（`case_fold_class_is_symmetric_over_a_sweep`）。
pub fn class_key(c: char) -> char {
    if c.is_ascii() {
        // ASCII 档不查表：类的最小码点就是它的大写形式（'k' 的类含 U+212A KELVIN SIGN，
        // 最小仍是 U+004B）。这条捷径不是推演，由 `case_fold_eq_on_ascii_matches_unicode`
        // 对 128×128 全对穷举钉住——它同时也是**字节快路径仍然成立**的依据。
        return c.to_ascii_uppercase();
    }
    CLASS_KEYS.with(|cache| {
        if let Some(&key) = cache.borrow().get(&c) {
            return key;
        }
        let key = expand_class_min(c);
        cache.borrow_mut().insert(c, key);
        key
    })
}

/// 用正则引擎那份表展开单字符类，取类内最小码点。
fn expand_class_min(c: char) -> char {
    use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};
    let mut class = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
    // 表不可用（编译时未开 `unicode-case`）时退化为"这个字符不折叠"：宁可少命中，
    // 也不让查找面板整条崩——这个 crate 里没有能让它不可用的构建配置，留红是纸上的。
    if class.try_case_fold_simple().is_err() {
        return c;
    }
    // 类里的区间已按码点升序 canonical 过，故第一段起点就是全类最小。
    class.iter().next().map_or(c, |range| range.start())
}

/// 给定大小写口径下的**比较键**：敏感＝原字符本身，不敏感＝该字符的折叠类代表。
///
/// 判等与扫描共用这一个函数 ⇒ "敏感档不许折叠"这条不靠调用方记住。
/// （第 306 轮探针实测：字符扫描档一度直接拿 [`class_key`] 比，敏感档在
/// `"ab AB ab"` 上多命中一处 `AB`——当时靠"敏感档永远走字节档"侥幸不发作，
/// 但两条扫描路因此不等价，正是本仓要消灭的形状。）
pub fn compare_key(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        c
    } else {
        class_key(c)
    }
}

/// 两个字符在给定大小写口径下是否算相等——**全仓唯一**的折叠判等。
///
/// `case_sensitive=false` 走 Unicode 简单折叠（与正则 `(?i)` 同一份表、同一个算法）。
/// 字面查找、字面替换、跨行查找、app 层「选中文本是否等于查询」四侧都问这一句；
/// 字节快路径只在"整段是 ASCII"时使用，那时它与本函数逐对等价（见上）。
pub fn case_fold_eq(a: char, b: char, case_sensitive: bool) -> bool {
    compare_key(a, case_sensitive) == compare_key(b, case_sensitive)
}
