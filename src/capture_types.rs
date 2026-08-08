//! 后端无关的公共类型与自研后端分发。

use std::sync::{Arc, Mutex};

use image::RgbaImage;

use crate::backend::{kwin_screenshot2, pipewire_screencast, wlr_screencopy, x11};
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
    /// KWin ScreenShot2 DBus 接口（KDE Plasma 专用，铁律放宽后启用）。
    /// 窗口级/区域/全屏静默抓取（遮挡/最小化窗口真实内容，对标 Spectacle）。
    KwinScreenShot2,
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
            Backend::KwinScreenShot2 => "kwin-screenshot2",
            Backend::WindowsWgc => "windows-wgc",
        }
    }
}

/// 路由模式：后端选择的控制方式。
///
/// 默认 `Auto` = 按桌面类型智能分发（`routing::detect_routing` 的推荐顺序）。
/// 调用方可用 `Only` / `Order` / `Prefer` 参数化指定路由方案，实现"灵活路由
/// 切换指定模式"。路由决策可先调用 `routing::detect_routing()` 拿到
/// `RoutingPlan`（含可直接回填的 `RouteMode` 参数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMode {
    /// 自动：按桌面/会话类型智能分发到"最轻专用通道"。
    Auto,
    /// 仅使用指定后端，失败不自动回退。
    Only(Backend),
    /// 按给定优先级依次尝试（显式回退链）。
    Order(Vec<Backend>),
    /// 优先指定后端，失败后按自动推荐顺序回退。
    Prefer(Backend),
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
    /// 截图时是否请求后端隐藏调用方自身窗口。
    ///
    /// 语义字段：Wayland portal screencast 无 hide-caller-windows 等价项，
    /// GNOME 需集成方在截图前应用层隐藏自身 UI（悬浮球/设置窗等）；KWin 的
    /// hide-caller-windows 因本项目禁用 KWin ScreenShot2 不再涉及。供集成方
    /// 感知并统一处理。
    pub hide_own_windows: bool,
    /// 窗口目标（多选）。为空时按屏幕模式截图（全屏/区域/全输出）；
    /// 非空时对每个命中窗口分别截图（每个窗口一张）。
    pub window_matches: Vec<WindowMatch>,
    /// 窗口内组件子区域（相对窗口左上角的 x,y,w,h）。配合 window_matches 使用。
    pub component: Option<(i32, i32, i32, i32)>,
    /// 路由模式：如何选择后端（默认 `Auto`，按桌面类型智能分发）。
    ///
    /// 可传 `Only` / `Order` / `Prefer` 参数化指定路由方案；推荐先调用
    /// `routing::detect_routing()` 获取 `RoutingPlan`，再回填其 `route` 字段。
    pub route: RouteMode,
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
            hide_own_windows: true,
            window_matches: Vec::new(),
            component: None,
            route: RouteMode::Auto,
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

/// 当前会话适用（能力探测）的自研后端列表（按优先级排序）。
///
/// 优先级：wlr-screencopy（免授权、零弹窗）→ PipeWire screencast（其余
/// Wayland）→ KWin ScreenShot2（KDE 窗口级/区域）→ X11（原生 X11 会话；
/// XWayland 下 root 抓取不可用，仅作最后兜底，窗口对象抓取不受影响）。
/// 此函数只做"能力探测"；实际路由顺序由 `CaptureRequest.route` /
/// `routing::resolve_route` 决定（桌面感知）。
pub fn available_backends() -> Vec<Backend> {
    let mut backends = Vec::new();
    if wlr_screencopy::available() {
        backends.push(Backend::WlrScreencopy);
    }
    if pipewire_screencast::available() {
        backends.push(Backend::PipeWireScreencast);
    }
    if kwin_screenshot2::available() {
        backends.push(Backend::KwinScreenShot2);
    }
    if x11::available() {
        // XWayland（Wayland 会话且 DISPLAY 存在）下 X11 root 抓取不可用，
        // 但 XComposite 窗口抓取仍可用，故保留并排在最后兜底。
        backends.push(Backend::X11);
    }
    backends
}

/// 当前正在使用的流式后端（PipeWire screencast 会话，跨调用共享）。
static ACTIVE_PIPEWIRE: Mutex<Option<Arc<Mutex<pipewire_screencast::PipeWireSession>>>> =
    Mutex::new(None);

/// 获取（必要时创建）进程内共享的 PipeWire 会话。
fn shared_pipewire_session() -> Arc<Mutex<pipewire_screencast::PipeWireSession>> {
    let mut guard = ACTIVE_PIPEWIRE.lock().expect("pipewire session mutex poisoned");
    if guard.is_none() {
        *guard = Some(Arc::new(Mutex::new(pipewire_screencast::PipeWireSession::new())));
    }
    guard.as_ref().unwrap().clone()
}

