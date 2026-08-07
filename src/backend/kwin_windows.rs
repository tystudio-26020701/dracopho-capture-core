//! KDE 窗口枚举（KWin scripting D-Bus，纯 DBus、无新依赖、无 LGPL 派生代码）。
//!
//! 方案调研结论：KDE 窗口枚举有两条路——
//! 1. Plasma 专有 Wayland 协议 `plasma-window-management.xml`（LGPL-2.1-or-later，
//!   需 vendor 协议 + wayland-scanner 代码生成，引入 LGPL 派生代码）；
//! 2. **KWin scripting D-Bus**（本实现）：调用 `org.kde.kwin.Scripting` 加载
//!    一次性 JS 脚本，脚本在 KWin 内用 `workspace.stackingOrder`/`windowList()`
//!    枚举窗口（含 `internalId` UUID），再经 `callDBus` 把 JSON 回传本进程。
//!    与 kdotool（Rust，生产可用）及 mark-shot 自带
//!    `mark-shot-window-detection-kde`（生产在用）同机制，功能完整。
//!
//! 选型 2：零协议 vendor、零许可风险、零新增依赖（仅 zbus，已有）。
//!
//! 安全设计（防窗口列表注入）：
//! - 每次调用生成不可猜测的随机回传路径（`/org/dracopho/WindowDetection/<token>`），
//!   并把该路径写进脚本——伪造方需要同时知道总线唯一名与随机路径；
//! - 回传回调校验发送者 = `org.kde.KWin` 的当前总线属主，非法调用直接丢弃；
//! - 一次性脚本写入私有目录（`XDG_RUNTIME_DIR` 或临时目录下 `dracopho-capture-<pid>`
//!   ，0o700），文件 `O_EXCL` 创建 + 0o600，杜绝符号链接劫持与内容泄漏。

use std::io::Read;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use zbus::interface;

use crate::window::WindowInfo;

const SCRIPTING_SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_IFACE: &str = "org.kde.kwin.Script";

/// 脚本回传 JSON 的目标（本进程私有接口，路径带随机 token）。
const RESULT_PATH_PREFIX: &str = "/org/dracopho/WindowDetection";
const RESULT_IFACE: &str = "org.dracopho.WindowDetection";

/// 接收 KWin 脚本经 `callDBus` 回传的 JSON 负载（只收第一份，且校验发送者）。
struct WindowResultHandler {
    tx: Mutex<Option<mpsc::Sender<String>>>,
    /// `org.kde.KWin` 的总线属主唯一名；无法解析时为 None（跳过发送者校验）。
    expected_sender: Option<String>,
}

#[interface(name = "org.dracopho.WindowDetection")]
impl WindowResultHandler {
    fn result(
        &self,
        #[zbus(header)]
        hdr: zbus::message::Header<'_>,
        payload: String,
    ) -> zbus::fdo::Result<()> {
        // 发送者校验：只接受来自 `org.kde.KWin` 属主的回调，丢弃伪造调用。
        if let Some(expected) = self.expected_sender.as_deref() {
            let sender = hdr.sender().map(|s| s.to_string());
            if sender.as_deref() != Some(expected) {
                return Ok(());
            }
        }
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(payload);
        }
        Ok(())
    }
}

/// 生成一次性 KWin JS 脚本：枚举窗口（含 internalId UUID）并回传 JSON。
fn script_source(unique_name: &str, result_path: &str) -> String {
    format!(
        r#"(function() {{
    var results = [];
    var seen = {{}};
    var windows = workspace.stackingOrder;
    if (!windows || typeof windows.length !== "number") {{
        try {{ windows = workspace.windowList(); }} catch (e) {{ windows = []; }}
    }}
    for (var i = 0; i < windows.length; ++i) {{
        var w = windows[i];
        if (!w) continue;
        var rect = null;
        try {{ rect = w.frameGeometry; }} catch (e) {{}}
        if (!rect || typeof rect.width !== "number") {{
            rect = {{ x: w.x, y: w.y, width: w.width, height: w.height }};
        }}
        if (!rect || rect.width <= 1 || rect.height <= 1) continue;
        var uuid = "";
        try {{ uuid = String(w.internalId || ""); }} catch (e) {{}}
        var key = uuid || (rect.x + "," + rect.y + "," + rect.width + "," + rect.height);
        if (seen[key]) continue;
        seen[key] = true;
        var cls = "";
        try {{ cls = String(w.resourceClass || w.windowClass || ""); }} catch (e) {{}}
        var inst = "";
        try {{ inst = String(w.resourceName || ""); }} catch (e) {{}}
        var title = "";
        try {{ title = String(w.caption || ""); }} catch (e) {{}}
        var output = "";
        try {{ output = String((w.output && w.output.name) || ""); }} catch (e) {{}}
        results.push({{
            id: uuid,
            title: title,
            class: cls,
            instance: inst,
            pid: (typeof w.pid === "number") ? w.pid : -1,
            x: Math.round(rect.x),
            y: Math.round(rect.y),
            width: Math.round(rect.width),
            height: Math.round(rect.height),
            minimized: (w.minimized === true),
            visible: (w.visible !== false),
            output: output,
            zOrder: i
        }});
    }}
    try {{
        callDBus("{unique}", "{result_path}", "{result_iface}", "result",
                 JSON.stringify({{ windows: results }}));
    }} catch (e) {{
        print("dracopho-kde-wm callDBus failed: " + e);
    }}
}})();
"#,
        unique = unique_name,
        result_path = result_path,
        result_iface = RESULT_IFACE
    )
}

