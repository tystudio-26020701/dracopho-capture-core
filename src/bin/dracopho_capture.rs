//! dracopho-capture —— DracoPho 自研截屏核心命令行入口（验证工具）。
//!
//! 用法：
//!   dracopho-capture --list-backends          列出可用自研后端
//!   dracopho-capture --list-routing           打印当前会话的智能路由方案
//!   dracopho-capture --list-windows           列出窗口（JSON）
//!   dracopho-capture --authorize              交互授权一次（ScreenCast 持久化）
//!   dracopho-capture --capture-to <file> [--region x,y,w,h] [--include-cursor] [--output <name>] [--backend <name>]
//!                                            无头屏幕/区域截图到文件（--output 指定显示器；--backend 指定路由）
//!   dracopho-capture --capture-to <dir> --window <sel> [--window ...] [--window-by auto|id|title|class|index|pid|process] [--component x,y,w,h]
//!                                            无头按窗口/进程截图（多选，输出到目录）
//!
//! 铁律：无头模式（未带 --authorize）绝不弹窗、绝不创建窗口、不干扰用户进程。

use std::process::ExitCode;

use dracopho_capture_core::capture_types::{
    available_backends, Backend, CaptureRequest, RouteMode,
};

fn print_usage() {
    eprintln!(
        "dracopho-capture (DracoPho 自研截屏核心)\n\
         用法:\n  \
         dracopho-capture --list-backends\n  \
         dracopho-capture --list-routing\n  \
         dracopho-capture --list-windows\n  \
         dracopho-capture --list-outputs\n  \
         dracopho-capture --authorize\n  \
         dracopho-capture --capture-to <file|dir> [--region x,y,w,h] [--include-cursor] [--output <name>] [--backend <name>]\n  \
         dracopho-capture --capture-to <dir> --window <sel> [--window <sel>...] [--window-by <mode>] [--component x,y,w,h]\n\
         选项:\n  \
         --list-backends          列出可用自研后端\n  \
         --list-routing           打印当前会话的智能路由方案（SessionKind + 推荐顺序）\n  \
         --list-windows           列出窗口（JSON）\n  \
         --authorize              交互授权一次（ScreenCast 持久化，保存恢复 token）\n  \
         --capture-to <path>      截图输出（文件；窗口模式为目录）\n  \
         --region x,y,w,h         捕获区域（逻辑坐标）\n  \
         --output <name>          首选输出名（如 HDMI-1）；GNOME 多流选屏时按
                                   position/size 匹配该显示器\n  \
         --backend <name>         指定后端路由（only）：pipewire-screencast|wlr-screencopy|x11|kwin-screenshot2\n  \
         --window <sel>           窗口选择器（可重复）；<sel> 匹配 id/标题/class/序号/pid/进程名\n  \
         --window-by <mode>       auto|id|title|class|index|pid|process\n  \
         --component x,y,w,h      窗口内组件子区域\n  \
         --include-cursor         捕获鼠标指针（尽力而为）"
    );
}

