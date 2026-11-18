#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
#[cfg(windows)]
mod desktop;
mod extract;
mod image_api;
mod image_install;
mod install;
mod language;
mod local_config;
mod memory;
mod model;
mod ocr;
mod review;
mod settings;
mod translate;

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use model::{Bundle, Catalog, Term, save_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use settings::Settings;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use translate::{ProviderConfig, validate_translation};

#[derive(Clone, Default, Serialize)]
struct Progress {
    running: bool,
    processed: usize,
    total: usize,
    message: String,
    in_flight: usize,
    cache_hits: usize,
    translation_requests: usize,
}

struct App {
    token: String,
    data_dir: PathBuf,
    catalog: Mutex<Option<Catalog>>,
    progress: Mutex<Progress>,
    image_progress: Mutex<assets::Progress>,
    ocr_status: Mutex<Option<ocr::Status>>,
    image_cancel: AtomicBool,
    busy: AtomicBool,
    task_succeeded: Arc<AtomicBool>,
    cancel: AtomicBool,
    client: reqwest::Client,
}
type Shared = Arc<App>;

struct ApiError(anyhow::Error);
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":format!("{:#}",self.0)})),
        )
            .into_response()
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl App {
    fn enter(&self) -> Result<BusyGuard<'_>> {
        ensure!(
            self.busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "正在处理任务，请完成或停止后再操作"
        );
        self.task_succeeded.store(false, Ordering::Release);
        Ok(BusyGuard(&self.busy))
    }
    fn persist(&self, catalog: &Catalog) -> Result<()> {
        save_json(&self.data_dir.join(catalog.file_name()), catalog)?;
        save_json(&self.data_dir.join("last.json"), &catalog.file_name())
    }
    fn memory_path(&self) -> PathBuf {
        self.data_dir.join("translation-memory.json")
    }
    fn remember_translations(&self, catalog: &Catalog) -> Result<()> {
        let mut memory = memory::Memory::load(&self.memory_path())?;
        if memory.seed(catalog) {
            memory.save(&self.memory_path())?;
        }
        Ok(())
    }
}

async fn authorize(
    State(app): State<Shared>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let correct = request
        .headers()
        .get("x-session-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == app.token);
    if !correct {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"会话已失效，请使用程序启动时打开的页面"})),
        )
            .into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(request).await
}

async fn security(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("cache-control", "no-store".parse().unwrap());
    headers.insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data: blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}

async fn status(State(app): State<Shared>) -> Json<Value> {
    let catalog = app.catalog.lock().await;
    let summary = catalog.as_ref().map(|c| json!({
        "title":c.title,"root":c.root,"launch_path":c.launch_path,"engine":c.engine.label(),
        "source_language":c.source_language,"target_language":c.target_language,
        "entries":c.entries.len(),"translated":c.entries.iter().filter(|e|e.translation.is_some()).count(),
        "failed":c.entries.iter().filter(|e|e.error.is_some()).count(),"warnings":c.warnings,
        "installed":install::installed(c),"installation":install::installation_status(c),"can_launch":c.launch_path.is_some(),
        "glossary_terms":c.glossary.len()
    }));
    Json(
        json!({"catalog":summary,"progress":app.progress.lock().await.clone(),"busy":app.busy.load(Ordering::Acquire),"task_succeeded":app.task_succeeded.load(Ordering::Acquire)}),
    )
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    query: String,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    page: usize,
}
async fn entries(State(app): State<Shared>, Query(page): Query<Page>) -> ApiResult<Json<Value>> {
    let guard = app.catalog.lock().await;
    let c = guard.as_ref().context("请先加载游戏")?;
    let query = page.query.to_lowercase();
    let filtered: Vec<_> = c
        .entries
        .iter()
        .filter(|e| {
            (page.filter != "pending" || e.translation.is_none())
                && (page.filter != "translated" || e.translation.is_some())
                && (page.filter != "failed" || e.error.is_some())
                && (page.filter != "review" || !review::issues(c, e).is_empty())
                && (query.is_empty()
                    || e.original.to_lowercase().contains(&query)
                    || e.translation
                        .as_ref()
                        .is_some_and(|v| v.to_lowercase().contains(&query)))
        })
        .collect();
    let total = filtered.len();
    let offset = page
        .page
        .min(total.saturating_sub(1) / 50)
        .saturating_mul(50);
    Ok(Json(
        json!({"total":total,"page":offset/50,"entries":filtered.into_iter().skip(offset).take(50).map(|e| {
            let mut value = serde_json::to_value(e).unwrap();
            value["issues"] = json!(review::issues(c, e));
            value
        }).collect::<Vec<_>>()}),
    ))
}