/// 生成 n 字节随机十六进制（/dev/urandom；失败时退化为 pid+时间戳）。
fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if ok {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    let fallback = format!(
        "{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut out = String::new();
    for c in fallback.bytes().take(n * 2) {
        out.push_str(&format!("{c:02x}"));
    }
    out
}

fn load_script(
    conn: &zbus::blocking::Connection,
    path: &str,
    plugin: &str,
) -> Result<i32, String> {
    let reply = conn
        .call_method(
            Some(SCRIPTING_SERVICE),
            SCRIPTING_PATH,
            Some(SCRIPTING_IFACE),
            "loadScript",
            &(path.to_string(), plugin.to_string()),
        )
        .map_err(|e| format!("KWin loadScript failed: {e}"))?;
    let id: i32 = reply
        .body()
        .deserialize()
        .map_err(|e| format!("KWin loadScript reply decode failed: {e}"))?;
    Ok(id)
}

fn run_script(conn: &zbus::blocking::Connection, script_id: i32) -> Result<(), String> {
    let script_path = format!("{SCRIPTING_PATH}/Script{script_id}");
    conn.call_method(
        Some(SCRIPTING_SERVICE),
        script_path.as_str(),
        Some(SCRIPT_IFACE),
        "run",
        &(),
    )
    .map_err(|e| format!("KWin Script.run failed: {e}"))?;
    Ok(())
}

fn unload_script(conn: &zbus::blocking::Connection, plugin: &str) {
    let _ = conn.call_method(
        Some(SCRIPTING_SERVICE),
        SCRIPTING_PATH,
        Some(SCRIPTING_IFACE),
        "unloadScript",
        &(plugin.to_string(),),
    );
}

/// 等待脚本回传负载；期间用廉价 DBus 调用泵动连接，驱动对象服务器派发回调。
fn wait_payload(
    conn: &zbus::blocking::Connection,
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(payload) = rx.try_recv() {
            return Some(payload);
        }
        if Instant::now() >= deadline {
            return None;
        }
        // 任意一次阻塞 DBus 调用都会驱动内部运行时，让 socket reader 与
        // obj_server 任务处理进入的回调（KWin 脚本 callDBus 到本进程唯一名）。
        let _ = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetId",
            &(),
        );
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// 进程私有一次性脚本目录：`$XDG_RUNTIME_DIR/dracopho-capture-<pid>`（回退
/// 临时目录），创建即收紧 0o700。
fn private_script_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("dracopho-capture-{}", std::process::id()));
    if let Ok(()) = std::fs::create_dir_all(&dir) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

