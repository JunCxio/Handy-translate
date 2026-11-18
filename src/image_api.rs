use crate::{
    ApiResult, BusyGuard, Shared,
    assets::{self, Project, Region},
    image_install, memory,
    model::{Catalog, Entry, digest},
    ocr,
    translate::{self, ProviderConfig},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::atomic::Ordering,
};

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/images/state", get(state))
        .route("/images/ocr", get(check_ocr))
        .route("/images/ocr/install", post(install_ocr))
        .route("/images/scan", post(scan))
        .route("/images/preview", get(preview))
        .route("/images/process", post(process))
        .route("/images/edit", post(edit))
        .route("/images/import", post(import))
        .route("/images/install", post(install))
        .route("/images/uninstall", post(uninstall))
        .route("/images/trace", post(trace))
        .route("/images/stop", post(stop))
}
async fn catalog(app: &Shared) -> Result<Catalog> {
    app.catalog.lock().await.clone().context("请先加载游戏")
}
fn project(app: &Shared, c: &Catalog) -> Result<Project> {
    assets::load(&app.data_dir, c)?.context("请先扫描引用素材")
}
async fn state(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let c = catalog(&app).await?;
    let ocr = app
        .ocr_status
        .lock()
        .await
        .clone()
        .filter(|s| s.language == c.source_language);
    Ok(Json(
        json!({"project":assets::load(&app.data_dir,&c)?,"installation":image_install::status(&c),"progress":app.image_progress.lock().await.clone(),"ocr":ocr}),
    ))
}
async fn check_ocr(State(app): State<Shared>) -> ApiResult<Json<ocr::Status>> {
    let c = catalog(&app).await?;
    let mut status = app.ocr_status.lock().await;
    if let Some(current) = status.as_ref().filter(|s| s.running) {
        return Ok(Json(current.clone()));
    }
    let next = tokio::task::spawn_blocking(move || ocr::check(&c.source_language))
        .await
        .map_err(anyhow::Error::from)?;
    *status = Some(next.clone());
    Ok(Json(next))
}
async fn install_ocr(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let busy = app.enter()?;
    let c = catalog(&app).await?;
    crate::ensure_api(
        ocr::supports_install(&c.source_language),
        "当前应用内安装仅支持日语 OCR 包；其他语言请在 Windows 可选功能中安装",
    )?;
    let language = c.source_language.clone();
    let mut status = tokio::task::spawn_blocking(move || ocr::check(&language))
        .await
        .map_err(anyhow::Error::from)?;
    if status.available {
        *app.ocr_status.lock().await = Some(status);
        return Ok(Json(json!({"ok":true})));
    }
    status.running = true;
    status.message = "等待系统授权或正在下载、安装日语 OCR 包。请在 Windows 授权弹窗中允许，安装期间保持程序打开。此过程可能需要几分钟。".into();
    *app.ocr_status.lock().await = Some(status);
    std::mem::forget(busy);
    tokio::spawn(async move {
        let _busy = BusyGuard(&app.busy);
        let language = c.source_language;
        let next = tokio::task::spawn_blocking(move || {
            let result = ocr::install();
            let mut next = ocr::check(&language);
            match result {
                Ok(reboot) => {
                    next.reboot_required = reboot;
                    next.installable = false;
                    next.message = if reboot {
                        "Windows 已安装组件，需要重启系统完成。请先保存工作，重启后再检测。".into()
                    } else if next.available {
                        "日语 OCR 包已安装并通过检测，可以继续本地识别。".into()
                    } else {
                        "系统安装已结束，但 OCR 尚不可用。请重启程序或系统后重新检测。".into()
                    };
                }
                Err(error) => next.message = format!("{error:#}"),
            }
            next
        })
        .await;
        let mut status = app.ocr_status.lock().await;
        match next {
            Ok(next) => *status = Some(next),
            Err(error) => {
                if let Some(status) = status.as_mut() {
                    status.running = false;
                    status.message = format!("安装任务异常结束，请重新检测：{error}");
                }
            }
        }
    });
    Ok(Json(json!({"ok":true})))
}
async fn scan(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    let data = app.data_dir.clone();
    let cc = c.clone();
    let p = tokio::task::spawn_blocking(move || assets::scan(&data, &cc))
        .await
        .map_err(anyhow::Error::from)??;
    assets::save(&app.data_dir, &c, &p)?;
    Ok(Json(json!({"ok":true,"count":p.assets.len()})))
}
#[derive(Deserialize)]
struct Preview {
    id: String,
    #[serde(default)]
    kind: String,
}
async fn preview(
    State(app): State<Shared>,
    Query(q): Query<Preview>,
) -> ApiResult<impl IntoResponse> {
    let c = catalog(&app).await?;
    let p = project(&app, &c)?;
    let a = p
        .assets
        .iter()
        .find(|a| a.id == q.id)
        .context("素材不存在")?;
    let bytes = if q.kind == "rendered" {
        let bytes =
            fs::read(assets::output_path(&app.data_dir, &c, a)?).map_err(anyhow::Error::from)?;
        crate::ensure_api(
            a.rendered.as_ref() == Some(&digest(&bytes)),
            "译图已变化，请重新生成或导入",
        )?;
        bytes
    } else {
        assets::source_png(&c, a)?
    };
    Ok(([(header::CONTENT_TYPE, "image/png")], bytes))
}
#[derive(Deserialize)]
struct Process {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    scope: String,
    action: String,
    #[serde(default)]
    mode: String,
    service: Option<ProviderConfig>,
    #[serde(default = "yes")]
    use_cache: bool,
}
fn yes() -> bool {
    true
}
async fn process(
    State(app): State<Shared>,
    Json(mut request): Json<Process>,
) -> ApiResult<Json<Value>> {
    crate::ensure_api(
        matches!(
            request.action.as_str(),
            "recognize" | "translate" | "render" | "auto"
        ),
        "未知素材操作",
    )?;
    crate::ensure_api(
        request.scope.is_empty() || request.scope == "all",
        "未知素材范围",
    )?;
    crate::ensure_api(
        request.scope != "all" || (request.ids.is_empty() && request.action == "auto"),
        "全部处理不能同时指定所选素材，且仅支持一键处理",
    )?;
    let recognizes = matches!(request.action.as_str(), "recognize" | "auto");
    if recognizes {
        crate::ensure_api(
            matches!(request.mode.as_str(), "local" | "vision"),
            "请选择识别方式",
        )?;
    }
    if matches!(request.action.as_str(), "translate" | "auto")
        || (recognizes && request.mode == "vision")
    {
        let config = request.service.as_ref().context("请配置翻译服务")?;
        config.validate()?;
        if recognizes && request.mode == "vision" {
            crate::ensure_api(
                matches!(
                    config.provider.as_str(),
                    "openai" | "responses" | "anthropic"
                ),
                "当前服务不支持视觉输入，请更换视觉模型或选择本地 OCR",
            )?;
        }
    }
    let busy = app.enter()?;
    let c = catalog(&app).await?;
    let p = project(&app, &c)?;
    if request.scope == "all" {
        request.ids = p.assets.iter().map(|a| a.id.clone()).collect();
        crate::ensure_api(!request.ids.is_empty(), "请先扫描候选素材")?;
    } else {
        crate::ensure_api(
            !request.ids.is_empty()
                && request.ids.len() <= 100
                && request.ids.iter().collect::<BTreeSet<_>>().len() == request.ids.len(),
            "请选择 1 到 100 张不同素材",
        )?;
    }

    crate::ensure_api(
        request
            .ids
            .iter()
            .all(|id| p.assets.iter().any(|a| &a.id == id)),
        "所选素材不属于当前项目",
    )?;
    if recognizes && request.mode == "local" {
        let ids: BTreeSet<_> = request.ids.iter().collect();
        let mut needs_ocr = false;
        for a in p.assets.iter().filter(|a| ids.contains(&a.id)) {
            if request.action == "auto"
                && (a.rendered.is_some() || a.recognized.is_some() || !a.regions.is_empty())
            {
                continue;
            }
            let signature = recognition_signature(&c, a, &request)?;
            if request.use_cache
                && (a.recognized.as_ref() == Some(&signature)
                    || app
                        .data_dir
                        .join("images/recognition")
                        .join(format!("{signature}.json"))
                        .is_file())
            {
                continue;
            }
            needs_ocr = true;
            break;
        }
        if needs_ocr {
            let language = c.source_language.clone();
            tokio::task::spawn_blocking(move || assets::check_local_ocr(&language))
                .await
                .map_err(anyhow::Error::from)??;
        }
    }
    *app.image_progress.lock().await = assets::Progress {
        running: true,
        total: request.ids.len(),
        message: "正在处理所选素材".into(),
        ..Default::default()
    };
    app.image_cancel.store(false, Ordering::Release);
    std::mem::forget(busy);
    tokio::spawn(async move {
        let _busy = BusyGuard(&app.busy);
        let result = run(&app, &c, p, &request).await;
        let mut progress = app.image_progress.lock().await;
        progress.running = false;
        app.task_succeeded.store(
            result.as_ref().is_ok_and(|failed| *failed == 0)
                && !app.image_cancel.load(Ordering::Acquire)
                && progress.processed == progress.total,
            Ordering::Release,
        );
        progress.message = match result {
            Ok(failed) => format!(
                "{}：已处理 {} / {} 张，译图 {} 张，未检出文字 {} 张，失败 {} 张；结果已保存{}",
                if app.image_cancel.load(Ordering::Acquire) {
                    "已停止"
                } else {
                    "处理完成"
                },
                progress.processed,
                progress.total,
                progress.ready,
                progress.skipped,
                failed,
                if progress.unique_texts > 0 {
                    format!(
                        "。待译文字 {} 处，去重后 {} 条，缓存命中 {} 条，文本翻译请求 {} 次",
                        progress.text_regions,
                        progress.unique_texts,
                        progress.cached_texts,
                        progress.translation_requests
                    )
                } else {
                    String::new()
                }
            ),
            Err(e) => format!("处理停止，已完成的结果已保留：{e:#}"),
        };
    });
    Ok(Json(json!({"ok":true})))
}
fn recognition_signature(c: &Catalog, a: &assets::Asset, r: &Process) -> Result<String> {
    Ok(digest(serde_json::to_vec(&(
        1,
        &a.hash,
        &r.mode,
        &c.source_language,
        if r.mode == "vision" {
            r.service
                .as_ref()
                .map(|s| (&s.provider, &s.base_url, &s.model))
        } else {
            None
        },
    ))?))
}