#[derive(Deserialize)]
struct Scan {
    path: String,
    source_language: String,
    target_language: String,
}
async fn scan(State(app): State<Shared>, Json(request): Json<Scan>) -> ApiResult<Json<Value>> {
    let _guard = app.enter()?;
    let mut catalog = tokio::task::spawn_blocking(move || {
        extract::scan(
            &request.path,
            &request.source_language,
            &request.target_language,
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))??;
    let old_path = app.data_dir.join(catalog.file_name());
    let mut installed_restored = 0;
    if old_path.is_file() {
        let old: Catalog =
            serde_json::from_slice(&fs::read(old_path).map_err(anyhow::Error::from)?)
                .context("已有翻译记录损坏，已停止加载以保留文件")?;
        review::validate_glossary(&old.glossary)?;
        catalog.glossary = old.glossary.clone();
        let old_entries: std::collections::HashMap<_, _> =
            old.entries.iter().map(|e| (e.id.as_str(), e)).collect();
        for e in &mut catalog.entries {
            if let Some(previous) = old_entries
                .get(e.id.as_str())
                .filter(|p| p.original == e.original)
            {
                if let Some(t) = &previous.translation {
                    if validate_translation(&catalog.engine, &e.original, t).is_ok() {
                        e.translation = Some(t.clone());
                        e.translation_policy = previous.translation_policy.clone();
                    }
                }
            }
        }
    } else if install::installed(&catalog)
        && install::installation_status(&catalog).language == catalog.target_language
    {
        match install::restore_translations(&mut catalog) {
            Ok(count) => installed_restored = count,
            Err(error) => catalog
                .warnings
                .push(format!("游戏内译文未自动恢复：{error:#}")),
        }
    }
    if let Err(error) = app.remember_translations(&catalog) {
        catalog
            .warnings
            .push(format!("翻译记录已恢复，但缓存未更新：{error:#}"));
    }
    app.persist(&catalog)?;
    let restored = catalog
        .entries
        .iter()
        .filter(|e| e.translation.is_some())
        .count();
    let complete = restored == catalog.entries.len();
    *app.catalog.lock().await = Some(catalog);
    *app.progress.lock().await = Progress {
        message: if installed_restored > 0 {
            format!("已从游戏恢复 {installed_restored} 条译文并保存到本地，无需重复翻译")
        } else if complete {
            "已提取文本翻译完成，图片文字需另行处理".into()
        } else {
            format!("已恢复 {restored} 条本地译文，可继续未完成的文本")
        },
        ..Default::default()
    };
    Ok(Json(
        json!({"ok":true,"restored_from_game":installed_restored}),
    ))
}

async fn restore_installed(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let mut guard = app.catalog.lock().await;
    let mut catalog = guard.clone().context("请先加载游戏")?;
    let restored = install::restore_translations(&mut catalog)?;
    app.persist(&catalog)?;
    *guard = Some(catalog);
    let message = format!("已从游戏恢复 {restored} 条缺失译文并保存，本地已有译文保持不变。");
    app.progress.lock().await.message = message.clone();
    Ok(Json(
        json!({"ok":true,"restored":restored,"message":message}),
    ))
}

#[derive(Deserialize)]
struct TranslationRequest {
    #[serde(flatten)]
    config: ProviderConfig,
    #[serde(default)]
    ids: Option<Vec<String>>,
    #[serde(default)]
    instructions: String,
    #[serde(default = "use_cache_default")]
    use_cache: bool,
}
fn use_cache_default() -> bool {
    true
}

async fn start(
    State(app): State<Shared>,
    Json(request): Json<TranslationRequest>,
) -> ApiResult<Json<Value>> {
    request.config.validate()?;
    let guard = app.enter()?;
    let catalog = app.catalog.lock().await.clone().context("请先加载游戏")?;
    review::validate_glossary(&catalog.glossary)?;
    ensure_api(
        request.instructions.chars().count() <= 2000,
        "补充要求最多 2,000 字符",
    )?;
    ensure_api(
        request.instructions.trim().is_empty()
            || matches!(
                request.config.provider.as_str(),
                "openai" | "responses" | "anthropic"
            ),
        "补充翻译要求仅适用于大模型接口，请清空要求或切换服务",
    )?;
    ensure_api(
        request.ids.is_some() || request.instructions.trim().is_empty(),
        "补充要求需要在选中重译时使用",
    )?;
    let total = if let Some(ids) = &request.ids {
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        ensure_api(
            !ids.is_empty() && ids.len() <= 500 && ids.len() == unique.len(),
            "请选择 1 到 500 条不同文本进行重译",
        )?;
        ensure_api(
            ids.iter()
                .all(|id| catalog.entries.iter().any(|e| &e.id == id)),
            "所选文本不属于当前项目，请重新选择",
        )?;
        ids.len()
    } else {
        catalog
            .entries
            .iter()
            .filter(|e| e.translation.is_none())
            .count()
    };
    ensure_api(total > 0, "所有文本已有译文，可以修改译文或切换目标语言")?;
    app.cancel.store(false, Ordering::Release);
    *app.progress.lock().await = Progress {
        running: true,
        processed: 0,
        total,
        message: "正在连接翻译服务".into(),
        in_flight: 0,
        cache_hits: 0,
        translation_requests: 0,
    };
    std::mem::forget(guard);
    tokio::spawn(async move {
        let result = run_job(&app, &request).await;
        let mut progress = app.progress.lock().await;
        progress.running = false;
        progress.in_flight = 0;
        app.task_succeeded.store(
            result.is_ok()
                && !app.cancel.load(Ordering::Acquire)
                && progress.processed == progress.total,
            Ordering::Release,
        );
        progress.message = match result {
            Ok(()) if app.cancel.load(Ordering::Acquire) => {
                "已停止，已保存的译文会在下次继续时复用".into()
            }
            Ok(()) => format!(
                "本轮完成 {} 条，其中复用缓存 {} 条，文本翻译请求 {} 次；译文已保存，可安装到游戏",
                progress.processed, progress.cache_hits, progress.translation_requests
            ),
            Err(e) => format!("翻译已停止：{e:#}。有效译文已保留，请处理后继续。"),
        };
        app.busy.store(false, Ordering::Release);
    });
    Ok(Json(json!({"ok":true})))
}
fn ensure_api(condition: bool, message: &str) -> ApiResult<()> {
    if !condition {
        return Err(anyhow::anyhow!("{message}").into());
    }
    Ok(())
}

async fn run_job(app: &Shared, request: &TranslationRequest) -> Result<()> {
    let config = &request.config;
    let mut catalog = app.catalog.lock().await.clone().context("翻译项目不存在")?;
    let mut memory = if request.use_cache {
        Some(memory::Memory::load(&app.memory_path())?)
    } else {
        None
    };
    if request.ids.is_none() {
        if let Some(memory) = &memory {
            let policy = memory::policy(&catalog, "");
            let mut hits = 0;
            for index in 0..catalog.entries.len() {
                if catalog.entries[index].translation.is_none() {
                    if let Some(text) = memory.get(&catalog, &catalog.entries[index]) {
                        let entry = &mut catalog.entries[index];
                        entry.translation = Some(text);
                        entry.error = None;
                        entry.translation_policy = Some(policy.clone());
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                app.persist(&catalog)?;
                *app.catalog.lock().await = Some(catalog.clone());
                let mut progress = app.progress.lock().await;
                progress.processed = hits;
                progress.cache_hits = hits;
                progress.message = format!("已复用并保存 {hits} 条缓存，正在翻译其余文本");
            }
        }
    }
    let catalog = Arc::new(catalog);
    let entries = catalog
        .entries
        .iter()
        .filter(|e| {
            request
                .ids
                .as_ref()
                .map_or(e.translation.is_none(), |ids| ids.contains(&e.id))
        })
        .cloned();
    let mut batches = translate::batches(entries, config);
    let mut tasks = tokio::task::JoinSet::new();
    let mut failure = None;
    let mut first_batch_saved = false;
    loop {
        while tasks.len()
            < if first_batch_saved {
                config.concurrency
            } else {
                1
            }
            && failure.is_none()
            && !app.cancel.load(Ordering::Acquire)
        {
            let Some(batch) = batches.pop_front() else {
                break;
            };
            let catalog = catalog.clone();
            let config = config.clone();
            let client = app.client.clone();
            let instructions = request.instructions.clone();
            app.progress.lock().await.translation_requests += 1;
            tasks.spawn(async move {
                let result =
                    translate::translate_batch(&client, &config, &catalog, &batch, &instructions)
                        .await;
                (batch, result)
            });
        }
        app.progress.lock().await.in_flight = tasks.len();
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        let (batch, result) = match joined {
            Ok(value) => value,
            Err(error) => {
                if failure.is_none() {
                    failure = Some(anyhow::anyhow!("翻译任务异常：{error}"));
                }
                continue;
            }
        };
        let success = result.is_ok();
        // 以最新已保存目录合并完成的批次，避免乱序返回覆盖其他批次。
        let mut lock = app.catalog.lock().await;
        let mut next = lock.as_ref().context("翻译项目不存在")?.clone();
        match &result {
            Ok(values) => {
                for (id, text) in values {
                    if let Some(e) = next.entries.iter_mut().find(|e| &e.id == id) {
                        e.translation = Some(text.clone());
                        e.error = None;
                        e.translation_policy =
                            Some(memory::policy(&catalog, &request.instructions));
                    }
                }
            }
            Err(error) => {
                for e in &mut next.entries {
                    if batch.iter().any(|b| b.id == e.id) {
                        e.error = Some(format!("{error:#}"));
                    }
                }
            }
        }
        let mut saved = app.persist(&next);
        if saved.is_ok() {
            *lock = Some(next.clone());
            if let Some(memory) = &mut memory {
                if success && memory.seed(&next) {
                    saved = memory.save(&app.memory_path());
                }
            }
        }
        drop(lock);
        let mut p = app.progress.lock().await;
        if success && saved.is_ok() {
            first_batch_saved = true;
            p.processed += batch.len();
        }
        if failure.is_none() {
            failure = saved.err().or_else(|| result.err());
        }
        p.in_flight = tasks.len();
        p.message = if failure.is_some() {
            format!("批次处理失败，正在保存其余 {} 个请求的结果", tasks.len())
        } else if app.cancel.load(Ordering::Acquire) {
            format!("停止派发新批次，等待 {} 个请求完成并保存", tasks.len())
        } else {
            format!(
                "本轮已翻译并保存 {} / {} 条，{} 个请求进行中",
                p.processed,
                p.total,
                tasks.len()
            )
        };
    }
    if let Some(error) = failure {
        Err(error)
    } else {
        Ok(())
    }
}

async fn stop(State(app): State<Shared>) -> Json<Value> {
    app.cancel.store(true, Ordering::Release);
    Json(json!({"ok":true,"message":"已停止派发新批次，正在等待已提交的请求完成并保存"}))
}

async fn load_settings(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let mut settings = Settings::load(&app.data_dir.join("settings.json"))?;
    let mut warning = String::new();
    if local_config::is_local(&settings.preset) {
        settings.service.api_key.clear();
        if let Err(e) = local_config::read(&settings.preset)
            .and_then(|local| local_config::restore_key(&mut settings.service, local))
        {
            warning = format!("{e:#}");
        }
    }
    let mut value = serde_json::to_value(settings).map_err(anyhow::Error::from)?;
    value["config_warning"] = json!(warning);
    Ok(Json(value))
}
#[derive(Deserialize)]
struct LocalSource {
    preset: String,
}
async fn read_local_config(
    Json(source): Json<LocalSource>,
) -> ApiResult<Json<local_config::LocalConfig>> {
    Ok(Json(local_config::read(&source.preset)?))
}
async fn save_settings(
    State(app): State<Shared>,
    Json(settings): Json<Settings>,
) -> ApiResult<Json<Value>> {
    settings.save(&app.data_dir.join("settings.json"))?;
    Ok(Json(
        json!({"ok":true,"message":"配置已保存，重新启动后会恢复"}),
    ))
}
async fn list_models(
    State(app): State<Shared>,
    Json(config): Json<ProviderConfig>,
) -> ApiResult<Json<Value>> {
    let _guard = app.enter()?;
    Ok(Json(
        json!({"models":translate::models(&app.client, &config).await?}),
    ))
}
async fn test_service(
    State(app): State<Shared>,
    Json(config): Json<ProviderConfig>,
) -> ApiResult<Json<Value>> {
    let _guard = app.enter()?;
    translate::test_connection(&app.client, &config).await?;
    Ok(Json(
        json!({"ok":true,"message":"连接成功，模型已返回有效测试译文"}),
    ))
}

#[derive(Deserialize)]
struct Edit {
    id: String,
    translation: Option<String>,
}
async fn edit(State(app): State<Shared>, Json(edit): Json<Edit>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let mut lock = app.catalog.lock().await;
    let mut c = lock.clone().context("请先加载游戏")?;
    let policy = memory::policy(&c, "");
    let e = c
        .entries
        .iter_mut()
        .find(|e| e.id == edit.id)
        .context("未找到这条原文")?;
    if let Some(text) = &edit.translation {
        validate_translation(&c.engine, &e.original, text)?;
    }
    e.translation = edit.translation;
    e.error = None;
    e.translation_policy = e.translation.as_ref().map(|_| policy);
    let original = e.original.clone();
    let cleared = e.translation.is_none();
    app.persist(&c)?;
    *lock = Some(c.clone());
    let cache = (|| -> Result<()> {
        let mut memory = memory::Memory::load(&app.memory_path())?;
        memory.seed(&c);
        if cleared {
            memory.remove(&c, &original);
        }
        memory.save(&app.memory_path())
    })();
    Ok(Json(json!({"ok":true,"message":match cache {
        Ok(()) => if cleared { "译文已清除，相关缓存已移除。".into() } else { "译文已保存。更新游戏内译文后生效。".into() },
        Err(error) => format!("项目已保存，但缓存未更新：{error:#}"),
    }})))
}

async fn glossary(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let guard = app.catalog.lock().await;
    let c = guard.as_ref().context("请先加载游戏")?;
    Ok(Json(json!({"terms":c.glossary})))
}

#[derive(Deserialize)]
struct GlossaryRequest {
    terms: Vec<Term>,
}

async fn save_glossary(
    State(app): State<Shared>,
    Json(request): Json<GlossaryRequest>,
) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    review::validate_glossary(&request.terms)?;
    let mut guard = app.catalog.lock().await;
    let mut catalog = guard.clone().context("请先加载游戏")?;
    catalog.glossary = request.terms;
    app.persist(&catalog)?;
    *guard = Some(catalog);
    Ok(Json(
        json!({"ok":true,"message":"术语表已保存。仅影响之后的翻译；已有译文可检查后选中重译。"}),
    ))
}

async fn check_translations(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let guard = app.catalog.lock().await;
    let c = guard.as_ref().context("请先加载游戏")?;
    let pending = c.entries.iter().filter(|e| e.translation.is_none()).count();
    let flagged = c
        .entries
        .iter()
        .filter(|e| !review::issues(c, e).is_empty())
        .count();
    Ok(Json(
        json!({"checked":c.entries.len()-pending,"pending":pending,"flagged":flagged}),
    ))
}

async fn export(State(app): State<Shared>) -> ApiResult<Response> {
    let guard = app.catalog.lock().await;
    let c = guard.as_ref().context("请先加载游戏")?;
    let body = serde_json::to_vec_pretty(&c.bundle()).map_err(anyhow::Error::from)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=translations.json",
            ),
        ],
        body,
    )
        .into_response())
}

