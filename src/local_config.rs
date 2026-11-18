use crate::translate::{
    ProviderConfig, default_auth_mode, default_batch_size, default_concurrency,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Serialize)]
pub struct LocalConfig {
    #[serde(flatten)]
    pub service: ProviderConfig,
    pub model_options: Vec<String>,
}

pub fn is_local(preset: &str) -> bool {
    matches!(preset, "codex" | "claude_code")
}

pub fn read(preset: &str) -> Result<LocalConfig> {
    let home = || -> Result<PathBuf> {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
            .context("无法确定当前用户目录")
    };
    let inherited: BTreeMap<String, String> = env::vars().collect();
    match preset {
        "codex" => {
            let root = match env::var_os("CODEX_HOME") {
                Some(path) => PathBuf::from(path),
                None => home()?.join(".codex"),
            };
            let text = fs::read_to_string(root.join("config.toml"))
                .context("无法读取 Codex 的 config.toml，请先配置 Codex API 服务")?;
            let auth = read_json(root.join("auth.json"))?;
            codex(&text, &auth, &inherited)
        }
        "claude_code" => {
            let root = match env::var_os("CLAUDE_CONFIG_DIR") {
                Some(path) => PathBuf::from(path),
                None => home()?.join(".claude"),
            };
            claude(
                &read_json(root.join("settings.json"))?,
                &read_json(root.join("settings.local.json"))?,
                &inherited,
            )
        }
        _ => bail!("请选择 Codex 或 Claude Code 本机配置"),
    }
}

fn read_json(path: PathBuf) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("本机 API 配置文件不是有效 JSON"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e).context("无法读取本机 API 配置文件"),
    }
}

fn config(
    provider: &str,
    base_url: String,
    model: String,
    api_key: String,
    auth_mode: String,
) -> Result<LocalConfig> {
    let service = ProviderConfig {
        provider: provider.into(),
        base_url,
        model,
        api_key,
        auth_mode,
        concurrency: default_concurrency(),
        batch_size: default_batch_size(),
    };
    service.validate_service()?;
    let model_options = if service.model.is_empty() {
        vec![]
    } else {
        vec![service.model.clone()]
    };
    Ok(LocalConfig {
        service,
        model_options,
    })
}

