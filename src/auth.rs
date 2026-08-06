//! 授权恢复 token 的本地持久化。
//!
//! ScreenCast 持久化授权（xdg-desktop-portal 1.14+）：`--authorize` 交互授权一次
//! 后保存 restore token；此后无头模式用 token 恢复会话，无需再次弹窗。

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
}
