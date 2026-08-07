//! 自研 PipeWire screencast 后端。
//!
//! 仅使用 xdg-desktop-portal **ScreenCast**（PipeWire 流）+ 自研 PipeWire 客户端。
//! 严禁调用 portal Screenshot / GNOME screenshot_area / KWin ScreenShot2。
//!
//! 无头铁律：`allow_interactive_portal=false` 时绝不调用 portal `Start`
//! （该调用会触发合成器授权选择器弹窗），而是直接报错退出。

use std::env;
use std::os::fd::OwnedFd;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use pipewire as pw;
use pw::properties::properties;
use pw::spa::{self, pod};
use tokio::runtime::Runtime;

use crate::capture_types::{
    CaptureRequest, CaptureResult, Backend, DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR,
};

/// 调试日志（DRACOPHO_CAPTURE_DEBUG=1 时输出到 stderr）。
fn debug_log(msg: &str) {
    if env::var("DRACOPHO_CAPTURE_DEBUG")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        eprintln!("dracopho-capture: {msg}");
    }
}

use pw::spa::pod::{ChoiceValue, Object, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id as SpaId};

/// PipeWire 流线程共享状态。
struct SharedState {
    latest: Mutex<Option<image::RgbaImage>>,
    /// 最新帧的到达时间（毫秒，单调时钟）。用于陈旧帧过滤与帧率节流。
    latest_time_ms: Mutex<u64>,
    error: Mutex<Option<String>>,
    info: Mutex<Option<spa::param::video::VideoInfoRaw>>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            latest: Mutex::new(None),
            latest_time_ms: Mutex::new(0),
            error: Mutex::new(None),
            info: Mutex::new(None),
        })
    }
}

/// 是否可能可用：Wayland 会话 + DBus 会话总线可用。
pub fn available() -> bool {
    let session_type = env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
    let has_dbus = env::var("DBUS_SESSION_BUS_ADDRESS").is_ok();
    session_type == "wayland" && has_dbus
}

/// 复用的 PipeWire screencast 会话。
pub struct PipeWireSession {
    node_id: u32,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    cursor_included: bool,
    started: bool,
    handle: Option<thread::JoinHandle<()>>,
    stop_tx: Option<mpsc::Sender<()>>,
    state: Option<Arc<SharedState>>,
}