async fn import(State(app): State<Shared>, Json(bundle): Json<Bundle>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let mut guard = app.catalog.lock().await;
    let mut c = guard.clone().context("请先加载游戏")?;
    ensure_api(
        bundle.schema == 1
            && bundle.game_id == c.game_id
            && bundle.engine == c.engine
            && bundle.source_language == c.source_language
            && bundle.target_language == c.target_language,
        "译文文件的游戏、语言或版本与当前项目不一致",
    )?;
    if let Some(terms) = bundle.glossary {
        review::validate_glossary(&terms)?;
        c.glossary = terms;
    }
    let policy = memory::policy(&c, "");
    for (original, text) in &bundle.translations {
        let entry = c
            .entries
            .iter_mut()
            .find(|e| &e.original == original)
            .context("导入文件包含当前游戏中不存在的原文")?;
        validate_translation(&c.engine, original, text)?;
        entry.translation = Some(text.clone());
        entry.error = None;
        entry.translation_policy = Some(policy.clone());
    }
    app.persist(&c)?;
    *guard = Some(c.clone());
    let warning = app
        .remember_translations(&c)
        .err()
        .map(|error| format!("项目已保存，但缓存未更新：{error:#}"))
        .unwrap_or_default();
    Ok(Json(
        json!({"ok":true,"count":bundle.translations.len(),"warning":warning}),
    ))
}

