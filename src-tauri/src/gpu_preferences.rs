//! Windows 只为本次选定的 Java 临时设置 GPU 偏好；退出时比较后恢复原值。
use std::path::Path;

#[cfg(windows)]
mod platform {
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::{LazyLock, Mutex};

    const USER_GPU_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
    const HIGH_PERFORMANCE: &str = "GpuPreference=2;";
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const NOT_FOUND: i32 = 2;
    const MORE_DATA: i32 = 234;
    const MAX_VALUE_BYTES: u32 = 64 * 1024;
    static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(key: *mut c_void, subkey: *const u16, reserved: u32, class: *mut u16, options: u32, access: u32, security: *const c_void, result: *mut *mut c_void, disposition: *mut u32) -> i32;
        fn RegQueryValueExW(key: *mut c_void, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, size: *mut u32) -> i32;
        fn RegSetValueExW(key: *mut c_void, name: *const u16, reserved: u32, kind: u32, data: *const u8, size: u32) -> i32;
        fn RegDeleteValueW(key: *mut c_void, name: *const u16) -> i32;
        fn RegCloseKey(key: *mut c_void) -> i32;
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Value { kind: u32, data: Vec<u8> }

    fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }

    fn high_performance_value(original: Option<&Value>) -> Result<Value, String> {
        let text = if let Some(value) = original {
            if value.kind != REG_SZ || value.data.len() % 2 != 0 {
                return Err("GPU 原有偏好不是有效字符串，请在 Windows 图形设置中重新保存该 Java 的偏好，或关闭高性能 GPU 后重试".into());
            }
            let mut units: Vec<u16> = value.data.chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])).collect();
            if units.last() == Some(&0) { units.pop(); }
            if units.contains(&0) {
                return Err("GPU 原有偏好包含无效字符，请在 Windows 图形设置中重新保存该 Java 的偏好后重试".into());
            }
            String::from_utf16(&units).map_err(|_| "GPU 原有偏好编码无效，请在 Windows 图形设置中重新保存该 Java 的偏好，或关闭高性能 GPU 后重试")?
        } else { String::new() };
        let mut applied = String::with_capacity(text.len() + HIGH_PERFORMANCE.len());
        let mut replaced = false;
        for segment in text.split_inclusive(';') {
            let terminated = segment.ends_with(';');
            let body = segment.strip_suffix(';').unwrap_or(segment);
            if body.split_once('=').is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("GpuPreference")) {
                applied.push_str(HIGH_PERFORMANCE.trim_end_matches(';'));
                if terminated { applied.push(';'); }
                replaced = true;
            } else {
                // Auto HDR 等设置共用同一 REG_SZ；原样保留不属于 GPU 选择的片段。
                applied.push_str(segment);
            }
        }
        if !replaced {
            if !applied.is_empty() && !applied.ends_with(';') { applied.push(';'); }
            applied.push_str(HIGH_PERFORMANCE);
        }
        let data: Vec<u8> = wide(&applied).into_iter().flat_map(u16::to_le_bytes).collect();
        if data.len() > MAX_VALUE_BYTES as usize {
            return Err("GPU 偏好过长，请在 Windows 图形设置中重新保存该 Java 的偏好后重试".into());
        }
        Ok(Value { kind: REG_SZ, data })
    }


    struct Key(*mut c_void);
    impl Key {
        fn open() -> Result<Self, String> {
            let name = wide(USER_GPU_KEY);
            let mut key = std::ptr::null_mut();
            // SAFETY: HKCU 为 Win32 预定义伪句柄，输出句柄只在 Key 内拥有。
            let status = unsafe { RegCreateKeyExW(0x80000001_u32 as i32 as isize as *mut c_void, name.as_ptr(), 0, std::ptr::null_mut(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut()) };
            if status != 0 { return Err(format!("无法设置当前用户 GPU 偏好（{status}），请关闭高性能 GPU 后重试")); }
            Ok(Self(key))
        }
        fn read(&self, name: &[u16]) -> Result<Option<Value>, String> {
            let (mut kind, mut size) = (0, 0);
            // SAFETY: 第一次只取长度；第二次使用恰好该长度的可写缓冲区。
            let status = unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut size) };
            if status == NOT_FOUND { return Ok(None); }
            if ![0, MORE_DATA].contains(&status) || size > MAX_VALUE_BYTES { return Err("读取 GPU 原有偏好失败，请关闭高性能 GPU 后重试".into()); }
            let mut data = vec![0; size as usize];
            let status = unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, data.as_mut_ptr(), &mut size) };
            if status != 0 { return Err("GPU 偏好在读取期间变化，请重试启动".into()); }
            data.truncate(size as usize);
            Ok(Some(Value { kind, data }))
        }
        fn write(&self, name: &[u16], value: Option<&Value>) -> Result<(), String> {
            // SAFETY: 缓冲区在同步调用期间有效，长度已在 read/构造处限定。
            let status = unsafe {
                match value {
                    Some(value) => RegSetValueExW(self.0, name.as_ptr(), 0, value.kind, value.data.as_ptr(), value.data.len() as u32),
                    None => RegDeleteValueW(self.0, name.as_ptr()),
                }
            };
            if status == 0 || (value.is_none() && status == NOT_FOUND) { Ok(()) }
            else { Err(format!("写入 GPU 偏好失败（{status}），请检查 Windows 图形设置")) }
        }
    }
    impl Drop for Key { fn drop(&mut self) { unsafe { RegCloseKey(self.0); } } }

    pub struct Guard {
        name: Vec<u16>,
        identity: String,
        original: Option<Value>,
        applied: Value,
    }
    impl Guard {
        pub fn acquire(java: &Path) -> Result<Self, String> {
            let java = std::fs::canonicalize(java).map_err(|e| format!("所选 Java 已不存在：{e}"))?;
            let display = java.to_string_lossy();
            let normalized = if let Some(path) = display.strip_prefix(r"\\?\UNC\") { format!(r"\\{path}") }
                else { display.strip_prefix(r"\\?\").unwrap_or(&display).to_string() };
            let identity = normalized.to_lowercase();
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if active.contains(&identity) { return Err("所选 Java 的 GPU 偏好正在被另一个游戏使用，请退出游戏后重试".into()); }
            let name = wide(&normalized);
            let key = Key::open()?;
            let original = key.read(&name)?;
            let applied = high_performance_value(original.as_ref())?;
            key.write(&name, Some(&applied))?;
            active.insert(identity.clone());
            Ok(Self { name, identity, original, applied })
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            let result = (|| {
                let key = Key::open()?;
                // 用户在游戏运行期间主动修改或删除偏好时，保留该操作。
                if key.read(&self.name)?.as_ref() == Some(&self.applied) {
                    key.write(&self.name, self.original.as_ref())?;
                }
                Ok::<_, String>(())
            })();
            active.remove(&self.identity);
            if let Err(error) = result { eprintln!("[gpu] {error}"); }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn string_value(text: &str) -> Value {
            Value { kind: REG_SZ, data: wide(text).into_iter().flat_map(u16::to_le_bytes).collect() }
        }

        #[test]
        fn preference_merge_preserves_unrelated_segments() {
            for (original, expected) in [
                ("AutoHDREnable=1;GpuPreference=1;SwapEffectUpgradeEnable=1;", "AutoHDREnable=1;GpuPreference=2;SwapEffectUpgradeEnable=1;"),
                ("AutoHDREnable=1;SwapEffectUpgradeEnable=0", "AutoHDREnable=1;SwapEffectUpgradeEnable=0;GpuPreference=2;"),
                ("Other=值;;gpupreference=0;Unknown=1;GpuPreference=1", "Other=值;;GpuPreference=2;Unknown=1;GpuPreference=2"),
                ("", HIGH_PERFORMANCE),
            ] {
                assert_eq!(high_performance_value(Some(&string_value(original))).unwrap(), string_value(expected));
            }
            assert_eq!(high_performance_value(None).unwrap(), string_value(HIGH_PERFORMANCE));
        }

        #[test]
        fn guard_restores_original_and_preserves_user_change() {
            let root = std::env::temp_dir().join(format!("nowhere-gpu-{}", crate::store::random_token()));
            std::fs::create_dir_all(&root).unwrap();
            let java = root.join("java.exe");
            std::fs::write(&java, b"test-path-only").unwrap();
            let guard = Guard::acquire(&java).unwrap();
            let name = guard.name.clone();
            let key = Key::open().unwrap();
            assert_eq!(key.read(&name).unwrap(), Some(guard.applied.clone()));
            assert!(Guard::acquire(&java).is_err());
            drop(guard);
            assert_eq!(key.read(&name).unwrap(), None);
            let original = string_value("AutoHDREnable=1;GpuPreference=1;SwapEffectUpgradeEnable=1;");
            key.write(&name, Some(&original)).unwrap();
            let guard = Guard::acquire(&java).unwrap();
            assert_eq!(key.read(&name).unwrap(), Some(string_value("AutoHDREnable=1;GpuPreference=2;SwapEffectUpgradeEnable=1;")));
            drop(guard);
            assert_eq!(key.read(&name).unwrap(), Some(original.clone()));
            let guard = Guard::acquire(&java).unwrap();
            let changed = string_value("AutoHDREnable=0;GpuPreference=0;SwapEffectUpgradeEnable=1;");
            key.write(&name, Some(&changed)).unwrap();
            drop(guard);
            assert_eq!(key.read(&name).unwrap(), Some(changed));
            for invalid in [
                Value { kind: 3, data: vec![1, 2] },
                Value { kind: REG_SZ, data: vec![1] },
                Value { kind: REG_SZ, data: vec![0, 0xd8, 0, 0] },
                string_value("GpuPreference=1;\0AutoHDREnable=1;"),
            ] {
                key.write(&name, Some(&invalid)).unwrap();
                assert!(Guard::acquire(&java).is_err());
                assert_eq!(key.read(&name).unwrap(), Some(invalid));
            }
            key.write(&name, None).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(windows)]
pub use platform::Guard;
#[cfg(not(windows))]
pub struct Guard;

pub fn prepare(java: &Path, enabled: bool) -> Result<Option<Guard>, String> {
    if !enabled { return Ok(None); }
    #[cfg(windows)]
    { Guard::acquire(java).map(Some) }
    #[cfg(not(windows))]
    { let _ = java; Err("当前系统不支持 Windows GPU 偏好，请关闭高性能 GPU 后重试".into()) }
}