impl Default for PipeWireSession {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeWireSession {
    pub fn new() -> Self {
        Self {
            node_id: 0,
            position: None,
            size: None,
            cursor_included: false,
            started: false,
            handle: None,
            stop_tx: None,
            state: None,
        }
    }
}

/// 获取 X11 root window 的 portal 窗口标识符（`x11:XID` 格式）。
///
/// 供交互授权时作为 ScreenCast `Start` 的父窗口：GNOME 后端据此把授权
/// 对话框关联到当前显示器。X11 不可用（纯 Wayland 无 DISPLAY）时返回 None。
fn x11_root_window_identifier() -> Option<ashpd::WindowIdentifier> {
    use x11rb::connection::Connection as X11ConnectionTrait;
    let conn = crate::backend::x11::connection().ok()?;
    let root = conn.setup().roots.first()?.root;
    Some(ashpd::WindowIdentifier::from_xid(root as u64))
}
pub(crate) fn capture_with_session(
    request: &CaptureRequest,
    session: &mut PipeWireSession,
) -> Result<CaptureResult, String> {
    ensure_started(request, session)?;

    match next_frame(session, request.minimum_frame_time_ms, 5000)? {
        Some((image, frame_time)) => {
            // 按请求区域裁剪（流几何 = portal 报告的 position/size）。
            let mut out = image;
            if let Some((rx, ry, rw, rh)) = request.source_geometry {
                if let (Some((sx, sy)), Some((sw, sh))) = (session.position, session.size) {
                    if let Some(cropped) = crate::capture_types::crop_to_geometry(
                        &out,
                        (sx, sy, sw, sh),
                        (rx, ry, rw, rh),
                    ) {
                        out = cropped;
                    }
                }
            }
            Ok(CaptureResult {
                image: Some(out),
                error: None,
                source_geometry: request.source_geometry,
                output_name: None,
                backend: Backend::PipeWireScreencast,
                frame_time_ms: frame_time,
            })
        }
        None => Err("pipewire screencast did not produce a frame within 5s".to_string()),
    }
}

/// 确保会话已启动（首次按授权策略启动；token 存在时静默恢复）。
pub(crate) fn ensure_started(
    request: &CaptureRequest,
    session: &mut PipeWireSession,
) -> Result<(), String> {
    if session.started {
        return Ok(());
    }
    // 交互模式：启动会话（会弹一次授权选择器）。
    // 无头模式（铁律严禁弹窗）：仅当存在持久化恢复 token 时才尝试静默恢复；
    // 无 token 直接报错，绝不触发选择器。
    if !request.allow_interactive_portal {
        let token = crate::auth::restore_token();
        if token.is_none() {
            return Err(
                "portal screencast requires interactive authorization; run once with the GUI to grant it"
                    .to_string(),
            );
        }
        // 无头模式还必须静默校验 token 仍可恢复：portal 前端在 token 失效时
        // 会忽略它并正常弹选择器（这正是"无头后台截图干扰用户"的根源）。
        // 直接查询权限存储做预检（auth::verify_restore_token，库内部自动执行；
        // 集成方也可主动调用），绝不调用会弹窗的 Start。
        // 预检结论处理：
        //   - Ok(true)  确认有效 → 正常静默恢复；
        //   - Ok(false) 确认失效 → 立即失败（绝不调用会弹选择器的 Start）；
        //   - Err        预检本身失败（DBus 抖动/解析失败，无法确认）→ 视为
        //     不确定，退化为带 10s 防线的一次 Start 尝试（宁可短暂失败，也不
        //     因预检误判把本来能静默恢复的部署硬性卡死）。
        match crate::auth::verify_restore_token(token.as_deref().unwrap()) {
            Ok(true) => {}
            Ok(false) => {
                return Err(
                    "screencast restore token is no longer valid; re-run --authorize".to_string(),
                );
            }
            Err(e) => {
                debug_log(&format!(
                    "restore-token preflight could not be verified ({e}); proceeding with a guarded Start attempt"
                ));
            }
        }
    }
    session.start_session(request)
}

/// 等待下一帧（滚动截图逐帧拉取 / 单帧捕获共用）。
///
/// - `min_frame_time_ms`：陈旧帧过滤——只返回到达时间 ≥ 该值的帧。
/// - 返回 `Some((image, frame_time_ms))`；超时无新帧返回 `None`；流错误返回 Err。
pub(crate) fn next_frame(
    session: &PipeWireSession,
    min_frame_time_ms: u64,
    timeout_ms: u64,
) -> Result<Option<(image::RgbaImage, u64)>, String> {
    let state = session.state.as_ref().ok_or("pipewire state missing")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Some(err) = state.error.lock().unwrap().clone() {
            return Err(err);
        }
        let (image, time) = {
            let img = state.latest.lock().unwrap().clone();
            let t = *state.latest_time_ms.lock().unwrap();
            (img, t)
        };
        if let Some(image) = image {
            if time >= min_frame_time_ms {
                return Ok(Some((image, time)));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(std::time::Duration::from_millis(30));
    }
}

impl Drop for PipeWireSession {
    fn drop(&mut self) {
        self.stop();
    }
}

impl PipeWireSession {
    /// 启动 portal ScreenCast 会话并连接 PipeWire 流。
    fn start_session(&mut self, request: &CaptureRequest) -> Result<(), String> {
        let rt = Runtime::new().map_err(|e| format!("failed to create async runtime: {e}"))?;

        // 授权 token：已授权（--authorize）后无头模式可静默恢复会话。
        let restore_token = crate::auth::restore_token();

        // 交互授权：对话框弹出后由用户决定，**不做硬编码超时**。portal 的
        // Request 语义就是"弹出对话框、等待用户响应"（OBS / GNOME 截图工具
        // 同款），用户点选后自然返回，想取消直接 Ctrl+C。
        // 唯一要防的是"对话框根本弹不出来"——无 GUI 环境（无 DISPLAY 且无
        // WAYLAND_DISPLAY）时对话框不可能显示，等待毫无意义，立即失败返回，
        // 让调用方（库 / CLI）快速拿到失败信息。
        // 无头恢复：token 失效已被 verify_restore_token 提前拦截（毫秒级），
        // 这里的 10s 仅作"首帧迟迟未到"的最后防线，不是授权等待。
        let portal_timeout: Option<std::time::Duration> = if request.allow_interactive_portal {
            let has_gui = std::env::var("DISPLAY")
                .map(|d| !d.is_empty())
                .unwrap_or(false)
                || std::env::var("WAYLAND_DISPLAY")
                    .map(|d| !d.is_empty())
                    .unwrap_or(false);
            if !has_gui {
                return Err(
                    "interactive authorization requires a GUI session (DISPLAY and WAYLAND_DISPLAY are \
                     both missing): the portal dialog cannot be shown — run --authorize from the desktop \
                     session, or use headless mode with an already-saved restore token"
                        .to_string(),
                );
            }
            None
        } else {
            Some(std::time::Duration::from_secs(10))
        };

        // portal 协商（tokio 运行时内）。交互路径无超时（等用户操作），
        // 无头路径带 10s 防线。
        let portal_result = rt.block_on(async {
            let operation = async {
                let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
                let session = proxy
                    .create_session()
                    .await
                    .map_err(|e| format!("ScreenCast CreateSession failed: {e}"))?;
                debug_log("portal CreateSession done");

                // 光标策略：请求包含鼠标时优先 Embedded，否则 Hidden。
                let available = proxy
                    .available_cursor_modes()
                    .await
                    .map_err(|e| e.to_string())?;
                let cursor_mode = if request.include_cursor
                    && available.contains(CursorMode::Embedded)
                {
                    CursorMode::Embedded
                } else {
                    CursorMode::Hidden
                };

                // 持久化授权（persist_mode=EXPLICITLY_REVOKED）：无论交互授权还是
                // 无头恢复都必须保持持久化，否则授权不会跨会话保留，下次恢复
                // 又失效弹窗。
                //
                // portal 规范称 restore_token 单次轮换（Start 返回新 token）；
                // GNOME 50 实证同一 token 可持续有效。无论哪种行为，统一保存
                // Start 返回的 token 即可（永久生效，跨进程、跨重启，直到
                // portal 权限被撤销）。
                let persist_mode = PersistMode::ExplicitlyRevoked;
                let restore = restore_token.as_deref();

                // multiple=true：交互选源一次即可选中多个显示器，Start 返回多个流，
                // 每个流带 position/size 标识对应显示器（与 GNOME/Ubuntu 选屏一致）；
                // 之后按 preferred_output 在流中匹配目标显示器，未指定时取第一个。
                proxy
                    .select_sources(
                        &session,
                        cursor_mode,
                        SourceType::Monitor.into(),
                        true,
                        restore,
                        persist_mode,
                    )
                    .await
                    .map_err(|e| format!("ScreenCast SelectSources failed: {e}"))?;
                debug_log("portal SelectSources call returned");

                // Start 会弹授权选择器（首次）；无头路径仅在 token 有效时静默通过。
                // 交互授权时传 X11 root window 作为父窗口标识符：GNOME 后端需要
                // 父窗口才能把授权对话框关联到当前屏幕并正确显示（缺失时出现
                // "Failed to associate portal window with parent window"，对话框
                // 可能不渲染）。纯 Wayland 无 DISPLAY 时退化为 None。
                let parent = if request.allow_interactive_portal {
                    x11_root_window_identifier()
                } else {
                    None
                };
                debug_log("portal SelectSources done; calling Start（等待 Response）");
                // ashpd 的 start().await 内部已等到 portal Response 信号；
                // 交互授权时这一步就是"等用户点选"，无头恢复时应毫秒级返回。
                let start_request = proxy
                    .start(&session, parent.as_ref())
                    .await
                    .map_err(|e| format!("ScreenCast Start call failed: {e}"))?;
                debug_log("portal Start Response received");
                let streams = start_request
                    .response()
                    .map_err(|e| format!("ScreenCast Start failed: {e}"))?;
                debug_log(&format!(
                    "portal Start Response received; {} stream(s)",
                    streams.streams().len()
                ));
                let selected = pick_stream(streams.streams(), request.preferred_output.as_deref())
                    .ok_or_else(|| "ScreenCast Start returned no stream".to_string())?;

                debug_log("calling OpenPipeWireRemote");
                let fd = proxy
                    .open_pipe_wire_remote(&session)
                    .await
                    .map_err(|e| format!("ScreenCast OpenPipeWireRemote failed: {e}"))?;
                debug_log("portal OpenPipeWireRemote returned");

                let token = streams.restore_token().map(|s| s.to_string());

                Ok::<_, String>((
                    fd,
                    selected.pipe_wire_node_id(),
                    selected.position(),
                    selected.size(),
                    request.include_cursor && cursor_mode == CursorMode::Embedded,
                    token,
                ))
            };
            match portal_timeout {
                Some(duration) => tokio::time::timeout(duration, operation).await,
                None => match operation.await {
                    Ok(v) => Ok(Ok(v)),
                    Err(e) => Ok(Err(e)),
                },
            }
        });

        let (fd, node_id, position, size, cursor_included, new_token) = match portal_result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(e),
            // 仅无头路径可能走到超时（交互路径无超时；无 GUI 已在进入前立即失败）。
            // 注意：超时 ≠ token 失效。token 失效已被 verify_restore_token 在 Start
            // 之前拦截；能走到这里说明 portal 协商某一步在 10s 内未返回，如实报错。
            Err(_) => {
                return Err(
                    "portal screencast negotiation timed out (10s) in headless mode; the restore token passed pre-validation — check portal/PipeWire service health, or re-run --authorize"
                        .to_string(),
                )
            }
        };

        // 保存 Start 返回的 restore_token：portal 规范称 token 单次轮换，GNOME 50
        // 实证可能返回同一 token；无论哪种，都以 Start 返回者为准持久化，保证
        // 下次静默恢复有效。此操作对交互授权与无头恢复一视同仁，使授权永久
        // 生效（跨进程、跨重启）。
        if let Some(t) = new_token.as_ref() {
            crate::auth::save_restore_token(t);
        }
        self.node_id = node_id;
        self.position = position;
        self.size = size;
        self.cursor_included = cursor_included;

        let state = SharedState::new();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread_state = state.clone();
        let handle = thread::Builder::new()
            .name("dracopho-pw-screencast".to_string())
            .spawn(move || {
                run_pipewire_thread(fd, node_id, thread_state, stop_rx);
            })
            .map_err(|e| format!("failed to spawn PipeWire thread: {e}"))?;

        self.started = true;
        self.handle = Some(handle);
        self.stop_tx = Some(stop_tx);
        self.state = Some(state);
        Ok(())
    }

    /// 停止并回收 PipeWire 流线程。
    fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.started = false;
        self.state = None;
    }
}

