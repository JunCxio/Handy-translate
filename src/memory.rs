use crate::{
    model::{Catalog, Entry, atomic_write, digest},
    translate::validate_translation,
};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Default)]
pub struct Memory(BTreeMap<String, String>);

pub fn policy(catalog: &Catalog, instructions: &str) -> String {
    let terms: BTreeMap<_, _> = catalog
        .glossary
        .iter()
        .map(|term| (&term.source, &term.target))
        .collect();
    digest(serde_json::to_vec(&(1, terms, instructions.trim())).unwrap())
}

fn key(catalog: &Catalog, original: &str) -> String {
    digest(
        serde_json::to_vec(&(
            &catalog.engine,
            &catalog.source_language,
            &catalog.target_language,
            policy(catalog, ""),
            original,
        ))
        .unwrap(),
    )
}

impl Memory {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        ensure!(
            fs::metadata(path)?.len() <= 64 * 1024 * 1024,
            "翻译缓存超过 64 MB，请备份并清理 translation-memory.json，或关闭缓存复用"
        );
        Ok(Self(serde_json::from_slice(&fs::read(path)?).context(
            "翻译缓存无法读取，请保留 translation-memory.json 并检查，或关闭缓存复用",
        )?))
    }

    pub fn get(&self, catalog: &Catalog, entry: &Entry) -> Option<String> {
        // shortcut: 跨游戏仅按完全相同原文复用；遇到语境歧义时关闭复用或选中重译。
        self.0
            .get(&key(catalog, &entry.original))
            .filter(|text| validate_translation(&catalog.engine, &entry.original, text).is_ok())
            .cloned()
    }

    pub fn seed(&mut self, catalog: &Catalog) -> bool {
        let policy = policy(catalog, "");
        let mut changed = false;
        for entry in &catalog.entries {
            let eligible = entry.translation_policy.as_deref() == Some(policy.as_str())
                || (entry.translation_policy.is_none() && catalog.glossary.is_empty());
            if let Some(text) = entry.translation.as_ref().filter(|text| {
                eligible && validate_translation(&catalog.engine, &entry.original, text).is_ok()
            }) {
                let key = key(catalog, &entry.original);
                if self.0.get(&key) != Some(text) {
                    self.0.insert(key, text.clone());
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn remove(&mut self, catalog: &Catalog, original: &str) {
        self.0.remove(&key(catalog, original));
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(&self.0)?;
        ensure!(
            bytes.len() <= 64 * 1024 * 1024,
            "翻译缓存超过 64 MB，请备份并清理 translation-memory.json，或关闭缓存复用"
        );
        atomic_write(path, &bytes).context("项目译文已保存，但翻译缓存写入失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{extract, model::Term};

    #[test]
    fn cache_is_exact_persistent_and_separated_by_language_engine_and_policy() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("js")).unwrap();
        fs::create_dir_all(temp.path().join("data")).unwrap();
        fs::write(temp.path().join("js/rmmz_core.js"), "").unwrap();
        fs::write(temp.path().join("js/plugins.js"), "var $plugins = [];").unwrap();
        fs::write(
            temp.path().join("data/Items.json"),
            r#"[null,{"name":"Potion"}]"#,
        )
        .unwrap();
        let mut c = extract::scan(temp.path().to_str().unwrap(), "en", "zh-CN").unwrap();
        c.entries[0].translation = Some("药水".into());
        let mut memory = Memory::default();
        assert!(memory.seed(&c));
        let file = temp.path().join("memory.json");
        memory.save(&file).unwrap();
        let mut memory = Memory::load(&file).unwrap();
        c.game_id = "another game".into();
        assert_eq!(memory.get(&c, &c.entries[0]).unwrap(), "药水");
        c.engine = crate::model::Engine::RpgMakerMv;
        assert!(memory.get(&c, &c.entries[0]).is_none());
        c.engine = crate::model::Engine::RpgMakerMz;
        c.source_language = "ja".into();
        assert!(memory.get(&c, &c.entries[0]).is_none());
        c.source_language = "en".into();
        let mut different = c.entries[0].clone();
        different.original.push(' ');
        assert!(memory.get(&c, &different).is_none());
        c.target_language = "ko".into();
        assert!(memory.get(&c, &c.entries[0]).is_none());
        c.target_language = "zh-CN".into();
        c.glossary = vec![Term {
            source: "Potion".into(),
            target: "恢复药".into(),
        }];
        assert!(memory.get(&c, &c.entries[0]).is_none());
        assert!(!memory.seed(&c));
        c.glossary.clear();
        c.entries[0].translation_policy = Some(policy(&c, "更自然"));
        c.entries[0].translation = Some("特殊译法".into());
        assert!(!memory.seed(&c));
        memory.remove(&c, "Potion");
        assert!(memory.get(&c, &c.entries[0]).is_none());
        fs::write(&file, "invalid").unwrap();
        assert!(Memory::load(&file).is_err());
    }
}
