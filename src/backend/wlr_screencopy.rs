//! 自研 wlr-screencopy 后端（wlroots 系合成器）。
//!
//! 直接走 wlr-screencopy-unstable-v1 协议 + wl_shm 共享内存读取，
//! 无需门户、零弹窗。GNOME/KDE 不支持该协议时 `available()` 后由
//! PipeWire screencast 兜底。

use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::sync::{mpsc, Arc, Mutex};

use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::Event as RegistryEvent;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

use crate::capture_types::{CaptureRequest, CaptureResult};

/// DRM fourcc 常量。
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;

/// 帧描述（width, height, stride, format）。
type FrameDesc = (u32, u32, i32, u32);

/// 单个帧的用户数据。
struct FrameData {
    shm: WlShm,
    result_tx: mpsc::Sender<Result<image::RgbaImage, String>>,
    desc: Arc<Mutex<Option<FrameDesc>>>,
    mapped: Arc<Mutex<Option<(usize, usize)>>>,
    pool: Arc<Mutex<Option<WlShmPool>>>,
    buffer: Arc<Mutex<Option<WlBuffer>>>,
}

/// 应用状态。
struct State {
    manager: Option<ZwlrScreencopyManagerV1>,
    shm: Option<WlShm>,
    outputs: Vec<WlOutput>,
    /// wl_output v4 name 事件缓存（用于按 --display 选择输出）。
    output_names: HashMap<ObjectId, String>,
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: RegistryEvent,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let RegistryEvent::Global { name, interface, .. } = event {
            if interface == "zwlr_screencopy_manager_v1" {
                state.manager =
                    Some(registry.bind::<ZwlrScreencopyManagerV1, (), State>(name, 1, qh, ()));
            } else if interface == "wl_shm" {
                state.shm = Some(registry.bind::<WlShm, (), State>(name, 1, qh, ()));
            } else if interface == "wl_output" {
                state
                    .outputs
                    .push(registry.bind::<WlOutput, (), State>(name, 4, qh, ()));
            }
        }
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore ZwlrScreencopyManagerV1);

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.output_names.insert(output.id(), name);
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for State {
    fn event(
        _state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        data: &FrameData,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(v) => v,
                };
                if !matches!(
                    format,
                    DRM_FORMAT_ARGB8888
                        | DRM_FORMAT_XRGB8888
                        | DRM_FORMAT_ABGR8888
                        | DRM_FORMAT_XBGR8888
                ) {
                    let _ = data
                        .result_tx
                        .send(Err(format!("unsupported screencopy format 0x{format:x}")));
                    return;
                }
                let size = (stride as i64) * (height as i64);
                if size <= 0 || size > i32::MAX as i64 {
                    let _ = data
                        .result_tx
                        .send(Err("invalid screencopy buffer size".to_string()));
                    return;
                }
                // 创建 memfd + wl_shm pool + buffer，并请求 compositor 复制。
                unsafe {
                    let fd = libc::memfd_create(
                        c"dracopho-capture-screencopy".as_ptr(),
                        libc::MFD_CLOEXEC,
                    );
                    if fd < 0 {
                        let _ = data.result_tx.send(Err(format!(
                            "memfd_create failed: {}",
                            std::io::Error::last_os_error()
                        )));
                        return;
                    }
                    if libc::ftruncate(fd, size as libc::off_t) != 0 {
                        let _ = data.result_tx.send(Err("ftruncate failed".to_string()));
                        libc::close(fd);
                        return;
                    }
                    let ptr = libc::mmap(
                        std::ptr::null_mut(),
                        size as usize,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    );
                    if ptr == libc::MAP_FAILED {
                        let _ = data.result_tx.send(Err("mmap failed".to_string()));
                        libc::close(fd);
                        return;
                    }
                    let borrowed = BorrowedFd::borrow_raw(fd);
                    let pool = data.shm.create_pool(borrowed, size as i32, qh, ());
                    let shm_format = match format {
                        DRM_FORMAT_ARGB8888 => wl_shm::Format::Argb8888,
                        DRM_FORMAT_XRGB8888 => wl_shm::Format::Xrgb8888,
                        DRM_FORMAT_ABGR8888 => wl_shm::Format::Abgr8888,
                        DRM_FORMAT_XBGR8888 => wl_shm::Format::Xbgr8888,
                        _ => wl_shm::Format::Xrgb8888,
                    };
                    let buffer = pool.create_buffer(
                        0,
                        width as i32,
                        height as i32,
                        stride as i32,
                        shm_format,
                        qh,
                        (),
                    );
                    *data.desc.lock().unwrap() = Some((width, height, stride as i32, format));
                    *data.mapped.lock().unwrap() = Some((ptr as usize, size as usize));
                    *data.pool.lock().unwrap() = Some(pool);
                    *data.buffer.lock().unwrap() = Some(buffer.clone());
                    frame.copy(&buffer);
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                let image = {
                    let desc = *data.desc.lock().unwrap();
                    let mapped = *data.mapped.lock().unwrap();
                    match (desc, mapped) {
                        (Some((width, height, stride, format)), Some((ptr, len)))
                            if ptr != 0 && len > 0 =>
                        {
                            let bytes =
                                unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                            convert_shm_frame(bytes, width, height, stride as usize, format)
                        }
                        _ => None,
                    }
                };
                let _ = match image {
                    Some(img) => data.result_tx.send(Ok(img)),
                    None => data
                        .result_tx
                        .send(Err("screencopy buffer is not available on ready".to_string())),
                };
                // 释放映射与 proxy。
                if let Some((ptr, len)) = data.mapped.lock().unwrap().take() {
                    unsafe {
                        libc::munmap(ptr as *mut libc::c_void, len);
                    }
                }
                data.pool.lock().unwrap().take();
                data.buffer.lock().unwrap().take();
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                let _ = data
                    .result_tx
                    .send(Err("wlr-screencopy capture failed".to_string()));
            }
            _ => {}
        }
    }
}