/// 支持的 raw 像素格式（每像素 4 字节优先）。
const SUPPORTED_FORMATS: &[spa::param::video::VideoFormat] = &[
    spa::param::video::VideoFormat::BGRA,
    spa::param::video::VideoFormat::BGRx,
    spa::param::video::VideoFormat::xBGR,
    spa::param::video::VideoFormat::RGBA,
    spa::param::video::VideoFormat::RGBx,
    spa::param::video::VideoFormat::ARGB,
    spa::param::video::VideoFormat::ABGR,
    spa::param::video::VideoFormat::xRGB,
    spa::param::video::VideoFormat::RGB,
    spa::param::video::VideoFormat::BGR,
];

/// 从 portal Start 返回的多流中选择目标显示器流。
///
/// multiple=true 时，Start 对每个被选中的显示器各返回一个流，每个流带
/// `position`/`size`（合成器逻辑坐标）标识对应显示器。按 `preferred_output`
/// 名称解析出的几何匹配对应流；**未指定或未命中时回退第一个流**。
fn pick_stream<'a>(
    streams: &'a [ashpd::desktop::screencast::Stream],
    preferred_output: Option<&str>,
) -> Option<&'a ashpd::desktop::screencast::Stream> {
    if streams.is_empty() {
        return None;
    }
    let summary: Vec<(u32, Option<(i32, i32)>, Option<(i32, i32)>)> = streams
        .iter()
        .map(|s| (s.pipe_wire_node_id(), s.position(), s.size()))
        .collect();
    // 未指定 preferred_output → 直接取第一个流（绝不能返回 None，
    // 否则"未指定显示器"的正常请求会被误判为"无流"）。
    let Some(name) = preferred_output else {
        return Some(&streams[0]);
    };
    let geometry = crate::output::find_output(name).map(|o| o.geometry);
    // 精确匹配 position/size 与目标显示器几何；失败则回退第一个。
    let idx = match_stream_index(&summary, geometry);
    debug_log(&format!(
        "pick_stream: preferred_output={name} geometry={geometry:?} -> stream[{idx}] (node={}, pos={:?}, size={:?})",
        summary[idx].0, summary[idx].1, summary[idx].2
    ));
    Some(&streams[idx])
}

