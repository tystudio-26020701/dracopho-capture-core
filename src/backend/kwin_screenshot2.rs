//! 自研 KWin ScreenShot2 后端（KDE Plasma 专用，铁律放宽后启用）。
//!
//! 走 KWin 的 `org.kde.KWin.ScreenShot2` DBus 接口：调用方传入管道 FD，
//! KWin 把"原始像素"写入管道并在回复 vardict 中给出元数据
//! （type/width/height/stride/format/scale）。窗口级抓取由 KWin 直接渲染
//! 目标窗口的合成缓冲，遮挡/最小化窗口也能拿到真实内容（对标 Spectacle）。
//!
//! 通道定位（最轻专用通道原则）：
//! - KDE 窗口级：KWin ScreenShot2 `CaptureWindow`（静默、精确、零干扰）；
//! - KDE 区域/全屏：portal ScreenCast 优先，本后端作为回退路由（`CaptureArea`
//!   / `CaptureScreen` / `CaptureWorkspace`）。
//!
//! 注意：仅 KDE Plasma 提供该接口；其他合成器 `available()` 恒为 false。

use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::RgbaImage;
use zbus::zvariant::Fd;

use crate::capture_types::{Backend, CaptureRequest, CaptureResult, WindowObjectInfo};

const SERVICE: &str = "org.kde.KWin.ScreenShot2";
const PATH: &str = "/org/kde/KWin/ScreenShot2";
const IFACE: &str = "org.kde.KWin.ScreenShot2";

/// 元数据合理性上限（防御来自合成器的恶意/异常尺寸，防止超大分配）。
const MAX_DIM: u32 = 16384;
const MAX_TOTAL_BYTES: u64 = 1 << 30; // 1 GiB

/// 是否可用：KDE Plasma 会话 + `org.kde.KWin.ScreenShot2` 服务存在。
///
/// 能力探测缓存（OnceLock）：合成器接口存在性在会话内固定，进程内只探测一次，
/// 避免每次截图都付出一次 session-bus DBus 往返。
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let st = std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
        if st != "wayland" {
            return false;
        }
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        if !(desktop.contains("kde") || desktop.contains("plasma"))
            && std::env::var_os("KDE_SESSION_VERSION").is_none()
        {
            return false;
        }
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return false;
        };
        let reply = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(SERVICE,),
        );
        match reply {
            Ok(r) => r.body().deserialize::<bool>().unwrap_or(false),
            Err(_) => false,
        }
    })
}

/// 构造 Screenshot 选项 dict（`a{sv}`，屏幕/区域路径用）。
fn build_options(request: &CaptureRequest) -> HashMap<String, zbus::zvariant::Value<'_>> {
    let mut options = HashMap::new();
    options.insert("include-cursor".to_string(), zbus::zvariant::Value::from(request.include_cursor));
    // native-resolution：HiDPI 下保留物理像素，与逻辑坐标换算一致。
    options.insert("native-resolution".to_string(), zbus::zvariant::Value::from(true));
    // KWin 按调用进程 PID 匹配并隐藏自身窗口（悬浮球/设置窗等），
    // 是图形合成层级的排除，而非截图前逐个 hide。
    options.insert("hide-caller-windows".to_string(), zbus::zvariant::Value::from(request.hide_own_windows));
    options
}

fn fail(error: impl Into<String>) -> CaptureResult {
    CaptureResult::failure(Backend::KwinScreenShot2, error)
}

/// 创建 O_CLOEXEC 管道，返回 (读端, 写端)。
fn make_pipe() -> Result<(i32, i32), String> {
    let mut fds = [0i32; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        return Err(format!("pipe2 failed: {}", std::io::Error::last_os_error()));
    }
    Ok((fds[0], fds[1]))
}

/// 从管道读端读取 `total` 字节（带超时）。
fn read_pipe(fd: i32, total: usize, timeout_ms: u64) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; total];
    let mut got = 0usize;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while got < total {
        if Instant::now() >= deadline {
            return Err("KWin ScreenShot2 pipe read timed out".to_string());
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let pr = unsafe { libc::poll(&mut pfd, 1, 100) };
        if pr < 0 {
            return Err(format!("poll failed: {}", std::io::Error::last_os_error()));
        }
        if pr == 0 {
            continue;
        }
        let n = unsafe {
            libc::read(
                fd,
                buf[got..].as_mut_ptr() as *mut libc::c_void,
                (total - got) as libc::size_t,
            )
        };
        if n < 0 {
            return Err(format!("read failed: {}", std::io::Error::last_os_error()));
        }
        if n == 0 {
            break; // EOF
        }
        got += n as usize;
    }
    if got < total {
        return Err(format!("KWin ScreenShot2 delivered a truncated frame ({got}/{total} bytes)"));
    }
    Ok(buf)
}

