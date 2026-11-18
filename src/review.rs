use crate::{
    model::{Catalog, Entry, Term},
    translate::validate_translation,
};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

pub fn validate_glossary(terms: &[Term]) -> Result<()> {
    ensure!(terms.len() <= 200, "术语表最多支持 200 项");
    let mut sources = BTreeSet::new();
    for term in terms {
        ensure!(
            !term.source.trim().is_empty() && !term.target.trim().is_empty(),
            "术语原文和译文不能为空"
        );
        ensure!(
            term.source == term.source.trim() && term.target == term.target.trim(),
            "术语两端不能包含空白"
        );
        ensure!(
            term.source.chars().count() <= 200 && term.target.chars().count() <= 200,
            "单项术语最多 200 字符"
        );
        ensure!(
            !term
                .source
                .chars()
                .chain(term.target.chars())
                .any(|c| c.is_control() || "\\[]{}%".contains(c)),
            "术语应为单行普通文本，不能包含控制符、变量括号或百分号"
        );
        ensure!(
            !term.source.contains("__GT_") && !term.target.contains("__GT_"),
            "术语不能包含保留标记 __GT_"
        );
        ensure!(
            sources.insert(&term.source),
            "术语原文重复：{}",
            term.source
        );
    }
    Ok(())
}

pub fn issues(catalog: &Catalog, entry: &Entry) -> Vec<String> {
    let Some(text) = &entry.translation else {
        return vec![];
    };
    let mut issues = Vec::new();
    if let Err(error) = validate_translation(&catalog.engine, &entry.original, text) {
        issues.push(error.to_string());
    }
    if text.trim() == entry.original.trim() && entry.original.chars().any(char::is_alphabetic) {
        issues.push("译文与原文相同，人名等可能无需翻译".into());
    }
    if catalog.source_language.starts_with("ja")
        && !catalog.target_language.starts_with("ja")
        && text.chars().any(|c| {
            ('\u{3041}'..='\u{3096}').contains(&c) || ('\u{30a1}'..='\u{30fa}').contains(&c)
        })
    {
        issues.push("疑似残留日文假名，请人工确认".into());
    }
    if catalog.source_language.starts_with("ko")
        && !catalog.target_language.starts_with("ko")
        && text.chars().any(|c| ('\u{ac00}'..='\u{d7a3}').contains(&c))
    {
        issues.push("疑似残留韩文，请人工确认".into());
    }
    // 与翻译时相同，较长术语先匹配，避免重叠术语产生误报。
    let mut terms: Vec<_> = catalog.glossary.iter().collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.source.len()));
    let mut remaining = entry.original.clone();
    for term in terms {
        if remaining.contains(&term.source) {
            if !text.contains(&term.target) {
                issues.push(format!(
                    "术语不一致：{} 应译为 {}",
                    term.source, term.target
                ));
            }
            remaining = remaining.replace(&term.source, "");
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glossary_rejects_duplicates_and_control_sequences() {
        let term = Term {
            source: "結衣".into(),
            target: "结衣".into(),
        };
        assert!(validate_glossary(std::slice::from_ref(&term)).is_ok());
        assert!(validate_glossary(&[term.clone(), term]).is_err());
        assert!(
            validate_glossary(&[Term {
                source: "name".into(),
                target: r"\V[1]".into()
            }])
            .is_err()
        );
    }

    #[test]
    fn review_flags_unchanged_residual_language_and_overlapping_terms() {
        let mut c: Catalog = serde_json::from_value(serde_json::json!({
            "schema":1,"game_id":"test","title":"test","root":"","engine":"rpg_maker_mz",
            "source_language":"ja","target_language":"zh-CN","warnings":[],"entries":[]
        }))
        .unwrap();
        let mut entry: Entry = serde_json::from_value(serde_json::json!({"id":"one","original":"結衣ちゃんです","translation":"結衣ちゃんです","contexts":[],"error":null})).unwrap();
        assert!(issues(&c, &entry).iter().any(|s| s.contains("原文相同")));
        assert!(issues(&c, &entry).iter().any(|s| s.contains("日文假名")));
        c.glossary = vec![
            Term {
                source: "結衣".into(),
                target: "结衣".into(),
            },
            Term {
                source: "結衣ちゃん".into(),
                target: "小结衣".into(),
            },
        ];
        entry.translation = Some("是小结衣".into());
        assert!(issues(&c, &entry).is_empty());
        entry.translation = Some("是朋友".into());
        assert_eq!(issues(&c, &entry).len(), 1);
        entry.translation = None;
        assert!(issues(&c, &entry).is_empty());
    }
}
