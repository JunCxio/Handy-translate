use crate::model::{Catalog, Engine, atomic_write, digest, save_json};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

const BLOCK: &str = "\n/* GAME_TRANSLATOR_BEGIN */\n;(function () {\n    if (!$plugins.some(function (p) { return p.name === 'GameTranslator'; })) {\n        $plugins.push({ name: 'GameTranslator', status: true, description: 'Handy-translate本地翻译', parameters: {} });\n    }\n}());\n/* GAME_TRANSLATOR_END */\n";
const DIRECTORY: &str = ".game-translator";
const MANIFEST: &str = ".game-translator/install.json";

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    engine: Engine,
    files: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    compiled: Vec<String>,
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    ensure!(
        !rel.is_absolute() && rel.components().all(|c| matches!(c, Component::Normal(_))),
        "安装清单包含无效路径"
    );
    let path = root.join(rel);
    let mut cursor = root.to_path_buf();
    for component in rel.components() {
        cursor.push(component);
        if let Ok(meta) = fs::symlink_metadata(&cursor) {
            ensure!(
                !meta.file_type().is_symlink(),
                "安装目标不能是符号链接：{}",
                cursor.display()
            );
        }
    }
    Ok(path)
}

fn read_manifest(root: &Path) -> Result<Manifest> {
    let m: Manifest = serde_json::from_slice(&fs::read(safe_path(root, MANIFEST)?)?)
        .context("安装清单损坏，请检查 .game-translator 备份目录")?;
    ensure!(m.schema == 1, "不支持该安装清单版本");
    let allowed = [
        "js/plugins/GameTranslator.js",
        "data/GameTranslatorTranslations.json",
        "zzz_game_translator.rpy",
        "GameTranslatorTranslations.json",
    ];
    ensure!(
        m.files.keys().all(|p| allowed.contains(&p.as_str())),
        "安装清单包含非适配器文件"
    );
    ensure!(
        m.compiled.iter().all(|p| p == "zzz_game_translator.rpyc"),
        "安装清单包含未知缓存文件"
    );
    Ok(m)
}

fn check_files(root: &Path, manifest: &Manifest) -> Result<()> {
    for (relative, hashes) in &manifest.files {
        let path = safe_path(root, relative)?;
        if path.exists() {
            ensure!(
                hashes.contains(&digest(fs::read(&path)?)),
                "文件已被其他程序修改，为保护改动已停止操作：{}",
                path.display()
            );
        }
    }
    Ok(())
}

fn registration_range(text: &str) -> Result<Option<std::ops::Range<usize>>> {
    let begin = "/* GAME_TRANSLATOR_BEGIN */";
    let end = "/* GAME_TRANSLATOR_END */";
    let begins = text.matches(begin).count();
    let ends = text.matches(end).count();
    if begins == 0 && ends == 0 {
        return Ok(None);
    }
    ensure!(
        begins == 1 && ends == 1,
        "插件注册段缺失或重复，已停止操作以保留改动"
    );
    let start = text.find(begin).unwrap();
    let finish = text.find(end).unwrap() + end.len();
    ensure!(finish > start, "插件注册段顺序无效");
    let code = text[start..finish].replace("\r\n", "\n");
    // 改名前安装的注册段仍需安全识别，以便更新或卸载。
    let previous = BLOCK.replace("Handy-translate", "游译");
    ensure!(
        code == BLOCK.trim_matches('\n') || code == previous.trim_matches('\n'),
        "插件注册代码已被修改，已停止操作以保留改动"
    );
    let prefix = &text[..start];
    let start = start
        - if prefix.ends_with("\r\n") {
            2
        } else if prefix.ends_with('\n') {
            1
        } else {
            0
        };
    let suffix = &text[finish..];
    let finish = finish
        + if suffix.starts_with("\r\n") {
            2
        } else if suffix.starts_with('\n') {
            1
        } else {
            0
        };
    Ok(Some(start..finish))
}

