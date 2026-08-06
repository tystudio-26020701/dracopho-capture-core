//! 窗口枚举与选择。
//!
//! 自研实现，不调用任何"系统自带截图"服务：
//! - X11：直接用 X11 协议枚举 `_NET_CLIENT_LIST_STACKING` 并读取各窗口的
//!   title / WM_CLASS / _NET_WM_PID / frame extents。
//! - GNOME Wayland：经随软件自带的 MarkShotScrollHelper 扩展的
//!   `WindowGeometries`（D-Bus）获取窗口列表。
//! - wlroots 系：wlr-foreign-toplevel 协议（后续迭代）。
//!
//! 无头铁律：枚举是纯查询，不建窗口、不弹窗、不干扰任何用户进程。

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as XE, GetPropertyReply};

use crate::backend::x11;

/// 窗口信息（与 C++ mark-shot WindowInfo 对齐）。
#[derive(Debug, Clone, Default)]
pub struct WindowInfo {
    /// 稳定标识（X11 为十六进制窗口 id；GNOME 扩展为空）。
    pub id: String,
    pub title: String,
    pub class: String,
    pub instance: String,
    /// 属主进程 id，未知为 -1。
    pub pid: i64,
    /// 窗口全局坐标矩形 (x, y, w, h)。
    pub geometry: (i32, i32, i32, i32),
    /// 显示器名/序号（尽力而为）。
    pub monitor: String,
    /// 工作区（尽力而为）。
    pub workspace: String,
    /// 堆叠顺序（自底向上），未知为 None。
    pub z_order: Option<i32>,
}

/// 窗口匹配规则（对齐 C++ `--window-by`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowMatch {
    /// 精确 id（X11 十六进制窗口 id）。
    Id(String),
    /// 标题精确或子串（大小写不敏感）。
    Title(String),
    /// WM_CLASS class / app_id。
    Class(String),
    /// WM_CLASS instance / app_name。
    Instance(String),
    /// 枚举序号。
    Index(usize),
    /// 属主进程 id。
    Pid(i64),
    /// 进程名（/proc 读取，尽力而为）。
    Process(String),
    /// 自动匹配：id → 精确标题 → class → 序号 → pid → 子串。
    Auto(String),
}

impl WindowMatch {
    /// 判断窗口是否命中。
    pub fn matches(&self, info: &WindowInfo, index: usize) -> bool {
        match self {
            WindowMatch::Id(spec) => !info.id.is_empty() && info.id == *spec,
            WindowMatch::Title(spec) => {
                !info.title.is_empty()
                    && (info.title == *spec || info.title.to_lowercase().contains(&spec.to_lowercase()))
            }
            WindowMatch::Class(spec) => {
                !info.class.is_empty()
                    && (info.class == *spec
                        || info.class.to_lowercase().contains(&spec.to_lowercase())
                        || info.instance.to_lowercase().contains(&spec.to_lowercase()))
            }
            WindowMatch::Instance(spec) => {
                !info.instance.is_empty() && info.instance.to_lowercase().contains(&spec.to_lowercase())
            }
            WindowMatch::Index(wanted) => *wanted == index,
            WindowMatch::Pid(wanted) => *wanted > 0 && info.pid == *wanted,
            WindowMatch::Process(spec) => {
                info.pid > 0 && process_name_for_pid(info.pid).is_some_and(|name| {
                    name == *spec
                        || name.to_lowercase().contains(&spec.to_lowercase())
                        || spec.to_lowercase().contains(&name.to_lowercase())
                })
            }
            WindowMatch::Auto(spec) => {
                if !info.id.is_empty() && info.id == *spec {
                    return true;
                }
                if info.title == *spec {
                    return true;
                }
                if info.class == *spec || info.instance == *spec {
                    return true;
                }
                if let Ok(wanted) = spec.parse::<usize>() {
                    if wanted == index {
                        return true;
                    }
                }
                if info.pid > 0 && *spec == info.pid.to_string() {
                    return true;
                }
                if !info.title.is_empty() && info.title.to_lowercase().contains(&spec.to_lowercase()) {
                    return true;
                }
                if !info.class.is_empty() && info.class.to_lowercase().contains(&spec.to_lowercase()) {
                    return true;
                }
                !info.instance.is_empty()
                    && info.instance.to_lowercase().contains(&spec.to_lowercase())
            }
        }
    }
}

/// 从字符串解析匹配规则（对齐 C++ `--window-by` 的 auto/id/title/class/index/pid/process）。
pub fn parse_match(spec: &str, by: Option<&str>) -> Result<WindowMatch, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty window selector".to_string());
    }
    let mode = by.unwrap_or("auto").to_lowercase();
    match mode.as_str() {
        "id" => Ok(WindowMatch::Id(spec.to_string())),
        "title" => Ok(WindowMatch::Title(spec.to_string())),
        "class" => Ok(WindowMatch::Class(spec.to_string())),
        "instance" => Ok(WindowMatch::Instance(spec.to_string())),
        "index" => spec
            .parse::<usize>()
            .map(WindowMatch::Index)
            .map_err(|_| "invalid index selector".to_string()),
        "pid" => spec
            .parse::<i64>()
            .map(WindowMatch::Pid)
            .map_err(|_| "invalid pid selector".to_string()),
        "process" => Ok(WindowMatch::Process(spec.to_string())),
        "auto" => Ok(WindowMatch::Auto(spec.to_string())),
        other => Err(format!("invalid --window-by mode \"{other}\"")),
    }
}

