//! 命名实例使用 versions/<name> 作为独立运行根目录。
//! 真实 MC 文件仍位于其内部 versions/<mc>，避免向 Mojang 查询自定义名称；
//! loader、gameData、mods/saves 与该实例一起隔离，删除一个实例不会破坏另一个。

use std::io::Write;
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::launcher::game_data::GameData;
use minecraft_java_rs_core::launcher::options::{LaunchOptions, LoaderConfig};
use minecraft_java_rs_core::models::loader::{LoaderType, ResolvedLoaderArtifact};
use serde::{Deserialize, Serialize};

use crate::{loader_catalog, store};

const PROFILE_FILE: &str = "nickel.profile.json";
const PROFILE_SCHEMA: u32 = 1;
const PROFILE_MAX_BYTES: u64 = 64 * 1024;
const NAME_MAX_BYTES: usize = 160;
const PACK_PENDING_FILE: &str = ".nickel-pack.pending";
const MAX_PACK_NAME_ATTEMPTS: usize = 10_000;
static RESERVATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 扫描与目录占位共用锁，避免空目录创建到 pending 落盘之间出现半成品。
pub(crate) fn reservation_guard() -> std::sync::MutexGuard<'static, ()> {
    RESERVATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// 整合包目录直接使用最终身份安装，避免移动后核心缓存中的绝对路径失效。
/// pending 标记撤销前扫描不可见；清理只能删除 token 仍属于本事务的目录。
pub struct ReservedProfile {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub profile: NamedProfile,
    owner: String,
    committed: bool,
    cleanup_attempted: bool,
}

pub fn is_pending_pack(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join(PACK_PENDING_FILE)).is_ok()
}

fn pack_directory_name(display_name: &str, suffix: usize) -> String {
    let mut base: String = display_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while base.contains("..") {
        base = base.replace("..", "_");
    }
    base = base.trim_matches(|c| c == ' ' || c == '.').to_string();
    if base.is_empty() {
        base = "Modpack".into();
    }
    let ending = if suffix == 0 {
        String::new()
    } else {
        format!("（{suffix}）")
    };
    while base.len() + ending.len() > NAME_MAX_BYTES {
        base.pop();
    }
    base = base.trim_end_matches([' ', '.']).to_string();
    if validate_name(&base).is_err() {
        base.insert(0, '_');
    }
    while base.len() + ending.len() > NAME_MAX_BYTES {
        base.pop();
    }
    base = base.trim_end_matches([' ', '.']).to_string();
    format!("{base}{ending}")
}

pub fn reserve_pack(
    root: &Path,
    display_name: &str,
    mc: &str,
    loader: Option<LoaderType>,
    build: &str,
) -> Result<ReservedProfile, String> {
    if !valid_minecraft_version(mc)
        || (loader.is_some()
            && (!loader_catalog::valid_version(build) || matches!(build, "latest" | "recommended")))
    {
        return Err("整合包的 Minecraft 或加载器版本无效，请选择明确版本".into());
    }
    let guard = reservation_guard();
    std::fs::create_dir_all(root).map_err(|e| format!("创建游戏目录失败：{e}"))?;
    let root = std::fs::canonicalize(root).map_err(|e| format!("解析游戏目录失败：{e}"))?;
    let versions = root.join("versions");
    if std::fs::symlink_metadata(&versions).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("versions 目录不能是符号链接".into());
    }
    std::fs::create_dir_all(&versions).map_err(|e| format!("创建版本目录失败：{e}"))?;
    for suffix in 0..MAX_PACK_NAME_ATTEMPTS {
        let name = pack_directory_name(display_name, suffix);
        validate_name(&name)?;
        let occupied = std::fs::read_dir(&versions)
            .map_err(|e| format!("读取版本目录失败：{e}"))?
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().to_lowercase() == name.to_lowercase());
        if occupied {
            continue;
        }
        let directory = checked_dir(&root, &name)?;
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("创建整合包目录失败：{e}")),
        }
        let owner = store::random_token();
        if let Err(e) = std::fs::write(directory.join(PACK_PENDING_FILE), &owner) {
            let _ = std::fs::remove_dir(&directory);
            return Err(format!("预留整合包目录失败：{e}"));
        }
        let mut reserved = ReservedProfile {
            root,
            directory,
            owner,
            committed: false,
            cleanup_attempted: false,
            profile: NamedProfile {
                schema_version: PROFILE_SCHEMA,
                name,
                minecraft_version: mc.into(),
                loader,
                loader_version: build.into(),
                loader_artifact: None,
                ready: false,
            },
        };
        // pending 已建立后才释放锁；后续错误显式清理，避免 Drop 重入占位锁。
        drop(guard);
        if let Err(error) = reserved.profile.write(&reserved.directory) {
            return Err(match reserved.cleanup() {
                Ok(()) => error,
                Err(cleanup) => format!("{error}；{cleanup}"),
            });
        }
        return Ok(reserved);
    }
    Err("同名实例过多，请整理版本目录后重试".into())
}

