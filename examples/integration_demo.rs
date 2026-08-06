//! 库 API 集成示例：常驻进程内一次授权后连续执行多种截图模式。
//!
//! 这是**库**的正确用法演示：ScreenCast 授权会话由进程内静态复用（首次授权
//! 后同进程后续截图静默）。跨进程的 restore token 在 GNOME 上是一次性的，
//! 因此集成方（mark-shot 常驻托盘/守护）应持有本库的会话，而不是每次新进程。
//!
//! 用法（先在 GNOME 桌面运行一次 --authorize，然后本示例无需再授权弹窗）：
//!   dracopho-capture-demo <out-dir>
//!
//! 依次演示：全屏 → 区域 → 多窗口 → 组件子区域。

use std::process::ExitCode;

use dracopho_capture_core::capture_types::{
    capture_frame, capture_windows, CaptureRequest,
};
use dracopho_capture_core::window::{parse_match, WindowMatch};

fn main() -> ExitCode {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/dracopho-demo".to_string());
    std::fs::create_dir_all(&dir).expect("create output dir");

    // 1. 全屏。
    let req = CaptureRequest {
        source_geometry: None,
        allow_interactive_portal: true, // 仅首次（无 token 时）会弹一次授权
        ..Default::default()
    };
    let result = capture_frame(&req);
    match result.image {
        Some(img) => {
            let path = format!("{dir}/01-fullscreen.png");
            img.save(&path).expect("save");
            println!("01 全屏: {path} ({}x{})", img.width(), img.height());
        }
        None => eprintln!("01 全屏失败: {}", result.error.unwrap_or_default()),
    }

    // 2. 区域（当前窗口区域）。
    let req = CaptureRequest {
        source_geometry: Some((0, 0, 800, 600)),
        ..Default::default()
    };
    let result = capture_frame(&req);
    match result.image {
        Some(img) => {
            let path = format!("{dir}/02-region.png");
            img.save(&path).expect("save");
            println!("02 区域 0,0,800x600: {path} ({}x{})", img.width(), img.height());
        }
        None => eprintln!("02 区域失败: {}", result.error.unwrap_or_default()),
    }

    // 3. 多窗口（按 class / 标题子串匹配）。
    let windows = dracopho_capture_core::window::list_windows(false);
    println!("03 窗口列表: {}", windows.len());
    for (i, w) in windows.iter().enumerate() {
        println!("   [{i}] title=\"{}\" class={} geo={},{},{}x{}",
            w.title, w.class, w.geometry.0, w.geometry.1, w.geometry.2, w.geometry.3);
    }

    let req = CaptureRequest {
        window_matches: vec![
            WindowMatch::Class("codium".to_string()),
            WindowMatch::Title("DracoPho".to_string()),
        ],
        ..Default::default()
    };
    for (i, capture) in capture_windows(&req).iter().enumerate() {
        match &capture.image {
            Some(img) => {
                let path = format!("{dir}/03-window-{i}.png");
                img.save(&path).expect("save");
                let mode = if capture.object_capture { "object" } else { "region" };
                println!("   窗口[{i}] \"{}\" -> {path} ({}x{}) [{mode}]", capture.window.title, img.width(), img.height());
            }
            None => {
                eprintln!("   窗口[{i}] \"{}\" 失败: {}", capture.window.title, capture.error.as_deref().unwrap_or("未知"));
            }
        }
    }

    // 4. 组件子区域（窗口内 200x120）。
    let req = CaptureRequest {
        window_matches: vec![parse_match("codium", Some("class")).expect("selector")],
        component: Some((0, 0, 200, 120)),
        ..Default::default()
    };
    for (i, capture) in capture_windows(&req).iter().enumerate() {
        match &capture.image {
            Some(img) => {
                let path = format!("{dir}/04-component-{i}.png");
                img.save(&path).expect("save");
                println!("   组件[{i}] \"{}\" -> {path} ({}x{})", capture.window.title, img.width(), img.height());
            }
            None => eprintln!("   组件[{i}] 失败: {}", capture.error.as_deref().unwrap_or("未知")),
        }
    }

    ExitCode::SUCCESS
}