/// 纯函数：在多流中选择与目标几何 (gx, gy, gw, gh) 精确匹配的流索引；
/// 未命中返回 0（第一个）。测试与 pick_stream 共用同一匹配语义。
fn match_stream_index(
    streams: &[(u32, Option<(i32, i32)>, Option<(i32, i32)>)],
    target: Option<(i32, i32, i32, i32)>,
) -> usize {
    let Some((gx, gy, gw, gh)) = target else {
        return 0;
    };
    streams
        .iter()
        .position(|(_, pos, size)| *pos == Some((gx, gy)) && *size == Some((gw, gh)))
        .unwrap_or(0)
}

/// 构造单个 raw 格式协商参数（可选 modifier 变体）。
fn build_raw_format_object(
    format: spa::param::video::VideoFormat,
    with_modifier: bool,
) -> Object {
    let mut properties: Vec<Property> = vec![
        Property::new(
            spa::param::format::FormatProperties::MediaType.as_raw(),
            Value::Id(SpaId(spa::param::format::MediaType::Video.as_raw())),
        ),
        Property::new(
            spa::param::format::FormatProperties::MediaSubtype.as_raw(),
            Value::Id(SpaId(spa::param::format::MediaSubtype::Raw.as_raw())),
        ),
        Property::new(
            spa::param::format::FormatProperties::VideoFormat.as_raw(),
            Value::Id(SpaId(format.as_raw())),
        ),
    ];
    if with_modifier {
        properties.push(Property::new(
            spa::param::format::FormatProperties::VideoModifier.as_raw(),
            Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: DRM_FORMAT_MOD_INVALID as i64,
                    alternatives: vec![DRM_FORMAT_MOD_INVALID as i64, DRM_FORMAT_MOD_LINEAR as i64],
                },
            ))),
        ));
    }
    Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties,
    }
}