/// 将 wl_shm 帧数据转换为 RGBA8。
fn convert_shm_frame(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    format: u32,
) -> Option<image::RgbaImage> {
    let needed = stride * height as usize;
    if data.len() < needed {
        return None;
    }
    let mut image = image::RgbaImage::new(width, height);
    let pixels = image.as_mut();
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let px = row + x as usize * 4;
            let (r, g, b, a) = match format {
                DRM_FORMAT_ARGB8888 => (data[px + 2], data[px + 1], data[px], data[px + 3]),
                DRM_FORMAT_XRGB8888 => (data[px + 2], data[px + 1], data[px], 255),
                DRM_FORMAT_ABGR8888 => (data[px], data[px + 1], data[px + 2], data[px + 3]),
                DRM_FORMAT_XBGR8888 => (data[px], data[px + 1], data[px + 2], 255),
                _ => (0, 0, 0, 255),
            };
            let dst = (y as usize * width as usize + x as usize) * 4;
            pixels[dst] = r;
            pixels[dst + 1] = g;
            pixels[dst + 2] = b;
            pixels[dst + 3] = a;
        }
    }
    Some(image)
}

/// 是否可能可用：Wayland 会话。
pub fn available() -> bool {
    std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase() == "wayland"
}

pub(crate) fn capture(request: &CaptureRequest) -> CaptureResult {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            return CaptureResult::failure(
                crate::capture_types::Backend::WlrScreencopy,
                format!("wayland connect failed: {e}"),
            )
        }
    };
    let mut state = State {
        manager: None,
        shm: None,
        outputs: Vec::new(),
        output_names: HashMap::new(),
    };
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();

    let display = conn.display();
    let _registry = display.get_registry(&qh, ());
    if queue.roundtrip(&mut state).is_err() {
        return CaptureResult::failure(
            crate::capture_types::Backend::WlrScreencopy,
            "wayland registry roundtrip failed",
        );
    }

    let Some(manager) = state.manager.as_ref() else {
        return CaptureResult::failure(
            crate::capture_types::Backend::WlrScreencopy,
            "wlr-screencopy protocol not available on this compositor",
        );
    };
    let Some(shm) = state.shm.as_ref() else {
        return CaptureResult::failure(
            crate::capture_types::Backend::WlrScreencopy,
            "wl_shm not available",
        );
    };
    // 选择输出：优先按请求的输出名匹配（--display），否则第一个输出。
    let output = if let Some(name) = request.preferred_output.as_deref() {
        state
            .outputs
            .iter()
            .find(|o| state.output_names.get(&o.id()).is_some_and(|n| n == name))
    } else {
        None
    }
    .or_else(|| state.outputs.first());
    let Some(output) = output else {
        return CaptureResult::failure(
            crate::capture_types::Backend::WlrScreencopy,
            "no wl_output available",
        );
    };

    let (tx, rx) = mpsc::channel();
    let desc = Arc::new(Mutex::new(None));
    let mapped = Arc::new(Mutex::new(None));
    let pool = Arc::new(Mutex::new(None));
    let buffer = Arc::new(Mutex::new(None));
    let data = FrameData {
        shm: shm.clone(),
        result_tx: tx,
        desc,
        mapped,
        pool,
        buffer,
    };

    // 区域抓取（capture_output_region）或整输出抓取。
    // all_outputs 时忽略区域，抓整个输出。
    let frame = match (request.all_outputs, request.source_geometry) {
        (false, Some((x, y, w, h))) if w > 0 && h > 0 => {
            manager.capture_output_region(0, output, x, y, w, h, &qh, data)
        }
        _ => manager.capture_output(0, output, &qh, data),
    };
    // 持续派发事件直到收到 ready/failed 或超时：ready 的到达时机由合成器决定，
    // 固定两次 roundtrip 不可靠（复制完成可能跨多个事件循环）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Ok(received) = rx.try_recv() {
            break received;
        }
        if std::time::Instant::now() >= deadline {
            break Err("wlr-screencopy timed out waiting for a frame".to_string());
        }
        if queue.blocking_dispatch(&mut state).is_err() {
            break Err("wayland dispatch failed while waiting for frame".to_string());
        }
    };
    let _ = frame.destroy();

    match result {
        Ok(image) => CaptureResult {
            image: Some(image),
            error: None,
            source_geometry: request.source_geometry,
            output_name: request.preferred_output.clone(),
            backend: crate::capture_types::Backend::WlrScreencopy,
            frame_time_ms: 0,
        },
        Err(e) => CaptureResult::failure(crate::capture_types::Backend::WlrScreencopy, e),
    }
}

#[cfg(test)]
mod tests {
    use super::{convert_shm_frame, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888};

    #[test]
    fn converts_argb_shm_to_rgba() {
        // ARGB8888: 每像素 [B, G, R, A]
        let data = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let img = convert_shm_frame(&data, 2, 1, 8, DRM_FORMAT_ARGB8888).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x33, 0x22, 0x11, 0x44]);
        assert_eq!(&px[4..8], &[0x77, 0x66, 0x55, 0x88]);
    }

    #[test]
    fn converts_xrgb_shm_to_rgba() {
        let data = [0x01u8, 0x02, 0x03, 0xff, 0x04, 0x05, 0x06, 0xff];
        let img = convert_shm_frame(&data, 2, 1, 8, DRM_FORMAT_XRGB8888).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x03, 0x02, 0x01, 0xff]);
        assert_eq!(&px[4..8], &[0x06, 0x05, 0x04, 0xff]);
    }

    #[test]
    fn rejects_short_buffer() {
        let data = [0u8; 4];
        assert!(convert_shm_frame(&data, 100, 100, 400, DRM_FORMAT_XRGB8888).is_none());
    }
}
