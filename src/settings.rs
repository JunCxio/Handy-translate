use crate::{
    local_config,
    model::{save_json, validate_languages},
    translate::{ProviderConfig, default_auth_mode, default_batch_size, default_concurrency},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(flatten)]
    pub service: ProviderConfig,
    pub source_language: String,
    pub target_language: String,
    pub remember_key: bool,
    pub model_options: Vec<String>,
    pub preset: String,
    pub use_cache: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            service: ProviderConfig {
                provider: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                model: String::new(),
                api_key: String::new(),
                auth_mode: default_auth_mode(),
                concurrency: default_concurrency(),
                batch_size: default_batch_size(),
            },
            source_language: "auto".into(),
            target_language: "zh-CN".into(),
            remember_key: false,
            model_options: Vec::new(),
            preset: "custom".into(),
            use_cache: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)
            .context("配置文件损坏，请重新填写并保存配置")?;
        let mut settings: Self =
            serde_json::from_value(value.clone()).context("配置格式无效，请重新填写并保存配置")?;
        settings.service.api_key.clear();
        if settings.remember_key && !value["encrypted_api_key"].is_null() {
            let encrypted: Vec<u8> = serde_json::from_value(value["encrypted_api_key"].clone())
                .context("加密密钥格式无效")?;
            settings.service.api_key = String::from_utf8(protect(&encrypted, false)?)
                .context("无法读取保存的密钥，请重新输入并保存配置")?;
        }
        Ok(settings)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.service.validate_service()?;
        anyhow::ensure!(
            matches!(
                self.preset.as_str(),
                "custom" | "deepseek" | "codex" | "claude_code"
            ),
            "不支持的服务预设"
        );
        if local_config::is_local(&self.preset) {
            let mut service = self.service.clone();
            local_config::restore_key(&mut service, local_config::read(&self.preset)?)?;
        }
        validate_languages(&self.source_language, &self.target_language)?;
        anyhow::ensure!(
            self.model_options.len() <= 5000
                && self.model_options.iter().all(|name| name.len() <= 512),
            "模型列表过大，请重新获取模型后保存"
        );
        let mut safe = self.clone();
        safe.service.api_key.clear();
        if local_config::is_local(&self.preset) {
            safe.remember_key = false;
        }
        let mut value = serde_json::to_value(safe)?;
        if self.remember_key
            && !local_config::is_local(&self.preset)
            && !self.service.api_key.trim().is_empty()
        {
            value["encrypted_api_key"] =
                serde_json::to_value(protect(self.service.api_key.as_bytes(), true)?)?;
        }
        save_json(path, &value)
    }
}

#[cfg(windows)]
fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI 的密文绑定当前 Windows 用户，返回缓冲区由 LocalFree 释放。
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        anyhow::ensure!(
            ok != 0,
            "无法使用 Windows 加解密密钥，请重新输入或取消记住密钥"
        );
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}

#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("记住密钥仅支持 Windows，请取消勾选后保存")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_save_without_plaintext_key_and_can_forget_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::default();
        settings.service.api_key = "test-secret-key".into();
        settings.service.model = "test-model".into();
        settings.model_options = vec!["first".into(), "second".into()];
        settings.save(&path).unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("test-secret-key")
        );
        assert!(Settings::load(&path).unwrap().service.api_key.is_empty());
        assert_eq!(
            Settings::load(&path).unwrap().model_options,
            vec!["first", "second"]
        );
        #[cfg(windows)]
        {
            settings.remember_key = true;
            settings.save(&path).unwrap();
            assert!(
                !fs::read_to_string(&path)
                    .unwrap()
                    .contains("test-secret-key")
            );
            assert_eq!(
                Settings::load(&path).unwrap().service.api_key,
                "test-secret-key"
            );
            settings.remember_key = false;
            settings.save(&path).unwrap();
            assert!(Settings::load(&path).unwrap().service.api_key.is_empty());
            assert!(
                !fs::read_to_string(&path)
                    .unwrap()
                    .contains("encrypted_api_key")
            );
        }
    }
}