/// 构造 SPA_PARAM_Buffers 对象（指定 buffer 数、块、大小、步长与数据类型）。
fn build_buffers_object(size: i32, stride: i32, data_type_mask: u32) -> Object {
    // key 常量来自 spa/param/buffers.h。
    const KEY_BUFFERS: u32 = 1;
    const KEY_BLOCKS: u32 = 2;
    const KEY_SIZE: u32 = 3;
    const KEY_STRIDE: u32 = 4;
    const KEY_DATA_TYPE: u32 = 6;

    let mut properties = vec![
        Property::new(
            KEY_BUFFERS,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: 8,
                    min: 2,
                    max: 16,
                },
            ))),
        ),
        Property::new(KEY_BLOCKS, Value::Int(1)),
        Property::new(KEY_SIZE, Value::Int(size)),
        Property::new(KEY_STRIDE, Value::Int(stride)),
    ];
    properties.push(Property::new(
        KEY_DATA_TYPE,
        Value::Choice(ChoiceValue::Int(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Flags {
                default: data_type_mask as i32,
                flags: vec![data_type_mask as i32],
            },
        ))),
    ));
    Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties,
    }
}

/// 序列化一组格式对象为 SPA POD。
fn serialize_format_params(objects: &[pod::Object]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::with_capacity(objects.len());
    for object in objects {
        let (serialized, _) = pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pod::Value::Object(object.clone()),
        )
        .map_err(|e| format!("failed to serialize SPA format param: {e}"))?;
        out.push(serialized.into_inner());
    }
    Ok(out)
}

/// PipeWire 流线程主逻辑。
///
/// 关键：`context` / `core` / `stream` / `listener` 必须在等待停止信号期间
/// **保持存活**——它们在设置闭包内创建，若闭包返回即 drop，PipeWire 对象会
/// 全部销毁（pw_stream_destroy / pw_core_disconnect），流永远无法运行。
/// 因此设置闭包把持有对象返回给函数体，由函数体在等待期间持有。
///
/// 销毁顺序也必须是 PipeWire 要求的：pw_stream_destroy / pw_core_disconnect /
/// pw_context_destroy 都必须在**持有 ThreadLoop 锁**的上下文执行，否则打印
/// "pw_stream_destroy called from wrong context"。故等待停止信号后先取锁、
/// 持锁 drop 对象、再释放锁、最后 stop 线程。
fn run_pipewire_thread(
    fd: OwnedFd,
    node_id: u32,
    state: Arc<SharedState>,
    stop_rx: mpsc::Receiver<()>,
) {
    // ThreadLoop::new 内部会调用 pw::init()。
    let loop_ = match unsafe { pw::thread_loop::ThreadLoop::new(Some("dracopho-capture"), None) } {
        Ok(l) => l,
        Err(e) => {
            *state.error.lock().unwrap() = Some(format!("failed to create PipeWire loop: {e}"));
            let _ = stop_rx.recv();
            return;
        }
    };
    loop_.start();

    // 设置阶段（持锁）：创建并连接流，返回需要保持存活的对象。
    // ThreadLoop 锁在闭包返回时自动释放，之后 loop 线程才能派发回调。
    let setup = (|| -> Result<(pw::stream::Stream, pw::stream::StreamListener<Arc<SharedState>>), String> {
        let _guard = loop_.lock();
        let context = pw::context::Context::new(&loop_).map_err(|e| e.to_string())?;
        let core = context.connect_fd(fd, None).map_err(|e| e.to_string())?;

        let props = properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        };

        let stream = pw::stream::Stream::new(&core, "dracopho-screencast", props)
            .map_err(|e| e.to_string())?;

        let listener = stream
            .add_local_listener_with_user_data(state.clone())
            .state_changed(|_, shared, _old, new| {
                if let pw::stream::StreamState::Error(message) = new {
                    *shared.error.lock().unwrap() = Some(format!("PipeWire stream error: {message}"));
                }
            })
            .param_changed(|stream, shared, id, param| {
                handle_param_changed(stream, shared, id, param);
            })
            .process(|stream, shared| {
                handle_process(stream, shared);
            })
            .register()
            .map_err(|e| format!("failed to register PipeWire stream listener: {e}"))?;

        // 构造协商参数。共享内存（无 modifier）变体排在前面：DMABUF 导入
        // 尚未实现，优先让合成器协商 MemFd/MemPtr，规避取帧失败。
        let mut objects = Vec::with_capacity(SUPPORTED_FORMATS.len() * 2);
        for format in SUPPORTED_FORMATS {
            objects.push(build_raw_format_object(*format, false));
            objects.push(build_raw_format_object(*format, true));
        }
        let serialized = serialize_format_params(&objects)?;
        let mut params: Vec<&spa::pod::Pod> = serialized
            .iter()
            .map(|bytes| spa::pod::Pod::from_bytes(bytes).ok_or(()))
            .collect::<Result<_, _>>()
            .map_err(|_| "failed to wrap SPA format params".to_string())?;

        stream
            .connect(
                spa::utils::Direction::Input,
                Some(node_id),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(|e| format!("failed to connect PipeWire stream: {e}"))?;
        Ok((stream, listener))
    })();

    let held = match setup {
        Ok(held) => held,
        Err(e) => {
            *state.error.lock().unwrap() = Some(e);
            let _ = stop_rx.recv();
            loop_.stop();
            return;
        }
    };

    // 等待停止信号。期间 stream/listener（连带 core/context）保持存活，
    // loop 线程持续派发 param_changed/process 回调。
    let _ = stop_rx.recv();

    // 持锁销毁 PipeWire 对象（stream → core → context 连带释放）。
    let _guard = loop_.lock();
    drop(held);
    drop(_guard);

    loop_.stop();
}

