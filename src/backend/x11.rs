//! 自研 X11 后端（XComposite 命名 pixmap / XGetImage 直接抓取）。
//!
//! 不调用任何"系统自带截图"服务：直接使用 X11 协议（x11rb 绑定）。

use std::sync::{Arc, Mutex};

use x11rb::connection::Connection;
use x11rb::protocol::composite::{ConnectionExt as CompositeExt, Redirect};
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::capture_types::{CaptureRequest, CaptureResult, WindowObjectInfo};

/// 全局连接缓存（同进程复用，避免反复建立 X 连接）。
static CONN: Mutex<Option<Arc<RustConnection>>> = Mutex::new(None);

pub(crate) fn connection() -> Result<Arc<RustConnection>, String> {
    let mut guard = CONN.lock().map_err(|_| "x11 connection mutex poisoned".to_string())?;
    if guard.is_none() {
        let (conn, _screen) =
            RustConnection::connect(None).map_err(|e| format!("X11 connect failed: {e}"))?;
        // 预查询 Composite 扩展（供窗口内容抓取使用）。
        let _ = conn.query_extension(b"Composite");
        *guard = Some(Arc::new(conn));
    }
    Ok((*guard.as_ref().unwrap()).clone())
}

/// 是否可能可用：DISPLAY 环境变量存在（X11 会话或 XWayland）。
pub fn available() -> bool {
    !std::env::var("DISPLAY").unwrap_or_default().is_empty()
}

/// X11 根窗口信息。
fn root_info(conn: &RustConnection) -> Result<(u32, u8), String> {
    let setup = conn.setup();
    let root = setup.roots.first().ok_or("no X11 screen")?;
    Ok((root.root, root.root_depth))
}