impl ReservedProfile {
    pub fn commit(
        &mut self,
        data: &GameData,
        display_name: &str,
        source: &serde_json::Value,
    ) -> Result<(), String> {
        let _guard = reservation_guard();
        self.ensure_owned()?;
        self.profile.complete(&self.directory, data)?;
        let descriptor_path = self.directory.join(format!("{}.json", self.profile.name));
        let text = std::fs::read_to_string(&descriptor_path)
            .map_err(|e| format!("读取实例描述失败：{e}"))?;
        let mut descriptor: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("读取实例描述失败：{e}"))?;
        descriptor["nickelCustom"] = serde_json::json!({ "name": display_name });
        descriptor["nickelPack"] = source.clone();
        atomic_write(
            &descriptor_path,
            &serde_json::to_vec_pretty(&descriptor).map_err(|e| e.to_string())?,
        )?;
        std::fs::remove_file(self.directory.join(PACK_PENDING_FILE))
            .map_err(|e| format!("发布实例失败：{e}"))?;
        self.committed = true;
        Ok(())
    }

    /// 与扫描互斥；内容全部删除之前保留 pending，清理错误由任务终态展示。
    pub fn cleanup(&mut self) -> Result<(), String> {
        if self.committed {
            return Ok(());
        }
        self.cleanup_attempted = true;
        let _guard = reservation_guard();
        match std::fs::symlink_metadata(&self.directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("读取待清理实例失败：{error}")),
            Ok(_) => {}
        }
        self.ensure_owned()?;
        for entry in
            std::fs::read_dir(&self.directory).map_err(|e| format!("读取待清理实例失败：{e}"))?
        {
            let entry = entry.map_err(|e| format!("读取待清理文件失败：{e}"))?;
            if entry.file_name() == PACK_PENDING_FILE {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|e| format!("读取待清理文件失败：{e}"))?;
            let result = if kind.is_dir() {
                std::fs::remove_dir_all(entry.path())
            } else {
                std::fs::remove_file(entry.path())
            };
            result.map_err(|e| {
                format!(
                    "清理未完成实例失败，请关闭占用文件后重试（{}）：{e}",
                    self.directory.display()
                )
            })?;
        }
        self.ensure_owned()?;
        // 空目录先移出 versions 再撤销标记；最后删除失败也不会被扫描为实例。
        let retired = self
            .root
            .join(format!(".nickel-pack-cleanup-{}", self.owner));
        if self.directory != retired {
            if std::fs::symlink_metadata(&retired).is_ok() {
                return Err("实例清理目录已存在，请整理后重试".into());
            }
            std::fs::rename(&self.directory, &retired)
                .map_err(|e| format!("移除待清理实例失败：{e}"))?;
            self.directory = retired;
        }
        let marker = self.directory.join(PACK_PENDING_FILE);
        std::fs::remove_file(&marker)
            .map_err(|e| format!("清理实例占位失败（{}）：{e}", self.directory.display()))?;
        if let Err(error) = std::fs::remove_dir(&self.directory) {
            let restore = std::fs::write(&marker, &self.owner);
            return Err(match restore {
                Ok(()) => format!("清理空目录失败（{}）：{error}", self.directory.display()),
                Err(restore) => format!(
                    "清理空目录失败（{}）：{error}；恢复占位失败：{restore}",
                    self.directory.display()
                ),
            });
        }
        Ok(())
    }

    fn ensure_owned(&self) -> Result<(), String> {
        checked_dir(&self.root, &self.profile.name)?;
        let metadata = std::fs::symlink_metadata(&self.directory)
            .map_err(|e| format!("读取预留目录失败：{e}"))?;
        let marker = self.directory.join(PACK_PENDING_FILE);
        let marker_metadata =
            std::fs::symlink_metadata(&marker).map_err(|e| format!("读取预留标记失败：{e}"))?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || !marker_metadata.is_file()
            || marker_metadata.file_type().is_symlink()
            || std::fs::read_to_string(marker).ok().as_deref() != Some(self.owner.as_str())
        {
            return Err("整合包目录占位已改变，请重新安装".into());
        }
        Ok(())
    }
}