/// 从 `results` vardict 读取元数据并读取管道像素，转为 RGBA8。
///
/// 调用方负责在所有路径关闭 `read_fd`（成功与失败一律关闭）。
fn finish_pipe(
    read_fd: i32,
    results: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<RgbaImage, String> {
    let u = |k: &str| -> u32 { results.get(k).and_then(|v| value_u32(&*v)).unwrap_or(0) };
    let width = u("width");
    let height = u("height");
    let stride = u("stride");
    let format = u("format");
    if width == 0 || height == 0 || stride == 0 {
        return Err("KWin ScreenShot2 returned invalid buffer metadata".to_string());
    }
    // 防御异常元数据（防止超大分配）：单轴上限 + 检查算术总字节上限。
    if width > MAX_DIM || height > MAX_DIM {
        return Err(format!(
            "KWin ScreenShot2 buffer is implausibly large ({width}x{height})"
        ));
    }
    if stride < width * 4 {
        return Err(format!(
            "KWin ScreenShot2 stride {stride} < width*4 {}",
            width * 4
        ));
    }
    let total = (stride as u64) * (height as u64);
    if total > MAX_TOTAL_BYTES {
        return Err(format!("KWin ScreenShot2 buffer is implausibly large ({total} bytes)"));
    }
    let data = read_pipe(read_fd, total as usize, 4000)?;
    convert_qimage(&data, width, height, stride as usize, format)
}

/// 从 `a{sv}` 取值并解包可能的 variant 层（递归），返回 u32。
fn value_u32(v: &zbus::zvariant::Value<'_>) -> Option<u32> {
    match v {
        zbus::zvariant::Value::Value(b) => value_u32(b.as_ref()),
        zbus::zvariant::Value::U32(x) => Some(*x),
        _ => None,
    }
}

/// 解除预乘 alpha（straight alpha 输出，与 X11 路径一致）。
fn unmultiply(r: u8, g: u8, b: u8, a: u8) -> (u8, u8, u8, u8) {
    if a == 0 || a == 255 {
        return (r, g, b, a);
    }
    let a_u = a as u32;
    let fix = |c: u8| ((c as u32 * 255 + a_u / 2) / a_u).min(255) as u8;
    (fix(r), fix(g), fix(b), a)
}

/// 把 QImage 原始像素（`stride` 每行字节）转为 RGBA8。
///
/// 支持 KWin 实际会写出的常见 32-bit 格式（字节序为小端）：
///   Format_RGB32(4) / ARGB32(5) / ARGB32_Premultiplied(6)
///   / RGBX8888(16) / RGBA8888(17) / RGBA8888_Premultiplied(18)
fn convert_qimage(data: &[u8], width: u32, height: u32, stride: usize, format: u32) -> Result<RgbaImage, String> {
    let needed = stride * height as usize;
    if data.len() < needed {
        return Err("KWin ScreenShot2 frame buffer is smaller than expected".to_string());
    }
    let mut image = RgbaImage::new(width, height);
    let pixels = image.as_mut();
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let px = row + x as usize * 4;
            let (r, g, b, a) = match format {
                // [B, G, R, X]
                4 => (data[px + 2], data[px + 1], data[px], 255),
                // [B, G, R, A]
                5 => (data[px + 2], data[px + 1], data[px], data[px + 3]),
                // [B, G, R, A] 预乘
                6 => unmultiply(data[px + 2], data[px + 1], data[px], data[px + 3]),
                // [R, G, B, X]
                16 => (data[px], data[px + 1], data[px + 2], 255),
                // [R, G, B, A]
                17 => (data[px], data[px + 1], data[px + 2], data[px + 3]),
                // [R, G, B, A] 预乘
                18 => unmultiply(data[px], data[px + 1], data[px + 2], data[px + 3]),
                _ => {
                    return Err(format!(
                        "unsupported QImage::Format {format} from KWin ScreenShot2"
                    ))
                }
            };
            let dst = (y as usize * width as usize + x as usize) * 4;
            pixels[dst] = r;
            pixels[dst + 1] = g;
            pixels[dst + 2] = b;
            pixels[dst + 3] = a;
        }
    }
    Ok(image)
}

/// 调用 ScreenShot2 方法并读回图像。
///
/// `body_args` 须已追加管道写端 FD（`zbus::zvariant::Fd`）；返回 `results`
/// vardict 后读取像素，并在**所有路径**（成功/失败）关闭读端，杜绝 fd 泄漏。
fn invoke_and_read(
    conn: &zbus::blocking::Connection,
    method: &str,
    body_args: impl zbus::zvariant::Type + serde::Serialize,
    read_fd: i32,
) -> Result<RgbaImage, String> {
    let reply = match conn.call_method(Some(SERVICE), PATH, Some(IFACE), method, &body_args) {
        Ok(r) => r,
        Err(e) => {
            unsafe { libc::close(read_fd) };
            return Err(format!("KWin ScreenShot2 {method} failed: {e}"));
        }
    };
    let results: HashMap<String, zbus::zvariant::OwnedValue> =
        match reply.body().deserialize() {
            Ok(v) => v,
            Err(e) => {
                unsafe { libc::close(read_fd) };
                return Err(format!("KWin ScreenShot2 {method} reply decode failed: {e}"));
            }
        };
    let image = finish_pipe(read_fd, &results);
    unsafe { libc::close(read_fd) };
    image
}

