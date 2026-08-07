//! 路由层：按桌面/会话类型把捕获请求智能分发到"最轻专用通道"。
//!
//! 架构核心（与 mark-shot 集成层一致）：**没有一条万能重管道，只有一组最轻
//! 专用通道 + 按桌面分发的路由层**。
//!
//! | 场景 | 通道 |
//! | --- | --- |
//! | wlroots（sway/hyprland/niri…） | 自研 wlr-screencopy 客户端（免 portal、免授权、免 PipeWire） |
//! | GNOME/KDE 整屏/区域/流式 | portal ScreenCast + 自研 PipeWire 客户端（唯一合法像素通道） |
//! | 原生 X11 | 自研 XGetImage + XComposite（零授权、窗口级真实内容） |
//! | KDE 窗口级 | KWin ScreenShot2（铁律放宽后启用，静默、精确、零干扰） |
//!
//! 调用方可通过 `CaptureRequest.route`（`RouteMode`）参数化指定路由方案；
//! 也可先调用 [`detect_routing`] 拿到 `RoutingPlan`（含可直接回填 `CaptureRequest`
//! 的路由参数），实现"自动智能感知 + 返回参数指定路由"。

use std::env;

use crate::capture_types::{Backend, RouteMode};

/// 会话/桌面类型（路由决策的基础）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// GNOME Wayland（portal ScreenCast 为主通道）。
    WaylandGnome,
    /// KDE Plasma（portal ScreenCast 整屏/区域 + KWin ScreenShot2 窗口级）。
    WaylandKde,
    /// wlroots 系合成器（sway / hyprland / niri / river / wayfire / labwc…）。
    WaylandWlroots,
    /// 其他 Wayland 合成器。
    WaylandOther,
    /// 原生 X11 会话。
    NativeX11,
    /// 无法确定。
    Unknown,
}

impl SessionKind {
    pub fn name(self) -> &'static str {
        match self {
            SessionKind::WaylandGnome => "wayland-gnome",
            SessionKind::WaylandKde => "wayland-kde",
            SessionKind::WaylandWlroots => "wayland-wlroots",
            SessionKind::WaylandOther => "wayland-other",
            SessionKind::NativeX11 => "x11",
            SessionKind::Unknown => "unknown",
        }
    }
}

/// 路由方案：智能感知的结果，含可直接回填 `CaptureRequest.route` 的参数。
#[derive(Debug, Clone)]
pub struct RoutingPlan {
    /// 识别出的会话/桌面类型。
    pub session: SessionKind,
    /// 按优先级排列的推荐后端（已做能力过滤）。
    pub recommended: Vec<Backend>,
    /// 可直接赋给 `CaptureRequest.route` 的路由参数（`Order` 形式）。
    pub route: RouteMode,
    /// 补充说明（如 XWayland 存在等）。
    pub notes: Vec<String>,
}

fn session_type() -> String {
    env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase()
}

fn current_desktop() -> String {
    env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase()
}

fn has_display() -> bool {
    env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false)
}

fn has_dbus() -> bool {
    env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
}

/// wlroots 系合成器：环境变量或 XDG_CURRENT_DESKTOP 命中。
fn is_wlroots() -> bool {
    const SOCKET_ENVS: &[&str] = &[
        "SWAYSOCK",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "NIRI_SOCKET",
        "WAYFIRE_SOCKET",
        "LABWC_SOCKET",
    ];
    if SOCKET_ENVS.iter().any(|k| env::var_os(k).is_some()) {
        return true;
    }
    let desktop = current_desktop();
    ["sway", "hyprland", "niri", "river", "wayfire", "labwc"]
        .iter()
        .any(|k| desktop.contains(k))
}

/// KDE Plasma 会话。
fn is_kde() -> bool {
    let desktop = current_desktop();
    (desktop.contains("kde") || desktop.contains("plasma")) || env::var_os("KDE_SESSION_VERSION").is_some()
}

/// GNOME 会话。
fn is_gnome() -> bool {
    current_desktop().contains("gnome")
}

/// 静态能力判断（不弹窗、不建会话、不触发任何交互）。
fn capability_ok(b: Backend) -> bool {
    match b {
        Backend::WlrScreencopy => session_type() == "wayland",
        Backend::PipeWireScreencast => session_type() == "wayland" && has_dbus(),
        Backend::X11 => has_display(),
        // KWin ScreenShot2 单独做 DBus 探测（available()），这里不拦截。
        Backend::KwinScreenShot2 => true,
        Backend::WindowsWgc => cfg!(windows),
        Backend::None => false,
    }
}

