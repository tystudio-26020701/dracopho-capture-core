//! 授权恢复 token 的本地持久化与无头预检。
//!
//! ScreenCast 持久化授权（xdg-desktop-portal 1.14+）：`--authorize` 交互授权一次
//! 后保存 restore token；此后无头模式用 token 恢复会话，无需再次弹窗。
//!
//! 预检（`verify_restore_token` / `verify_saved_token`）既可被库在执行无头
//! `Start` 前自动调用（静默拦截失效 token，绝不让合成器选择器弹出），也可由
//! 调用程序直接调用——"预检 + 持久化"做成库可执行、调用方可执行两种形式。

use std::fs;
use std::path::PathBuf;

/// token 文件路径：`~/.config/dracopho-capture-core/screencast-token`。
fn token_path() -> Option<PathBuf> {
    Some(
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
            })
            .join("dracopho-capture-core"),
    )
}

/// 读取已保存的 restore token。
pub fn restore_token() -> Option<String> {
    let path = token_path()?.join("screencast-token");
    fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 保存 restore token（交互授权成功后调用）。
pub fn save_restore_token(token: &str) {
    let Some(path) = token_path() else {
        return;
    };
    if let Err(e) = fs::create_dir_all(&path) {
        eprintln!("dracopho-capture: cannot create config dir: {e}");
        return;
    }
    let file = path.join("screencast-token");
    if let Err(e) = fs::write(&file, token) {
        eprintln!("dracopho-capture: cannot save screencast token: {e}");
        return;
    }
    // 敏感凭证：仅属主可读写。
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
}

/// 删除已保存的 token。
pub fn clear_restore_token() {
    if let Some(path) = token_path() {
        let _ = fs::remove_file(path.join("screencast-token"));
    }
}

/// 是否已授权。
pub fn authorized() -> bool {
    restore_token().is_some()
}

/// portal 权限存储的 D-Bus 名称与路径（无头模式静默校验 token 用）。
const PERMISSION_STORE_NAME: &str = "org.freedesktop.impl.portal.PermissionStore";
const PERMISSION_STORE_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";
/// ScreenCast 会话权限存储表名（与 xdg-desktop-portal 的 SCREEN_CAST_PERMISSION_TABLE 一致）。
const SCREENCAST_PERMISSION_TABLE: &str = "screencast";

/// 解析 portal 眼中本进程的 host app_id（复刻 xdg-desktop-portal 的
/// `xdp_app_info_host` / `get_app_from_pid` 逻辑）：
///   1. 读 /proc/self/cgroup，取最后一段为 systemd user unit 名；
///   2. 若以 "app-" 开头，按两条正则解析出 ApplicationID；
///   3. 要求存在 `<ApplicationID>.desktop`（`g_desktop_app_info_new` 语义）。
/// 任一环节失败 → 返回空串（portal 对 host 应用解析失败时 app_id 为空串，
/// 此时权限存储按空 app_id 保存/查询）。
fn resolve_host_app_id() -> String {
    use regex::Regex;
    let cgroup = match std::fs::read_to_string("/proc/self/cgroup") {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let unit = cgroup
        .lines()
        .next()
        .and_then(|l| l.rsplit('/').next())
        .unwrap_or("")
        .to_string();
    if !unit.starts_with("app-") {
        return String::new();
    }
    // 与 xdp-app-info-host.c 相同的两条正则。
    let re1 = Regex::new(r"^app-(?:[[:alnum:]]+\-)?(.+?)(?:\-[[:alnum:]]*)(?:\.scope|\.slice)$")
        .unwrap();
    let re2 = Regex::new(r"^app-(?:[[:alnum:]]+\-)?(.+?)(?:@[[:alnum:]]*|\-autostart)?\.service$")
        .unwrap();
    let app_id = re1
        .captures(&unit)
        .or_else(|| re2.captures(&unit))
        .and_then(|c| c.get(1))
        .map(|m| unescape_systemd_unit(m.as_str()))
        .unwrap_or_default();
    if app_id.is_empty() {
        return String::new();
    }
    // 需要存在 <app_id>.desktop（g_desktop_app_info_new 等价检查）。
    if find_desktop_file(&app_id) {
        app_id
    } else {
        String::new()
    }
}

/// 解码 systemd unit 名中的 `\xHH` 转义（与 portal 的 cunescape(RELAX) 近似）。
fn unescape_systemd_unit(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&input[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 在 XDG 数据目录的 applications 中查找 `<app_id>.desktop`
/// （近似 `g_desktop_app_info_new` 的搜索范围）。
fn find_desktop_file(app_id: &str) -> bool {
    let name = format!("{app_id}.desktop");
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(home));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/share"));
    }
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
            dirs.push(std::path::PathBuf::from(d));
        }
    } else {
        dirs.push(std::path::PathBuf::from("/usr/local/share"));
        dirs.push(std::path::PathBuf::from("/usr/share"));
    }
    dirs.iter()
        .any(|d| d.join("applications").join(&name).exists())
}

