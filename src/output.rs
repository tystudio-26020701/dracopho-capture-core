//! 输出（显示器）枚举与选择。
//!
//! 自研实现：
//! - X11：XRandR 枚举真实物理输出的名称与像素几何。
//! - Wayland：portal screencast 流的 position/size 即所选显示器几何，单流下
//!   无法编程指定具体显示器；wlroots 系经 wlr-screencopy 的 `-o` 选择。
//!
//! 无头铁律：纯查询，不建窗口、不弹窗。

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as RandrExt;

use crate::backend::x11;

/// 输出（显示器）信息。
#[derive(Debug, Clone, Default)]
pub struct OutputInfo {
    /// 输出名称（如 "HDMI-1" / "DP-1"）。
    pub name: String,
    /// 输出全局逻辑坐标矩形 (x, y, w, h)。
    pub geometry: (i32, i32, i32, i32),
}

/// 枚举当前会话的输出（显示器）。
///
/// X11 会话用 XRandR 枚举真实物理输出；XWayland 下返回 XWayland 可见的
/// 输出（尽力而为）。原生 Wayland（无 X11 DISPLAY）返回空 Vec——显示器
/// 几何由 screencast 流提供（见 `CaptureResult.source_geometry`）。
pub fn list_outputs() -> Vec<OutputInfo> {
    x11_outputs()
}

/// 按名称查找输出。
pub fn find_output(name: &str) -> Option<OutputInfo> {
    list_outputs().into_iter().find(|o| o.name == name)
}

/// X11 XRandR 输出枚举。
fn x11_outputs() -> Vec<OutputInfo> {
    let conn = match x11::connection() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let root = match conn.setup().roots.first() {
        Some(s) => s.root,
        None => return Vec::new(),
    };

    let resources = match conn.randr_get_screen_resources_current(root) {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };
    let config_timestamp = resources.config_timestamp;

    let mut out = Vec::new();
    for crtc in resources.crtcs {
        let info = match conn.randr_get_crtc_info(crtc, config_timestamp) {
            Ok(cookie) => match cookie.reply() {
                Ok(r) => r,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        // 仅统计激活的 CRTC（mode != 0 且 outputs 非空）。
        if info.mode == 0 || info.outputs.is_empty() || info.width == 0 || info.height == 0 {
            continue;
        }
        // 取该 CRTC 上第一个输出的名称。
        let name = info
            .outputs
            .first()
            .and_then(|output_id| {
                conn.randr_get_output_info(*output_id, config_timestamp)
                    .ok()?
                    .reply()
                    .ok()
            })
            .map(|o| String::from_utf8_lossy(&o.name).into_owned())
            .unwrap_or_default();
        out.push(OutputInfo {
            name,
            geometry: (info.x as i32, info.y as i32, info.width as i32, info.height as i32),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::OutputInfo;

    #[test]
    fn output_info_default_is_empty() {
        let o = OutputInfo::default();
        assert!(o.name.is_empty());
        assert_eq!(o.geometry, (0, 0, 0, 0));
    }
}