/// 智能感知当前会话并给出推荐路由方案。
///
/// 返回值可直接使用：`plan.route` 赋给 `CaptureRequest.route` 即把感知到的
/// 路由参数化固定下来；`plan.recommended` 为按优先级排序的推荐后端列表。
pub fn detect_routing() -> RoutingPlan {
    let st = session_type();
    let display = has_display();
    let mut notes: Vec<String> = Vec::new();

    let session = if st == "wayland" {
        if is_wlroots() {
            SessionKind::WaylandWlroots
        } else if is_kde() {
            SessionKind::WaylandKde
        } else if is_gnome() {
            SessionKind::WaylandGnome
        } else {
            SessionKind::WaylandOther
        }
    } else if st == "x11" {
        SessionKind::NativeX11
    } else if display {
        // 无 session type 但有 DISPLAY：视为 X11 会话。
        SessionKind::NativeX11
    } else {
        SessionKind::Unknown
    };

    if st == "wayland" && display {
        notes.push("XWayland 存在：X11 后端仅作最后兜底，窗口对象抓取不受影响".to_string());
    }

    // 桌面感知的推荐顺序：每桌面只走最轻专用通道。
    let mut recommended: Vec<Backend> = match session {
        SessionKind::WaylandWlroots => vec![Backend::WlrScreencopy, Backend::PipeWireScreencast],
        SessionKind::WaylandGnome => vec![Backend::PipeWireScreencast],
        // KDE 整屏/区域/流式：portal ScreenCast（唯一合法像素通道，含授权门）。
        // KWin ScreenShot2 仅用于**窗口级**（`window_object_backends`）或调用方
        // 显式 `Only`/`Prefer` 指定——不进入默认整屏路由，避免绕过 portal 授权
        // 静默抓取整屏。
        SessionKind::WaylandKde => vec![Backend::PipeWireScreencast],
        // 未识别合成器：保持能力探测行为（先尝试 wlr-screencopy，失败快速回退），
        // 不因检测列表缺项而丢掉此前可用的通道。
        SessionKind::WaylandOther => vec![Backend::WlrScreencopy, Backend::PipeWireScreencast],
        SessionKind::NativeX11 => vec![Backend::X11],
        SessionKind::Unknown => vec![
            Backend::WlrScreencopy,
            Backend::PipeWireScreencast,
            Backend::X11,
        ],
    };
    if display && !recommended.contains(&Backend::X11) {
        recommended.push(Backend::X11);
    }
    // 能力过滤（静态、零交互）。
    recommended.retain(|b| capability_ok(*b));

    let route = RouteMode::Order(recommended.clone());
    RoutingPlan {
        session,
        recommended,
        route,
        notes,
    }
}

/// 解析路由模式为有序后端列表（`capture_frame` 依此逐个尝试）。
///
/// - `Auto` → 自动感知的推荐顺序；
/// - `Only(b)` → 仅 `b`；
/// - `Order(v)` → 显式顺序；
/// - `Prefer(b)` → `b` 在前，其余按自动推荐顺序补齐。
///
/// `Only` / `Order` 不触发会话感知（零开销）；`Auto` / `Prefer` 走感知
/// （KDE 的 ScreenShot2 能力探测已缓存，进程内仅一次 DBus 往返）。
pub fn resolve_route(mode: &RouteMode) -> Vec<Backend> {
    match mode {
        RouteMode::Only(b) => vec![*b],
        RouteMode::Order(v) => v.clone(),
        RouteMode::Auto | RouteMode::Prefer(_) => {
            let recommended = detect_routing().recommended;
            match mode {
                RouteMode::Auto => recommended,
                RouteMode::Prefer(b) => {
                    let mut v = vec![*b];
                    v.extend(recommended.into_iter().filter(|x| x != b));
                    v
                }
                _ => unreachable!(),
            }
        }
    }
}

/// 是否 KDE Plasma Wayland 会话（供窗口枚举等模块复用同一判定，避免多份
/// 桌面检测逻辑漂移）。
pub fn is_kde_wayland() -> bool {
    detect_routing().session == SessionKind::WaylandKde
}

/// 是否 GNOME Wayland 会话（复用同一判定）。
pub fn is_gnome_wayland() -> bool {
    detect_routing().session == SessionKind::WaylandGnome
}

/// 窗口"对象级"抓取尝试顺序（`capture_window_object_content` 使用）。
///
/// - KDE Plasma 且 ScreenShot2 可用（`kwin_screenshot2::available()` 已缓存，
///   进程内仅一次 DBus 探测）：`[KwinScreenShot2, X11]`；
/// - 其余：`[X11]`（原生 X11 / XWayland XComposite）。
///
/// 注意：ScreenShot2 在 `detect_routing` 的整屏推荐中**有意不出现**（整屏走
/// portal 授权门），故此处按会话类型直接探测可用性，而非依赖 `recommended`
/// 成员判断（那永远是 false）。
pub fn window_object_backends() -> Vec<Backend> {
    let plan = detect_routing();
    let mut v = Vec::new();
    if plan.session == SessionKind::WaylandKde && crate::backend::kwin_screenshot2::available() {
        v.push(Backend::KwinScreenShot2);
    }
    v.push(Backend::X11);
    v
}

#[cfg(test)]
mod tests {
    use super::{capability_ok, is_kde, is_wlroots};
    use crate::capture_types::Backend;

    #[test]
    fn capability_ok_reflects_environment() {
        // 无环境变量时 X11 能力取决于 DISPLAY；本测试只验证静态分支不 panic。
        let _ = capability_ok(Backend::X11);
        let _ = capability_ok(Backend::None);
        assert!(!capability_ok(Backend::None));
    }

    #[test]
    fn kde_and_wlroots_heuristics() {
        std::env::remove_var("KDE_SESSION_VERSION");
        std::env::set_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME");
        assert!(!is_kde());
        assert!(!is_wlroots());
        std::env::set_var("XDG_CURRENT_DESKTOP", "KDE");
        assert!(is_kde());
        std::env::set_var("XDG_CURRENT_DESKTOP", "sway");
        assert!(is_wlroots());
        std::env::remove_var("XDG_CURRENT_DESKTOP");
        std::env::set_var("SWAYSOCK", "/tmp/sway-ipc.sock");
        assert!(is_wlroots());
        std::env::remove_var("SWAYSOCK");
    }
}
