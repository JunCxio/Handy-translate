use crate::model::{Catalog, Engine, atomic_write, digest, save_json};
use anyhow::{Context, Result, ensure};
use image::{ImageReader, Rgba};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Cursor,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub original: String,
    #[serde(default)]
    pub translation: Option<String>,
    #[serde(default = "default_background")]
    pub background: String,
    #[serde(default = "default_foreground")]
    pub foreground: String,
}
fn default_background() -> String {
    "#ffffff".into()
}
fn default_foreground() -> String {
    "#202020".into()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub path: String,
    pub hash: String,
    pub references: Vec<String>,
    pub loaded: bool,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub recognized: Option<String>,
    #[serde(default)]
    pub regions: Vec<Region>,
    #[serde(default)]
    pub rendered: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    pub game_id: String,
    pub root: String,
    pub source_language: String,
    pub target_language: String,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Default, Serialize)]
pub struct Progress {
    pub running: bool,
    pub processed: usize,
    pub total: usize,
    pub cache_hits: usize,
    pub ready: usize,
    pub skipped: usize,
    pub text_regions: usize,
    pub unique_texts: usize,
    pub cached_texts: usize,
    pub translation_requests: usize,
    pub message: String,
}

pub fn directory(data: &Path, catalog: &Catalog) -> PathBuf {
    data.join("images").join(digest(catalog.file_name()))
}
pub fn load(data: &Path, c: &Catalog) -> Result<Option<Project>> {
    let path = directory(data, c).join("project.json");
    if !path.exists() {
        return Ok(None);
    }
    ensure!(
        fs::metadata(&path)?.len() <= 16 * 1024 * 1024,
        "素材记录过大"
    );
    let p: Project = serde_json::from_slice(&fs::read(path)?)
        .context("素材记录损坏，已保留文件，请检查 images 目录")?;
    ensure!(
        p.schema == 1
            && p.game_id == c.game_id
            && p.root == c.root
            && p.source_language == c.source_language
            && p.target_language == c.target_language,
        "素材记录与当前游戏或语言不一致"
    );
    ensure!(p.assets.len() <= 20000, "素材记录过多");
    for a in &p.assets {
        ensure!(
            a.id == digest(a.path.as_bytes())
                && a.hash.len() == 64
                && a.hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "素材标识或摘要无效"
        );
        safe_path(Path::new(&c.root), &a.path)?;
    }
    Ok(Some(p))
}
pub fn save(data: &Path, c: &Catalog, p: &Project) -> Result<()> {
    save_json(&directory(data, c).join("project.json"), p)
}
pub fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !relative.is_empty() && !relative.contains(['\\', ':']) && !relative.starts_with('/'),
        "素材路径无效"
    );
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        ensure!(
            matches!(part, Component::Normal(_)),
            "素材路径不能越出游戏目录"
        );
        path.push(part);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(!meta.file_type().is_symlink(), "素材路径不能包含符号链接");
        }
    }
    Ok(path)
}

