use crate::model::{Catalog, Engine, Entry, digest, validate_languages};
use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

static RENPY_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(?:[A-Za-z_][\w.]*\s+)*(?:[rubRUB]{0,2})?["']"#).unwrap());
static UI_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:text|textbutton|label|old)\s+(?:_\(\s*)?(?:[rubRUB]{0,2})?["']"#).unwrap()
});

pub fn scan(input: &str, source: &str, target: &str) -> Result<Catalog> {
    validate_languages(source, target)?;
    let selected = PathBuf::from(input.trim().trim_matches('"'))
        .canonicalize()
        .context("找不到游戏路径，请选择游戏目录或主程序")?;
    ensure!(
        selected.is_dir()
            || (selected.is_file()
                && selected
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("exe"))),
        "请拖入游戏主程序（.exe）或游戏文件夹，不支持压缩包和其他文件"
    );
    let launch_path = if selected.is_file()
        && selected
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        Some(selected.to_string_lossy().into_owned())
    } else {
        None
    };
    let base = if selected.is_file() {
        selected.parent().unwrap().to_path_buf()
    } else {
        selected
    };
    let mut candidates = vec![base.clone()];
    candidates.push(base.join("www"));
    let found = candidates.iter().find_map(|root| {
        if root.join("js/rmmz_core.js").is_file() {
            Some((root.clone(), Engine::RpgMakerMz))
        } else if root.join("js/rpg_core.js").is_file() {
            Some((root.clone(), Engine::RpgMakerMv))
        } else {
            None
        }
    });
    let (root, engine) = if let Some(v) = found {
        v
    } else if base.join("game").is_dir()
        && (base.join("renpy").is_dir() || base.join("lib").is_dir())
    {
        (base.join("game"), Engine::Renpy)
    } else if base.file_name().is_some_and(|n| n == "game")
        && base
            .parent()
            .is_some_and(|p| p.join("renpy").is_dir() || p.join("lib").is_dir())
    {
        (base, Engine::Renpy)
    } else {
        bail!(
            "未识别到已支持的引擎。当前支持 RPG Maker MV/MZ，以及保留可读 .rpy 脚本的 Ren’Py 游戏。"
        )
    };
    let root = root.canonicalize()?;
    let mut strings = BTreeMap::<String, Vec<String>>::new();
    let mut title = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut warnings = Vec::new();
    if engine == Engine::Renpy {
        title = root
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into();
        let mut files = Vec::new();
        collect_rpy(&root, &mut files, 0)?;
        files.sort();
        ensure!(
            !files.is_empty(),
            "没有可读 .rpy 脚本。首版不解包 .rpa，也不反编译 .rpyc。"
        );
        for path in files {
            let contents = read_text(&path)?;
            let relative = path.strip_prefix(&root)?.to_string_lossy();
            for (index, text) in renpy_lines(&contents) {
                add(&mut strings, &text, format!("{}:{}", relative, index + 1));
            }
        }
        warnings.push("Ren’Py 首版读取单行对白、菜单项及常见 text/textbutton 文本；不提取 Python 动态表达式、多行字符串和归档脚本。".into());
        warnings.push(
            "Ren’Py 适配会接入 Text.set_text，需在实际游戏中验证插件兼容性；它不自动更换游戏字体。"
                .into(),
        );
    } else {
        let data = root.join("data");
        ensure!(
            root.join("js/plugins.js").is_file() && data.is_dir(),
            "游戏缺少 js/plugins.js 或 data 目录，暂不支持这种打包结构"
        );
        let mut files = Vec::new();
        for entry in fs::read_dir(&data)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path.extension().is_some_and(|e| e == "json")
                && supported_rpg_file(&path)
            {
                files.push(path);
            }
        }
        files.sort();
        for path in files {
            let value: Value = serde_json::from_str(&read_text(&path)?)
                .with_context(|| format!("游戏 JSON 无法解析：{}", path.display()))?;
            let file = path.file_name().unwrap().to_string_lossy();
            if file == "System.json" {
                if let Some(t) = value["gameTitle"].as_str() {
                    title = t.into();
                }
                for key in [
                    "gameTitle",
                    "currencyUnit",
                    "elements",
                    "skillTypes",
                    "weaponTypes",
                    "armorTypes",
                    "equipTypes",
                    "terms",
                ] {
                    collect_string_values(&value[key], &format!("{file}/{key}"), &mut strings);
                }
            } else {
                walk_rpg(&value, &file, &mut strings);
            }
        }
        warnings.push("读取标准数据库、对白、选项和地图显示名；图片文字、插件参数、脚本动态生成的文本暂不提取。".into());
        warnings.push("安装后请完全退出并重启游戏。中文显示依赖游戏字体包含中文字形。".into());
    }
    ensure!(
        !strings.is_empty(),
        "没有提取到可翻译文本，请检查游戏文件是否为可读格式"
    );
    let source = if source == "auto" {
        crate::language::detect(strings.keys())?
    } else {
        source.to_owned()
    };
    validate_languages(&source, target)?;
    // shortcut: 同一游戏相同原文共用译文，需要按场景区分语义时改为位置映射。
    let entries = strings
        .into_iter()
        .map(|(original, contexts)| Entry {
            id: digest(original.as_bytes()),
            original,
            contexts,
            translation: None,
            error: None,
            translation_policy: None,
        })
        .collect();
    Ok(Catalog {
        schema: 1,
        game_id: digest(root.to_string_lossy().as_bytes()),
        title,
        root: root.to_string_lossy().into(),
        launch_path,
        engine,
        source_language: source,
        target_language: target.into(),
        entries,
        warnings,
        glossary: vec![],
    })
}