/// 解析后端名（大小写不敏感，支持别名）。
fn parse_backend(name: &str) -> Option<Backend> {
    match name.to_lowercase().replace('_', "-").as_str() {
        "pipewire" | "pipewire-screencast" | "screencast" => Some(Backend::PipeWireScreencast),
        "wlr" | "wlr-screencopy" | "screencopy" => Some(Backend::WlrScreencopy),
        "x11" => Some(Backend::X11),
        "kwin" | "kwin-screenshot2" | "kwin-screenshot" => Some(Backend::KwinScreenShot2),
        "windows-wgc" | "wgc" => Some(Backend::WindowsWgc),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::from(2);
    }

    let mut capture_to: Option<String> = None;
    let mut region: Option<(i32, i32, i32, i32)> = None;
    let mut component: Option<(i32, i32, i32, i32)> = None;
    let mut include_cursor = false;
    let mut authorize = false;
    let mut list_backends = false;
    let mut list_windows = false;
    let mut list_outputs = false;
    let mut list_routing = false;
    let mut window_selectors: Vec<String> = Vec::new();
    let mut window_by: Option<String> = None;
    let mut output: Option<String> = None;
    let mut backend: Option<Backend> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        match arg.as_str() {
            "--list-backends" => list_backends = true,
            "--list-windows" => list_windows = true,
            "--list-outputs" => list_outputs = true,
            "--list-routing" => list_routing = true,
            "--authorize" => authorize = true,
            "--include-cursor" => include_cursor = true,
            "--backend" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--backend 缺少后端名参数");
                    return ExitCode::from(2);
                }
                match parse_backend(&args[i]) {
                    Some(b) => backend = Some(b),
                    None => {
                        eprintln!("未知后端: {}", args[i]);
                        return ExitCode::from(2);
                    }
                }
            }
            "--capture-to" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--capture-to 缺少路径参数");
                    return ExitCode::from(2);
                }
                capture_to = Some(args[i].clone());
            }
            "--window" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--window 缺少选择器参数");
                    return ExitCode::from(2);
                }
                window_selectors.push(args[i].clone());
            }
            "--window-by" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--window-by 缺少模式参数");
                    return ExitCode::from(2);
                }
                window_by = Some(args[i].clone());
            }
            "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--output 缺少输出名参数");
                    return ExitCode::from(2);
                }
                output = Some(args[i].clone());
            }
            "--region" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--region 缺少 x,y,w,h 参数");
                    return ExitCode::from(2);
                }
                match parse_rect(&args[i]) {
                    Some(r) => region = Some(r),
                    None => {
                        eprintln!("--region 期望 x,y,w,h 四个正整数");
                        return ExitCode::from(2);
                    }
                }
            }
            "--component" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--component 缺少 x,y,w,h 参数");
                    return ExitCode::from(2);
                }
                match parse_rect(&args[i]) {
                    Some(r) => component = Some(r),
                    None => {
                        eprintln!("--component 期望 x,y,w,h 四个正整数");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("未知参数: {other}");
                print_usage();
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    if list_backends {
        println!("available backends:");
        for backend in available_backends() {
            println!("  - {}", backend.name());
        }
        return ExitCode::SUCCESS;
    }

    if list_routing {
        let plan = dracopho_capture_core::routing::detect_routing();
        println!("session: {}", plan.session.name());
        println!("notes:");
        for n in &plan.notes {
            println!("  - {n}");
        }
        println!("recommended backends (priority order):");
        for b in &plan.recommended {
            println!("  - {}", b.name());
        }
        println!(
            "route param (assign to CaptureRequest.route): {:?}",
            plan.route
        );
        return ExitCode::SUCCESS;
    }

    if list_outputs {
        let outputs = dracopho_capture_core::output::list_outputs();
        println!("outputs: {}", outputs.len());
        for o in outputs.iter() {
            println!(
                "  name={} geo={},{},{}x{}",
                o.name, o.geometry.0, o.geometry.1, o.geometry.2, o.geometry.3
            );
        }
        return ExitCode::SUCCESS;
    }

    if list_windows {
        let windows = dracopho_capture_core::window::list_windows(false);
        println!("count: {}", windows.len());
        for (index, w) in windows.iter().enumerate() {
            let pid = if w.pid > 0 {
                w.pid.to_string()
            } else {
                "-".to_string()
            };
            println!(
                "[{index}] id={} title=\"{}\" class={} instance={} pid={pid} geo={},{},{}x{}",
                if w.id.is_empty() { "-".to_string() } else { w.id.clone() },
                w.title,
                w.class,
                w.instance,
                w.geometry.0,
                w.geometry.1,
                w.geometry.2,
                w.geometry.3,
            );
        }
        return ExitCode::SUCCESS;
    }

    let mut request = CaptureRequest {
        source_geometry: region,
        include_cursor,
        allow_interactive_portal: authorize,
        component,
        preferred_output: output,
        route: match backend {
            Some(b) => RouteMode::Only(b),
            None => RouteMode::Auto,
        },
        ..Default::default()
    };

    // 窗口模式：解析选择器。
    if !window_selectors.is_empty() {
        if let Some(path) = capture_to.as_deref() {
            let is_dir = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
            if !is_dir {
                eprintln!("窗口模式要求 --capture-to 指向已存在目录");
                return ExitCode::from(2);
            }
        }
        let mut matches = Vec::new();
        for sel in &window_selectors {
            match dracopho_capture_core::window::parse_match(sel, window_by.as_deref()) {
                Ok(m) => matches.push(m),
                Err(e) => {
                    eprintln!("选择器 \"{sel}\" 无效: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        request.window_matches = matches;
        return run_window_capture(&request, capture_to.as_deref().unwrap_or("."));
    }

    if authorize {
        // --authorize 的语义是"完成一次 ScreenCast 授权并持久化 token"。
        // 成功判定 = 本次调用拿到了帧且持久化 token 存在：
        //   - 首次授权：弹出选择器，用户同意后 token 从无到有；
        //   - 已有有效 token：静默恢复直接出帧。
        // 注意不能以"token 内容变化"判成功：portal 规范称 restore_token
        // 单次轮换，但 GNOME 50 实证 Start 可返回同一 token 持续有效；
        // 以内容变化判定会让"重新授权"在 token 不轮换时误报失败。
        // 用户拒绝/超时 → capture_frame 失败（image 为 None），如实报错。
        let result = dracopho_capture_core::capture_types::capture_frame(&request);
        let authorized_now = result.image.is_some()
            && dracopho_capture_core::auth::restore_token().is_some();
        if authorized_now {
            eprintln!("已授权。此后无头截图将静默复用该授权。");
            ExitCode::SUCCESS
        } else {
            eprintln!(
                "授权失败: {}",
                result.error.as_deref().unwrap_or("no authorization was granted")
            );
            ExitCode::from(1)
        }
    } else if let Some(path) = capture_to {
        let result = dracopho_capture_core::capture_types::capture_frame(&request);
        match result.image {
            Some(image) => {
                if let Err(e) = image.save(&path) {
                    eprintln!("写入 {path} 失败: {e}");
                    return ExitCode::from(1);
                }
                println!(
                    "captured: {} ({}x{}) via {}",
                    path,
                    image.width(),
                    image.height(),
                    result.backend.name()
                );
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("capture failed: {}", result.error.as_deref().unwrap_or("未知错误"));
                ExitCode::from(1)
            }
        }
    } else {
        eprintln!("未指定操作");
        print_usage();
        ExitCode::from(2)
    }
}

fn parse_rect(text: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut nums = [0i32; 4];
    for (idx, part) in parts.iter().enumerate() {
        nums[idx] = part.trim().parse::<i32>().ok()?;
    }
    if nums[2] <= 0 || nums[3] <= 0 {
        return None;
    }
    Some((nums[0], nums[1], nums[2], nums[3]))
}

fn run_window_capture(request: &CaptureRequest, dir: &str) -> ExitCode {
    let captures = dracopho_capture_core::capture_types::capture_windows(request);
    let mut failed = false;
    for (index, capture) in captures.iter().enumerate() {
        match &capture.image {
            Some(image) => {
                let title = if capture.window.title.is_empty() {
                    format!("window-{index}")
                } else {
                    let t: String = capture
                        .window
                        .title
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '-' })
                        .collect();
                    t
                };
                let path = format!("{dir}/{title}-{index}.png");
                if let Err(e) = image.save(&path) {
                    eprintln!("写入 {path} 失败: {e}");
                    failed = true;
                    continue;
                }
                let mode = if capture.object_capture {
                    "object"
                } else {
                    "region"
                };
                println!(
                    "[{index}] selector={} title=\"{}\" -> {path} ({}x{}) [{mode}]",
                    capture.selector,
                    capture.window.title,
                    image.width(),
                    image.height(),
                );
            }
            None => {
                eprintln!(
                    "[{index}] selector={} title=\"{}\" 失败: {}",
                    capture.selector,
                    capture.window.title,
                    capture.error.as_deref().unwrap_or("未知错误")
                );
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