impl Drop for ReservedProfile {
    fn drop(&mut self) {
        if !self.committed && !self.cleanup_attempted {
            let _ = self.cleanup();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedProfile {
    pub schema_version: u32,
    pub name: String,
    pub minecraft_version: String,
    #[serde(default)]
    pub loader: Option<LoaderType>,
    #[serde(default)]
    pub loader_version: String,
    /// 目录/另存/自动安装共用的 resolved artifact；补全时无需重新猜测 URL。
    #[serde(default)]
    pub loader_artifact: Option<ResolvedLoaderArtifact>,
    #[serde(default)]
    pub ready: bool,
}

pub fn validate_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.trim() != name || name.len() > NAME_MAX_BYTES {
        return Err(format!(
            "实例名称不能为空、不能以空格开头或结尾，且长度不能超过 {NAME_MAX_BYTES} 字节"
        ));
    }
    if name == "."
        || name.contains("..")
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err("实例名称不能包含路径分隔符、控制字符、..、尾随句点或 <>:\"|?*".into());
    }
    // 外层 <name>.json 不能覆盖实例 profile、核心缓存或版本设置。
    let descriptor = format!("{name}.json");
    if [PROFILE_FILE, "gameData.json", "nickel.settings.json"]
        .iter()
        .any(|file| descriptor.eq_ignore_ascii_case(file))
    {
        return Err("实例名称与内部数据文件冲突，请换一个名称".into());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        return Err("实例名称属于系统保留文件名，请换一个名称".into());
    }
    Ok(name)
}

pub fn valid_minecraft_version(value: &str) -> bool {
    // 官方愚人节版本 3D Shareware v1.34 含空格；文件身份校验不能套用 Maven 坐标规则。
    validate_name(value).is_ok()
}

pub fn has_profile(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join(PROFILE_FILE)).is_ok()
}

fn checked_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let versions = root.join("versions");
    if std::fs::symlink_metadata(&versions).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("versions 目录不能是符号链接，请选择实际游戏目录".into());
    }
    let path = versions.join(name);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("实例目录不能是符号链接".into());
    }
    Ok(path)
}

pub fn reserve(
    root: &Path,
    name: &str,
    mc: &str,
    loader: Option<LoaderType>,
    build: &str,
) -> Result<(NamedProfile, PathBuf), String> {
    let _guard = reservation_guard();
    if !valid_minecraft_version(mc) || (!build.is_empty() && !loader_catalog::valid_version(build))
    {
        return Err("Minecraft 或加载器版本号无效".into());
    }
    let directory = checked_dir(root, name)?;
    let versions = root.join("versions");
    std::fs::create_dir_all(&versions).map_err(|e| format!("创建版本目录失败：{e}"))?;
    // 跨平台统一拒绝仅大小写不同的名称；create_dir 再原子占位以阻止并发覆盖。
    for entry in std::fs::read_dir(&versions).map_err(|e| format!("读取版本目录失败：{e}"))?
    {
        let entry = entry.map_err(|e| format!("读取版本条目失败：{e}"))?;
        if entry.file_name().to_string_lossy().to_lowercase() == name.to_lowercase() {
            return Err(format!("实例名称 {name} 已存在，请使用其它名称"));
        }
    }
    std::fs::create_dir(&directory)
        .map_err(|e| format!("无法创建实例 {name}，请确认名称未被使用：{e}"))?;
    let profile = NamedProfile {
        schema_version: PROFILE_SCHEMA,
        name: name.into(),
        minecraft_version: mc.into(),
        loader,
        loader_version: build.into(),
        loader_artifact: None,
        ready: false,
    };
    if let Err(error) = profile.write(&directory) {
        let _ = std::fs::remove_dir(&directory);
        return Err(error);
    }
    Ok((profile, directory))
}

