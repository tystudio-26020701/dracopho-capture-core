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

use pw::spa::pod::{ChoiceValue, Object, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id as SpaId};

/// PipeWire 流线程共享状态。
struct SharedState {
    latest: Mutex<Option<image::RgbaImage>>,
    error: Mutex<Option<String>>,
    info: Mutex<Option<spa::param::video::VideoInfoRaw>>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            latest: Mutex::new(None),
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

pub(crate) fn capture_with_session(
    request: &CaptureRequest,
    session: Option<&mut PipeWireSession>,
) -> Result<CaptureResult, String> {
    let session = match session {
        Some(s) => s,
        None => return Err("pipewire session is not initialized".to_string()),
    };

    if !session.started {
        // 交互模式：启动会话（会弹一次授权选择器）。
        // 无头模式（铁律严禁弹窗）：仅当存在持久化恢复 token 时才尝试静默恢复；
        // 无 token 直接报错，绝不触发选择器。token 失效导致的弹窗由 start 的
        // 短超时兜底——超时后进程退出会取消 portal 请求并关闭选择器。
        if !request.allow_interactive_portal && crate::auth::restore_token().is_none() {
            return Err(
                "portal screencast requires interactive authorization; run once with the GUI to grant it".to_string(),
            );
        }
        session.start_session(request)?;
    }

    // 等待/复用最新帧。
    let state = session.state.as_ref().ok_or("pipewire state missing")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(err) = state.error.lock().unwrap().clone() {
            return Err(err);
        }
        if let Some(image) = state.latest.lock().unwrap().clone() {
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
            return Ok(CaptureResult {
                image: Some(out),
                error: None,
                source_geometry: request.source_geometry,
                output_name: None,
                backend: Backend::PipeWireScreencast,
                frame_time_ms: 0,
            });
        }
        if std::time::Instant::now() >= deadline {
            return Err("pipewire screencast did not produce a frame within 5s".to_string());
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

        // 交互授权留给用户操作的时间较长；无头 token 恢复必须快速失败，
        // 避免 token 失效时 portal 选择器无限等待（铁律：无头严禁干扰用户）。
        let portal_timeout = if request.allow_interactive_portal {
            std::time::Duration::from_secs(120)
        } else {
            std::time::Duration::from_secs(10)
        };

        // portal 协商（tokio 运行时内，超时保护）。
        let portal_result = rt.block_on(async {
            tokio::time::timeout(portal_timeout, async {
                let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
                let session = proxy
                    .create_session()
                    .await
                    .map_err(|e| format!("ScreenCast CreateSession failed: {e}"))?;

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
                // 无头恢复都必须保持持久化，否则 Start 不会返回新 restore_token，
                // token 轮换链断掉，下次恢复又失效弹窗。
                //
                // restore_token 是单次的：用完作废，必须用 Start 返回的新 token
                // 继续下一轮（永久生效，跨进程、跨重启，直到 portal 权限被撤销）。
                let persist_mode = PersistMode::ExplicitlyRevoked;
                let restore = restore_token.as_deref();

                proxy
                    .select_sources(
                        &session,
                        cursor_mode,
                        SourceType::Monitor.into(),
                        false,
                        restore,
                        persist_mode,
                    )
                    .await
                    .map_err(|e| format!("ScreenCast SelectSources failed: {e}"))?;

                // Start 会弹授权选择器（首次）；无头路径仅在 token 有效时静默通过。
                let streams = proxy
                    .start(&session, None)
                    .await
                    .map_err(|e| e.to_string())?
                    .response()
                    .map_err(|e| format!("ScreenCast Start failed: {e}"))?;
                let first = streams
                    .streams()
                    .first()
                    .ok_or_else(|| "ScreenCast Start returned no stream".to_string())?;

                let fd = proxy
                    .open_pipe_wire_remote(&session)
                    .await
                    .map_err(|e| format!("ScreenCast OpenPipeWireRemote failed: {e}"))?;

                let token = streams.restore_token().map(|s| s.to_string());

                Ok::<_, String>((
                    fd,
                    first.pipe_wire_node_id(),
                    first.position(),
                    first.size(),
                    request.include_cursor && cursor_mode == CursorMode::Embedded,
                    token,
                ))
            })
            .await
        });

        let (fd, node_id, position, size, cursor_included, new_token) = match portal_result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(if request.allow_interactive_portal {
                    "screencast authorization timed out".to_string()
                } else {
                    "screencast restore token is no longer valid; re-run --authorize".to_string()
                })
            }
        };

        // 轮换并保存 restore_token：restore_token 是单次的，Start 返回的新 token
        // 必须保存，否则下次恢复会因旧 token 作废而再次弹窗授权。此操作对交互
        // 授权与无头恢复一视同仁，使授权永久生效（跨进程、跨重启）。
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
