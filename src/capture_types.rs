//! 后端无关的公共类型与自研后端分发。

use std::sync::Mutex;

use image::RgbaImage;

use crate::backend::{pipewire_screencast, wlr_screencopy, x11};
use crate::window::WindowMatch;

/// DMA-BUF modifier "无效" 常量（与 drm_fourcc.h 的 DRM_FORMAT_MOD_INVALID 一致）。
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
/// DMA-BUF linear modifier 常量。
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// 自研后端类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// 无可用后端（错误汇报用）。
    None,
    /// 自研 PipeWire 客户端（经 xdg-desktop-portal ScreenCast，全 Wayland 合成器）。
    /// 注意：这里只使用 ScreenCast（PipeWire），严禁使用 portal Screenshot。
    PipeWireScreencast,
    /// 自研 wlr-screencopy 协议客户端（wlroots 系合成器，无需门户）。
    WlrScreencopy,
    /// 自研 X11 直接抓取（XComposite / XGetImage）。
    X11,
    /// Windows Graphics Capture（平台公开 API，供 Windows 封装层使用）。
    WindowsWgc,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::None => "none",
            Backend::PipeWireScreencast => "pipewire-screencast",
            Backend::WlrScreencopy => "wlr-screencopy",
            Backend::X11 => "x11",
            Backend::WindowsWgc => "windows-wgc",
        }
    }
}

/// 后端无关的捕获请求。
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    /// 全局逻辑坐标下的捕获矩形；空 = 整屏。
    pub source_geometry: Option<(i32, i32, i32, i32)>,
    /// 首选输出名（尽力而为，如 "HDMI-1"）。
    pub preferred_output: Option<String>,
    /// 捕获整个虚拟桌面为一张图。
    pub all_outputs: bool,
    /// 是否包含鼠标指针。
    pub include_cursor: bool,
    /// 流式后端限帧率，0 = 后端默认。
    pub target_fps: u32,
    /// 忽略早于该时刻（毫秒）的流帧。
    pub minimum_frame_time_ms: u64,
    /// 无头铁律：恒为 false，任何后端不得触发交互授权。
    pub allow_interactive_portal: bool,
    /// 窗口目标（多选）。为空时按屏幕模式截图（全屏/区域/全输出）；
    /// 非空时对每个命中窗口分别截图（每个窗口一张）。
    pub window_matches: Vec<WindowMatch>,
    /// 窗口内组件子区域（相对窗口左上角的 x,y,w,h）。配合 window_matches 使用。
    pub component: Option<(i32, i32, i32, i32)>,
}

impl Default for CaptureRequest {
    fn default() -> Self {
        Self {
            source_geometry: None,
            preferred_output: None,
            all_outputs: false,
            include_cursor: false,
            target_fps: 0,
            minimum_frame_time_ms: 0,
            allow_interactive_portal: false,
            window_matches: Vec::new(),
            component: None,
        }
    }
}

/// 后端无关的捕获结果。
#[derive(Debug)]
pub struct CaptureResult {
    pub image: Option<RgbaImage>,
    pub error: Option<String>,
    /// image 实际表示的全局坐标。
    pub source_geometry: Option<(i32, i32, i32, i32)>,
    /// 实际命中的输出名。
    pub output_name: Option<String>,
    pub backend: Backend,
    pub frame_time_ms: u64,
}

impl CaptureResult {
    pub fn failure(backend: Backend, error: impl Into<String>) -> Self {
        Self {
            image: None,
            error: Some(error.into()),
            source_geometry: None,
            output_name: None,
            backend,
            frame_time_ms: 0,
        }
    }
}

/// 窗口对象信息（供窗口内容抓取）。
#[derive(Debug, Clone)]
pub struct WindowObjectInfo {
    /// 后端窗口标识（X11 为十六进制 XID 字符串）。
    pub id: String,
    /// 窗口全局坐标矩形。
    pub rect: (i32, i32, i32, i32),
}

/// 当前会话适用的自研后端列表（按优先级排序，不含任何系统截图服务）。
///
/// 优先级：wlr-screencopy（免授权、零弹窗）→ PipeWire screencast（其余
/// Wayland）→ X11（原生 X11 会话；XWayland 下 root 抓取不可用，仅作最后兜底，
/// 窗口对象抓取不受影响）。
pub fn available_backends() -> Vec<Backend> {
    let mut backends = Vec::new();
    if wlr_screencopy::available() {
        backends.push(Backend::WlrScreencopy);
    }
    if pipewire_screencast::available() {
        backends.push(Backend::PipeWireScreencast);
    }
    if x11::available() {
        // XWayland（Wayland 会话且 DISPLAY 存在）下 X11 root 抓取不可用，
        // 但 XComposite 窗口抓取仍可用，故保留并排在最后兜底。
        backends.push(Backend::X11);
    }
    backends
}