fn collect_images(
    root: &Path,
    relative: &str,
    depth: usize,
    files: &mut Vec<String>,
) -> Result<()> {
    ensure!(
        depth <= 16 && files.len() <= 20000,
        "图片目录过深或文件过多"
    );
    for file in fs::read_dir(safe_path(root, relative)?)? {
        let file = file?;
        let kind = file.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let path = format!("{relative}/{}", file.file_name().to_string_lossy());
        if kind.is_dir() {
            collect_images(root, &path, depth + 1, files)?;
        } else if [".png", ".png_", ".rpgmvp"]
            .iter()
            .any(|ext| path.to_lowercase().ends_with(ext))
        {
            files.push(path);
        }
    }
    Ok(())
}
pub fn logical(path: &str) -> String {
    for ext in [".png_", ".rpgmvp", ".png"] {
        if let Some(p) = path.strip_suffix(ext) {
            return format!("{p}.png");
        }
    }
    path.into()
}
fn add_reference(found: &mut BTreeMap<String, BTreeSet<String>>, paths: &[String], source: &str) {
    for path in paths {
        let refs = found.entry(path.clone()).or_default();
        if refs.len() < 6 {
            refs.insert(source.into());
        }
    }
}
fn references(
    value: &Value,
    aliases: &BTreeMap<String, Vec<String>>,
    found: &mut BTreeMap<String, BTreeSet<String>>,
    context: &str,
    depth: usize,
) {
    if depth > 40 {
        return;
    }
    match value {
        Value::String(text) => {
            if let Some(paths) = aliases.get(text) {
                add_reference(found, paths, &format!("{context}：名称引用"));
            }
            if text.starts_with(['{', '[']) && text.len() <= 1024 * 1024 {
                if let Ok(nested) = serde_json::from_str::<Value>(text) {
                    references(&nested, aliases, found, context, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                references(item, aliases, found, context, depth + 1);
            }
        }
        Value::Object(object) => {
            if object.get("code").and_then(Value::as_u64) == Some(231) {
                if let Some(name) = object
                    .get("parameters")
                    .and_then(|p| p.get(1))
                    .and_then(Value::as_str)
                {
                    if let Some(paths) = aliases.get(&format!("img/pictures/{name}.png")) {
                        add_reference(found, paths, &format!("{context}：显示图片事件"));
                    }
                }
            }
            for item in object.values() {
                references(item, aliases, found, context, depth + 1);
            }
        }
        _ => {}
    }
}

pub fn scan(data: &Path, c: &Catalog) -> Result<Project> {
    ensure!(
        c.engine != Engine::Renpy,
        "首版图片素材模块支持 RPG Maker MV/MZ，Ren’Py 文本翻译仍可使用"
    );
    let root = Path::new(&c.root);
    let mut files = Vec::new();
    collect_images(root, "img", 0, &mut files)?;
    files.sort();
    let mut aliases = BTreeMap::<String, Vec<String>>::new();
    for path in &files {
        let logical = logical(path);
        let stem = logical.trim_end_matches(".png");
        for alias in [
            logical.clone(),
            stem.into(),
            stem.splitn(3, '/').nth(2).unwrap_or(stem).into(),
            stem.rsplit('/').next().unwrap_or(stem).into(),
        ] {
            let list = aliases.entry(alias).or_default();
            if !list.contains(path) {
                list.push(path.clone());
            }
        }
    }
    let mut found = BTreeMap::new();
    let mut warnings=vec!["引用表示可能使用，不保证剧情一定到达；名称匹配可能有同名候选。动态拼接文件名可通过素材记录补充。仅支持 PNG 及 RPG Maker 加密 PNG。".into()];
    for file in fs::read_dir(root.join("data"))? {
        let file = file?;
        if !file.file_type()?.is_file() || file.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let name = file.file_name().to_string_lossy().to_string();
        if name.contains("Translator") || name.contains("Translations") {
            continue;
        }
        ensure!(
            file.metadata()?.len() <= 32 * 1024 * 1024,
            "素材引用数据文件过大：{name}"
        );
        match serde_json::from_slice::<Value>(&fs::read(file.path())?) {
            Ok(value) => references(&value, &aliases, &mut found, &name, 0),
            Err(_) => warnings.push(format!("无法解析 {name}，其中的图片引用未统计")),
        }
    }
    // 插件注册文件中的参数是数据；只解析 JSON 字面量，不执行游戏脚本。
    let plugins = fs::read_to_string(root.join("js/plugins.js"))?;
    if let (Some(start), Some(end)) = (plugins.find('['), plugins.rfind("];").map(|i| i + 1)) {
        if start < end {
            if let Ok(value) = serde_json::from_str::<Value>(&plugins[start..end]) {
                references(&value, &aliases, &mut found, "插件参数", 0);
            }
        }
    }
    let loaded = crate::image_install::loaded(root)?;
    for path in &loaded {
        if let Some(paths) = aliases.get(path) {
            add_reference(&mut found, paths, "运行时加载记录");
        }
    }
    let old = load(data, c)?;
    let old: BTreeMap<_, _> = old
        .as_ref()
        .map(|p| p.assets.iter().map(|a| (a.path.as_str(), a)).collect())
        .unwrap_or_default();
    let mut assets = Vec::new();
    for (path, refs) in found {
        let file = safe_path(root, &path)?;
        if fs::metadata(&file)?.len() > 16 * 1024 * 1024 {
            warnings.push(format!("跳过超过 16 MB 的素材：{path}"));
            continue;
        }
        let hash = digest(fs::read(file)?);
        let asset = old
            .get(path.as_str())
            .filter(|a| a.hash == hash)
            .map(|a| (*a).clone());
        let mut asset = asset.unwrap_or(Asset {
            id: digest(path.as_bytes()),
            path: path.clone(),
            hash,
            references: vec![],
            loaded: false,
            width: 0,
            height: 0,
            recognized: None,
            regions: vec![],
            rendered: None,
            error: None,
        });
        asset.loaded = loaded.contains(&logical(&path));
        asset.references = refs.into_iter().collect();
        assets.push(asset);
    }
    Ok(Project {
        schema: 1,
        game_id: c.game_id.clone(),
        root: c.root.clone(),
        source_language: c.source_language.clone(),
        target_language: c.target_language.clone(),
        assets,
        warnings,
    })
}

pub fn source_png(c: &Catalog, a: &Asset) -> Result<Vec<u8>> {
    let mut bytes = fs::read(safe_path(Path::new(&c.root), &a.path)?)?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024 && digest(&bytes) == a.hash,
        "原图已变化，请重新扫描素材后再处理"
    );
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ensure!(
            bytes.len() > 32
                && bytes[..16]
                    == [
                        0x52, 0x50, 0x47, 0x4d, 0x56, 0, 0, 0, 0, 3, 1, 0, 0, 0, 0, 0
                    ],
            "不是受支持的 RPG Maker 加密 PNG"
        );
        let system: Value =
            serde_json::from_slice(&fs::read(Path::new(&c.root).join("data/System.json"))?)?;
        let key = system["encryptionKey"]
            .as_str()
            .context("游戏缺少图片资源密钥")?;
        ensure!(
            key.len() == 32 && key.is_ascii(),
            "游戏图片资源密钥格式无效"
        );
        bytes.drain(..16);
        for i in 0..16 {
            bytes[i] ^=
                u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).context("图片资源密钥格式无效")?;
        }
    }
    dimensions(&bytes)?;
    Ok(bytes)
}
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "只接受 PNG 图片");
    let mut offset = 8usize;
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into()?) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        ensure!(
            kind != b"acTL",
            "暂不支持动画 PNG 素材，替换为静态图会丢失动画"
        );
        if kind == b"IDAT" {
            break;
        }
        offset = offset.checked_add(length + 12).context("PNG 数据无效")?;
    }
    let (w, h) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    ensure!(
        w > 0 && h > 0 && w <= 8192 && h <= 8192 && (w as u64) * (h as u64) <= 24_000_000,
        "图片尺寸超过当前处理限制"
    );
    Ok((w, h))
}
pub fn output_path(data: &Path, c: &Catalog, a: &Asset) -> Result<PathBuf> {
    let hash = a.rendered.as_ref().context("尚未生成或导入译图")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
        "译图摘要无效"
    );
    Ok(directory(data, c)
        .join("rendered")
        .join(format!("{hash}.png")))
}
pub fn validate_regions(regions: &[Region], w: u32, h: u32) -> Result<()> {
    ensure!(regions.len() <= 300, "单图最多 300 个文字区域");
    for r in regions {
        ensure!(
            r.width > 0
                && r.height > 0
                && r.x.saturating_add(r.width) <= w
                && r.y.saturating_add(r.height) <= h,
            "文字区域越出图片边界"
        );
        ensure!(
            !r.original.trim().is_empty() && r.original.len() <= 6000,
            "识别文字为空或过长"
        );
        if let Some(t) = &r.translation {
            ensure!(!t.trim().is_empty() && t.len() <= 12000, "译文为空或过长");
        }
        color(&r.background)?;
        color(&r.foreground)?;
    }
    Ok(())
}
fn color(value: &str) -> Result<Rgba<u8>> {
    ensure!(
        value.len() == 7
            && value.starts_with('#')
            && value[1..].bytes().all(|c| c.is_ascii_hexdigit()),
        "颜色需为 #RRGGBB"
    );
    Ok(Rgba([
        u8::from_str_radix(&value[1..3], 16)?,
        u8::from_str_radix(&value[3..5], 16)?,
        u8::from_str_radix(&value[5..7], 16)?,
        255,
    ]))
}
pub fn sample_colors(bytes: &[u8], regions: &mut [Region]) -> Result<()> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    for r in regions {
        let mut counts = BTreeMap::<[u8; 3], u32>::new();
        for x in r.x.saturating_sub(2)
            ..r.x
                .saturating_add(r.width)
                .saturating_add(2)
                .min(img.width())
        {
            for y in [
                r.y.saturating_sub(2),
                r.y.saturating_add(r.height)
                    .saturating_add(1)
                    .min(img.height() - 1),
            ] {
                let p = img.get_pixel(x, y);
                *counts
                    .entry([p[0] / 16 * 16, p[1] / 16 * 16, p[2] / 16 * 16])
                    .or_default() += 1;
            }
        }
        if let Some((rgb, _)) = counts.into_iter().max_by_key(|(_, count)| *count) {
            r.background = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            r.foreground =
                if (rgb[0] as u32 * 299 + rgb[1] as u32 * 587 + rgb[2] as u32 * 114) > 128000 {
                    "#202020"
                } else {
                    "#ffffff"
                }
                .into();
        }
    }
    Ok(())
}
pub fn render(bytes: &[u8], regions: &[Region]) -> Result<Vec<u8>> {
    use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
    let (w, h) = dimensions(bytes)?;
    validate_regions(regions, w, h)?;
    ensure!(
        !regions.is_empty() && regions.iter().all(|r| r.translation.is_some()),
        "请先识别并翻译全部保留的文字区域"
    );
    let font_path = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("Fonts/msyh.ttc");
    let font = FontArc::try_from_vec(
        fs::read(font_path).context("缺少 Microsoft YaHei 字体，请导入外部编辑的替换 PNG")?,
    )
    .map_err(|_| anyhow::anyhow!("无法读取排版字体"))?;
    let mut img = image::load_from_memory(bytes)?.to_rgba8();
    for r in regions {
        let text = r.translation.as_deref().unwrap();
        let mut fitted = None;
        // shortcut: 当前逐字排版用于中日韩及拉丁文字；复杂文字需使用外部排版后导入。
        ensure!(!text.chars().any(|ch|matches!(ch as u32,0x0590..=0x0dff|0x0e00..=0x0fff|0x1780..=0x17ff|0xfb1d..=0xfdff|0xfe70..=0xfeff)),"当前排版不支持该语言的连写或字形组合，请导入外部编辑的替换 PNG");
        for size in (7..=r.height.min(72)).rev() {
            let scaled = font.as_scaled(PxScale::from(size as f32));
            let mut lines = vec![String::new()];
            let mut used = 0.0f32;
            let mut too_wide = false;
            for ch in text.chars() {
                ensure!(
                    ch == '\n' || !font.glyph_id(ch).0.eq(&0),
                    "字体缺少字符，请导入外部排版的替换 PNG"
                );
                let width = scaled.h_advance(font.glyph_id(ch));
                if ch != '\n' && width > r.width.saturating_sub(4) as f32 {
                    too_wide = true;
                    break;
                }
                if ch == '\n'
                    || (!lines.last().unwrap().is_empty()
                        && used + width > r.width.saturating_sub(4) as f32)
                {
                    lines.push(String::new());
                    used = 0.0;
                }
                if ch != '\n' {
                    lines.last_mut().unwrap().push(ch);
                    used += width;
                }
            }
            if !too_wide
                && lines.len() as f32 * size as f32 * 1.22 <= r.height.saturating_sub(2) as f32
            {
                fitted = Some((size, lines));
                break;
            }
        }
        let (size, lines) = fitted.context("译文无法放入文字区域，请缩短译文或扩大区域")?;
        let bg = color(&r.background)?;
        let fg = color(&r.foreground)?;
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                img.put_pixel(x, y, bg);
            }
        }
        let scaled = font.as_scaled(size as f32);
        for (line_no, line) in lines.iter().enumerate() {
            let mut x = r.x as f32 + 2.0;
            let y = r.y as f32 + 1.0 + scaled.ascent() + line_no as f32 * size as f32 * 1.22;
            for ch in line.chars() {
                let id = font.glyph_id(ch);
                let glyph = id.with_scale_and_position(size as f32, point(x, y));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let px = bounds.min.x as i64 + gx as i64;
                        let py = bounds.min.y as i64 + gy as i64;
                        if px >= r.x as i64
                            && px < (r.x + r.width) as i64
                            && py >= r.y as i64
                            && py < (r.y + r.height) as i64
                        {
                            let base = img.get_pixel_mut(px as u32, py as u32);
                            for c in 0..3 {
                                base[c] = (base[c] as f32 * (1.0 - coverage)
                                    + fg[c] as f32 * coverage)
                                    .round() as u8;
                            }
                        }
                    });
                }
                x += scaled.h_advance(id);
            }
        }
    }
    let mut output = Cursor::new(Vec::new());
    img.write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[cfg(windows)]