/// param_changed 回调：解析协商结果并下发 Buffers 参数。
fn handle_param_changed(
    stream: &pw::stream::StreamRef,
    shared: &mut Arc<SharedState>,
    id: u32,
    param: Option<&spa::pod::Pod>,
) {
    let Some(param) = param else { return };
    if id != spa::param::ParamType::Format.as_raw() {
        return;
    }

    let mut info = spa::param::video::VideoInfoRaw::default();
    if info.parse(param).is_err() {
        return;
    }
    let format = info.format();
    if info.size().width == 0 || info.size().height == 0 {
        return;
    }
    let bytes_per_pixel = raw_bytes_per_pixel(format);
    if bytes_per_pixel <= 0 {
        return;
    }

    // 判断是否协商出带 modifier 的 DMA-BUF 格式。
    // 必须用 SPA_VIDEO_FLAG_MODIFIER 标志判断，不能只看 modifier 值：
    // 无 modifier 时 VideoInfoRaw 的 modifier 字段是 0，而 0 != DRM_FORMAT_MOD_INVALID，
    // 只看值会把"无 modifier"误判成"带 modifier"，从而强制 DMABUF alloc 失败
    // （"error alloc buffers: 无效的参数"）。
    let has_modifier = info.flags().bits() & spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0
        && info.modifier() != DRM_FORMAT_MOD_INVALID;
    let stride = info.size().width as i32 * bytes_per_pixel;
    let size = stride as u32 * info.size().height;

    // 构造 SPA_PARAM_Buffers 对象。
    let data_type_mask = if has_modifier {
        1u32 << spa::buffer::DataType::DmaBuf.as_raw()
    } else {
        (1u32 << spa::buffer::DataType::MemPtr.as_raw())
            | (1u32 << spa::buffer::DataType::MemFd.as_raw())
    };
    let buffers_object = build_buffers_object(size as i32, stride, data_type_mask);

    let (serialized, _) = match pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(buffers_object),
    ) {
        Ok(v) => v,
        Err(_) => return,
    };
    let bytes = serialized.into_inner();
    let Some(pod) = spa::pod::Pod::from_bytes(&bytes) else {
        return;
    };
    let mut params = [pod];
    if stream.update_params(&mut params).is_err() {
        return;
    }

    *shared.info.lock().unwrap() = Some(info);
}

