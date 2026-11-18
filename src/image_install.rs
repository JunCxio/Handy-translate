use crate::{
    assets,
    model::{Catalog, Engine, atomic_write, digest, save_json},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const MANIFEST: &str = ".handy-images/install.json";
const PLUGIN: &str = "js/plugins/HandyImageTranslator.js";
const DATA: &str = "data/HandyImageTranslations.json";
const BLOCK: &str = "\n/* HANDY_IMAGE_TRANSLATOR_BEGIN */\n;$plugins.push({name:'HandyImageTranslator',status:true,description:'Handy-translate图片素材',parameters:{}});\n/* HANDY_IMAGE_TRANSLATOR_END */\n";

#[derive(Default, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    files: BTreeMap<String, Vec<String>>,
}
#[derive(Default, Serialize)]
pub struct Status {
    pub installed: bool,
    pub record: bool,
    pub images: usize,
    pub error: String,
}
fn range(text: &str) -> Result<Option<std::ops::Range<usize>>> {
    let begin = "/* HANDY_IMAGE_TRANSLATOR_BEGIN */";
    let end = "/* HANDY_IMAGE_TRANSLATOR_END */";
    if !text.contains(begin) && !text.contains(end) {
        return Ok(None);
    }
    ensure!(
        text.matches(begin).count() == 1 && text.matches(end).count() == 1,
        "图片插件注册段缺失或重复"
    );
    let start = text.find(begin).unwrap();
    let finish = text.find(end).unwrap() + end.len();
    ensure!(
        start < finish && text[start..finish].replace("\r\n", "\n") == BLOCK.trim_matches('\n'),
        "图片插件注册段被修改，已保留改动"
    );
    let start = start
        - if text[..start].ends_with("\r\n") {
            2
        } else if text[..start].ends_with('\n') {
            1
        } else {
            0
        };
    let finish = finish
        + if text[finish..].starts_with("\r\n") {
            2
        } else if text[finish..].starts_with('\n') {
            1
        } else {
            0
        };
    Ok(Some(start..finish))
}
fn read(root: &Path) -> Result<Manifest> {
    ensure!(
        fs::metadata(assets::safe_path(root, MANIFEST)?)?.len() <= 4 * 1024 * 1024,
        "图片安装清单过大"
    );
    let m: Manifest = serde_json::from_slice(&fs::read(assets::safe_path(root, MANIFEST)?)?)?;
    ensure!(m.schema == 1 && m.files.len() <= 20002, "图片安装清单无效");
    for (p, hashes) in &m.files {
        let image = p
            .strip_prefix(".handy-images/files/")
            .and_then(|p| p.strip_suffix(".png"));
        ensure!(
            p == PLUGIN
                || p == DATA
                || image.is_some_and(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit())),
            "图片安装清单包含未知文件"
        );
        ensure!(
            !hashes.is_empty()
                && hashes
                    .iter()
                    .all(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit())),
            "图片安装摘要无效"
        );
    }
    Ok(m)
}
fn check(root: &Path, m: &Manifest) -> Result<()> {
    for (p, hashes) in &m.files {
        let file = assets::safe_path(root, p)?;
        if file.exists() {
            ensure!(
                hashes.contains(&digest(fs::read(&file)?)),
                "图片插件文件已被其他程序修改：{}",
                file.display()
            );
        }
    }
    Ok(())
}
pub fn loaded(root: &Path) -> Result<BTreeSet<String>> {
    let file = assets::safe_path(root, ".handy-images/loaded.json")?;
    if !file.exists() {
        return Ok(BTreeSet::new());
    }
    ensure!(
        fs::metadata(&file)?.len() <= 4 * 1024 * 1024,
        "素材加载记录过大"
    );
    let items: Vec<String> =
        serde_json::from_slice(&fs::read(file)?).context("素材加载记录损坏")?;
    ensure!(items.len() <= 20000, "素材加载记录条数过多");
    for p in &items {
        ensure!(
            p.starts_with("img/") && p.ends_with(".png"),
            "素材加载记录路径无效"
        );
        assets::safe_path(root, p)?;
    }
    Ok(items.into_iter().collect())
}
pub fn status(c: &Catalog) -> Status {
    let root = Path::new(&c.root);
    if !root.join(MANIFEST).exists() {
        return Status::default();
    }
    let result = (|| -> Result<Status> {
        let m = read(root)?;
        check(root, &m)?;
        let d: serde_json::Value =
            serde_json::from_slice(&fs::read(assets::safe_path(root, DATA)?)?)?;
        Ok(Status {
            installed: true,
            record: d["record"].as_bool().unwrap_or(false),
            images: d["images"].as_object().map_or(0, |m| m.len()),
            error: String::new(),
        })
    })();
    result.unwrap_or_else(|e| Status {
        installed: true,
        error: format!("{e:#}"),
        ..Default::default()
    })
}
pub fn install(
    c: &Catalog,
    images: Option<BTreeMap<String, Vec<u8>>>,
    record: Option<bool>,
) -> Result<()> {
    ensure!(
        c.engine != Engine::Renpy,
        "图片安装当前支持 RPG Maker MV/MZ"
    );
    let root = Path::new(&c.root);
    let file = assets::safe_path(root, MANIFEST)?;
    let mut m = if file.exists() {
        let m = read(root)?;
        check(root, &m)?;
        m
    } else {
        Manifest {
            schema: 1,
            ..Default::default()
        }
    };
    let plugins_path = assets::safe_path(root, "js/plugins.js")?;
    let plugins = fs::read_to_string(&plugins_path)?;
    let registration = range(&plugins)?;
    ensure!(
        registration.is_none() || file.exists(),
        "图片插件注册已存在但缺少安装清单，请保留目录并检查"
    );
    let mut current: serde_json::Value = if m.files.contains_key(DATA) && root.join(DATA).exists() {
        serde_json::from_slice(&fs::read(assets::safe_path(root, DATA)?)?)?
    } else {
        json!({"schema":1,"record":false,"images":{}})
    };
    let mut writes = BTreeMap::new();
    if let Some(images) = images {
        ensure!(!images.is_empty(), "没有可安装的译图");
        let mut map = BTreeMap::new();
        for (logical, bytes) in images {
            ensure!(
                logical.starts_with("img/") && logical.ends_with(".png"),
                "图片逻辑路径无效"
            );
            assets::safe_path(root, &logical)?;
            assets::dimensions(&bytes)?;
            let p = format!(".handy-images/files/{}.png", digest(&bytes));
            map.insert(logical, p.clone());
            writes.insert(p, bytes);
        }
        current["images"] = json!(map);
        current["language"] = json!(c.target_language);
    }
    if let Some(record) = record {
        current["record"] = json!(record);
    }
    writes.insert(
        PLUGIN.into(),
        include_bytes!("../adapters/HandyImageTranslator.js").to_vec(),
    );
    writes.insert(DATA.into(), serde_json::to_vec_pretty(&current)?);
    for (p, bytes) in &writes {
        let target = assets::safe_path(root, p)?;
        ensure!(
            m.files.contains_key(p) || !target.exists(),
            "同名文件已存在，为保护原文件已停止：{p}"
        );
        let hashes = m.files.entry(p.clone()).or_default();
        let hash = digest(bytes);
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    // 先记下允许写入的摘要；中途退出后可重试或只删除已登记的文件。
    save_json(&file, &m)?;
    for (p, bytes) in writes {
        atomic_write(&assets::safe_path(root, &p)?, &bytes)?;
    }
    if registration.is_none() {
        atomic_write(&plugins_path, format!("{plugins}{BLOCK}").as_bytes())?;
    }
    Ok(())
}
pub fn uninstall(c: &Catalog) -> Result<()> {
    let root = Path::new(&c.root);
    let file = assets::safe_path(root, MANIFEST)?;
    ensure!(file.exists(), "尚未安装图片插件");
    let m = read(root)?;
    check(root, &m)?;
    let plugins_path = assets::safe_path(root, "js/plugins.js")?;
    let mut plugins = fs::read_to_string(&plugins_path)?;
    if let Some(r) = range(&plugins)? {
        plugins.replace_range(r, "");
        atomic_write(&plugins_path, plugins.as_bytes())?;
    }
    for p in m.files.keys() {
        let target = assets::safe_path(root, p)?;
        if target.exists() {
            fs::remove_file(target)?;
        }
    }
    fs::remove_file(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_install_restore_and_dynamic_records() {
        let (t, mut c, bytes) = crate::assets::tests::fixture();
        let root = Path::new(&c.root);
        c.entries[0].translation = Some("药水".into());
        crate::install::install(&c).unwrap();
        let text = fs::read(root.join("data/GameTranslatorTranslations.json")).unwrap();
        install(
            &c,
            Some(BTreeMap::from([(
                "img/pictures/help.png".into(),
                bytes.clone(),
            )])),
            Some(true),
        )
        .unwrap();
        assert_eq!(status(&c).images, 1);
        assert!(status(&c).record);
        assert!(crate::install::installation_status(&c).current);
        fs::write(
            root.join(".handy-images/loaded.json"),
            r#"["img/pictures/unused.png"]"#,
        )
        .unwrap();
        let p = assets::scan(&t.path().join("state"), &c).unwrap();
        assert!(
            p.assets
                .iter()
                .any(|a| a.path.ends_with("unused.png") && a.loaded)
        );
        install(&c, None, Some(false)).unwrap();
        assert!(!status(&c).record);
        assert_eq!(status(&c).images, 1);
        uninstall(&c).unwrap();
        assert!(!status(&c).installed);
        assert_eq!(
            fs::read(root.join("data/GameTranslatorTranslations.json")).unwrap(),
            text
        );
        assert_eq!(fs::read(root.join("img/pictures/help.png")).unwrap(), bytes);
        assert!(crate::install::installation_status(&c).current);
        install(&c, None, Some(true)).unwrap();
        crate::install::uninstall(&c).unwrap();
        assert!(status(&c).installed);
        assert!(
            fs::read_to_string(root.join("js/plugins.js"))
                .unwrap()
                .contains("HANDY_IMAGE_TRANSLATOR_BEGIN")
        );
        uninstall(&c).unwrap();
    }
    #[test]
    fn refuses_collisions_modified_owned_files_and_tampered_manifest() {
        let (_t, c, _) = crate::assets::tests::fixture();
        let root = Path::new(&c.root);
        fs::write(root.join(PLUGIN), b"original").unwrap();
        assert!(install(&c, None, Some(true)).is_err());
        assert_eq!(fs::read(root.join(PLUGIN)).unwrap(), b"original");
        fs::remove_file(root.join(PLUGIN)).unwrap();
        install(&c, None, Some(true)).unwrap();
        fs::write(root.join(PLUGIN), b"external edit").unwrap();
        assert!(uninstall(&c).is_err());
        assert!(root.join(MANIFEST).is_file());
        assert!(install(&c, None, Some(false)).is_err());
        fs::write(
            root.join(MANIFEST),
            r#"{"schema":1,"files":{"data/Items.json":["a"]}}"#,
        )
        .unwrap();
        assert!(uninstall(&c).is_err());
        assert!(root.join("data/Items.json").is_file());
    }
}