/// 当前正在使用的流式后端（PipeWire screencast 会话复用）。
static ACTIVE_PIPEWIRE: Mutex<Option<pipewire_screencast::PipeWireSession>> =
    Mutex::new(None);

/// 捕获一帧并返回后端无关结果。
///
/// 后端选择优先级：
///   1. wlr-screencopy（wlroots 系合成器，无需门户，零弹窗）
///   2. PipeWire screencast（其余 Wayland：GNOME / KDE 等）
///   3. X11 自研抓取（X11 会话或 XWayland 回退）
///
/// 失败时依次回退到下一个后端；全部失败返回错误。
pub fn capture_frame(request: &CaptureRequest) -> CaptureResult {
    // 注意：allow_interactive_portal 仅由显式交互流程（--authorize / GUI 集成）
    // 置 true；无头模式（headless/MCP 等）调用方必须保持 false，本核心不校验。
    let mut errors: Vec<String> = Vec::new();

    for backend in available_backends() {
        let result = match backend {
            Backend::WlrScreencopy => wlr_screencopy::capture(request),
            Backend::PipeWireScreencast => {
                let mut guard = ACTIVE_PIPEWIRE
                    .lock()
                    .expect("pipewire session mutex poisoned");
                match pipewire_screencast::capture_with_session(request, guard.as_mut()) {
                    Ok(result) => result,
                    Err(need_restart) => {
                        let _ = need_restart;
                        // 会话失效：销毁重建后重试一次。
                        *guard = None;
                        let mut fresh = pipewire_screencast::PipeWireSession::new();
                        pipewire_screencast::capture_with_session(request, Some(&mut fresh))
                            .unwrap_or_else(|e| CaptureResult::failure(Backend::PipeWireScreencast, e))
                    }
                }
            }
            Backend::X11 => x11::capture(request),
            Backend::WindowsWgc => {
                return CaptureResult::failure(
                    Backend::WindowsWgc,
                    "windows-wgc 由 Windows 封装层提供，本核心暂不直接实现",
                );
            }
            Backend::None => {
                return CaptureResult::failure(Backend::None, "no backend selected");
            }
        };
        if result.image.is_some() {
            return result;
        }
        if let Some(err) = result.error {
            errors.push(format!("{}: {}", backend.name(), err));
        }
    }
    CaptureResult::failure(
        Backend::none(),
        if errors.is_empty() {
            "no capture backend available (need wayland compositor with screencast, or X11)"
                .to_string()
        } else {
            errors.join("\n")
        },
    )
}

/// 停止当前复用的流式后端会话（滚动截图暂停/失败时调用）。
pub fn stop_active_stream() {
    if let Ok(mut guard) = ACTIVE_PIPEWIRE.lock() {
        *guard = None;
    }
}

/// 抓取单个窗口自身内容。不支持或失败返回 None，调用方回退区域抓取。
pub fn capture_window_object_content(
    window: &WindowObjectInfo,
    include_cursor: bool,
) -> Option<RgbaImage> {
    x11::capture_window_content(window, include_cursor).ok()
}

/// 单个窗口的捕获结果。
#[derive(Debug)]
pub struct WindowCapture {
    /// 命中的窗口信息（用于回显匹配结果）。
    pub window: crate::window::WindowInfo,
    /// 匹配时使用的选择器原文（便于多选时区分）。
    pub selector: String,
    pub image: Option<RgbaImage>,
    pub error: Option<String>,
    /// 是否拿到了窗口自身内容（X11 XComposite）；false 表示区域抓取回退。
    pub object_capture: bool,
}