/// process 回调：取出最新帧并转换为 RGBA。
fn handle_process(stream: &pw::stream::StreamRef, shared: &mut Arc<SharedState>) {
    // 取最新一帧（旧帧提前归还）。
    let mut latest: Option<pw::buffer::Buffer> = None;
    while let Some(buffer) = stream.dequeue_buffer() {
        if let Some(previous) = latest.take() {
            drop(previous);
        }
        latest = Some(buffer);
    }
    let Some(mut buffer) = latest else {
        return;
    };

    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    let info = shared.info.lock().unwrap().clone();
    let Some(info) = info else {
        return;
    };

    let data = &mut datas[0];
    let chunk_offset = data.chunk().offset();
    let chunk_stride = data.chunk().stride();
    let chunk_size = data.chunk().size();
    let image = read_frame(data, chunk_offset, chunk_stride, chunk_size, &info);
    if let Some(image) = image {
        *shared.latest.lock().unwrap() = Some(image);
        // 单调时钟（毫秒）作为帧时间戳，供陈旧帧过滤 / 帧率节流 / 录制时间线。
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        *shared.latest_time_ms.lock().unwrap() = now_ms;
    }
    // buffer 在作用域结束（drop）时自动归还流。
}

/// 每像素字节数。
fn raw_bytes_per_pixel(format: spa::param::video::VideoFormat) -> i32 {
    match format {
        spa::param::video::VideoFormat::BGRA
        | spa::param::video::VideoFormat::BGRx
        | spa::param::video::VideoFormat::xBGR
        | spa::param::video::VideoFormat::RGBA
        | spa::param::video::VideoFormat::RGBx
        | spa::param::video::VideoFormat::ARGB
        | spa::param::video::VideoFormat::ABGR
        | spa::param::video::VideoFormat::xRGB => 4,
        spa::param::video::VideoFormat::RGB | spa::param::video::VideoFormat::BGR => 3,
        _ => 0,
    }
}

/// 经 EGL 导入 DMA-BUF 帧并返回 RGBA 图像。
///
/// - `fd` / `mapoffset` / `chunk_offset` / `chunk_stride`：spa_data 的 plane0 描述。
/// - 格式、尺寸、modifier 取自协商结果 `info`。
/// - EGL 不可用或导入失败时返回 None（调用方按"帧未产出"处理，稍后超时报错）。
fn import_dmabuf_frame(
    _data: &spa::buffer::Data,
    fd: i64,
    mapoffset: i64,
    chunk_offset: u32,
    chunk_stride: i32,
    info: &spa::param::video::VideoInfoRaw,
) -> Option<image::RgbaImage> {
    if fd < 0 {
        return None;
    }
    let width = info.size().width;
    let height = info.size().height;
    if width == 0 || height == 0 {
        return None;
    }
    let bpp = raw_bytes_per_pixel(info.format());
    if bpp <= 0 {
        return None;
    }
    let stride = if chunk_stride != 0 {
        chunk_stride
    } else {
        width as i32 * bpp
    };
    if stride <= 0 {
        return None;
    }
    let offset = mapoffset + chunk_offset as i64;
    // modifier 必须依据 SPA_VIDEO_FLAG_MODIFIER 标志传递：无标志时传 None，
    // 避免把无 modifier 的 0 值当作有效 modifier 传给 EGL 导入。
    let has_modifier = info.flags().bits() & spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0
        && info.modifier() != DRM_FORMAT_MOD_INVALID;
    crate::egl_dmabuf::import_dmabuf(
        fd as i32,
        offset,
        stride,
        width,
        height,
        info.format(),
        has_modifier.then_some(info.modifier()),
    )
    .map(|frame| frame.image)
}

