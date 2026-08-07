//! dracopho-capture-core —— DracoPho 自研截屏核心（Rust）。
//!
//! # 设计
//! - 开源标准协议 + 自研客户端（PipeWire screencast、wlr-screencopy）或自研
//!   直接抓取（X11 XComposite/XGetImage）；KDE 窗口级走 KWin ScreenShot2
//!   （铁律放宽后启用，对标 Spectacle）。
//! - 无头模式严禁弹窗/建窗/交互授权（交互授权仅在 `allow_interactive_portal`
//!   显式为 true 时触发一次）。
//! - 路由层（`routing`）：按桌面类型把请求智能分发到"最轻专用通道"；调用方
//!   可用 `CaptureRequest.route`（`RouteMode`）参数化指定路由方案。

pub mod capture_types;

pub mod auth;

pub mod egl_dmabuf;

pub mod window;

pub mod output;

pub mod routing;

pub mod backend {
    pub mod pipewire_screencast;
    pub mod wlr_screencopy;
    pub mod x11;
    pub mod kwin_screenshot2;
    pub mod kwin_windows;
}
