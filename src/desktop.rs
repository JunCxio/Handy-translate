use anyhow::{Context, Result};
use std::{
    cell::Cell,
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::{run_return::EventLoopExtRunReturn, windows::WindowExtWindows},
    window::{Icon, WindowBuilder},
};
use wry::{DragDropEvent, NewWindowResponse, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

fn local_navigation(url: &str, origin: &str) -> bool {
    url == origin || url.starts_with(&format!("{origin}/"))
}

fn drop_message(event: DragDropEvent) -> Option<serde_json::Value> {
    match event {
        DragDropEvent::Enter { .. } => Some(serde_json::json!({"kind":"enter"})),
        DragDropEvent::Leave => Some(serde_json::json!({"kind":"leave"})),
        DragDropEvent::Drop { paths, .. } => Some(serde_json::json!({
            "kind":"drop",
            "paths":paths.iter().map(|path|path.to_string_lossy()).collect::<Vec<_>>()
        })),
        _ => None,
    }
}

pub fn run(url: &str, data_dir: &Path, task_succeeded: Arc<AtomicBool>) -> Result<()> {
    let mut event_loop = EventLoopBuilder::<serde_json::Value>::with_user_event().build();
    let drag_proxy = event_loop.create_proxy();
    let window = Rc::new(
        WindowBuilder::new()
            .with_title("Handy-translate")
            .with_window_icon(Some(
                Icon::from_rgba(include_bytes!("../assets/icon.rgba").to_vec(), 64, 64)
                    .context("无法加载应用图标")?,
            ))
            .with_inner_size(LogicalSize::new(1280.0, 900.0))
            .with_min_inner_size(LogicalSize::new(900.0, 640.0))
            .build(&event_loop)
            .context("无法创建桌面窗口")?,
    );
    let success_icon = Icon::from_rgba(include_bytes!("../assets/success.rgba").to_vec(), 32, 32)
        .context("无法加载处理成功图标")?;
    let mut success_shown = false;
    let origin = url
        .split("/#")
        .next()
        .context("本机服务地址无效")?
        .to_owned();
    let download_origin = format!("blob:{origin}/");
    let dialog_window = window.clone();
    let downloading = Rc::new(Cell::new(false));
    let started = downloading.clone();
    let mut context = WebContext::new(Some(data_dir.join("webview")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(url)
        .with_devtools(false)
        .with_browser_accelerator_keys(false)
        .with_navigation_handler(move |next| local_navigation(&next, &origin))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_drag_drop_handler(move |event| {
            if let Some(message) = drop_message(event) {
                let _ = drag_proxy.send_event(message);
            }
            true
        })
        .with_download_started_handler(move |source, destination| {
            started.set(false);
            if !source.starts_with(&download_origin) {
                return false;
            }
            match rfd::FileDialog::new()
                .set_parent(dialog_window.as_ref())
                .set_title("导出译文")
                .add_filter("JSON 译文文件", &["json"])
                .set_file_name("translations.json")
                .save_file()
            {
                Some(path) => {
                    *destination = path;
                    started.set(true);
                    true
                }
                None => false,
            }
        })
        .with_download_completed_handler(move |_, _, success| {
            if downloading.replace(false) && !success {
                rfd::MessageDialog::new()
                    .set_title("导出失败")
                    .set_description("译文文件未能保存，请检查目录权限和磁盘空间后重试。")
                    .set_level(rfd::MessageLevel::Error)
                    .show();
            }
        })
        .build(window.as_ref())
        .context("无法创建应用界面，请安装 Microsoft Edge WebView2 Runtime 后重试")?;

    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));
        let succeeded = task_succeeded.load(Ordering::Acquire);
        if succeeded != success_shown {
            window.set_overlay_icon(succeeded.then_some(&success_icon));
            success_shown = succeeded;
        }
        if let Event::UserEvent(message) = &event {
            if let Err(error) = webview.evaluate_script(&format!(
                "window.dispatchEvent(new CustomEvent('game-file-drag', {{detail:{message}}}));"
            )) {
                eprintln!("无法传递拖入的游戏：{error}");
            }
        }
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_drop_preserves_native_paths_and_ignores_pointer_motion() {
        let path = std::path::PathBuf::from("D:\\Games\\ゲーム 'test'\\Game.exe");
        let message = drop_message(DragDropEvent::Drop {
            paths: vec![path.clone()],
            position: (10, 20),
        })
        .unwrap();
        assert_eq!(message["kind"], "drop");
        assert_eq!(message["paths"][0], path.to_str().unwrap());
        assert_eq!(drop_message(DragDropEvent::Leave).unwrap()["kind"], "leave");
        assert!(drop_message(DragDropEvent::Over { position: (10, 20) }).is_none());
    }

    #[test]
    fn success_overlay_icon_loads_from_the_shipped_pixels() {
        let pixels = include_bytes!("../assets/success.rgba");
        assert_eq!(pixels.len(), 32 * 32 * 4);
        Icon::from_rgba(pixels.to_vec(), 32, 32).unwrap();
    }

    #[test]
    fn navigation_stays_in_the_application() {
        let origin = "http://127.0.0.1:12345";
        assert!(local_navigation(origin, origin));
        assert!(local_navigation(&format!("{origin}/#token"), origin));
        for other in [
            "https://example.com/",
            "file:///C:/Windows/",
            "http://127.0.0.1:12345.evil.example/",
            "http://127.0.0.1:123456/",
        ] {
            assert!(!local_navigation(other, origin));
        }
    }
}