/// 从 PipeWire buffer 读取一帧并转为 RGBA8。
fn read_frame(
    data: &mut spa::buffer::Data,
    chunk_offset: u32,
    chunk_stride: i32,
    chunk_size: u32,
    info: &spa::param::video::VideoInfoRaw,
) -> Option<image::RgbaImage> {
    let width = info.size().width;
    let height = info.size().height;
    let bpp = raw_bytes_per_pixel(info.format());
    if width == 0 || height == 0 || bpp <= 0 {
        return None;
    }
    let stride = if chunk_stride != 0 {
        chunk_stride.unsigned_abs()
    } else {
        width * bpp as u32
    };
    if stride < width * bpp as u32 {
        return None;
    }
    let chunk_size = chunk_size as usize;
    if chunk_size == 0 {
        return None;
    }

    let data_type = data.type_();
    let raw = data.as_raw();
    let fd = raw.fd;
    let maxsize = raw.maxsize as usize;
    let mapoffset = raw.mapoffset;

    let src: Vec<u8> = match data_type {
        spa::buffer::DataType::MemPtr => {
            let Some(bytes) = data.data() else {
                return None;
            };
            let offset = chunk_offset as usize;
            let end = (offset + chunk_size).min(bytes.len());
            bytes[offset..end].to_vec()
        }
        spa::buffer::DataType::MemFd => {
            if fd < 0 || maxsize == 0 {
                return None;
            }
            unsafe {
                let ptr = libc::mmap(
                    std::ptr::null_mut(),
                    maxsize,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd as i32,
                    mapoffset as i64,
                );
                if ptr == libc::MAP_FAILED {
                    return None;
                }
                let slice = std::slice::from_raw_parts(ptr as *const u8, maxsize);
                let offset = chunk_offset as usize;
                let end = (offset + chunk_size).min(slice.len());
                let out = slice[offset..end].to_vec();
                libc::munmap(ptr, maxsize);
                out
            }
        }
        _ => {
            // DMA-BUF：CPU 不可直接读取，必须经 EGL 导入。
            // 直接返回（不进入共享内存路径），由下方 EGL 导入分支处理。
            return import_dmabuf_frame(data, fd, mapoffset as i64, chunk_offset, chunk_stride, info);
        }
    };

    let mut image = image::RgbaImage::new(width, height);
    let format = info.format();
    let needed = (height as usize).saturating_mul(stride as usize);
    if src.len() < needed {
        return None;
    }
    let pixels = image.as_mut();
    for y in 0..height {
        let row_start = (y as usize) * stride as usize;
        let dst_row = y as usize * width as usize * 4;
        let src_row = &src[row_start..row_start + stride as usize];
        for x in 0..width {
            let px = (x as usize) * bpp as usize;
            let (r, g, b, a) = match format {
                spa::param::video::VideoFormat::RGBA => {
                    (src_row[px], src_row[px + 1], src_row[px + 2], src_row[px + 3])
                }
                spa::param::video::VideoFormat::BGRA => {
                    (src_row[px + 2], src_row[px + 1], src_row[px], src_row[px + 3])
                }
                spa::param::video::VideoFormat::RGBx => (src_row[px], src_row[px + 1], src_row[px + 2], 255),
                spa::param::video::VideoFormat::BGRx => (src_row[px + 2], src_row[px + 1], src_row[px], 255),
                spa::param::video::VideoFormat::xRGB => (src_row[px + 1], src_row[px + 2], src_row[px + 3], 255),
                spa::param::video::VideoFormat::xBGR => (src_row[px + 3], src_row[px + 2], src_row[px + 1], 255),
                spa::param::video::VideoFormat::ARGB => (src_row[px + 1], src_row[px + 2], src_row[px + 3], src_row[px]),
                spa::param::video::VideoFormat::ABGR => (src_row[px + 3], src_row[px + 2], src_row[px + 1], src_row[px]),
                spa::param::video::VideoFormat::RGB => (src_row[px], src_row[px + 1], src_row[px + 2], 255),
                spa::param::video::VideoFormat::BGR => (src_row[px + 2], src_row[px + 1], src_row[px], 255),
                _ => continue,
            };
            let dst = dst_row + x as usize * 4;
            pixels[dst] = r;
            pixels[dst + 1] = g;
            pixels[dst + 2] = b;
            pixels[dst + 3] = a;
        }
    }
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::match_stream_index;

    #[test]
    fn picks_stream_matching_target_geometry() {
        // 两个显示器流：HDMI-2 在 (0,0,1920x1080)，HDMI-1 在 (1920,0,1680x1050)。
        let streams = [
            (0u32, Some((0, 0)), Some((1920, 1080))),
            (1u32, Some((1920, 0)), Some((1680, 1050))),
        ];
        // 目标 = 副屏 HDMI-1 的几何。
        assert_eq!(
            match_stream_index(&streams, Some((1920, 0, 1680, 1050))),
            1
        );
        // 目标 = 主屏 HDMI-2 的几何。
        assert_eq!(
            match_stream_index(&streams, Some((0, 0, 1920, 1080))),
            0
        );
    }

    #[test]
    fn unmatched_geometry_falls_back_to_first() {
        let streams = [
            (0u32, Some((0, 0)), Some((1920, 1080))),
            (1u32, Some((1920, 0)), Some((1680, 1050))),
        ];
        // 不存在的几何 → 回退第一个。
        assert_eq!(match_stream_index(&streams, Some((9999, 9999, 100, 100))), 0);
        // 未指定目标 → 第一个。
        assert_eq!(match_stream_index(&streams, None), 0);
        // 空流列表 → 0（调用方已保证非空）。
        assert_eq!(match_stream_index(&[], Some((0, 0, 10, 10))), 0);
    }
}