/// 把原始 fd 转为可放入 DBus body 的 `Fd`（写端随 body 析构自动关闭）。
fn dbus_fd(raw: i32) -> Fd<'static> {
    // Safety：raw 来自 pipe2 新建的写端，此处接管其所有权（析构时关闭）。
    Fd::from(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// 屏幕/区域/全屏抓取（作为 KDE 区域回退路由）。
pub(crate) fn capture(request: &CaptureRequest) -> CaptureResult {
    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(e) => return fail(format!("session bus connect failed: {e}")),
    };
    let (read_fd, write_fd) = match make_pipe() {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let options = build_options(request);

    let image = if request.all_outputs {
        let body = (options, dbus_fd(write_fd));
        invoke_and_read(&conn, "CaptureWorkspace", body, read_fd)
    } else if let Some((x, y, w, h)) = request.source_geometry {
        if w <= 0 || h <= 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return fail("invalid capture geometry");
        }
        let body = (x, y, w as u32, h as u32, options, dbus_fd(write_fd));
        invoke_and_read(&conn, "CaptureArea", body, read_fd)
    } else if let Some(name) = request.preferred_output.as_deref() {
        let body = (name.to_string(), options, dbus_fd(write_fd));
        invoke_and_read(&conn, "CaptureScreen", body, read_fd)
    } else {
        let body = (options, dbus_fd(write_fd));
        invoke_and_read(&conn, "CaptureActiveScreen", body, read_fd)
    };

    match image {
        Ok(image) => CaptureResult {
            image: Some(image),
            error: None,
            source_geometry: request.source_geometry,
            output_name: request.preferred_output.clone(),
            backend: Backend::KwinScreenShot2,
            frame_time_ms: 0,
        },
        Err(e) => fail(e),
    }
}

/// 抓取单个 KDE 窗口自身内容（`window.id` 为 KWin internalId UUID）。
///
/// 遮挡/最小化窗口同样有效（KWin 直接渲染目标窗口合成缓冲）。
/// 非 UUID 标识（如 X11 "0x…"）直接返回错误，由路由回退 X11 XComposite。
pub(crate) fn capture_window_content(
    window: &WindowObjectInfo,
    include_cursor: bool,
) -> Result<RgbaImage, String> {
    let uuid = window.id.trim();
    if uuid.starts_with("0x") || uuid.is_empty() {
        return Err("not a KWin window uuid (KWin ScreenShot2 requires the internalId)".to_string());
    }
    let conn = zbus::blocking::Connection::session()
        .map_err(|e| format!("session bus connect failed: {e}"))?;
    let (read_fd, write_fd) = make_pipe()?;

    let mut options = HashMap::new();
    options.insert("include-cursor".to_string(), zbus::zvariant::Value::from(include_cursor));
    // 含窗口装饰/阴影：与交互式高亮的窗口边界（含标题栏）一致。
    options.insert("include-decoration".to_string(), zbus::zvariant::Value::from(true));
    options.insert("include-shadow".to_string(), zbus::zvariant::Value::from(true));
    options.insert("native-resolution".to_string(), zbus::zvariant::Value::from(true));

    let body = (uuid.to_string(), options, dbus_fd(write_fd));
    invoke_and_read(&conn, "CaptureWindow", body, read_fd)
}

#[cfg(test)]
mod tests {
    use super::{convert_qimage, unmultiply};

    #[test]
    fn converts_argb32_premultiplied_to_rgba() {
        // QImage::Format_ARGB32_Premultiplied(6)：每像素 [B, G, R, A]（预乘）。
        // 像素1：不透明，alpha=255；像素2：alpha=128，RGB 预乘为一半。
        let data = [0x11u8, 0x22, 0x33, 0xff, 0x10, 0x20, 0x30, 0x80];
        let img = convert_qimage(&data, 2, 1, 8, 6).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x33, 0x22, 0x11, 0xff]);
        // 解除预乘：alpha=128，R=0x30*255/128≈96(0x60)，G=0x20*255/128=64，B=0x10*255/128=32。
        assert_eq!(px[4], 0x60); // R
        assert_eq!(px[5], 0x40); // G
        assert_eq!(px[6], 0x20); // B
        assert_eq!(px[7], 0x80); // A
    }

    #[test]
    fn converts_rgb32_to_rgba() {
        // QImage::Format_RGB32(4)：每像素 [B, G, R, X]。
        let data = [0x01u8, 0x02, 0x03, 0xff];
        let img = convert_qimage(&data, 1, 1, 4, 4).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x03, 0x02, 0x01, 0xff]);
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(convert_qimage(&[0u8; 4], 1, 1, 4, 99).is_err());
    }

    #[test]
    fn unmultiplies_straight() {
        assert_eq!(unmultiply(0x80, 0x40, 0x20, 0xff), (0x80, 0x40, 0x20, 0xff));
        assert_eq!(unmultiply(0, 0, 0, 0), (0, 0, 0, 0));
    }
}