fn codex(text: &str, auth: &Value, inherited: &BTreeMap<String, String>) -> Result<LocalConfig> {
    // TOML 解析错误可能携带原文行，不能把原始诊断返回给界面。
    let root: toml::Value = toml::from_str(text)
        .map_err(|_| anyhow::anyhow!("Codex config.toml 格式无效，请检查本机配置"))?;
    let profile = if let Some(name) = root.get("profile").and_then(toml::Value::as_str) {
        Some(
            root.get("profiles")
                .and_then(|p| p.get(name))
                .context("Codex 当前 profile 不存在")?,
        )
    } else {
        None
    };
    let selected = |key: &str| {
        profile
            .and_then(|p| p.get(key))
            .or_else(|| root.get(key))
            .and_then(toml::Value::as_str)
    };
    let provider = selected("model_provider").unwrap_or("openai");
    let details = root.get("model_providers").and_then(|p| p.get(provider));
    ensure!(
        provider == "openai" || details.is_some(),
        "Codex 选中的服务缺少 model_providers 配置"
    );
    let field = |key: &str| {
        details
            .and_then(|d| d.get(key))
            .and_then(toml::Value::as_str)
    };
    let base_url = field("base_url")
        .map(str::to_owned)
        .or_else(|| {
            (provider == "openai").then(|| {
                inherited
                    .get("OPENAI_BASE_URL")
                    .cloned()
                    .unwrap_or_else(|| "https://api.openai.com/v1".into())
            })
        })
        .context("Codex 服务未配置 base_url")?;
    let wire_api = match field("wire_api").unwrap_or("responses") {
        "responses" => "responses",
        "chat" => "openai",
        _ => bail!("Codex 服务协议不受支持，仅支持 Responses 和 Chat Completions"),
    };
    let requires_auth = details
        .and_then(|d| d.get("requires_openai_auth"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(provider == "openai");
    let key = if let Some(name) = field("env_key") {
        inherited
            .get(name)
            .cloned()
            .context("Codex env_key 指定的环境变量未设置，请设置后重新打开工具")?
    } else if let Some(key) = field("experimental_bearer_token") {
        key.into()
    } else if requires_auth {
        inherited
            .get("OPENAI_API_KEY")
            .cloned()
            .or_else(|| auth["OPENAI_API_KEY"].as_str().map(str::to_owned))
            .context("Codex 未配置独立 API 密钥；ChatGPT 登录凭据不能用于此接入，请配置 API 密钥")?
    } else {
        bail!("Codex 服务缺少 API 密钥，请配置 env_key 或 experimental_bearer_token")
    };
    ensure!(!key.trim().is_empty(), "Codex API 密钥为空，请检查本机配置");
    config(
        wire_api,
        base_url,
        selected("model").unwrap_or_default().into(),
        key,
        default_auth_mode(),
    )
}

fn claude(
    settings: &Value,
    local: &Value,
    inherited: &BTreeMap<String, String>,
) -> Result<LocalConfig> {
    let mut vars = inherited.clone();
    for value in [settings, local] {
        if let Some(values) = value.get("env") {
            for (name, value) in values
                .as_object()
                .context("Claude Code 的 env 配置需要是对象")?
            {
                if name.starts_with("ANTHROPIC_") {
                    vars.insert(
                        name.clone(),
                        value
                            .as_str()
                            .context("Claude Code 的环境变量需要是字符串")?
                            .into(),
                    );
                }
            }
        }
    }
    let get = |key: &str| vars.get(key).filter(|v| !v.trim().is_empty()).cloned();
    let (auth_mode, key) = if let Some(key) = get("ANTHROPIC_AUTH_TOKEN") {
        ("bearer", key)
    } else {
        ("api_key", get("ANTHROPIC_API_KEY").context("Claude Code 未配置 ANTHROPIC_API_KEY 或 ANTHROPIC_AUTH_TOKEN；仅登录账号无法用于此 API 接入")?)
    };
    let model = get("ANTHROPIC_MODEL")
        .or_else(|| {
            local["model"]
                .as_str()
                .or_else(|| settings["model"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let resolve = |name: &str| match name {
        "sonnet" => get("ANTHROPIC_DEFAULT_SONNET_MODEL").unwrap_or_default(),
        "opus" => get("ANTHROPIC_DEFAULT_OPUS_MODEL").unwrap_or_default(),
        "haiku" => get("ANTHROPIC_DEFAULT_HAIKU_MODEL").unwrap_or_default(),
        "" => get("ANTHROPIC_DEFAULT_SONNET_MODEL").unwrap_or_default(),
        _ => name.to_owned(),
    };
    let mut result = config(
        "anthropic",
        get("ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into()),
        resolve(&model),
        key,
        auth_mode.into(),
    )?;
    for name in ["SONNET", "OPUS", "HAIKU"] {
        if let Some(model) = get(&format!("ANTHROPIC_DEFAULT_{name}_MODEL")) {
            result.model_options.push(model);
        }
    }
    result.model_options.sort();
    result.model_options.dedup();
    Ok(result)
}

pub fn restore_key(saved: &mut ProviderConfig, local: LocalConfig) -> Result<()> {
    ensure!(
        saved.provider == local.service.provider
            && saved.auth_mode == local.service.auth_mode
            && saved.base_url.trim().trim_end_matches('/')
                == local.service.base_url.trim().trim_end_matches('/'),
        "本机服务地址、协议或认证方式已改变，请点击“重新读取本机配置”后保存；当前未恢复密钥"
    );
    saved.api_key = local.service.api_key;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_reads_profiles_environment_and_inline_token_without_oauth() {
        let text = r#"
model = "old"
profile = "game"
[profiles.game]
model = "configured-model"
model_provider = "custom"
[model_providers.custom]
base_url = "https://example.test/v1"
wire_api = "responses"
experimental_bearer_token = "inline-secret"
requires_openai_auth = false
"#;
        let parsed = codex(text, &Value::Null, &BTreeMap::new()).unwrap();
        assert_eq!(parsed.service.model, "configured-model");
        assert_eq!(parsed.service.api_key, "inline-secret");
        let env = BTreeMap::from([("TEST_CODEX_KEY".into(), "env-secret".into())]);
        let text = text.replace(
            "experimental_bearer_token = \"inline-secret\"",
            "env_key = \"TEST_CODEX_KEY\"",
        );
        assert_eq!(
            codex(&text, &Value::Null, &env).unwrap().service.api_key,
            "env-secret"
        );
        assert!(codex(&text, &Value::Null, &BTreeMap::new()).is_err());
        assert!(
            codex(
                "",
                &serde_json::json!({"tokens":{"access_token":"oauth-secret"}}),
                &BTreeMap::new()
            )
            .is_err()
        );
        let error = codex(
            "experimental_bearer_token = 'secret' invalid",
            &Value::Null,
            &env,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(!error.contains("secret"));
        assert_eq!(
            codex(
                "",
                &serde_json::json!({"OPENAI_API_KEY":"api-secret"}),
                &BTreeMap::new()
            )
            .unwrap()
            .service
            .api_key,
            "api-secret"
        );
    }
    #[test]
    fn claude_merges_user_environment_resolves_alias_and_selects_authentication() {
        let settings = serde_json::json!({"model":"sonnet","env":{
            "ANTHROPIC_API_KEY":"key", "ANTHROPIC_DEFAULT_SONNET_MODEL":"actual-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL":"actual-haiku"}});
        let local = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"token", "ANTHROPIC_BASE_URL":"https://gateway.test/anthropic"}});
        let parsed = claude(&settings, &local, &BTreeMap::new()).unwrap();
        assert_eq!(parsed.service.auth_mode, "bearer");
        assert_eq!(parsed.service.api_key, "token");
        assert_eq!(parsed.service.model, "actual-sonnet");
        assert_eq!(parsed.model_options, ["actual-haiku", "actual-sonnet"]);
        assert_eq!(
            claude(&settings, &Value::Null, &BTreeMap::new())
                .unwrap()
                .service
                .auth_mode,
            "api_key"
        );
        assert!(claude(&Value::Null, &Value::Null, &BTreeMap::new()).is_err());
    }
    #[test]
    fn refuses_key_restore_when_service_destination_or_authentication_changes() {
        let parsed = || {
            config(
                "responses",
                "https://first.test/v1".into(),
                "model".into(),
                "secret".into(),
                default_auth_mode(),
            )
            .unwrap()
        };
        let mut saved = parsed().service;
        saved.api_key.clear();
        saved.base_url = "https://other.test/v1".into();
        assert!(restore_key(&mut saved, parsed()).is_err());
        assert!(saved.api_key.is_empty());
        saved.base_url = "https://first.test/v1/".into();
        restore_key(&mut saved, parsed()).unwrap();
        assert_eq!(saved.api_key, "secret");
    }
}