fn initialize_local_ocr() -> Result<()> {
    static MTA: std::sync::OnceLock<windows::core::Result<()>> = std::sync::OnceLock::new();
    // 激活工厂缓存跨线程保留，MTA 必须存续至进程结束，不能在每张图后关闭。
    MTA.get_or_init(|| unsafe { windows::Win32::System::Com::CoIncrementMTAUsage().map(|_| ()) })
        .clone()?;
    Ok(())
}
#[cfg(windows)]
fn local_ocr_engine(language: &str) -> Result<windows::Media::Ocr::OcrEngine> {
    use windows::Media::Ocr::OcrEngine;
    let requested = language.split('-').next().unwrap_or(language);
    let languages = OcrEngine::AvailableRecognizerLanguages()?;
    let mut selected = None;
    let mut available = Vec::new();
    for item in languages {
        let tag = item.LanguageTag()?.to_string();
        if tag.split('-').next() == Some(requested) {
            selected = Some(item);
        }
        available.push(tag);
    }
    let language=selected.with_context(||format!("本机没有 {language} 的 OCR 语言包（已安装：{}）。请先安装对应 Windows OCR 语言包后重试；不会自动上传图片或切换到视觉模型。",available.join("、")))?;
    Ok(OcrEngine::TryCreateFromLanguage(&language)?)
}
pub fn check_local_ocr(language: &str) -> Result<()> {
    #[cfg(windows)]
    {
        initialize_local_ocr()?;
        local_ocr_engine(language)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = language;
        anyhow::bail!("本地 OCR 当前需要 Windows");
    }
}
#[cfg(windows)]
pub fn local_ocr(path: &Path, language: &str) -> Result<Vec<Region>> {
    use windows::{
        Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat},
        Media::Ocr::OcrEngine,
        Storage::{FileAccessMode, StorageFile},
        core::HSTRING,
    };
    initialize_local_ocr()?;
    let engine = local_ocr_engine(language)?;
    let path = path
        .canonicalize()?
        .to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .to_owned();
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.join()?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
    let bitmap = BitmapDecoder::CreateAsync(&stream)?
        .join()?
        .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?
        .join()?;
    ensure!(
        bitmap.PixelWidth()? as u32 <= OcrEngine::MaxImageDimension()?
            && bitmap.PixelHeight()? as u32 <= OcrEngine::MaxImageDimension()?,
        "图片超过 Windows OCR 尺寸限制，可改用视觉模型"
    );
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut regions = Vec::new();
    for line in result.Lines()? {
        let mut x = f32::MAX;
        let mut y = f32::MAX;
        let mut right = 0f32;
        let mut bottom = 0f32;
        for word in line.Words()? {
            let r = word.BoundingRect()?;
            x = x.min(r.X);
            y = y.min(r.Y);
            right = right.max(r.X + r.Width);
            bottom = bottom.max(r.Y + r.Height);
        }
        if right > x && bottom > y {
            regions.push(Region {
                x: x.floor() as u32,
                y: y.floor() as u32,
                width: (right.ceil() - x.floor()) as u32,
                height: (bottom.ceil() - y.floor()) as u32,
                original: line.Text()?.to_string(),
                translation: None,
                background: default_background(),
                foreground: default_foreground(),
            });
        }
    }
    Ok(regions)
}
#[cfg(not(windows))]
pub fn local_ocr(_: &Path, _: &str) -> Result<Vec<Region>> {
    anyhow::bail!("本地 OCR 当前需要 Windows");
}

