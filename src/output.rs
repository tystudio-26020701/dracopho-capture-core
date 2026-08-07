//! 输出（显示器）枚举与选择。
//!
//! 自研实现：
//! - X11：XRandR 枚举真实物理输出的名称与像素几何。
//! - Wayland：wl_output v4 协议枚举输出名称与逻辑几何（自研客户端，零弹窗）。
//!   portal screencast 多流选屏依赖此枚举：每个流的 position/size 与
//!   `preferred_output` 名称解析出的几何匹配。
//!
//! 无头铁律：纯查询，不建窗口、不弹窗。

use std::collections::HashMap;

use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::Event as RegistryEvent;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use x11rb::connection::Connection as XConnection;
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
/// Wayland 会话优先用自研 wl_output 客户端枚举（逻辑几何，与 portal screencast
/// 流的 position/size 同坐标系，供多流选屏匹配）；无 Wayland 时回退 X11 XRandR。
pub fn list_outputs() -> Vec<OutputInfo> {
    let is_wayland = std::env::var("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .to_lowercase()
        == "wayland";
    if is_wayland {
        let wayland = wayland_outputs();
        if !wayland.is_empty() {
            return wayland;
        }
    }
    x11_outputs()
}

/// 按名称查找输出。
pub fn find_output(name: &str) -> Option<OutputInfo> {
    list_outputs().into_iter().find(|o| o.name == name)
}

/// Wayland wl_output v4 枚举状态。
struct WaylandState {
    outputs: Vec<WlOutput>,
    names: HashMap<ObjectId, String>,
    positions: HashMap<ObjectId, (i32, i32)>,
    sizes: HashMap<ObjectId, (i32, i32)>,
    done: HashMap<ObjectId, bool>,
}

impl WaylandState {
    fn new() -> Self {
        Self {
            outputs: Vec::new(),
            names: HashMap::new(),
            positions: HashMap::new(),
            sizes: HashMap::new(),
            done: HashMap::new(),
        }
    }
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: RegistryEvent,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let RegistryEvent::Global { name, interface, .. } = event {
            if interface == "wl_output" {
                state
                    .outputs
                    .push(registry.bind::<WlOutput, (), WaylandState>(name, 4, qh, ()));
            }
        }
    }
}

impl Dispatch<WlOutput, ()> for WaylandState {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = output.id();
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                state.positions.insert(id, (x, y));
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                // 只记录当前模式（is_current 标志 0x1）。GNOME 常以
                // Current|Preferred(0x3) 组合发送，必须用 contains 判断。
                let current = match &flags {
                    WEnum::Value(m) => m.contains(wl_output::Mode::Current),
                    WEnum::Unknown(v) => v & 0x1 != 0,
                };
                if current {
                    state.sizes.insert(id, (width, height));
                }
            }
            wl_output::Event::Scale { factor } => {
                // 物理像素 → 逻辑尺寸换算：逻辑宽高 = 物理宽高 / scale。
                if let Some((w, h)) = state.sizes.get(&id).copied() {
                    let f = factor.max(1);
                    state.sizes.insert(id, (w / f, h / f));
                }
            }
            wl_output::Event::Name { name } => {
                state.names.insert(id, name);
            }
            wl_output::Event::Done => {
                state.done.insert(id, true);
            }
            _ => {}
        }
    }
}

/// 自研 wl_output v4 枚举：返回 (名称, 逻辑几何)。
fn wayland_outputs() -> Vec<OutputInfo> {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut state = WaylandState::new();
    let mut queue = conn.new_event_queue::<WaylandState>();
    let qh = queue.handle();
    let display = conn.display();
    let _registry = display.get_registry(&qh, ());
    if queue.roundtrip(&mut state).is_err() {
        return Vec::new();
    }
    if state.outputs.is_empty() {
        return Vec::new();
    }

    // 持续派发直到全部输出收到 Done（几何/模式/名称/缩放），带超时兜底。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.done.len() < state.outputs.len() {
        if std::time::Instant::now() >= deadline {
            break;
        }
        if queue.blocking_dispatch(&mut state).is_err() {
            break;
        }
    }

    let mut out = Vec::new();
    for output in state.outputs.iter() {
        let id = output.id();
        let name = state.names.get(&id).cloned().unwrap_or_default();
        let Some((x, y)) = state.positions.get(&id).copied() else {
            continue;
        };
        let Some((w, h)) = state.sizes.get(&id).copied() else {
            continue;
        };
        if w <= 0 || h <= 0 {
            continue;
        }
        out.push(OutputInfo {
            name,
            geometry: (x, y, w, h),
        });
    }
    out
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
    use super::{wayland_outputs, OutputInfo};

    #[test]
    fn output_info_default_is_empty() {
        let o = OutputInfo::default();
        assert!(o.name.is_empty());
        assert_eq!(o.geometry, (0, 0, 0, 0));
    }

    /// 仅在真实 Wayland 会话下手动运行（`cargo test -- --ignored --nocapture`）。
    /// 无头 CI 环境无 WAYLAND_DISPLAY，故默认忽略。
    #[test]
    #[ignore]
    fn enumerates_wayland_outputs_on_live_session() {
        let out = wayland_outputs();
        assert!(!out.is_empty(), "wl_output enumeration returned nothing");
        for o in out {
            assert!(!o.name.is_empty());
            assert!(o.geometry.2 > 0 && o.geometry.3 > 0);
        }
    }
}