pub fn load(root: &Path, name: &str) -> Result<Option<NamedProfile>, String> {
    let directory = checked_dir(root, name)?;
    if is_pending_pack(&directory) {
        return Err("整合包尚未安装完成，请等待下载任务结束".into());
    }
    let path = directory.join(PROFILE_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取命名实例失败：{e}")),
    };
    if metadata.file_type().is_symlink() || metadata.len() > PROFILE_MAX_BYTES {
        return Err("命名实例元数据无效，请重新安装该实例".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取命名实例失败：{e}"))?;
    let profile: NamedProfile =
        serde_json::from_str(&text).map_err(|_| "命名实例元数据损坏，请重新安装该实例")?;
    if profile.schema_version != PROFILE_SCHEMA
        || profile.name != name
        || !valid_minecraft_version(&profile.minecraft_version)
        || (profile.loader.is_some() && !loader_catalog::valid_version(&profile.loader_version))
    {
        return Err("命名实例元数据不匹配，请重新安装该实例".into());
    }
    Ok(Some(profile))
}

impl NamedProfile {
    fn write(&self, directory: &Path) -> Result<(), String> {
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|e| format!("序列化命名实例失败：{e}"))?;
        atomic_write(&directory.join(PROFILE_FILE), &bytes)
    }

    pub fn set_loader_artifact(
        &mut self,
        directory: &Path,
        artifact: ResolvedLoaderArtifact,
    ) -> Result<(), String> {
        if self.loader.is_none() {
            return Err("原版实例不能写入加载器 Artifact".into());
        }
        self.loader_artifact = Some(artifact);
        self.write(directory)
    }

    pub fn apply(&self, options: &mut LaunchOptions, root: &Path) -> Result<(), String> {
        options.path = checked_dir(root, &self.name)?;
        options.version = self.minecraft_version.clone();
        options.instance = None;
        options.loader = LoaderConfig {
            loader_type: self.loader.clone(),
            build: self.loader_version.clone(),
            enable: self.loader.is_some(),
            path: None,
            config: None,
            artifact: self.loader_artifact.clone(),
        };
        Ok(())
    }

    pub fn complete(&mut self, directory: &Path, data: &GameData) -> Result<(), String> {
        if data.minecraft_version != self.minecraft_version || data.loader_type != self.loader {
            return Err("安装缓存与所选 Minecraft / 加载器不一致，请重新安装".into());
        }
        if self.loader.is_some() && matches!(self.loader_version.as_str(), "latest" | "recommended")
        {
            self.loader_version = resolved_loader_version(data)
                .ok_or("无法确定已安装的加载器版本，请重新选择具体构建")?;
        }
        let original = directory
            .join("versions")
            .join(&self.minecraft_version)
            .join(format!("{}.json", self.minecraft_version));
        let mut descriptor: serde_json::Value = std::fs::read_to_string(&original)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(
                serde_json::to_value(&data.minecraft_json)
                    .map_err(|e| format!("版本描述序列化失败：{e}"))?,
            );
        let object = descriptor
            .as_object_mut()
            .ok_or("原版版本描述不是 JSON 对象")?;
        object.insert("id".into(), self.name.clone().into());
        object.insert("inheritsFrom".into(), self.minecraft_version.clone().into());
        object.insert("nickelProfile".into(), serde_json::json!({"minecraftVersion": self.minecraft_version, "loader": self.loader, "loaderVersion": self.loader_version}));
        let descriptor_path = directory.join(format!("{}.json", self.name));
        if let Ok(existing) = std::fs::read_to_string(&descriptor_path) {
            if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&existing) {
                for key in ["nickelCustom", "nickelPack"] {
                    if let Some(value) = existing.get(key) {
                        object.insert(key.into(), value.clone());
                    }
                }
            }
        }
        atomic_write(
            &descriptor_path,
            &serde_json::to_vec_pretty(&descriptor)
                .map_err(|e| format!("序列化版本描述失败：{e}"))?,
        )?;
        self.ready = true;
        self.write(directory)
    }
}