/// X11 像素数据转换为 RGBA8。
///
/// XGetImage 的每像素字节数（bpp）由返回数据的实际长度决定，不能按 depth 推断：
/// depth 24 的屏幕经常以 32-bit（BGRX/XRGB）返回。字节序由连接的
/// image_byte_order 决定。
fn convert_pixels(
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    lsb_first: bool,
) -> Option<image::RgbaImage> {
    if bytes_per_pixel != 3 && bytes_per_pixel != 4 {
        return None;
    }
    let stride = ((width as usize * bytes_per_pixel + 3) / 4) * 4; // X11 行按 32-bit 对齐
    let needed = stride * height as usize;
    if data.len() < needed {
        return None;
    }
    let mut image = image::RgbaImage::new(width, height);
    let pixels = image.as_mut();
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let px = row + x as usize * bytes_per_pixel;
            let (r, g, b, a) = match (bytes_per_pixel, lsb_first) {
                (4, true) => (data[px + 2], data[px + 1], data[px], 255),
                (4, false) => (data[px + 1], data[px + 2], data[px + 3], 255),
                (3, true) => (data[px + 2], data[px + 1], data[px], 255),
                (3, false) => (data[px], data[px + 1], data[px + 2], 255),
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

pub(crate) fn capture(request: &CaptureRequest) -> CaptureResult {
    let conn = match connection() {
        Ok(c) => c,
        Err(e) => return CaptureResult::failure(crate::capture_types::Backend::X11, e),
    };
    let (root, _depth) = match root_info(&conn) {
        Ok(v) => v,
        Err(e) => return CaptureResult::failure(crate::capture_types::Backend::X11, e),
    };

    // 计算捕获区域（逻辑坐标）。优先级：
    //   1. 显式 source_geometry（非 all_outputs）
    //   2. preferred_output 命中某输出（--display）
    //   3. all_outputs 或未指定 → 整个虚拟桌面
    let geometry = match request.source_geometry {
        Some((x, y, w, h)) if !request.all_outputs => (x, y, w, h),
        _ => {
            // --display：按输出名裁剪到该输出几何。
            let mut matched = None;
            if !request.all_outputs {
                if let Some(name) = request.preferred_output.as_deref() {
                    matched = crate::output::find_output(name)
                        .map(|o| o.geometry)
                        .filter(|(_, _, w, h)| *w > 0 && *h > 0);
                }
            }
            if let Some(g) = matched {
                g
            } else {
                let setup = conn.setup();
                match setup.roots.first() {
                    Some(r) => (0, 0, r.width_in_pixels as i32, r.height_in_pixels as i32),
                    None => {
                        return CaptureResult::failure(
                            crate::capture_types::Backend::X11,
                            "no X11 screen",
                        )
                    }
                }
            }
        }
    };
    let (gx, gy, gw, gh) = geometry;
    if gw <= 0 || gh <= 0 {
        return CaptureResult::failure(
            crate::capture_types::Backend::X11,
            "invalid capture geometry",
        );
    }

    // X11 坐标不支持负值：若区域超出屏幕，裁剪到根窗口范围内。
    let setup = conn.setup();
    let root_w = setup.roots[0].width_in_pixels;
    let root_h = setup.roots[0].height_in_pixels;
    let clamp_x = gx.max(0);
    let clamp_y = gy.max(0);
    let clamp_w = (gx + gw).min(root_w as i32) - clamp_x;
    let clamp_h = (gy + gh).min(root_h as i32) - clamp_y;
    if clamp_w <= 0 || clamp_h <= 0 {
        return CaptureResult::failure(
            crate::capture_types::Backend::X11,
            "capture geometry outside the root window",
        );
    }

    let reply = match conn.get_image(
        ImageFormat::Z_PIXMAP,
        root,
        clamp_x as i16,
        clamp_y as i16,
        clamp_w as u16,
        clamp_h as u16,
        u32::MAX,
    ) {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => r,
            Err(e) => {
                // XWayland（Wayland 会话 + X11 DISPLAY）下 root GetImage 通常
                // BadMatch（XWayland root 为 depth 32 的已知限制），给出明确提示。
                let is_xwayland =
                    std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase() == "wayland";
                return CaptureResult::failure(
                    crate::capture_types::Backend::X11,
                    if is_xwayland {
                        format!("X11 root capture is unavailable on XWayland: {e}")
                    } else {
                        format!("X11 get_image failed: {e}")
                    },
                );
            }
        },
        Err(e) => {
            return CaptureResult::failure(
                crate::capture_types::Backend::X11,
                format!("X11 get_image request failed: {e}"),
            )
        }
    };

    let lsb = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST;
    let bpp = if reply.depth == 24 || reply.depth == 32 {
        reply.data.len() / (clamp_w as usize * clamp_h as usize)
    } else {
        0
    };
    let image = match convert_pixels(&reply.data, clamp_w as u32, clamp_h as u32, bpp, lsb) {
        Some(img) => img,
        None => {
            return CaptureResult::failure(
                crate::capture_types::Backend::X11,
                format!(
                    "unsupported X11 capture depth {} bpp {}",
                    reply.depth, bpp
                ),
            )
        }
    };

    // include_cursor：XFixes 读光标并合成到截图（X11 自研，不依赖系统截图）。
    let mut image = image;
    if request.include_cursor {
        paint_cursor_into(&conn, &mut image, (clamp_x, clamp_y));
    }

    let result_geom = if (gx, gy, gw, gh) == (clamp_x, clamp_y, clamp_w, clamp_h) {
        request.source_geometry
    } else {
        Some((clamp_x, clamp_y, clamp_w, clamp_h))
    };

    CaptureResult {
        image: Some(image),
        error: None,
        source_geometry: result_geom,
        output_name: None,
        backend: crate::capture_types::Backend::X11,
        frame_time_ms: 0,
    }
}

/// 用 XFixes 光标图像合成到截图（位置为光标热点在根窗口的坐标，
/// 再换算到截图区域内的偏移）。
fn paint_cursor_into(
    conn: &x11rb::rust_connection::RustConnection,
    image: &mut image::RgbaImage,
    region_origin: (i32, i32),
) {
    use x11rb::protocol::xfixes::ConnectionExt as XFixesExt;

    let reply = match conn.xfixes_get_cursor_image() {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => r,
            Err(_) => return,
        },
        Err(_) => return,
    };
    let (cw, ch) = (reply.width as i32, reply.height as i32);
    if cw <= 0 || ch <= 0 || reply.cursor_image.len() as i32 != cw * ch {
        return;
    }
    // 光标左上角在根窗口的坐标 = (x - xhot, y - yhot)。
    let cursor_left = reply.x as i32 - reply.xhot as i32;
    let cursor_top = reply.y as i32 - reply.yhot as i32;
    let (ox, oy) = region_origin;

    let img_w = image.width() as i32;
    let img_h = image.height() as i32;
    let pixels = image.as_mut();

    for py in 0..ch {
        let img_y = cursor_top + py - oy;
        if img_y < 0 || img_y >= img_h {
            continue;
        }
        for px in 0..cw {
            let img_x = cursor_left + px - ox;
            if img_x < 0 || img_x >= img_w {
                continue;
            }
            let argb = reply.cursor_image[(py * cw + px) as usize];
            let a = ((argb >> 24) & 0xff) as u8;
            if a == 0 {
                continue;
            }
            let r = ((argb >> 16) & 0xff) as u8;
            let g = ((argb >> 8) & 0xff) as u8;
            let b = (argb & 0xff) as u8;
            let dst = (img_y as usize * img_w as usize + img_x as usize) * 4;
            pixels[dst] = r;
            pixels[dst + 1] = g;
            pixels[dst + 2] = b;
            pixels[dst + 3] = a;
        }
    }
}