/// 捕获多个指定窗口（每个窗口一张图）。
///
/// - 匹配规则取 `request.window_matches`（可多选）；每个选择器命中所有匹配窗口。
/// - 窗口内容抓取：X11 走 XComposite（含遮挡/最小化窗口真实内容）；GNOME/Wayland
///   回退到全屏帧 + 窗口矩形裁剪（被遮挡窗口内容不可靠，error/object_capture 标注）。
/// - `request.component` 存在时，在每个窗口图上按相对子区域裁剪。
/// - 无头铁律：不建窗口、不弹窗、不干扰用户其他进程。
pub fn capture_windows(request: &CaptureRequest) -> Vec<WindowCapture> {
    let windows = crate::window::list_windows();
    let mut out = Vec::new();

    for selector in &request.window_matches {
        let mut matched = 0;
        for (index, info) in windows.iter().enumerate() {
            if !selector.matches(info, index) {
                continue;
            }
            matched += 1;
            let mut entry = WindowCapture {
                window: info.clone(),
                selector: match selector {
                    WindowMatch::Auto(s)
                    | WindowMatch::Id(s)
                    | WindowMatch::Title(s)
                    | WindowMatch::Class(s)
                    | WindowMatch::Instance(s)
                    | WindowMatch::Process(s) => s.clone(),
                    WindowMatch::Index(i) => i.to_string(),
                    WindowMatch::Pid(p) => p.to_string(),
                },
                image: None,
                error: None,
                object_capture: false,
            };

            // 优先窗口对象抓取（X11 XComposite）。
            let mut image = crate::window::capture_window_content(info);
            if image.is_some() {
                entry.object_capture = true;
            } else {
                // 回退：全屏/区域帧 + 窗口矩形裁剪。
                let mut region_request = request.clone();
                region_request.window_matches.clear();
                region_request.source_geometry = Some(info.geometry);
                region_request.all_outputs = false;
                let result = capture_frame(&region_request);
                match result.image {
                    Some(frame) => image = Some(frame),
                    None => entry.error = result.error,
                }
            }

            // 组件子区域裁剪。
            if let Some((cx, cy, cw, ch)) = request.component {
                if let Some(img) = image.as_ref() {
                    if let Some(cropped) = image::imageops::crop_imm(
                        img,
                        cx as u32,
                        cy as u32,
                        cw as u32,
                        ch as u32,
                    )
                    .to_image()
                    .into()
                    {
                        image = Some(cropped);
                    } else {
                        entry.error = Some("component sub-region is outside the window".to_string());
                    }
                }
            }

            entry.image = image;
            out.push(entry);
        }
        if matched == 0 {
            out.push(WindowCapture {
                window: crate::window::WindowInfo::default(),
                selector: match selector {
                    WindowMatch::Auto(s)
                    | WindowMatch::Id(s)
                    | WindowMatch::Title(s)
                    | WindowMatch::Class(s)
                    | WindowMatch::Instance(s)
                    | WindowMatch::Process(s) => s.clone(),
                    WindowMatch::Index(i) => i.to_string(),
                    WindowMatch::Pid(p) => p.to_string(),
                },
                image: None,
                error: Some("no window matched selector".to_string()),
                object_capture: false,
            });
        }
    }
    out
}

/// 内部辅助：以 Rgba8 编码的图片裁剪（按逻辑坐标与像素坐标换算）。
pub(crate) fn crop_to_geometry(
    image: &RgbaImage,
    stream_geometry: (i32, i32, i32, i32),
    requested: (i32, i32, i32, i32),
) -> Option<RgbaImage> {
    let (sx, sy, sw, sh) = stream_geometry;
    let (rx, ry, rw, rh) = requested;
    if sw <= 0 || sh <= 0 {
        return None;
    }
    // 比例换算：逻辑坐标 -> 图像像素。
    let scale_x = image.width() as f64 / sw as f64;
    let scale_y = image.height() as f64 / sh as f64;
    let left = ((rx - sx).max(0) as f64 * scale_x).round() as u32;
    let top = ((ry - sy).max(0) as f64 * scale_y).round() as u32;
    let right = (((rx + rw - sx).min(sw)) as f64 * scale_x).round() as u32;
    let bottom = (((ry + rh - sy).min(sh)) as f64 * scale_y).round() as u32;
    if right <= left || bottom <= top {
        return None;
    }
    Some(image::imageops::crop_imm(image, left, top, right - left, bottom - top).to_image())
}

#[allow(non_upper_case_globals)]
impl Backend {
    /// 兼容辅助：捕获失败时的后端标签。
    pub(crate) const fn none() -> Self {
        Backend::None
    }
}

#[cfg(test)]
mod tests {
    use super::crop_to_geometry;
    use image::RgbaImage;

    #[test]
    fn crops_region_correctly() {
        let mut img = RgbaImage::new(100, 50);
        for y in 0..50 {
            for x in 0..100 {
                img.put_pixel(x, y, image::Rgba([x as u8, y as u8, 0, 255]));
            }
        }
        let cropped = crop_to_geometry(&img, (0, 0, 100, 50), (10, 10, 20, 10)).expect("crop");
        assert_eq!((cropped.width(), cropped.height()), (20, 10));
        let px = cropped.get_pixel(0, 0);
        assert_eq!(px.0[0], 10);
        assert_eq!(px.0[1], 10);
    }

    #[test]
    fn crop_outside_returns_none() {
        let img = RgbaImage::new(10, 10);
        assert!(crop_to_geometry(&img, (0, 0, 10, 10), (50, 50, 10, 10)).is_none());
    }
}