fn resolved_loader_version(data: &GameData) -> Option<String> {
    let id = data.loader_version_id.as_deref()?;
    let mc = &data.minecraft_version;
    let version = match data.loader_type.as_ref()? {
        LoaderType::Fabric | LoaderType::LegacyFabric => id
            .strip_prefix("fabric-loader-")?
            .strip_suffix(&format!("-{mc}"))?,
        LoaderType::Quilt => id
            .strip_prefix("quilt-loader-")?
            .strip_suffix(&format!("-{mc}"))?,
        LoaderType::Forge => id.strip_prefix(&format!("{mc}-forge-"))?,
        LoaderType::NeoForge if mc == "1.20.1" => {
            // 历史 NeoForge 安装器的 profile 仍叫 1.20.1-forge-*。
            let build = id
                .strip_prefix(&format!("{mc}-forge-"))
                .or_else(|| id.strip_prefix(&format!("{mc}-neoforge-")))?;
            return loader_catalog::valid_version(build).then(|| format!("{mc}-{build}"));
        }
        LoaderType::NeoForge => id.strip_prefix("neoforge-")?,
        LoaderType::OptiFine => id.strip_prefix(&format!("{mc}-OptiFine_"))?,
        LoaderType::LiteLoader => id.strip_prefix(&format!("{mc}-LiteLoader-"))?,
    };
    loader_catalog::valid_version(version).then(|| version.to_string())
}

