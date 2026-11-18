use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

pub fn digest(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    RpgMakerMv,
    RpgMakerMz,
    Renpy,
}

impl Engine {
    pub fn label(&self) -> &'static str {
        match self {
            Self::RpgMakerMv => "RPG Maker MV",
            Self::RpgMakerMz => "RPG Maker MZ",
            Self::Renpy => "Ren’Py（可读脚本）",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub original: String,
    pub contexts: Vec<String>,
    pub translation: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_policy: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Term {
    pub source: String,
    pub target: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    pub game_id: String,
    pub title: String,
    pub root: String,
    #[serde(default)]
    pub launch_path: Option<String>,
    pub engine: Engine,
    pub source_language: String,
    pub target_language: String,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub glossary: Vec<Term>,
}

impl Catalog {
    pub fn file_name(&self) -> String {
        format!(
            "{}-{}.json",
            self.game_id,
            digest(format!("{}:{}", self.source_language, self.target_language))
        )
    }
    pub fn translations(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .filter_map(|e| {
                e.translation
                    .as_ref()
                    .map(|t| (e.original.clone(), t.clone()))
            })
            .collect()
    }
    pub fn bundle(&self) -> Bundle {
        Bundle {
            schema: 1,
            game_id: self.game_id.clone(),
            engine: self.engine.clone(),
            source_language: self.source_language.clone(),
            target_language: self.target_language.clone(),
            translations: self.translations(),
            glossary: Some(self.glossary.clone()),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: u32,
    pub game_id: String,
    pub engine: Engine,
    pub source_language: String,
    pub target_language: String,
    pub translations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glossary: Option<Vec<Term>>,
}

pub fn validate_languages(source: &str, target: &str) -> Result<()> {
    ensure!(
        !source.trim().is_empty() && !target.trim().is_empty(),
        "请填写原文语言和目标语言"
    );
    ensure!(source.len() <= 64 && target.len() <= 64, "语言名称过长");
    ensure!(source != target, "原文语言和目标语言不能相同");
    ensure!(target != "auto", "目标语言需要明确选择，不能使用自动识别");
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("文件没有父目录")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".gt-{:016x}.tmp", rand::random::<u64>()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
            .with_context(|| format!("无法保存 {}，请检查权限或文件占用", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}