/// 无头模式下静默校验 restore token 是否仍可恢复（不触发任何弹窗）。
///
/// portal 前端在 SelectSources 时用 restore_token 查权限存储并校验
/// `perms[app_id]` 与 data 非空；任一不满足即认为 token 失效，Start 会
/// **正常弹选择器**——这正是"无头模式后台截图干扰用户操作"的根源。本函数
/// 在调用 Start 前直接查询同一权限存储（org.freedesktop.impl.portal.PermissionStore），
/// 复刻前端的判定：
///   - token 不存在（NotFound）→ 失效；
///   - 权限未授予本进程解析出的 app_id → 失效；
///   - data 为空 → 失效。
/// 仅当全部通过才返回 true，调用方才能安全调用 Start（静默恢复）。
///
/// 本函数是"可由库执行、也可由调用程序执行"的预检：库在无头 `Start` 前自动
/// 调用；集成方（常驻托盘/守护）也可在录制启动前自行调用 `verify_saved_token`
/// 提前暴露"需要重新授权"。
pub fn verify_restore_token(token: &str) -> Result<bool, String> {
    use std::collections::HashMap;
    let conn = zbus::blocking::Connection::session()
        .map_err(|e| format!("session bus connect failed: {e}"))?;
    let reply = conn.call_method(
        Some(PERMISSION_STORE_NAME),
        PERMISSION_STORE_PATH,
        Some(PERMISSION_STORE_NAME),
        "Lookup",
        &(SCREENCAST_PERMISSION_TABLE, token),
    );
    let body = match reply {
        Ok(msg) => msg.body(),
        // token 不在权限存储中：portal 前端会认为 restore 失败并正常弹选择器。
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.portal.Error.NotFound" =>
        {
            return Ok(false);
        }
        Err(e) => return Err(format!("permission store lookup failed: {e}")),
    };
    let (perms, data): (HashMap<String, Vec<String>>, zbus::zvariant::OwnedValue) = body
        .deserialize()
        .map_err(|e| format!("permission store response decode failed: {e}"))?;
    let app_id = resolve_host_app_id();
    let granted = perms
        .get(&app_id)
        .map(|v| v.iter().any(|p| p == "yes"))
        .unwrap_or(false);
    if !granted {
        return Ok(false);
    }
    // data 非空（restore 数据必须存在，portal 前端同样要求 data != NULL）。
    let data_value: zbus::zvariant::Value = data.into();
    let data_ok = !matches!(
        &data_value,
        zbus::zvariant::Value::Structure(s) if s.fields().is_empty()
    ) && !matches!(&data_value, zbus::zvariant::Value::Value(b) if matches!(b.as_ref(), zbus::zvariant::Value::Structure(s) if s.fields().is_empty()));
    if !data_ok {
        return Ok(false);
    }
    // GNOME 后端额外要求 restore 数据引用的显示器仍在线（否则 Start 仍会
    // 弹选择器）。尽力解析 GNOME 的 restore 格式并对照当前显示器；
    // 解析失败（非 GNOME/格式变化）则跳过此层，仅依赖上面的权限校验。
    if let Some(missing) = gnome_restore_monitor_missing(&data_value) {
        if missing {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 便捷预检：当前持久化的 restore token 是否仍可静默恢复。
///
/// 供调用程序在录制/截图前主动执行，避免无头 `Start` 弹选择器。
/// 无 token 或连接失败视为不可恢复。
pub fn verify_saved_token() -> Result<bool, String> {
    match restore_token() {
        Some(token) => verify_restore_token(&token),
        None => Ok(false),
    }
}

/// 解析 GNOME portal 的 restore 数据，判断其引用的显示器是否全部仍在线。
///
/// 返回 `Some(true)` 表示至少一个引用显示器已离线（Start 将弹选择器）；
/// `Some(false)` 表示全部在线；`None` 表示无法解析（非 GNOME 或格式变化，
/// 调用方应跳过此层校验）。GNOME restore 数据格式：
///   v( s="GNOME", u=version, v( tt 创建/最近使用, a(uuv) 流列表 ) )
///   每个流 uuv = (id, source_type, v(data))；monitor 类型(1) 的 data 为
///   `vendor:product:serial` 匹配串（全部 unknown 时退化为 connector 名）。
fn gnome_restore_monitor_missing(data: &zbus::zvariant::Value<'_>) -> Option<bool> {
    use zbus::zvariant::Value;

    // 第一层：variant → (suv)
    let Value::Value(boxed1) = data else { return None };
    let Value::Structure(s1) = boxed1.as_ref() else { return None };
    let f1 = s1.fields();
    if f1.len() < 3 {
        return None;
    }
    // provider == "GNOME"
    let Value::Str(provider) = &f1[0] else { return None };
    if provider.as_str() != "GNOME" {
        return None;
    }
    // 第二层：v( tt a(uuv) )
    let Value::Value(boxed2) = &f1[2] else { return None };
    let Value::Structure(s2) = boxed2.as_ref() else { return None };
    let f2 = s2.fields();
    if f2.len() < 3 {
        return None;
    }
    let Value::Array(streams) = &f2[2] else { return None };

    // 提取引用显示器匹配串。
    let mut referenced: Vec<String> = Vec::new();
    for stream in streams.iter() {
        let Value::Structure(ss) = stream else { continue };
        let fs = ss.fields();
        if fs.len() < 3 {
            continue;
        }
        // source_type: 1 = MONITOR
        let Value::U32(source_type) = fs[1] else { continue };
        if source_type != 1 {
            continue;
        }
        let Value::Value(boxed_data) = &fs[2] else { continue };
        if let Value::Str(m) = boxed_data.as_ref() {
            referenced.push(m.to_string());
        }
    }
    if referenced.is_empty() {
        return Some(false);
    }

    // 当前显示器匹配串集合（GNOME DisplayConfig）。
    let current = gnome_current_monitor_match_strings();
    let any_missing = referenced
        .iter()
        .any(|r| !current.iter().any(|c| c == r));
    Some(any_missing)
}

/// 查询 GNOME `org.gnome.Mutter.DisplayConfig.GetCurrentState`，构建当前
/// 显示器的匹配串集合（`vendor:product:serial`，全部 unknown 时用 connector）。
/// 查询/解析失败返回空集合（调用方据此无法判定，跳过校验）。
fn gnome_current_monitor_match_strings() -> Vec<String> {
    use zbus::zvariant::Value;
    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let reply = match conn.call_method(
        Some("org.gnome.Mutter.DisplayConfig"),
        "/org/gnome/Mutter/DisplayConfig",
        Some("org.gnome.Mutter.DisplayConfig"),
        "GetCurrentState",
        &(),
    ) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let body: zbus::zvariant::OwnedValue = match reply.body().deserialize() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let value: Value = body.into();
    // 返回 (u serial, a((ssss)a(siiddada{sv})a{sv}) monitors, ...)
    let Value::Structure(root) = value else {
        return Vec::new();
    };
    let fields = root.fields();
    if fields.len() < 2 {
        return Vec::new();
    }
    let Value::Array(monitors) = &fields[1] else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for mon in monitors.iter() {
        let Value::Structure(m) = mon else { continue };
        let mf = m.fields();
        if mf.is_empty() {
            continue;
        }
        let Value::Structure(spec) = &mf[0] else { continue };
        let sf = spec.fields();
        if sf.len() < 4 {
            continue;
        }
        let mut parts = [String::new(), String::new(), String::new(), String::new()];
        for (i, f) in sf.iter().enumerate().take(4) {
            if let Value::Str(s) = f {
                parts[i] = s.to_string();
            }
        }
        // spec = (connector, vendor, product, serial)
        let match_string = if parts[1] == "unknown"
            && parts[2] == "unknown"
            && parts[3] == "unknown"
        {
            parts[0].clone()
        } else {
            format!("{}:{}:{}", parts[1], parts[2], parts[3])
        };
        if !match_string.is_empty() {
            out.push(match_string);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{clear_restore_token, restore_token, save_restore_token, token_path};
    use std::env;
    use std::sync::Mutex;

    // env 变量在进程内是全局的，两个测试并行修改 XDG_CONFIG_HOME 会互相干扰，
    // 用互斥锁串行化 env 修改。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_config<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "dracopho-capture-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        env::set_var("XDG_CONFIG_HOME", &dir);
        let result = f();
        env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn saves_and_reads_token() {
        with_temp_config(|| {
            assert!(!restore_token().is_some());
            save_restore_token("test-token-123");
            assert_eq!(restore_token().as_deref(), Some("test-token-123"));
            clear_restore_token();
            assert!(!restore_token().is_some());
        });
    }

    #[test]
    fn token_path_is_scoped() {
        with_temp_config(|| {
            let p = token_path().expect("path");
            assert!(p.ends_with("dracopho-capture-core"));
        });
    }

    #[test]
    fn unescapes_systemd_unit_hex() {
        assert_eq!(super::unescape_systemd_unit("codium"), "codium");
        assert_eq!(
            super::unescape_systemd_unit("org.gnome.Evolution\\x2dalarm\\x2dnotify"),
            "org.gnome.Evolution-alarm-notify"
        );
        assert_eq!(super::unescape_systemd_unit("foo\\x41"), "fooA");
        // 无效转义保持原样。
        assert_eq!(super::unescape_systemd_unit("bad\\xz"), "bad\\xz");
    }
}