/// 抓取单个窗口自身内容（XComposite 命名 pixmap）。
pub(crate) fn capture_window_content(
    window: &WindowObjectInfo,
    _include_cursor: bool,
) -> Result<image::RgbaImage, String> {
    let xid = window.id.trim_start_matches("0x");
    let window_id = u32::from_str_radix(xid, 16)
        .map_err(|_| format!("invalid X11 window id: {}", window.id))?;
    let (x, y, w, h) = window.rect;
    if w <= 0 || h <= 0 {
        return Err("invalid window rect".to_string());
    }
    let _ = (x, y);

    let conn = connection()?;
    // 协商 Composite 版本。
    conn.composite_query_version(0, 4)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("composite version query failed: {e}"))?;

    // 判断合成管理器是否运行（决定是否需主动重定向）。
    let sel = conn
        .intern_atom(false, b"_NET_WM_CM_S0")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    let compositor_running = conn
        .get_selection_owner(sel)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .owner
        != 0;

    let mut redirected = false;
    if !compositor_running {
        conn.composite_redirect_window(window_id, Redirect::AUTOMATIC)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| format!("composite redirect failed: {e}"))?;
        redirected = true;
    }

    // NameWindowPixmap 要求一个"有效但未使用"的 pixmap ID（服务器会创建并
    // 命名该 pixmap），不能预先 create_pixmap——预创建会导致 BadIDChoice。
    let pixmap = conn
        .generate_id()
        .map_err(|e| format!("generate_id failed: {e}"))?;
    conn.composite_name_window_pixmap(window_id, pixmap)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| format!("name_window_pixmap failed: {e}"))?;

    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, w as u16, h as u16, u32::MAX)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("get_image window content failed: {e}"))?;

    let _ = conn.free_pixmap(pixmap);
    if redirected {
        let _ = conn.composite_unredirect_window(window_id, Redirect::AUTOMATIC);
    }

    let lsb = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST;
    let bpp = if reply.depth == 24 || reply.depth == 32 {
        reply.data.len() / (w as usize * h as usize)
    } else {
        0
    };
    convert_pixels(&reply.data, w as u32, h as u32, bpp, lsb)
        .ok_or_else(|| format!("unsupported window capture depth {} bpp {}", reply.depth, bpp))
}

#[cfg(test)]
mod tests {
    use super::convert_pixels;

    #[test]
    fn converts_bgrx32_to_rgba() {
        // LSB-first, 4bpp (depth 24 屏幕的 32-bit 返回): 每像素 [B, G, R, X]
        let data = [0x11u8, 0x22, 0x33, 0xff, 0x55, 0x66, 0x77, 0xff];
        let img = convert_pixels(&data, 2, 1, 4, true).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x33, 0x22, 0x11, 0xff]);
        assert_eq!(&px[4..8], &[0x77, 0x66, 0x55, 0xff]);
    }

    #[test]
    fn converts_bgr24_to_rgba() {
        // LSB-first, 3bpp: 每像素 [B, G, R]，行按 32-bit 对齐（含 padding）
        let data = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x00, 0x00];
        let img = convert_pixels(&data, 2, 1, 3, true).expect("convert");
        let px = img.as_raw();
        assert_eq!(&px[0..4], &[0x03, 0x02, 0x01, 0xff]);
        assert_eq!(&px[4..8], &[0x06, 0x05, 0x04, 0xff]);
    }

    #[test]
    fn rejects_unknown_bpp() {
        assert!(convert_pixels(&[0u8; 8], 2, 1, 5, true).is_none());
    }
}
