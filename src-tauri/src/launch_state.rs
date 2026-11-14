//! 游戏进程占用登记；启动准备阶段即占用，退出或失败自动释放。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static RUNNING: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub struct RunningInstance(PathBuf);
impl Drop for RunningInstance {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

pub fn begin(directory: &Path) -> Result<RunningInstance, String> {
    let path = std::fs::canonicalize(directory).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.iter().any(|other| other.starts_with(&path) || path.starts_with(other)) {
        return Err("该实例正在启动或运行，请退出游戏后重试".into());
    }
    running.insert(path.clone());
    Ok(RunningInstance(path))
}

pub fn with_idle<T>(directory: &Path, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let path = std::fs::canonicalize(directory).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.iter().any(|other| other.starts_with(&path) || path.starts_with(other)) {
        return Err("该目录中有正在启动或运行的游戏，请退出游戏后重试".into());
    }
    action()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_child_blocks_root_until_exit() {
        let root = std::env::temp_dir().join(format!("nowhere-running-{}", crate::store::random_token()));
        let child = root.join("versions").join("test");
        std::fs::create_dir_all(&child).unwrap();
        let running = begin(&child).unwrap();
        assert!(with_idle(&root, || Ok(())).is_err());
        drop(running);
        assert!(with_idle(&root, || Ok(())).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