async fn install_game(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = app.catalog.lock().await.clone().context("请先加载游戏")?;
    install::install(&c)?;
    Ok(Json(
        json!({"ok":true,"message":"译文已安装。请完全退出并重启游戏后检查显示。"}),
    ))
}
async fn uninstall_game(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let _busy = app.enter()?;
    let c = app.catalog.lock().await.clone().context("请先加载游戏")?;
    install::uninstall(&c)?;
    Ok(Json(
        json!({"ok":true,"message":"适配器和游戏内译文已卸载，本工具中保存的翻译记录仍可复用"}),
    ))
}

async fn launch(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let guard = app.catalog.lock().await;
    let c = guard.as_ref().context("请先加载游戏")?;
    let path = PathBuf::from(
        c.launch_path
            .as_ref()
            .context("请通过“选择程序”加载游戏主程序")?,
    );
    ensure_api(path.is_file(), "游戏主程序已不存在")?;
    std::process::Command::new(&path)
        .current_dir(path.parent().context("主程序路径无效")?)
        .spawn()
        .context("启动游戏失败")?;
    Ok(Json(json!({"ok":true})))
}

async fn pick() -> ApiResult<Json<Value>> {
    let path = tokio::task::spawn_blocking(pick_windows)
        .await
        .map_err(|e| anyhow::anyhow!(e))??;
    Ok(Json(json!({"path":path})))
}
#[cfg(windows)]
fn pick_windows() -> Result<String> {
    Ok(rfd::FileDialog::new()
        .set_title("选择游戏主程序")
        .add_filter("游戏主程序", &["exe"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default())
}
#[cfg(not(windows))]
fn pick_windows() -> Result<String> {
    bail!("请直接填写游戏路径")
}

fn router(app: Shared) -> Router {
    let api = Router::new()
        .merge(image_api::router())
        .route("/state", get(status))
        .route("/settings", get(load_settings).post(save_settings))
        .route("/local-config", post(read_local_config))
        .route("/models", post(list_models))
        .route("/test-service", post(test_service))
        .route("/entries", get(entries))
        .route("/scan", post(scan))
        .route("/restore-installed", post(restore_installed))
        .route("/translate", post(start))
        .route("/stop", post(stop))
        .route("/entry", post(edit))
        .route("/glossary", get(glossary).post(save_glossary))
        .route("/review", get(check_translations))
        .route("/export", get(export))
        .route("/import", post(import))
        .route("/install", post(install_game))
        .route("/uninstall", post(uninstall_game))
        .route("/launch", post(launch))
        .route("/pick-game", post(pick))
        .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .nest("/api", api)
        .route(
            "/",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    include_str!("../web/index.html"),
                )
            }),
        )
        .route(
            "/icon.svg",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/svg+xml")],
                    include_str!("../assets/icon.svg"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/images.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("../web/images.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(middleware::from_fn(security))
        .with_state(app)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        #[cfg(windows)]
        if !std::env::args().any(|arg| arg == "--headless") {
            rfd::MessageDialog::new()
                .set_title("Handy-translate 启动失败")
                .set_description(format!("{error:#}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|s| s == "--help") {
        println!(
            "Handy-translate {}\n参数：--data-dir 路径  --port 端口  --headless\n默认打开桌面窗口；--headless 仅用于接口测试。\nAPI 密钥默认不保存，可选择使用 Windows 加密记住。",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let mut port = 0u16;
    let mut headless = false;
    let mut data_dir = std::env::current_exe()?
        .parent()
        .context("程序目录不存在")?
        .join("data");
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--data-dir" => {
                index += 1;
                data_dir = PathBuf::from(args.get(index).context("--data-dir 缺少路径")?);
            }
            "--port" => {
                index += 1;
                port = args
                    .get(index)
                    .context("--port 缺少端口")?
                    .parse()
                    .context("端口必须是数字")?;
            }
            "--headless" => headless = true,
            other => bail!("未知参数：{other}"),
        }
        index += 1;
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let task_succeeded = Arc::new(AtomicBool::new(false));
    let (url, server) =
        runtime.block_on(start_server(data_dir.clone(), port, task_succeeded.clone()))?;
    if headless {
        println!(
            "Handy-translate {}\n{url}\n服务仅监听本机。\n",
            env!("CARGO_PKG_VERSION")
        );
        return runtime.block_on(async {
            tokio::select! {
                result = server => result.context("本机服务任务异常结束")?.context("本机服务异常结束"),
                _ = tokio::signal::ctrl_c() => Ok(()),
            }
        });
    }
    #[cfg(windows)]
    let result = desktop::run(&url, &data_dir, task_succeeded);
    #[cfg(not(windows))]
    let result = Err(anyhow::anyhow!("桌面版目前仅支持 Windows"));
    server.abort();
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}

async fn start_server(
    data_dir: PathBuf,
    port: u16,
    task_succeeded: Arc<AtomicBool>,
) -> Result<(String, tokio::task::JoinHandle<std::io::Result<()>>)> {
    fs::create_dir_all(&data_dir).context("无法创建数据目录，可使用 --data-dir 指定可写目录")?;
    let last_path = data_dir.join("last.json");
    let catalog = if last_path.is_file() {
        let file: String = serde_json::from_slice(&fs::read(last_path)?)
            .context("最近项目记录损坏，请保留 data 目录并检查 last.json")?;
        ensure!(
            std::path::Path::new(&file).components().count() == 1 && file.ends_with(".json"),
            "最近项目路径无效"
        );
        Some(
            serde_json::from_slice::<Catalog>(&fs::read(data_dir.join(file))?)
                .context("翻译记录无法读取，请保留数据文件进行检查")?,
        )
    } else {
        None
    };
    let token = model::digest(rand::random::<[u8; 32]>());
    let app = Arc::new(App {
        token: token.clone(),
        data_dir,
        catalog: Mutex::new(catalog),
        progress: Mutex::new(Progress {
            message: "选择游戏后开始".into(),
            ..Default::default()
        }),
        image_progress: Mutex::new(assets::Progress::default()),
        ocr_status: Mutex::new(None),
        image_cancel: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        task_succeeded,
        cancel: AtomicBool::new(false),
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .context("无法启动本地服务")?;
    let url = format!(
        "http://127.0.0.1:{}/#{}",
        listener.local_addr()?.port(),
        token
    );
    let server = tokio::spawn(async move { axum::serve(listener, router(app)).await });
    Ok((url, server))
}