pub fn apply_for_launch(
    options: &mut LaunchOptions,
    root: &Path,
    name: &str,
) -> Result<bool, String> {
    let Some(profile) = load(root, name)? else {
        return Ok(false);
    };
    if !profile.ready {
        return Err("该实例尚未完成安装，请先补全文件或删除后重新安装".into());
    }
    profile.apply(options, root)?;
    Ok(true)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("目标文件缺少父目录")?;
    let temporary = parent.join(format!(".nickel-{}.part", store::random_token()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("创建临时文件失败：{e}"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("写入临时文件失败：{e}"))?;
        drop(file);
        // rename 在 Windows/Unix 均替换文件；不退化为直接写，失败时保留原文件。
        std::fs::rename(&temporary, path)
            .map_err(|e| format!("保存文件失败，请关闭占用目标文件的程序后重试：{e}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_april_fools_version_can_be_named() {
        assert!(valid_minecraft_version("3D Shareware v1.34"));
        assert!(!valid_minecraft_version("../escape"));
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("nickel-profiles-{}", store::random_token()))
    }

    #[test]
    fn validates_cross_platform_names() {
        for bad in [
            "",
            ".",
            "..",
            "../escape",
            "a/b",
            "a\\b",
            "a:b",
            "a?b",
            "bad\0",
            "name.",
            "name ",
            " name",
            "CON",
            "con.txt",
            "LPT1",
            "COM¹.txt",
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert!(validate_name("我的 1.20.1 Forge").is_ok());
        assert!(validate_name("1.21.4").is_ok());
    }

    #[test]
    fn reserves_unique_names_and_round_trips_metadata() {
        let root = root();
        let (profile, path) = reserve(
            &root,
            "My world",
            "1.20.1",
            Some(LoaderType::Forge),
            "47.4.0",
        )
        .unwrap();
        assert_eq!(path, root.join("versions/My world"));
        let loaded = load(&root, "My world").unwrap().unwrap();
        assert_eq!(loaded.minecraft_version, "1.20.1");
        assert_eq!(loaded.loader, Some(LoaderType::Forge));
        assert_eq!(loaded.loader_version, "47.4.0");
        assert!(!loaded.ready);
        assert!(reserve(&root, "My world", "1.21", None, "").is_err());
        assert!(
            reserve(
                &root,
                "my WORLD",
                "1.20.1",
                Some(LoaderType::Fabric),
                "0.16.10"
            )
            .is_err()
        );
        assert_eq!(profile.name, loaded.name);
        std::fs::write(path.join(PROFILE_FILE), "{broken").unwrap();
        assert!(load(&root, "My world").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_resolves_real_version_and_isolated_cache() {
        let root = root();
        let (mut profile, path) = reserve(
            &root,
            "Fabric world",
            "1.21.4",
            Some(LoaderType::Fabric),
            "0.16.10",
        )
        .unwrap();
        let params = crate::ResolvedLaunch {
            version: "Fabric world".into(),
            username: "Player".into(),
            memory_max: "2G".into(),
            game_dir: None,
        };
        let mut options = crate::build_options(&params, root.clone());
        assert!(apply_for_launch(&mut options, &root, "Fabric world").is_err());
        profile.ready = true;
        profile.write(&path).unwrap();
        assert!(apply_for_launch(&mut options, &root, "Fabric world").unwrap());
        assert_eq!(options.version, "1.21.4");
        assert_eq!(options.path, path);
        assert_eq!(options.save_dir(), path);
        assert_eq!(options.loader.build, "0.16.10");
        assert_eq!(options.loader.loader_type, Some(LoaderType::Fabric));
        assert_eq!(options.loader_dir("fabric"), path.join("loader/fabric"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_write_replaces_only_on_success() {
        let root = root();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("installer.jar");
        std::fs::write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let invalid = root.join("directory");
        std::fs::create_dir(&invalid).unwrap();
        assert!(atomic_write(&invalid, b"bad").is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn sample_data(mc: &str, loader: Option<LoaderType>, id: Option<&str>) -> GameData {
        serde_json::from_value(serde_json::json!({
            "minecraftJson": {"id": mc, "type": "release"},
            "minecraftVersion": mc, "minecraftJava": {"files": [], "path": ""},
            "loaderType": loader, "loaderVersionId": id,
        }))
        .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn failed_pack_cleanup_keeps_marker_and_can_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = root();
        let mut reserved = reserve_pack(&root, "Locked", "1.20.1", None, "").unwrap();
        let original = reserved.directory.clone();
        reserved
            .profile
            .complete(&original, &sample_data("1.20.1", None, None))
            .unwrap();
        let locked = original.join("locked.jar");
        std::fs::write(&locked, b"partial").unwrap();
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&locked)
            .unwrap();
        let error = reserved.cleanup().unwrap_err();
        assert!(error.contains("清理未完成实例失败"), "{error}");
        assert!(is_pending_pack(&original));
        assert_eq!(
            crate::instances::instance_scan(root.to_string_lossy().into()),
            "[]"
        );
        drop(handle);
        reserved.cleanup().unwrap();
        assert!(!original.exists());
        assert_eq!(
            crate::instances::instance_scan(root.to_string_lossy().into()),
            "[]"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_cleanup_never_exposes_partial_instance_to_concurrent_scan() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let root = root();
        let mut reserved = reserve_pack(&root, "Removing", "1.20.1", None, "").unwrap();
        reserved
            .profile
            .complete(&reserved.directory, &sample_data("1.20.1", None, None))
            .unwrap();
        let files = reserved.directory.join("mods");
        std::fs::create_dir(&files).unwrap();
        for index in 0..64 {
            std::fs::write(files.join(format!("{index}.jar")), b"partial").unwrap();
        }
        let scanning = Arc::new(AtomicBool::new(true));
        let active = scanning.clone();
        let scan_root = root.clone();
        let scanner = std::thread::spawn(move || {
            while active.load(Ordering::SeqCst) {
                assert_eq!(
                    crate::instances::instance_scan(scan_root.to_string_lossy().into()),
                    "[]"
                );
            }
        });
        reserved.cleanup().unwrap();
        scanning.store(false, Ordering::SeqCst);
        scanner.join().unwrap();
        assert_eq!(
            crate::instances::instance_scan(root.to_string_lossy().into()),
            "[]"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_retirement_conflict_preserves_pending_marker() {
        let root = root();
        let mut reserved = reserve_pack(&root, "Pack", "1.20.1", None, "").unwrap();
        let occupied = reserved
            .root
            .join(format!(".nickel-pack-cleanup-{}", reserved.owner));
        std::fs::write(&occupied, b"foreign").unwrap();
        assert!(reserved.cleanup().is_err());
        assert!(is_pending_pack(&reserved.directory));
        assert_eq!(std::fs::read(&occupied).unwrap(), b"foreign");
        assert_eq!(
            crate::instances::instance_scan(root.to_string_lossy().into()),
            "[]"
        );
        std::fs::remove_file(occupied).unwrap();
        reserved.cleanup().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_pack_names_are_unique_and_pending_is_hidden() {
        let root = root();
        let mut workers = Vec::new();
        for _ in 0..4 {
            let root = root.clone();
            workers.push(std::thread::spawn(move || {
                reserve_pack(&root, "同名包", "1.20.1", None, "").unwrap()
            }));
        }
        let reserved: Vec<_> = workers
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let mut names: Vec<_> = reserved
            .iter()
            .map(|item| item.profile.name.as_str())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["同名包", "同名包（1）", "同名包（2）", "同名包（3）"]
        );
        assert_eq!(
            crate::instances::instance_scan(root.to_string_lossy().into()),
            "[]"
        );
        assert!(load(&root, &reserved[0].profile.name).is_err());
        let paths: Vec<_> = reserved.iter().map(|item| item.directory.clone()).collect();
        drop(reserved);
        assert!(paths.iter().all(|path| !path.exists()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_cleanup_keeps_existing_and_foreign_directories() {
        let root = root();
        let (_, old) = reserve(&root, "Existing", "1.20.1", None, "").unwrap();
        let pending = reserve_pack(&root, "Existing", "1.20.1", None, "").unwrap();
        assert_eq!(pending.profile.name, "Existing（1）");
        let path = pending.directory.clone();
        std::fs::write(path.join(PACK_PENDING_FILE), "another-owner").unwrap();
        drop(pending);
        assert!(old.is_dir());
        assert!(path.is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_display_name_and_source_survive_repair_without_changing_identity() {
        let root = root();
        let data = sample_data("1.20.1", None, None);
        let source =
            serde_json::json!({"platform":"modrinth","projectId":"project","versionId":"release"});
        let mut first = reserve_pack(&root, "Pack", "1.20.1", None, "").unwrap();
        let mut second = reserve_pack(&root, "Pack", "1.20.1", None, "").unwrap();
        for item in [&mut first, &mut second] {
            item.commit(&data, "相同显示名", &source).unwrap();
            item.profile.complete(&item.directory, &data).unwrap();
            let descriptor: serde_json::Value = serde_json::from_slice(
                &std::fs::read(item.directory.join(format!("{}.json", item.profile.name))).unwrap(),
            )
            .unwrap();
            assert_eq!(descriptor["nickelPack"], source);
            assert_eq!(descriptor["nickelCustom"]["name"], "相同显示名");
            assert_eq!(descriptor["id"], item.profile.name);
        }
        let scan: serde_json::Value = serde_json::from_str(&crate::instances::instance_scan(
            root.to_string_lossy().into(),
        ))
        .unwrap();
        assert_eq!(scan.as_array().unwrap().len(), 2);
        assert!(
            scan.as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["displayName"] == "相同显示名" && entry["hasJson"] == true)
        );
        assert_ne!(first.profile.name, second.profile.name);
        let path = first.directory.clone();
        drop(first);
        assert!(path.is_dir());
        drop(second);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pack_directory_candidates_remain_cross_platform_safe() {
        for display in [
            "",
            "../unsafe/name",
            "CON",
            "gameData",
            &"测".repeat(100),
            &format!("{}.tail", "a".repeat(NAME_MAX_BYTES - 1)),
        ] {
            for suffix in [0, 1, MAX_PACK_NAME_ATTEMPTS - 1] {
                let name = pack_directory_name(display, suffix);
                assert!(validate_name(&name).is_ok(), "{name}");
            }
        }
    }

    #[test]
    fn internal_descriptor_names_are_rejected() {
        for name in [
            "gameData",
            "GAMEDATA",
            "nickel.profile",
            "nickel.settings",
            "CON .txt",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn completion_scans_actual_version_and_preserves_custom_metadata() {
        let root = root();
        let (mut profile, path) = reserve(
            &root,
            "My Fabric world",
            "1.21.4",
            Some(LoaderType::Fabric),
            "latest",
        )
        .unwrap();
        let data = sample_data(
            "1.21.4",
            Some(LoaderType::Fabric),
            Some("fabric-loader-0.16.10-1.21.4"),
        );
        std::fs::write(
            path.join("gameData.json"),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        let scan: serde_json::Value = serde_json::from_str(&crate::instances::instance_scan(
            root.to_string_lossy().into(),
        ))
        .unwrap();
        assert_eq!(scan[0]["hasJson"], false);
        assert!(crate::instances::pick_version_json(&path, &profile.name).is_none());
        profile.complete(&path, &data).unwrap();
        let loaded = load(&root, &profile.name).unwrap().unwrap();
        assert!(loaded.ready);
        assert_eq!(loaded.loader_version, "0.16.10");
        let scan: serde_json::Value = serde_json::from_str(&crate::instances::instance_scan(
            root.to_string_lossy().into(),
        ))
        .unwrap();
        assert_eq!(scan[0]["id"], profile.name);
        assert_eq!(scan[0]["minecraftVersion"], "1.21.4");
        assert_eq!(scan[0]["loader"], "fabric");
        assert_eq!(scan[0]["loaderVersion"], "0.16.10");
        assert_eq!(scan[0]["hasJson"], true);
        let descriptor_path = crate::instances::pick_version_json(&path, &profile.name).unwrap();
        let mut descriptor: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&descriptor_path).unwrap()).unwrap();
        descriptor["nickelCustom"] =
            serde_json::json!({"name": "Display title", "favorite": true});
        std::fs::write(&descriptor_path, serde_json::to_vec(&descriptor).unwrap()).unwrap();
        profile.complete(&path, &data).unwrap();
        let repaired: serde_json::Value =
            serde_json::from_slice(&std::fs::read(descriptor_path).unwrap()).unwrap();
        assert_eq!(repaired["nickelCustom"], descriptor["nickelCustom"]);
        assert!(
            profile
                .complete(&path, &sample_data("1.20.1", None, None))
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_instance_details_find_nested_minecraft_jar() {
        let root = root();
        let (mut profile, directory) = reserve(&root, "Vanilla world", "1.21.4", None, "").unwrap();
        let version_dir = directory.join("versions/1.21.4");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("1.21.4.jar"), b"jar").unwrap();
        profile
            .complete(&directory, &sample_data("1.21.4", None, None))
            .unwrap();
        let detail =
            crate::version_settings::version_detail(root.to_string_lossy().into(), profile.name)
                .unwrap();
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["hasJar"], true);
        assert_eq!(detail["jarSizeBytes"], 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completion_rejects_cross_loader_identity_conflicts() {
        let root = root();
        let (mut profile, path) = reserve(
            &root,
            "Conflict world",
            "1.20.1",
            Some(LoaderType::Fabric),
            "0.16.10",
        )
        .unwrap();
        let data = sample_data(
            "1.20.1",
            Some(LoaderType::NeoForge),
            Some("neoforge-21.1.1"),
        );
        let error = profile.complete(&path, &data).unwrap_err();
        assert!(error.contains("加载器不一致"), "{error}");
        assert!(!profile.ready);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_installed_loader_identifiers() {
        for (mc, loader, id, build) in [
            ("1.20.1", LoaderType::Forge, "1.20.1-forge-47.4.0", "47.4.0"),
            (
                "1.20.1",
                LoaderType::NeoForge,
                "1.20.1-forge-47.1.106",
                "1.20.1-47.1.106",
            ),
            (
                "1.20.1",
                LoaderType::NeoForge,
                "1.20.1-neoforge-47.1.106",
                "1.20.1-47.1.106",
            ),
            (
                "1.21.4",
                LoaderType::NeoForge,
                "neoforge-21.4.10",
                "21.4.10",
            ),
            (
                "1.21.4",
                LoaderType::Fabric,
                "fabric-loader-0.16.10-1.21.4",
                "0.16.10",
            ),
            (
                "1.20.1",
                LoaderType::OptiFine,
                "1.20.1-OptiFine_1.20.1_HD_U_I6",
                "1.20.1_HD_U_I6",
            ),
            (
                "1.12.2",
                LoaderType::LiteLoader,
                "1.12.2-LiteLoader-1.12.2-SNAPSHOT-r0",
                "1.12.2-SNAPSHOT-r0",
            ),
        ] {
            assert_eq!(
                resolved_loader_version(&sample_data(mc, Some(loader), Some(id))).as_deref(),
                Some(build)
            );
        }
    }
}
