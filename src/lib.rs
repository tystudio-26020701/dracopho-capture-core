//! dracopho-capture-core —— DracoPho 自研截屏核心（Rust）。
//!
//! # 铁律
//! - 严禁调用任何"系统自带截图"服务（xdg-desktop-portal Screenshot、
//!   GNOME Shell screenshot_area、KWin ScreenShot2）。
//! - 仅使用开源标准协议 + 自研客户端（PipeWire screencast、wlr-screencopy）
//!   或自研直接抓取（X11 XComposite/XGetImage）。
//! - 无头模式严禁弹窗/建窗/交互授权。

pub mod capture_types;

pub mod auth;

pub mod egl_dmabuf;

pub mod window;

pub mod output;

pub mod backend {
    pub mod pipewire_screencast;
    pub mod wlr_screencopy;
    pub mod x11;
}