async fn run(app: &Shared, c: &Catalog, mut p: Project, r: &Process) -> Result<usize> {
    let indices: Vec<_> = r
        .ids
        .iter()
        .map(|id| p.assets.iter().position(|a| &a.id == id).unwrap())
        .collect();
    let mut failed = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let result = async {
        if matches!(r.action.as_str(), "recognize" | "auto") {
            image_stage(
                app,
                c,
                &mut p,
                r,
                &indices,
                "recognize",
                &mut failed,
                &mut seen,
            )
            .await?;
        }
        if !app.image_cancel.load(Ordering::Acquire)
            && matches!(r.action.as_str(), "translate" | "auto")
        {
            translate_images(app, c, &mut p, r, &indices, &mut failed, &mut seen).await?;
        }
        if !app.image_cancel.load(Ordering::Acquire)
            && matches!(r.action.as_str(), "render" | "auto")
        {
            image_stage(
                app,
                c,
                &mut p,
                r,
                &indices,
                "render",
                &mut failed,
                &mut seen,
            )
            .await?;
        }
        Ok(())
    }
    .await;
    let mut progress = app.image_progress.lock().await;
    progress.processed = seen.len();
    progress.ready = indices
        .iter()
        .filter(|i| p.assets[**i].error.is_none() && p.assets[**i].rendered.is_some())
        .count();
    progress.skipped = indices
        .iter()
        .filter(|i| {
            let a = &p.assets[**i];
            a.error.is_none()
                && a.recognized.is_some()
                && a.regions.is_empty()
                && a.rendered.is_none()
        })
        .count();
    result.map(|()| failed.len())
}