/// 读取 /proc/<pid>/comm 获取进程名（Linux）。空表示未知。
fn process_name_for_pid(pid: i64) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = comm.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// 是否 GNOME Wayland 会话。
fn is_gnome_wayland() -> bool {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    session == "wayland" && desktop.contains("gnome")
}

/// 枚举当前会话的所有可见窗口。
///
/// 平台：X11（原生 X11 / XWayland 自研枚举）→ GNOME Wayland（自研扩展）。
/// 返回空 Vec 表示平台不支持枚举。
pub fn list_windows() -> Vec<WindowInfo> {
    if is_gnome_wayland() {
        let gnome = gnome_windows();
        if !gnome.is_empty() {
            return gnome;
        }
    }
    x11_windows()
}

// ---------------------------------------------------------------------------
// X11 自研枚举
// ---------------------------------------------------------------------------

fn intern_atom(conn: &x11rb::rust_connection::RustConnection, name: &[u8]) -> Option<Atom> {
    conn.intern_atom(false, name)
        .ok()?
        .reply()
        .ok()
        .map(|r| r.atom)
}

fn get_prop<B: Into<Atom>>(
    conn: &x11rb::rust_connection::RustConnection,
    window: u32,
    prop: Atom,
    type_: B,
    max_len: u32,
) -> Option<GetPropertyReply> {
    conn.get_property(false, window, prop, type_, 0, max_len)
        .ok()?
        .reply()
        .ok()
}