/// 捕获一帧并返回后端无关结果。
///
/// 路由语义（`request.route`，默认 `Auto`）：
/// - `Auto`：按桌面/会话类型智能分发（`routing::detect_routing`），每桌面
///   只用"最轻专用通道"：wlroots→wlr-screencopy、GNOME/KDE→portal
///   ScreenCast、原生 X11→XGetImage（KDE 窗口级走 KWin ScreenShot2，见
///   `capture_window_object_content`）。
/// - `Only` / `Order` / `Prefer`：调用方参数化指定路由方案，失败按显式或
///   自动推荐顺序回退。
///
/// 多屏幕语义（严禁混淆）：
/// - 多屏幕选择（`all_outputs=true` 的**屏幕集**）→ 请用 [`capture_outputs`]，
///   返回每个屏幕一张图，**不拼接**；
/// - 跨屏幕截图（`all_outputs=true` 整虚拟桌面，仅 X11 原生支持；或显式
///   `source_geometry` 区域）→ 单张组合/裁剪图（允许）。
///   Wayland 后端不支持组合整虚拟桌面，`all_outputs=true` 时会返回明确错误
///   并引导使用 `capture_outputs`。
///
/// 全部后端失败时返回错误。
pub fn capture_frame(request: &CaptureRequest) -> CaptureResult {
    let mut errors: Vec<String> = Vec::new();

    for backend in crate::routing::resolve_route(&request.route) {
        let result = match backend {
            Backend::WlrScreencopy => {
                if request.all_outputs {
                    errors.push(
                        "wlr-screencopy: combined all-outputs capture is not supported on Wayland; use capture_outputs() for the per-screen set"
                            .to_string(),
                    );
                    continue;
                }
                wlr_screencopy::capture(request)
            }
            Backend::PipeWireScreencast => {
                if request.all_outputs {
                    errors.push(
                        "pipewire-screencast: combined all-outputs capture is not supported on Wayland; use capture_outputs() for the per-screen set"
                            .to_string(),
                    );
                    continue;
                }
                let shared = shared_pipewire_session();
                let mut session = shared.lock().unwrap();
                match pipewire_screencast::capture_with_session(request, &mut session) {
                    Ok(result) => result,
                    Err(need_restart) => {
                        let _ = need_restart;
                        // 会话失效：销毁重建后重试一次。
                        drop(session);
                        let mut guard = ACTIVE_PIPEWIRE.lock().unwrap();
                        *guard = None;
                        drop(guard);
                        let shared = shared_pipewire_session();
                        let mut session = shared.lock().unwrap();
                        pipewire_screencast::capture_with_session(request, &mut session)
                            .unwrap_or_else(|e| CaptureResult::failure(Backend::PipeWireScreencast, e))
                    }
                }
            }
            Backend::X11 => x11::capture(request),
            Backend::KwinScreenShot2 => kwin_screenshot2::capture(request),
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

/// 捕获多个显示器，返回**每个屏幕一张图**的集合（屏幕集，不拼接）。
///
/// 语义（多屏幕 vs 跨屏幕，严禁混淆）：
/// - **多屏幕**（`all_outputs=true`，或未指定输出）：对每个显示器各返回一张
///   截图（`CaptureResult.output_name` 标识对应屏幕），**绝不拼接**；
/// - **跨屏幕截图**（显式 `source_geometry` 区域跨越多个显示器；或 X11 原生
///   的整虚拟桌面组合）：返回单张组合/裁剪图（允许，`capture_frame` 处理）。
///
/// 每个屏幕按 `preferred_output` 路由到该屏幕的流。PipeWire 后端每屏幕建立
/// 一次会话（恢复 token 静默复用，含一次 portal 协商 + 首帧等待），因此多屏
/// 请求**串行**捕获各屏，N 屏约 N ×（portal 协商 + 首帧等待）耗时；X11/wlr
/// 后端按输出几何各自抓取。
pub fn capture_outputs(request: &CaptureRequest) -> Vec<CaptureResult> {
    // 显式区域 = 单帧（跨屏时由后端整桌面裁剪组合）。
    if request.source_geometry.is_some() {
        return vec![capture_frame(request)];
    }

    // 目标屏幕名列表。
    let names: Vec<String> = if let Some(name) = request.preferred_output.as_deref() {
        vec![name.to_string()]
    } else {
        crate::output::list_outputs()
            .into_iter()
            .map(|o| o.name)
            .filter(|n| !n.is_empty())
            .collect()
    };

    let mut out = Vec::new();
    if names.is_empty() {
        // 无法枚举输出：退化为单帧（按默认路由）。
        let mut req = request.clone();
        req.all_outputs = false;
        out.push(capture_frame(&req));
        return out;
    }

    for name in names {
        let mut req = request.clone();
        req.all_outputs = false;
        req.preferred_output = Some(name.clone());
        let mut result = capture_frame(&req);
        // 后端对 preferred_output 的 output_name 回填不一致（x11/pipewire 返回
        // None）；capture_outputs 的契约是"每个 result 用 output_name 标识屏幕"，
        // 故在此统一补齐，保证调用方能按屏幕名区分。
        if result.output_name.is_none() {
            result.output_name = Some(name.clone());
        }
        // PipeWire 会话按 preferred_output 绑定单流：多屏之间重置共享会话，
        // 让每个屏幕用新会话匹配自身的流（恢复 token 静默）。
        if result.backend == Backend::PipeWireScreencast {
            stop_active_stream();
        }
        out.push(result);
    }
    out
}

/// 停止当前复用的流式后端会话（滚动截图暂停/失败时调用）。
pub fn stop_active_stream() {
    if let Ok(mut guard) = ACTIVE_PIPEWIRE.lock() {
        *guard = None;
    }
}

/// 流式捕获（滚动截图逐帧拉取 / 录制连续帧）。
///
/// 持有进程内共享的 PipeWire screencast 会话：`start_stream` 时若尚未授权，
/// 按授权策略弹一次选择器或静默恢复；此后 `next_frame` 持续返回最新帧。
pub struct Stream {
    session: Arc<Mutex<pipewire_screencast::PipeWireSession>>,
    target_fps: u32,
    last_frame_ms: Mutex<u64>,
}

/// 启动一个流式捕获会话。
///
/// - 返回的 `Stream` 可反复 `next_frame`。
/// - 授权语义与 `capture_frame` 一致：首次集成方传 `allow_interactive_portal=true`
///   触发一次授权，此后同进程静默。
/// - 流式仅由 PipeWire screencast 提供；若 `request.route` 显式排除该后端则报错。
pub fn start_stream(request: &CaptureRequest) -> Result<Stream, String> {
    let backends = crate::routing::resolve_route(&request.route);
    if !backends.iter().any(|b| *b == Backend::PipeWireScreencast) {
        return Err(
            "streaming capture requires the pipewire-screencast backend, but the route excludes it"
                .to_string(),
        );
    }
    let session = shared_pipewire_session();
    {
        let mut guard = session.lock().unwrap();
        pipewire_screencast::ensure_started(request, &mut guard)?;
    }
    Ok(Stream {
        session,
        target_fps: request.target_fps,
        last_frame_ms: Mutex::new(0),
    })
}

impl Stream {
    /// 拉取下一帧。
    ///
    /// - `min_frame_time_ms`：只返回到达时间 ≥ 该值的帧（滚动隐藏自身 UI 后，
    ///   用 `now+delay` 丢弃陈旧帧）。
    /// - `timeout_ms`：等待上限；超时返回 `None`。
    /// - `target_fps` 节流：相邻两次返回间隔 ≥ 1000/fps 毫秒（录制限帧）。
    /// - 返回 `(RgbaImage, frame_time_ms)`；帧时间戳供滚动/录制时间线使用。
    pub fn next_frame(
        &self,
        min_frame_time_ms: u64,
        timeout_ms: u64,
    ) -> Result<Option<(image::RgbaImage, u64)>, String> {
        if self.target_fps > 0 {
            let interval = 1000u64 / self.target_fps.max(1) as u64;
            let last = *self.last_frame_ms.lock().unwrap();
            if last > 0 {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let elapsed = now_ms.saturating_sub(last);
                if elapsed < interval {
                    std::thread::sleep(std::time::Duration::from_millis(
                        (interval - elapsed).min(timeout_ms),
                    ));
                }
            }
        }
        let guard = self.session.lock().unwrap();
        let frame = pipewire_screencast::next_frame(&guard, min_frame_time_ms, timeout_ms)?;
        if let Some((_, t)) = frame.as_ref() {
            *self.last_frame_ms.lock().unwrap() = *t;
        }
        Ok(frame)
    }

    /// 结束流式捕获（释放共享会话，滚动结束/失败时调用）。
    pub fn stop(&self) {
        stop_active_stream();
    }
}

/// 抓取单个窗口自身内容。不支持或失败返回 None，调用方回退区域抓取。
///
/// 按路由分发（`routing::window_object_backends`）：
/// - KDE Plasma：KWin ScreenShot2 CaptureWindow（原生 Wayland 窗口真实内容），
///   再回退 X11 XComposite（XWayland 窗口）；
/// - 其余桌面：X11 XComposite（原生 X11 / XWayland）。
pub fn capture_window_object_content(
    window: &WindowObjectInfo,
    include_cursor: bool,
) -> Option<RgbaImage> {
    for backend in crate::routing::window_object_backends() {
        let image = match backend {
            Backend::X11 => x11::capture_window_content(window, include_cursor).ok(),
            Backend::KwinScreenShot2 => {
                match kwin_screenshot2::capture_window_content(window, include_cursor) {
                    Ok(img) => Some(img),
                    Err(e) => {
                        // 仅调试输出：CaptureWindow 失败属正常回退路径。
                        if std::env::var("DRACOPHO_CAPTURE_DEBUG")
                            .map(|v| !v.is_empty() && v != "0")
                            .unwrap_or(false)
                        {
                            eprintln!(
                                "dracopho-capture: kwin CaptureWindow failed for id={}: {e}",
                                window.id
                            );
                        }
                        None
                    }
                }
            }
            _ => None,
        };
        if image.is_some() {
            return image;
        }
    }
    None
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
    // 窗口捕获需能定位 PID/进程（含最小化窗口），与 C++ headless 一致。
    let windows = crate::window::list_windows(true);
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