async fn image_stage(
    app: &Shared,
    c: &Catalog,
    p: &mut Project,
    r: &Process,
    indices: &[usize],
    action: &str,
    failed: &mut BTreeSet<usize>,
    seen: &mut BTreeSet<usize>,
) -> Result<()> {
    let automatic = r.action == "auto";
    for (position, &index) in indices.iter().enumerate() {
        if app.image_cancel.load(Ordering::Acquire) {
            break;
        }
        if failed.contains(&index) {
            continue;
        }
        seen.insert(index);
        let mut a = p.assets[index].clone();
        let mut vision_failed = false;
        app.image_progress.lock().await.message = format!(
            "{} {}/{}：{}",
            if action == "recognize" {
                "识别图片"
            } else {
                "生成译图"
            },
            position + 1,
            indices.len(),
            a.path
        );
        let result = async {
            let bytes = assets::source_png(c, &a)?;
            let (w, h) = assets::dimensions(&bytes)?;
            a.width = w;
            a.height = h;
            if automatic && a.rendered.is_some() {
                let output = fs::read(assets::output_path(&app.data_dir, c, &a)?)?;
                ensure!(
                    a.rendered.as_ref() == Some(&digest(&output)),
                    "译图已被修改，请检查后重新生成或导入"
                );
                app.image_progress.lock().await.cache_hits += 1;
                return Ok(());
            }
            if action == "render" {
                if a.regions.is_empty() && a.recognized.is_some() {
                    return Ok(());
                }
                let regions = a.regions.clone();
                let output = tokio::task::spawn_blocking(move || assets::render(&bytes, &regions))
                    .await
                    .map_err(anyhow::Error::from)??;
                assets::write_rendered(&app.data_dir, c, &mut a, &output)?;
                return Ok(());
            }
            if automatic && (a.recognized.is_some() || !a.regions.is_empty()) {
                return Ok(());
            }
            let signature = recognition_signature(c, &a, r)?;
            let cache = app
                .data_dir
                .join("images/recognition")
                .join(format!("{signature}.json"));
            let cached = if r.use_cache && cache.is_file() {
                ensure!(
                    fs::metadata(&cache)?.len() <= 4 * 1024 * 1024,
                    "图片识别缓存过大"
                );
                let regions: Vec<Region> = serde_json::from_slice(&fs::read(&cache)?)
                    .context("图片识别缓存损坏，请取消缓存复用后重新识别")?;
                assets::validate_regions(&regions, w, h)?;
                Some(regions)
            } else {
                None
            };
            if r.use_cache && a.recognized.as_ref() == Some(&signature) {
                app.image_progress.lock().await.cache_hits += 1;
                return Ok(());
            }
            let mut regions = if let Some(regions) = cached {
                app.image_progress.lock().await.cache_hits += 1;
                regions
            } else if r.mode == "vision" {
                match translate::recognize_image(
                    &app.client,
                    r.service.as_ref().unwrap(),
                    &bytes,
                    &c.source_language,
                )
                .await
                {
                    Ok(regions) => regions,
                    Err(error) => {
                        vision_failed = true;
                        return Err(error);
                    }
                }
            } else {
                let temporary = assets::directory(&app.data_dir, c).join("ocr-input.png");
                crate::model::atomic_write(&temporary, &bytes)?;
                let path = temporary.clone();
                let language = c.source_language.clone();
                let result =
                    tokio::task::spawn_blocking(move || assets::local_ocr(&path, &language))
                        .await
                        .map_err(anyhow::Error::from)?;
                let _ = fs::remove_file(temporary);
                result?
            };
            assets::validate_regions(&regions, w, h)?;
            assets::sample_colors(&bytes, &mut regions)?;
            crate::model::save_json(&cache, &regions)?;
            a.regions = regions;
            a.recognized = Some(signature);
            a.rendered = None;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        a.error = result.err().map(|e| {
            failed.insert(index);
            format!("{e:#}")
        });
        p.assets[index] = a;
        assets::save(&app.data_dir, c, p)?;
        app.image_progress.lock().await.processed = seen.len();
        if vision_failed {
            anyhow::bail!(
                "视觉识别失败，已停止后续图片请求：{}",
                p.assets[index].error.as_deref().unwrap()
            );
        }
    }
    Ok(())
}

async fn translate_images(
    app: &Shared,
    c: &Catalog,
    p: &mut Project,
    r: &Process,
    indices: &[usize],
    failed: &mut BTreeSet<usize>,
    seen: &mut BTreeSet<usize>,
) -> Result<()> {
    let mut cc = c.clone();
    cc.entries.clear();
    let mut originals = BTreeMap::<String, usize>::new();
    let mut locations: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut text_regions = 0;
    for &index in indices {
        if app.image_cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if failed.contains(&index) {
            continue;
        }
        seen.insert(index);
        let a = &mut p.assets[index];
        if r.action == "auto" && a.rendered.is_some() {
            continue;
        }
        let valid = (|| {
            let (w, h) = assets::dimensions(&assets::source_png(c, a)?)?;
            ensure!(
                !a.regions.is_empty() || a.recognized.is_some(),
                "请先识别文字，或手动添加文字区域"
            );
            assets::validate_regions(&a.regions, w, h)
        })();
        a.error = valid.err().map(|e| {
            failed.insert(index);
            format!("{e:#}")
        });
        if a.error.is_some() {
            continue;
        }
        for (region_index, region) in a
            .regions
            .iter()
            .enumerate()
            .filter(|(_, r)| r.translation.is_none())
        {
            text_regions += 1;
            let entry_index = if let Some(&entry_index) = originals.get(&region.original) {
                entry_index
            } else {
                let entry_index = cc.entries.len();
                cc.entries.push(Entry {
                    id: entry_index.to_string(),
                    original: region.original.clone(),
                    contexts: Vec::new(),
                    translation: None,
                    error: None,
                    translation_policy: Some(memory::policy(c, "")),
                });
                originals.insert(region.original.clone(), entry_index);
                locations.push(Vec::new());
                entry_index
            };
            locations[entry_index].push((index, region_index));
        }
    }
    assets::save(&app.data_dir, c, p)?;
    if cc.entries.is_empty() {
        return Ok(());
    }
    let mut memory = if r.use_cache {
        Some(memory::Memory::load(&app.memory_path())?)
    } else {
        None
    };
    let mut cached = 0;
    for (entry_index, entry) in cc.entries.iter_mut().enumerate() {
        if let Some(text) = memory.as_ref().and_then(|m| m.get(c, entry)) {
            entry.translation = Some(text.clone());
            cached += 1;
            for &(index, region_index) in &locations[entry_index] {
                p.assets[index].regions[region_index].translation = Some(text.clone());
                p.assets[index].rendered = None;
            }
        }
    }
    assets::save(&app.data_dir, c, p)?;
    {
        let mut progress = app.image_progress.lock().await;
        progress.text_regions = text_regions;
        progress.unique_texts = cc.entries.len();
        progress.cached_texts = cached;
        progress.cache_hits += cached;
        progress.processed = seen.len();
        progress.message = format!(
            "文字汇总 {} 处，去重后 {} 条，缓存命中 {} 条，待翻译 {} 条",
            text_regions,
            cc.entries.len(),
            cached,
            cc.entries.len() - cached
        );
    }
    let pending: Vec<_> = cc
        .entries
        .iter()
        .filter(|e| e.translation.is_none())
        .cloned()
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let config = r.service.as_ref().unwrap();
    let batches = translate::batches(pending.iter().cloned(), config);
    for (batch_index, batch) in batches.iter().enumerate() {
        if app.image_cancel.load(Ordering::Acquire) {
            break;
        }
        {
            let mut progress = app.image_progress.lock().await;
            progress.translation_requests += 1;
            progress.message = format!(
                "翻译去重文字：第 {} 批，本批 {} 条，共 {} 条",
                batch_index + 1,
                batch.len(),
                pending.len()
            );
        }
        // 批次出错即停止新请求，保留先前成功批次，避免对后续图片继续消耗用量。
        let values = translate::translate_batch(&app.client, config, &cc, batch, "").await?;
        for (id, text) in values {
            let entry_index: usize = id.parse()?;
            for &(index, region_index) in &locations[entry_index] {
                p.assets[index].regions[region_index].translation = Some(text.clone());
                p.assets[index].rendered = None;
            }
            cc.entries[entry_index].translation = Some(text);
        }
        assets::save(&app.data_dir, c, p)?;
        if let Some(memory) = &mut memory {
            memory.seed(&cc);
            memory.save(&app.memory_path())?;
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Edit {
    id: String,
    regions: Vec<Region>,
}
async fn edit(State(app): State<Shared>, Json(r): Json<Edit>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    let mut p = project(&app, &c)?;
    let a = p
        .assets
        .iter_mut()
        .find(|a| a.id == r.id)
        .context("素材不存在")?;
    let (w, h) = assets::dimensions(&assets::source_png(&c, a)?)?;
    assets::validate_regions(&r.regions, w, h)?;
    a.width = w;
    a.height = h;
    a.regions = r.regions;
    a.rendered = None;
    a.error = None;
    assets::save(&app.data_dir, &c, &p)?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct Import {
    id: String,
    png: String,
}
async fn import(State(app): State<Shared>, Json(r): Json<Import>) -> ApiResult<Json<Value>> {
    use base64::Engine as _;
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    let mut p = project(&app, &c)?;
    let a = p
        .assets
        .iter_mut()
        .find(|a| a.id == r.id)
        .context("素材不存在")?;
    let (w, h) = assets::dimensions(&assets::source_png(&c, a)?)?;
    a.width = w;
    a.height = h;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&r.png)
        .context("PNG 数据无效")?;
    assets::write_rendered(&app.data_dir, &c, a, &bytes)?;
    assets::save(&app.data_dir, &c, &p)?;
    Ok(Json(json!({"ok":true})))
}
async fn install(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    let p = project(&app, &c)?;
    let mut images = BTreeMap::new();
    for a in p.assets.iter().filter(|a| a.rendered.is_some()) {
        assets::source_png(&c, a)?;
        let bytes =
            fs::read(assets::output_path(&app.data_dir, &c, a)?).map_err(anyhow::Error::from)?;
        crate::ensure_api(
            a.rendered.as_ref() == Some(&digest(&bytes)),
            &format!("译图已被修改，请重新生成或导入：{}", a.path),
        )?;
        crate::ensure_api(
            assets::dimensions(&bytes)? == (a.width, a.height),
            "译图尺寸不一致",
        )?;
        crate::ensure_api(
            images.insert(assets::logical(&a.path), bytes).is_none(),
            "同一逻辑图片存在多份译图，请只保留游戏实际使用的版本",
        )?;
    }
    image_install::install(&c, Some(images), None)?;
    Ok(Json(
        json!({"ok":true,"message":"译图已安装。请完全退出并重启游戏；文本译文不受影响。"}),
    ))
}
async fn uninstall(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    image_install::uninstall(&c)?;
    Ok(Json(
        json!({"ok":true,"message":"已还原原始素材并关闭素材记录，本地图片项目与文本译文仍保留。"}),
    ))
}
#[derive(Deserialize)]
struct Trace {
    enabled: bool,
}
async fn trace(State(app): State<Shared>, Json(r): Json<Trace>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = catalog(&app).await?;
    image_install::install(&c, None, Some(r.enabled))?;
    Ok(Json(
        json!({"ok":true,"message":"素材记录设置已保存，请完全退出并重启游戏后生效。玩过相关场景后重新扫描，可补充已加载素材。"}),
    ))
}
async fn stop(State(app): State<Shared>) -> Json<Value> {
    app.image_cancel.store(true, Ordering::Release);
    Json(json!({"ok":true}))
}