/// 以 `O_EXCL` + 0o600 在私有目录写入一次性脚本。
fn write_private_script(dir: &std::path::Path, name: &str, contents: &str) -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(name);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true).mode(0o600);
    let mut file = opts
        .open(&path)
        .map_err(|e| format!("cannot create script file {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut file, contents.as_bytes())
        .map_err(|e| format!("cannot write script file {}: {e}", path.display()))?;
    Ok(path)
}

/// 枚举 KDE 窗口（KWin scripting，无头静默：不建窗、不弹窗、不干扰用户）。
///
/// `include_hidden=true` 时保留最小化窗口（供 PID/进程定位）。
/// 失败/非 KDE 返回空 Vec。
pub fn list_kde_windows(include_hidden: bool) -> Vec<WindowInfo> {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return Vec::new();
    };
    let Some(unique) = conn.unique_name().map(|u| u.to_string()) else {
        return Vec::new();
    };

    // 不可猜测的回传路径 token + 期望发送者（org.kde.KWin 总线属主）。
    let token = random_hex(12);
    let result_path = format!("{RESULT_PATH_PREFIX}/{token}");
    let expected_sender: Option<String> = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(SCRIPTING_SERVICE.to_string(),),
        )
        .ok()
        .and_then(|r| r.body().deserialize::<String>().ok());

    let (tx, rx) = mpsc::channel::<String>();
    let handler = WindowResultHandler {
        tx: Mutex::new(Some(tx)),
        expected_sender,
    };
    if !conn.object_server().at(result_path.as_str(), handler).unwrap_or(false) {
        // 路径已被占用（token 碰撞，理论上不可能）：放弃本次，避免串扰。
        return Vec::new();
    }

    // 一次性脚本：进程私有目录 + O_EXCL 创建，杜绝符号链接劫持/内容泄漏。
    let dir = private_script_dir();
    let script_name = format!(
        "wm-{}-{}.js",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let script_path = match write_private_script(&dir, &script_name, &script_source(&unique, &result_path)) {
        Ok(p) => p,
        Err(_) => {
            let _ = conn.object_server().remove::<WindowResultHandler, _>(result_path.as_str());
            return Vec::new();
        }
    };
    let plugin = format!("dracopho-kde-wm-{}", std::process::id());

    let payload = match load_script(&conn, &script_path.to_string_lossy(), &plugin) {
        Ok(id) if id >= 0 => {
            let _ = run_script(&conn, id);
            wait_payload(&conn, &rx, Duration::from_millis(2000))
        }
        Ok(id) => {
            // 加载成功但 id 为负：KWin 拒绝加载，无需 run。
            let _ = id;
            wait_payload(&conn, &rx, Duration::from_millis(300))
        }
        Err(_) => None,
    };
    unload_script(&conn, &plugin);
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_dir(&dir);
    let _ = conn.object_server().remove::<WindowResultHandler, _>(result_path.as_str());

    let Some(payload) = payload else {
        return Vec::new();
    };
    parse_windows(&payload, include_hidden)
}

/// 解析 KWin 脚本回传的 JSON 窗口列表。
fn parse_windows(payload: &str, include_hidden: bool) -> Vec<WindowInfo> {
    let root: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(windows) = root.get("windows").and_then(|w| w.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in windows.iter() {
        let Some(obj) = item.as_object() else { continue };
        let num = |k: &str| obj.get(k).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let w = num("width");
        let h = num("height");
        if w <= 0 || h <= 0 {
            continue;
        }
        if !include_hidden && obj.get("minimized").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let str_ = |k: &str| obj.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        out.push(WindowInfo {
            id: str_("id"),
            title: str_("title"),
            class: str_("class"),
            instance: str_("instance"),
            pid: obj.get("pid").and_then(|v| v.as_i64()).unwrap_or(-1),
            geometry: (num("x"), num("y"), w, h),
            monitor: str_("output"),
            z_order: Some(num("zOrder")),
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_windows;

    #[test]
    fn parses_kwin_json() {
        let payload = r#"{"windows":[
            {"id":"38b2f62d-1234-4abc-9def-0123456789ab","title":"DracoPho","class":"dracopho",
             "instance":"dracopho","pid":4242,"x":0,"y":0,"width":800,"height":600,
             "minimized":false,"visible":true,"output":"HDMI-1","zOrder":0},
            {"id":"","title":"Minimized","class":"foo","instance":"","pid":-1,
             "x":10,"y":10,"width":100,"height":50,"minimized":true,"visible":false,
             "output":"","zOrder":1}
        ]}"#;
        let windows = parse_windows(payload, false);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "38b2f62d-1234-4abc-9def-0123456789ab");
        assert_eq!(windows[0].title, "DracoPho");
        assert_eq!(windows[0].class, "dracopho");
        assert_eq!(windows[0].pid, 4242);
        assert_eq!(windows[0].geometry, (0, 0, 800, 600));
        assert_eq!(windows[0].monitor, "HDMI-1");
        assert_eq!(windows[0].z_order, Some(0));

        let all = parse_windows(payload, true);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].title, "Minimized");
        assert_eq!(all[1].geometry, (10, 10, 100, 50));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_windows("not json", false).is_empty());
        assert!(parse_windows("{}", false).is_empty());
    }

    #[test]
    fn random_hex_is_unique() {
        let a = super::random_hex(8);
        let b = super::random_hex(8);
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
    }
}