fn prop_u32s(reply: &GetPropertyReply) -> Vec<u32> {
    if reply.format != 32 {
        return Vec::new();
    }
    reply
        .value
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// X11 自研窗口枚举。
fn x11_windows() -> Vec<WindowInfo> {
    let conn = match x11::connection() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let root = match conn.setup().roots.first() {
        Some(s) => s.root,
        None => return Vec::new(),
    };

    let atom_client_list = intern_atom(&conn, b"_NET_CLIENT_LIST_STACKING");
    let atom_client_list_alt = intern_atom(&conn, b"_NET_CLIENT_LIST");
    let atom_wm_state = intern_atom(&conn, b"WM_STATE");
    let atom_net_wm_state = intern_atom(&conn, b"_NET_WM_STATE");
    let atom_hidden = intern_atom(&conn, b"_NET_WM_STATE_HIDDEN");
    let atom_frame_extents = intern_atom(&conn, b"_NET_FRAME_EXTENTS");
    let atom_net_wm_name = intern_atom(&conn, b"_NET_WM_NAME");
    let atom_wm_name = intern_atom(&conn, b"WM_NAME");
    let atom_wm_class = intern_atom(&conn, b"WM_CLASS");
    let atom_net_wm_pid = intern_atom(&conn, b"_NET_WM_PID");

    let read_text_prop = |window: u32| -> String {
        if let Some(reply) = get_prop(&conn, window, atom_net_wm_name.unwrap_or(0), AtomEnum::ANY, 1024) {
            if reply.format == 8 {
                let text = String::from_utf8_lossy(&reply.value).trim().to_string();
                if !text.is_empty() {
                    return text;
                }
            }
        }
        if let Some(reply) = get_prop(&conn, window, atom_wm_name.unwrap_or(0), AtomEnum::ANY, 1024) {
            if reply.format == 8 {
                return String::from_utf8_lossy(&reply.value).trim().to_string();
            }
        }
        String::new()
    };

    let read_class = |window: u32| -> (String, String) {
        let Some(reply) = get_prop(&conn, window, atom_wm_class.unwrap_or(0), AtomEnum::ANY, 1024)
        else {
            return (String::new(), String::new());
        };
        if reply.format != 8 {
            return (String::new(), String::new());
        }
        let parts: Vec<&str> = reply.value.split(|&b| b == 0).map(|s| std::str::from_utf8(s).unwrap_or("")).collect();
        let instance = parts.first().unwrap_or(&"").to_string();
        let class = parts.get(1).unwrap_or(&"").to_string();
        (instance, class)
    };

    let read_pid = |window: u32| -> i64 {
        let Some(reply) = get_prop(&conn, window, atom_net_wm_pid.unwrap_or(0), AtomEnum::CARDINAL, 1)
        else {
            return -1;
        };
        prop_u32s(&reply).first().copied().map(|v| v as i64).unwrap_or(-1)
    };

    let read_hidden = |window: u32| -> bool {
        let (Some(states), Some(hidden)) = (atom_net_wm_state, atom_hidden) else {
            return false;
        };
        let Some(reply) = get_prop(&conn, window, states, AtomEnum::ATOM, 64) else {
            return false;
        };
        prop_u32s(&reply).contains(&hidden)
    };

    let window_geometry = |window: u32| -> Option<(i32, i32, i32, i32)> {
        let geo = conn.get_geometry(window).ok()?.reply().ok()?;
        let trans = conn.translate_coordinates(window, root, 0, 0).ok()?.reply().ok()?;
        let (mut x, mut y) = (trans.dst_x as i32, trans.dst_y as i32);
        let (mut w, mut h) = (geo.width as i32, geo.height as i32);
        if let Some(reply) = get_prop(&conn, window, atom_frame_extents.unwrap_or(0), AtomEnum::CARDINAL, 4) {
            let ext = prop_u32s(&reply);
            if ext.len() >= 4 {
                x -= ext[0] as i32;
                y -= ext[2] as i32;
                w += (ext[0] + ext[1]) as i32;
                h += (ext[2] + ext[3]) as i32;
            }
        }
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((x, y, w, h))
    };

    let mut windows = Vec::new();
    let client_list = atom_client_list
        .and_then(|a| get_prop(&conn, root, a, AtomEnum::WINDOW, 4096))
        .or_else(|| {
            atom_client_list_alt.and_then(|a| get_prop(&conn, root, a, AtomEnum::WINDOW, 4096))
        });

    let list: Vec<u32> = match client_list {
        Some(reply) => prop_u32s(&reply),
        None => Vec::new(),
    };

    if !list.is_empty() {
        for (index, window) in list.into_iter().enumerate() {
            // 跳过隐藏/最小化窗口（无头截图默认不截隐藏窗口）。
            if read_hidden(window) {
                continue;
            }
            let Some(geometry) = window_geometry(window) else {
                continue;
            };
            let (instance, class) = read_class(window);
            let mut info = WindowInfo {
                id: format!("0x{window:x}"),
                title: read_text_prop(window),
                class,
                instance,
                pid: read_pid(window),
                geometry,
                z_order: Some(index as i32),
                ..Default::default()
            };
            // 忽略 WM_STATE Iconic（最小化）窗口。
            if let Some(reply) = get_prop(&conn, window, atom_wm_state.unwrap_or(0), AtomEnum::CARDINAL, 2) {
                let vals = prop_u32s(&reply);
                if vals.first().copied() == Some(3) {
                    continue;
                }
            }
            // 去重（XWayland 下可能重复）。
            if windows.iter().any(|w: &WindowInfo| w.id == info.id) {
                continue;
            }
            info.z_order = Some(windows.len() as i32);
            windows.push(info);
        }
        return windows;
    }

    // 无 _NET_CLIENT_LIST 时回退窗口树遍历。
    let mut stack = vec![root];
    while let Some(parent) = stack.pop() {
        let tree = match conn.query_tree(parent) {
            Ok(c) => match c.reply() {
                Ok(r) => r,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        for child in tree.children {
            stack.push(child);
        }
        if parent != root {
            if let Some(geometry) = window_geometry(parent) {
                let (instance, class) = read_class(parent);
                let info = WindowInfo {
                    id: format!("0x{parent:x}"),
                    title: read_text_prop(parent),
                    class,
                    instance,
                    pid: read_pid(parent),
                    geometry,
                    z_order: Some(windows.len() as i32),
                    ..Default::default()
                };
                windows.push(info);
            }
        }
    }
    windows
}

// ---------------------------------------------------------------------------
// GNOME 自研扩展枚举
// ---------------------------------------------------------------------------

/// 经 MarkShotScrollHelper 扩展（随软件自带）的 WindowGeometries 枚举窗口。
fn gnome_windows() -> Vec<WindowInfo> {
    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let body: String = match conn.call_method(
        Some("org.gnome.Shell"),
        "/org/gnome/Shell/Extensions/MarkShotScrollHelper",
        Some("org.gnome.Shell.Extensions.MarkShotScrollHelper"),
        "WindowGeometries",
        &(),
    ) {
        Ok(reply) => match reply.body().deserialize() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };

    let root: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let windows = root.get("windows").and_then(|w| w.as_array());
    let Some(windows) = windows else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (index, item) in windows.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let num = |key: &str| obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let x = num("x");
        let y = num("y");
        let w = num("width");
        let h = num("height");
        if w <= 0 || h <= 0 {
            continue;
        }
        let str_ = |key: &str| obj.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let info = WindowInfo {
            id: String::new(),
            title: str_("title"),
            class: str_("class"),
            instance: str_("instance"),
            pid: -1,
            geometry: (x, y, w, h),
            monitor: str_("monitor"),
            workspace: str_("workspace"),
            z_order: Some(index as i32),
        };
        out.push(info);
    }
    out
}

/// 抓取窗口自身内容（X11 XComposite 命名 pixmap）。
///
/// 仅 X11 支持；GNOME/Wayland 上返回 None，调用方回退到"全屏帧 + 窗口矩形裁剪"。
pub fn capture_window_content(info: &WindowInfo) -> Option<image::RgbaImage> {
    if info.id.is_empty() {
        return None;
    }
    crate::capture_types::capture_window_object_content(
        &crate::capture_types::WindowObjectInfo {
            id: info.id.clone(),
            rect: info.geometry,
        },
        false,
    )
}