pub fn write_rendered(data: &Path, c: &Catalog, a: &mut Asset, bytes: &[u8]) -> Result<()> {
    let dimensions = dimensions(bytes)?;
    ensure!(
        dimensions == (a.width, a.height),
        "替换 PNG 的尺寸必须与原图一致"
    );
    image::load_from_memory(bytes).context("替换 PNG 无法解码")?;
    let hash = digest(bytes);
    atomic_write(
        &directory(data, c)
            .join("rendered")
            .join(format!("{hash}.png")),
        bytes,
    )?;
    a.rendered = Some(hash);
    a.error = None;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn fixture() -> (tempfile::TempDir, Catalog, Vec<u8>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(root.join("img/pictures")).unwrap();
        fs::create_dir_all(root.join("js/plugins")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("js/rmmz_core.js"), "").unwrap();
        fs::write(root.join("js/plugins.js"), "var $plugins = [];\n").unwrap();
        fs::write(root.join("data/Items.json"), r#"[null,{"name":"Potion"}]"#).unwrap();
        fs::write(
            root.join("data/Map001.json"),
            r#"{"events":[{"list":[{"code":231,"parameters":[1,"help"]}]}]}"#,
        )
        .unwrap();
        let mut output = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(200, 100, Rgba([240, 240, 240, 255]))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        let bytes = output.into_inner();
        fs::write(root.join("img/pictures/help.png"), &bytes).unwrap();
        fs::write(root.join("img/pictures/unused.png"), &bytes).unwrap();
        let c = crate::extract::scan(root.to_str().unwrap(), "en", "zh-CN").unwrap();
        (temp, c, bytes)
    }
    #[test]
    fn scan_is_explicit_referenced_persistent_and_invalidates_changed_images() {
        let (t, c, _) = fixture();
        let data = t.path().join("state");
        assert!(load(&data, &c).unwrap().is_none());
        let mut p = scan(&data, &c).unwrap();
        assert_eq!(p.assets.len(), 1);
        assert!(
            p.assets[0]
                .references
                .iter()
                .any(|r| r.contains("显示图片"))
        );
        assert!(p.assets[0].regions.is_empty());
        p.assets[0].recognized = Some("cached".into());
        save(&data, &c, &p).unwrap();
        assert_eq!(
            scan(&data, &c).unwrap().assets[0].recognized.as_deref(),
            Some("cached")
        );
        fs::write(Path::new(&c.root).join("img/pictures/help.png"), b"changed").unwrap();
        assert!(source_png(&c, &p.assets[0]).is_err());
        assert!(scan(&data, &c).unwrap().assets[0].recognized.is_none());
        p.assets[0].id = "../../escape".into();
        save(&data, &c, &p).unwrap();
        assert!(load(&data, &c).is_err());
    }
    #[test]
    fn decrypts_mz_png_and_rejects_bad_key_or_traversal() {
        let (t, c, bytes) = fixture();
        let root = Path::new(&c.root);
        let key: [u8; 16] = [0x12; 16];
        let mut encrypted = vec![
            0x52, 0x50, 0x47, 0x4d, 0x56, 0, 0, 0, 0, 3, 1, 0, 0, 0, 0, 0,
        ];
        let mut payload = bytes.clone();
        for i in 0..16 {
            payload[i] ^= key[i];
        }
        encrypted.extend(payload);
        fs::write(root.join("img/pictures/help.png_"), &encrypted).unwrap();
        fs::remove_file(root.join("img/pictures/help.png")).unwrap();
        fs::write(
            root.join("data/System.json"),
            r#"{"encryptionKey":"12121212121212121212121212121212"}"#,
        )
        .unwrap();
        let p = scan(&t.path().join("state"), &c).unwrap();
        assert_eq!(source_png(&c, &p.assets[0]).unwrap(), bytes);
        fs::write(
            root.join("data/System.json"),
            r#"{"encryptionKey":"00000000000000000000000000000000"}"#,
        )
        .unwrap();
        assert!(source_png(&c, &p.assets[0]).is_err());
        for path in ["../x", "/absolute", "C:/x", "img/../x", "img\\x"] {
            assert!(safe_path(root, path).is_err());
        }
    }
    #[test]
    #[cfg(windows)]
    fn local_ocr_preflight_survives_separate_worker_threads() {
        for _ in 0..3 {
            let error = std::thread::spawn(|| check_local_ocr("zz").unwrap_err())
                .join()
                .unwrap();
            assert!(error.to_string().contains("OCR 语言包"));
        }
    }
    #[test]
    fn rendering_preserves_outside_region_and_checks_bounds_and_fit() {
        let (t, c, bytes) = fixture();
        let mut region = Region {
            x: 20,
            y: 20,
            width: 160,
            height: 60,
            original: "Help".into(),
            translation: Some("帮助".into()),
            background: "#ffffff".into(),
            foreground: "#202020".into(),
        };
        let output = render(&bytes, &[region.clone()]).unwrap();
        let img = image::load_from_memory(&output).unwrap().to_rgba8();
        assert_eq!(*img.get_pixel(0, 0), Rgba([240, 240, 240, 255]));
        assert_eq!(*img.get_pixel(20, 20), Rgba([255, 255, 255, 255]));
        assert_ne!(digest(&output), digest(&bytes));
        region.width = 1;
        assert!(render(&bytes, &[region.clone()]).is_err());
        region.x = 201;
        assert!(validate_regions(&[region.clone()], 200, 100).is_err());
        region.x = 20;
        region.width = 160;
        region.translation = Some("العربية".into());
        assert!(render(&bytes, &[region]).is_err());
        let mut a = scan(&t.path().join("state"), &c).unwrap().assets.remove(0);
        a.width = 1;
        a.height = 1;
        assert!(write_rendered(&t.path().join("state"), &c, &mut a, &bytes).is_err());
        let mut animated = bytes.clone();
        animated.splice(
            33..33,
            [
                0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        );
        assert!(
            dimensions(&animated)
                .unwrap_err()
                .to_string()
                .contains("动画")
        );
    }
}