pub fn installed(catalog: &Catalog) -> bool {
    Path::new(&catalog.root).join(MANIFEST).is_file()
}

#[derive(Default, Serialize)]
pub struct InstallationStatus {
    pub translated: usize,
    pub language: String,
    pub current: bool,
    pub error: String,
}

#[derive(Deserialize)]
struct InstalledTranslations {
    schema: u32,
    language: String,
    translations: BTreeMap<String, String>,
}

fn translation_data(catalog: &Catalog) -> &'static str {
    if catalog.engine == Engine::Renpy {
        "GameTranslatorTranslations.json"
    } else {
        "data/GameTranslatorTranslations.json"
    }
}

fn read_translations(catalog: &Catalog) -> Result<InstalledTranslations> {
    let path = safe_path(Path::new(&catalog.root), translation_data(catalog))?;
    ensure!(
        fs::metadata(&path)?.len() <= 32 * 1024 * 1024,
        "游戏内译文文件过大"
    );
    let data: InstalledTranslations = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(data.schema == 1, "不支持该游戏内译文版本");
    Ok(data)
}

pub fn restore_translations(catalog: &mut Catalog) -> Result<usize> {
    let root = Path::new(&catalog.root);
    let manifest = read_manifest(root)?;
    ensure!(
        manifest.engine == catalog.engine,
        "安装清单的引擎与当前游戏不一致"
    );
    ensure!(
        manifest.files.contains_key(translation_data(catalog)),
        "安装清单未包含译文文件"
    );
    check_files(root, &manifest)?;
    let data = read_translations(catalog)?;
    ensure!(
        data.language == catalog.target_language,
        "游戏内译文的目标语言与当前项目不一致"
    );
    let mut entries = catalog.entries.clone();
    let mut restored = 0;
    for entry in &mut entries {
        if entry.translation.is_none() {
            if let Some(text) = data.translations.get(&entry.original) {
                crate::translate::validate_translation(&catalog.engine, &entry.original, text)?;
                entry.translation = Some(text.clone());
                entry.translation_policy = Some("installed".into());
                entry.error = None;
                restored += 1;
            }
        }
    }
    catalog.entries = entries;
    Ok(restored)
}

pub fn installation_status(catalog: &Catalog) -> InstallationStatus {
    if !installed(catalog) {
        return InstallationStatus::default();
    }
    let read = || -> Result<InstallationStatus> {
        let root = Path::new(&catalog.root);
        let (plugin, expected) = if catalog.engine == Engine::Renpy {
            (
                "zzz_game_translator.rpy",
                include_bytes!("../adapters/zzz_game_translator.rpy").as_slice(),
            )
        } else {
            (
                "js/plugins/GameTranslator.js",
                include_bytes!("../adapters/GameTranslator.js").as_slice(),
            )
        };
        let value = read_translations(catalog)?;
        Ok(InstallationStatus {
            translated: value.translations.len(),
            current: value.language == catalog.target_language
                && value.translations == catalog.translations()
                && fs::read(safe_path(root, plugin)?)? == expected,
            language: value.language,
            error: String::new(),
        })
    };
    read().unwrap_or_else(|_| InstallationStatus {
        error: "无法核对游戏内译文，请检查游戏目录后重新安装".into(),
        ..Default::default()
    })
}

