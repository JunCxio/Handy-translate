use crate::assets;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Status {
    pub language: String,
    pub available: bool,
    pub installable: bool,
    pub running: bool,
    pub reboot_required: bool,
    pub message: String,
}

pub fn supports_install(language: &str) -> bool {
    cfg!(windows) && language.split('-').next() == Some("ja")
}

pub fn check(language: &str) -> Status {
    let result = assets::check_local_ocr(language);
    Status {
        language: language.into(),
        available: result.is_ok(),
        installable: result.is_err() && supports_install(language),
        running: false,
        reboot_required: false,
        message: match result {
            Ok(()) => "本地 OCR 已就绪，可以识别当前源语言。".into(),
            Err(error) => format!("{error:#}"),
        },
    }
}

#[cfg(windows)]
const INSTALL_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
try {
    $process = Start-Process -FilePath "$env:SystemRoot\System32\dism.exe" -ArgumentList @('/Online','/Add-Capability','/CapabilityName:Language.OCR~~~ja-JP~0.0.1.0','/NoRestart','/Quiet') -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    [Console]::Out.Write($process.ExitCode)
} catch {
    if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) {
        [Console]::Out.Write(1223)
    } else {
        exit 1
    }
}
"#;

fn install_result(output: &[u8]) -> Result<bool> {
    let code: i64 = std::str::from_utf8(output)?
        .trim()
        .parse()
        .context("Windows 安装器未返回有效结果，请重新检测 OCR 状态")?;
    ensure!(
        (i32::MIN as i64..=u32::MAX as i64).contains(&code),
        "Windows 安装器返回了无效状态"
    );
    match code as u32 {
        0 => Ok(false),
        3010 => Ok(true),
        1223 => bail!("已取消系统管理员授权，未开始安装。可以再次点击安装。"),
        value => bail!(
            "日语 OCR 包安装失败（代码 0x{value:08X}）。请检查网络和 Windows 更新是否可用；受组织更新策略限制时需联系管理员。详情见 Windows 的 Logs\\DISM\\dism.log。"
        ),
    }
}

#[cfg(windows)]
pub fn install() -> Result<bool> {
    use std::os::windows::process::CommandExt;
    let windows = std::path::PathBuf::from(
        std::env::var_os("SystemRoot").context("无法定位 Windows 系统目录")?,
    );
    let output =
        std::process::Command::new(windows.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .args(["-NoProfile", "-NonInteractive", "-Command", INSTALL_SCRIPT])
            .creation_flags(0x08000000)
            .output()
            .context("无法启动 Windows 组件安装器")?;
    ensure!(
        output.status.success(),
        "无法启动系统安装，请检查系统权限和 Windows PowerShell 是否可用，再重新检测"
    );
    install_result(&output.stdout)
}

#[cfg(not(windows))]
pub fn install() -> Result<bool> {
    bail!("OCR 包安装当前需要 Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_handles_success_reboot_cancellation_and_system_errors() {
        assert!(!install_result(b"0\r\n").unwrap());
        assert!(install_result(b"3010").unwrap());
        assert!(
            install_result(b"1223")
                .unwrap_err()
                .to_string()
                .contains("已取消")
        );
        assert!(
            install_result(b"-2146498220")
                .unwrap_err()
                .to_string()
                .contains("0x800F0954")
        );
        assert!(install_result(b"unexpected").is_err());
        assert!(install_result(b"4294967296").is_err());
        assert!(!supports_install("en"));
        assert!(!supports_install("ja'; exit 0; #"));
        #[cfg(windows)]
        {
            assert!(supports_install("ja-JP"));
            for required in [
                "Language.OCR~~~ja-JP~0.0.1.0",
                "/NoRestart",
                "-Verb RunAs",
                "-WindowStyle Hidden",
                "-Wait -PassThru",
            ] {
                assert!(INSTALL_SCRIPT.contains(required));
            }
        }
    }
}