fn read_text(path: &Path) -> Result<String> {
    ensure!(
        fs::metadata(path)?.len() <= 32 * 1024 * 1024,
        "单个脚本或数据文件超过 32 MB：{}",
        path.display()
    );
    Ok(fs::read_to_string(path)
        .with_context(|| format!("文件不是 UTF-8 文本：{}", path.display()))?
        .trim_start_matches('\u{feff}')
        .into())
}

fn supported_rpg_file(path: &Path) -> bool {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    matches!(
        name.as_ref(),
        "Actors"
            | "Classes"
            | "Skills"
            | "Items"
            | "Weapons"
            | "Armors"
            | "Enemies"
            | "Troops"
            | "States"
            | "CommonEvents"
            | "System"
    ) || name
        .strip_prefix("Map")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn add(out: &mut BTreeMap<String, Vec<String>>, text: &str, context: String) {
    if text.trim().is_empty() || !text.chars().any(char::is_alphabetic) {
        return;
    }
    let contexts = out.entry(text.to_owned()).or_default();
    if contexts.len() < 3 && !contexts.contains(&context) {
        contexts.push(context);
    }
}

fn collect_string_values(v: &Value, context: &str, out: &mut BTreeMap<String, Vec<String>>) {
    match v {
        Value::String(s) => add(out, s, context.into()),
        Value::Array(a) => {
            for (i, el) in a.iter().enumerate() {
                collect_string_values(el, &format!("{context}/{i}"), out);
            }
        }
        Value::Object(m) => {
            for (key, el) in m {
                collect_string_values(el, &format!("{context}/{key}"), out);
            }
        }
        _ => {}
    }
}

fn walk_rpg(v: &Value, context: &str, out: &mut BTreeMap<String, Vec<String>>) {
    match v {
        Value::Array(a) => {
            for (i, el) in a.iter().enumerate() {
                walk_rpg(el, &format!("{context}/{i}"), out);
            }
        }
        Value::Object(m) => {
            if let Some(code) = m.get("code").and_then(Value::as_i64) {
                let p = &v["parameters"];
                match code {
                    401 | 405 => {
                        if let Some(s) = p[0].as_str() {
                            add(out, s, context.into());
                        }
                    }
                    102 => collect_string_values(&p[0], context, out),
                    101 => {
                        if let Some(s) = p[4].as_str() {
                            add(out, s, context.into());
                        }
                    }
                    320 | 324 | 325 => {
                        if let Some(s) = p[1].as_str() {
                            add(out, s, context.into());
                        }
                    }
                    _ => {}
                }
                return;
            }
            for (key, el) in m {
                if key == "name"
                    && (m.contains_key("pages")
                        || context.starts_with("Troops.json")
                        || context.starts_with("CommonEvents.json"))
                {
                    continue;
                }
                let next = format!("{context}/{key}");
                if matches!(
                    key.as_str(),
                    "name"
                        | "nickname"
                        | "description"
                        | "profile"
                        | "displayName"
                        | "message1"
                        | "message2"
                        | "message3"
                        | "message4"
                ) {
                    if let Some(s) = el.as_str() {
                        add(out, s, next);
                    }
                } else if el.is_array() || el.is_object() {
                    walk_rpg(el, &next, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_rpy(root: &Path, files: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    ensure!(depth < 32, "脚本目录层级过深");
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if matches!(name.as_str(), "tl" | "cache" | "saves" | ".game-translator")
            || name.starts_with("zzz_game_translator")
        {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_rpy(&entry.path(), files, depth + 1)?;
        } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "rpy") {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn renpy_lines(contents: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut python_indent = None;
    let mut triple: Option<&str> = None;
    for (index, line) in contents.lines().enumerate() {
        let s = line.trim_start();
        let indent = line.len() - s.len();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if let Some(level) = python_indent {
            if indent > level {
                continue;
            }
            python_indent = None;
        }
        if let Some(delimiter) = triple {
            if s.contains(delimiter) {
                triple = None;
            }
            continue;
        }
        if s.ends_with(':')
            && (s.starts_with("python") || (s.starts_with("init ") && s.contains("python")))
        {
            python_indent = Some(indent);
            continue;
        }
        let delimiter = if s.contains("\"\"\"") {
            Some("\"\"\"")
        } else if s.contains("'''") {
            Some("'''")
        } else {
            None
        };
        if let Some(delimiter) = delimiter {
            if s.matches(delimiter).count() % 2 == 1 {
                triple = Some(delimiter);
            }
            continue;
        }
        if let Some(text) = renpy_string(line) {
            out.push((index, text));
        }
    }
    out
}

pub fn renpy_string(line: &str) -> Option<String> {
    let s = line.trim_start();
    let excluded = [
        "image ", "scene ", "show ", "hide ", "play ", "queue ", "stop ", "define ", "default ",
        "style ", "new ", "voice ", "return ", "raise ", "assert ", "print ", "jump ", "call ",
    ];
    if excluded.iter().any(|p| s.starts_with(p)) || !(RENPY_LINE.is_match(s) || UI_LINE.is_match(s))
    {
        return None;
    }
    let start = s.find(['"', '\''])?;
    let quote = s.as_bytes()[start] as char;
    let raw = start > 0 && matches!(s.as_bytes()[start - 1], b'r' | b'R');
    if s[start..].starts_with(&quote.to_string().repeat(3)) {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[start + 1..].chars();
    while let Some(c) = chars.next() {
        if c == quote {
            return Some(out);
        }
        if c == '\\' {
            let next = chars.next()?;
            if raw {
                out.push('\\');
                out.push(next);
                continue;
            }
            match next {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                'x' => {
                    let hex: String = chars.by_ref().take(2).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn scan_rejects_non_game_files_before_engine_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.zip");
        fs::write(&path, b"not a game executable").unwrap();
        assert!(
            scan(path.to_str().unwrap(), "ja", "zh-CN")
                .unwrap_err()
                .to_string()
                .contains(".exe")
        );
    }
    #[test]
    fn rpg_extracts_display_fields_without_script_or_note() {
        let data = json!({"note":"do_not_translate", "name":"勇者", "events":[{"code":401,"parameters":["こんにちは\\V[1]"]},{"code":355,"parameters":["dangerous_script()"]},{"code":102,"parameters":[["はい","いいえ"]]}]});
        let mut out = BTreeMap::new();
        walk_rpg(&data, "Map001.json", &mut out);
        assert!(
            out.contains_key("勇者")
                && out.contains_key("こんにちは\\V[1]")
                && out.contains_key("はい")
        );
        assert!(!out.contains_key("do_not_translate") && !out.contains_key("dangerous_script()"));
    }
    #[test]
    fn renpy_handles_dialogue_menu_ui_and_escapes() {
        assert_eq!(
            renpy_string(r#"e "こんにちは [name]""#).as_deref(),
            Some("こんにちは [name]")
        );
        assert_eq!(
            renpy_string(r#"    "Go\nnow": "#).as_deref(),
            Some("Go\nnow")
        );
        assert_eq!(renpy_string(r#"text _("Start")"#).as_deref(), Some("Start"));
        assert_eq!(renpy_string(r#"image bg = "room.png""#), None);
        assert_eq!(renpy_string(r#"$ url = "https://example.com""#), None);
        assert_eq!(
            renpy_lines(
                "init python:\n    return \"ignore\"\nlabel start:\n    e happy \"Hello\"\n"
            )
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>(),
            vec!["Hello"]
        );
    }
}