pub fn install(catalog: &Catalog) -> Result<()> {
    ensure!(
        !catalog.translations().is_empty(),
        "尚无译文，请先完成翻译或导入译文"
    );
    for entry in &catalog.entries {
        if let Some(text) = &entry.translation {
            crate::translate::validate_translation(&catalog.engine, &entry.original, text)?;
        }
    }
    let root = Path::new(&catalog.root);
    ensure!(root.is_dir(), "游戏目录已不存在");
    let renpy = catalog.engine == Engine::Renpy;
    let plugin = if renpy {
        "zzz_game_translator.rpy"
    } else {
        "js/plugins/GameTranslator.js"
    };
    let data = if renpy {
        "GameTranslatorTranslations.json"
    } else {
        "data/GameTranslatorTranslations.json"
    };
    let plugin_bytes = if renpy {
        include_bytes!("../adapters/zzz_game_translator.rpy").as_slice()
    } else {
        include_bytes!("../adapters/GameTranslator.js").as_slice()
    };
    let data_bytes = serde_json::to_vec_pretty(
        &json!({"schema":1,"language":catalog.target_language,"translations":catalog.translations()}),
    )?;
    let manifest_path = safe_path(root, MANIFEST)?;
    let existing = manifest_path.is_file();
    let mut manifest = if existing {
        let m = read_manifest(root)?;
        ensure!(m.engine == catalog.engine, "安装记录的引擎与当前游戏不一致");
        check_files(root, &m)?;
        if !renpy {
            ensure!(
                registration_range(&fs::read_to_string(safe_path(root, "js/plugins.js")?)?)?
                    .is_some(),
                "插件注册代码已被修改，请先恢复后再更新译文"
            );
        }
        m
    } else {
        for name in [plugin, data] {
            ensure!(
                !safe_path(root, name)?.exists(),
                "同名文件已存在，拒绝覆盖：{}",
                name
            );
        }
        if renpy {
            ensure!(
                !safe_path(root, "zzz_game_translator.rpyc")?.exists(),
                "同名 Ren’Py 编译缓存已存在，拒绝覆盖"
            );
        }
        ensure!(
            !safe_path(root, DIRECTORY)?.exists(),
            "发现遗留的 .game-translator 目录，请先检查并移走备份目录再安装"
        );
        Manifest {
            schema: 1,
            engine: catalog.engine.clone(),
            files: BTreeMap::new(),
            compiled: if renpy {
                vec!["zzz_game_translator.rpyc".into()]
            } else {
                vec![]
            },
        }
    };
    let plugin_path = safe_path(root, plugin)?;
    let data_path = safe_path(root, data)?;
    let mut plugin_registration = None;
    if !renpy && !existing {
        let path = safe_path(root, "js/plugins.js")?;
        let original = fs::read(&path)?;
        let text =
            std::str::from_utf8(&original).context("js/plugins.js 不是 UTF-8，无法安全注册插件")?;
        ensure!(
            !text.contains("GAME_TRANSLATOR_BEGIN") && !text.contains("GameTranslator"),
            "插件列表存在同名注册项，请先检查 js/plugins.js"
        );
        fs::create_dir_all(safe_path(root, DIRECTORY)?)?;
        atomic_write(
            &safe_path(root, ".game-translator/plugins.js.original")?,
            &original,
        )?;
        let mut updated = original;
        updated.extend_from_slice(BLOCK.as_bytes());
        plugin_registration = Some((path, updated));
    }
    // 清单保留当前和计划写入的摘要，使中断安装也能安全卸载。
    for (name, bytes) in [(plugin, plugin_bytes), (data, data_bytes.as_slice())] {
        let mut hashes = vec![digest(bytes)];
        let path = safe_path(root, name)?;
        if existing && path.is_file() {
            let old = digest(fs::read(path)?);
            if !hashes.contains(&old) {
                hashes.push(old);
            }
        }
        manifest.files.insert(name.into(), hashes);
    }
    save_json(&manifest_path, &manifest)?;
    let result = (|| -> Result<()> {
        atomic_write(&plugin_path, plugin_bytes)?;
        atomic_write(&data_path, &data_bytes)?;
        if let Some((path, bytes)) = plugin_registration {
            atomic_write(&path, &bytes)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !existing {
            if let Err(rollback) = uninstall(catalog) {
                bail!(
                    "安装失败：{error}；自动撤销未完成：{rollback}，请检查 .game-translator 备份"
                );
            }
        }
        return Err(error);
    }
    Ok(())
}

pub fn uninstall(catalog: &Catalog) -> Result<()> {
    let root = Path::new(&catalog.root);
    let manifest = read_manifest(root).context("当前游戏没有可卸载的安装记录")?;
    check_files(root, &manifest)?;
    let registration = if manifest.engine != Engine::Renpy {
        let path = safe_path(root, "js/plugins.js")?;
        let mut text = fs::read_to_string(&path)?;
        if let Some(range) = registration_range(&text)? {
            text.replace_range(range, "");
        }
        Some((path, text))
    } else {
        None
    };
    if let Some((path, text)) = registration {
        atomic_write(&path, text.as_bytes())?;
    }
    for relative in manifest.files.keys().chain(manifest.compiled.iter()) {
        let path = safe_path(root, relative)?;
        if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("无法删除 {}，请先关闭游戏", path.display()))?;
        }
    }
    fs::remove_file(safe_path(root, MANIFEST)?)?;
    let backup = safe_path(root, ".game-translator/plugins.js.original")?;
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    let _ = fs::remove_dir(safe_path(root, DIRECTORY)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use tempfile::tempdir;
    #[test]
    fn registration_tolerates_line_endings_but_rejects_code_changes() {
        let mut text = format!("var $plugins = [];\n{BLOCK}// keep\n").replace('\n', "\r\n");
        let range = registration_range(&text).unwrap().unwrap();
        text.replace_range(range, "");
        assert_eq!(text, "var $plugins = [];\r\n// keep\r\n");
        assert!(registration_range(&BLOCK.replace("status: true", "status: false")).is_err());
        assert!(
            registration_range(&BLOCK.replace("Handy-translate", "游译"))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn installation_updates_and_uninstalls_without_losing_other_plugin_edits() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("js/plugins")).unwrap();
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("js/rmmz_core.js"), "").unwrap();
        fs::write(root.join("js/plugins.js"), "var $plugins = [];\n").unwrap();
        fs::write(root.join("data/Items.json"), r#"[null,{"name":"Potion"}]"#).unwrap();
        let mut c = extract::scan(root.to_str().unwrap(), "en", "zh-CN").unwrap();
        c.entries[0].translation = Some("药水".into());
        install(&c).unwrap();
        assert!(installed(&c));
        assert_eq!(installation_status(&c).translated, 1);
        assert!(installation_status(&c).current);
        let p = root.join("js/plugins.js");
        let mut text = fs::read_to_string(&p).unwrap();
        text.push_str("\n// user edit\n");
        fs::write(&p, text).unwrap();
        c.entries[0].translation = Some("恢复药水".into());
        assert!(!installation_status(&c).current);
        install(&c).unwrap();
        assert!(installation_status(&c).current);
        let data_path = root.join("data/GameTranslatorTranslations.json");
        let data_bytes = fs::read(&data_path).unwrap();
        fs::write(&data_path, "invalid").unwrap();
        assert!(!installation_status(&c).error.is_empty());
        fs::write(&data_path, data_bytes).unwrap();
        uninstall(&c).unwrap();
        assert_eq!(installation_status(&c).translated, 0);
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "var $plugins = [];\n\n// user edit\n"
        );
        assert!(!root.join("js/plugins/GameTranslator.js").exists());
    }
    #[test]
    fn refuses_to_overwrite_unmanaged_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("game")).unwrap();
        fs::create_dir(root.join("renpy")).unwrap();
        fs::write(root.join("game/script.rpy"), "\"Hello\"\n").unwrap();
        let mut c = extract::scan(root.to_str().unwrap(), "en", "zh-CN").unwrap();
        c.entries[0].translation = Some("你好".into());
        fs::write(
            root.join("game/zzz_game_translator.rpy"),
            "existing content",
        )
        .unwrap();
        assert!(install(&c).is_err());
        assert_eq!(
            fs::read_to_string(root.join("game/zzz_game_translator.rpy")).unwrap(),
            "existing content"
        );
        assert!(safe_path(root, "../outside").is_err());
    }
}
