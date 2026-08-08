# 01 — Rust 集成 Quick Start

> 面向把 dracopho-capture-core 集成到 Rust 应用（如 mark-shot）的开发者。
> 目标：5 分钟内从零完成一次真实截图。

## 1. 添加依赖

```toml
[dependencies]
dracopho-capture-core = { path = "/path/to/dracopho-capture-core" }
```

**运行时零依赖**（不链接额外系统库，PipeWire/wayland/x11 均动态加载）。
**构建**需 libpipewire 开发包与 clang（libspa-sys 绑定生成，见 README
构建节）：Debian/Ubuntu 执行 `apt install libpipewire-0.3-dev`，并确保
`PKG_CONFIG_PATH` 指向 libpipewire 的 pkgconfig。

## 2. 全屏截图（最小可运行）

```rust
use dracopho_capture_core::capture_types::{capture_frame, CaptureRequest};

fn main() {
    // 首次在 GNOME/KDE 桌面上运行：allow_interactive_portal=true 会弹一次
    // ScreenCast 授权选择器，授权后自动持久化，此后可置回 false。
    let req = CaptureRequest {
        source_geometry: None,
        allow_interactive_portal: true,
        ..Default::default()
    };
    let result = capture_frame(&req);
    if let Some(img) = result.image {
        img.save("screen.png").expect("save");
        println!("captured via {}", result.backend.name());
    } else {
        eprintln!("capture failed: {}", result.error.unwrap_or_default());
    }
}
```

## 3. 结果结构（诚实上报）

`CaptureResult` 不会在失败时静默返回黑图：

```rust
pub struct CaptureResult {
    pub image: Option<RgbaImage>,      // 成功时 Some
    pub error: Option<String>,         // 失败时含可读原因
    pub source_geometry: Option<(i32,i32,i32,i32)>, // 实际坐标
    pub output_name: Option<String>,   // 命中的显示器名
    pub backend: Backend,              // 实际使用的后端
    pub frame_time_ms: u64,            // 帧时间戳
}
```

- 遮挡/最小化窗口、XWayland 无 root 抓取、portal 未授权——都会**如实报错**
  而非返回黑图。

## 4. 常用请求构造

```rust
use dracopho_capture_core::capture_types::{Backend, CaptureRequest, RouteMode};

fn main() {
    // 区域截图（逻辑坐标）
    let req = CaptureRequest {
        source_geometry: Some((0, 0, 800, 600)),
        ..Default::default()
    };

    // 指定显示器（Wayland 按多流 position/size 匹配；X11 按 XRandR 几何）
    let req = CaptureRequest {
        preferred_output: Some("HDMI-1".to_string()),
        ..Default::default()
    };

    // 指定路由：仅用 KWin ScreenShot2（KDE 窗口级/区域显式放宽）
    let req = CaptureRequest {
        source_geometry: Some((0, 0, 400, 300)),
        route: RouteMode::Only(Backend::KwinScreenShot2),
        ..Default::default()
    };
    // 分别传入 capture_frame(&req) 即可
}
```

## 5. 窗口截图

```rust
use dracopho_capture_core::capture_types::{capture_windows, CaptureRequest};
use dracopho_capture_core::window::{parse_match, WindowMatch};

fn main() {
    // 枚举窗口（X11 / GNOME 扩展 / KDE scripting 自动选择）
    for (i, w) in dracopho_capture_core::window::list_windows(true).iter().enumerate() {
        println!("[{i}] title={} class={}", w.title, w.class);
    }

    // 按 class + 标题子串捕获多个窗口（parse_match 返回 Result，用 expect 处理）
    let req = CaptureRequest {
        window_matches: vec![
            WindowMatch::Class("codium".to_string()),
            parse_match("DracoPho", Some("title")).expect("selector"),
        ],
        ..Default::default()
    };
    for c in capture_windows(&req) {
        if let Some(img) = c.image {
            img.save(format!("{}.png", c.window.title)).expect("save");
        }
        // c.object_capture == true 表示拿到窗口自身内容（X11 XComposite / KDE CaptureWindow）
        // false 表示回退区域裁剪（GNOME Wayland 无窗口对象通道，如实标注）
    }
}
```

## 6. 常见坑

| 现象 | 原因 | 解决 |
| --- | --- | --- |
| GNOME 首次截图弹选择器 | 未授权 | 首次 `allow_interactive_portal=true`，之后 false |
| KDE 全屏走 portal 报"requires interactive authorization" | 无 token | 先 `dracopho-capture --authorize` 或首次交互授权 |
| XWayland 下 `--region` 报 root capture 不可用 | XWayland root 无抓取 | 用窗口对象抓取（XComposite）或 ScreenShot2 |
| 窗口 `object_capture=false` | GNOME 无窗口对象通道 | 这是设计行为，区域裁剪 + 如实标注 |

## 7. 完整示例

见 [`examples/integration_demo.rs`](../../examples/integration_demo.rs)——依次演示
路由感知 → 全屏 → 区域 → 多窗口 → 组件 → 指定路由 → 多屏集合。
