//! 便携 ZIP 信封：标准 mrpack 载荷、显式首启标记与经过白名单筛选的外观和实例启动设置。
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use minecraft_java_rs_core::net::transfer::CancellationToken;
use minecraft_java_rs_core::utils::archive::{ArchiveLimits, write_zip_files};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::{app_paths, instances, modpacks, named_profiles, pack_export::PackExportRequest};

pub const MANIFEST_FILE: &str = "nickel.portable.json";
pub const FORMAT: &str = "nickel-portable";
pub const SCHEMA_VERSION: u32 = 1;
const PAYLOAD_FILE: &str = "pack.mrpack";
const PERSONALIZATION_FILE: &str = "personalization.json";
const INSTANCE_SETTINGS_FILE: &str = "instance-settings.json";
const GUIDE_FILE: &str = "使用说明.txt";
const WINDOWS_LAUNCHER: &str = "Nickel.exe";
const UNIX_LAUNCHER: &str = "Nickel-launcher";
const MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const BACKGROUND_MAX_BYTES: u64 = 32 * 1024 * 1024;
// 与本地标准整合包导入的 4 GiB 文件配额一致；外层 ZIP 只提高 mrpack 单条目配额。
const PAYLOAD_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const TEXT_MAX_BYTES: usize = 512;
const THEME_MODES: &[&str] = &["system", "light", "dark"];
const ACCENTS: &[&str] = &[
    "mono", "emerald", "blue", "violet", "amber", "rose",
    "nickel", "copper", "abyss", "paper", "fortyTwo", "custom",
];
const THEME_DECORATIONS: &[&str] = &["none", "lattice", "ribbons", "orbits", "folds"];
const BACKGROUND_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "mp4", "m4v", "webm", "ogv"];
const SOURCE_FILE: &str = "SOURCE.txt";
// 单文件启动器不会复制 bundle resources；固定嵌入许可，且保留 NOTICE 的相对链接布局。
const LAUNCHER_LICENSE_ENTRIES: &[(&str, &[u8])] = &[
    ("LICENSE", include_bytes!("../../LICENSE")),
    ("LICENSE-ASSETS", include_bytes!("../../LICENSE-ASSETS")),
    ("NOTICE.md", include_bytes!("../../NOTICE.md")),
    (
        "LICENSES/dependencies-NOTICES.txt",
        include_bytes!("../../LICENSES/dependencies-NOTICES.txt"),
    ),
    (
        "LICENSES/OFL-1.1.txt",
        include_bytes!("../../LICENSES/OFL-1.1.txt"),
    ),
    (
        "LICENSES/Apache-2.0.txt",
        include_bytes!("../../LICENSES/Apache-2.0.txt"),
    ),
    (
        "LICENSES/java-launch-wrapper-LICENCE.txt",
        include_bytes!("../../LICENSES/java-launch-wrapper-LICENCE.txt"),
    ),
    (
        "LICENSES/lwjgl-unsafe-agent-LICENSE.txt",
        include_bytes!("../../LICENSES/lwjgl-unsafe-agent-LICENSE.txt"),
    ),
    (
        "LICENSES/authlib-injector-COPYING.md",
        include_bytes!("../../LICENSES/authlib-injector-COPYING.md"),
    ),
    (
        "LICENSES/AGPLv3-with-authlib-injector-exception.txt",
        include_bytes!("../../LICENSES/AGPLv3-with-authlib-injector-exception.txt"),
    ),
    (
        "LICENSES/CC-BY-SA-4.0.txt",
        include_bytes!("../../LICENSES/CC-BY-SA-4.0.txt"),
    ),
    (
        "src-tauri/vendor/minecraft-java-rs-core/LICENSE",
        include_bytes!("../vendor/minecraft-java-rs-core/LICENSE"),
    ),
    (
        "src/assets/fonts/NOTICE.md",
        include_bytes!("../../src/assets/fonts/NOTICE.md"),
    ),
    (
        "src/assets/credits/README.md",
        include_bytes!("../../src/assets/credits/README.md"),
    ),
];
const LAUNCHER_SOURCE_TEXT: &str = concat!(
    "Nickel 源码入口\n\n启动器版本：",
    env!("CARGO_PKG_VERSION"),
    "\n公开源码仓库：",
    env!("CARGO_PKG_REPOSITORY"),
    "\n对应发行版本与源码归档：",
    env!("CARGO_PKG_REPOSITORY"),
    "/releases\n\n",
    "在对应发行版本页面获取源码归档；构建说明见源码中的 README.md 和 docs/development.md。\n",
    "本地维护的核心源码位于 src-tauri/vendor/minecraft-java-rs-core/，与启动器源码一同提供。\n",
    "许可与第三方声明见 LICENSE、LICENSE-ASSETS、NOTICE.md 和 LICENSES/。\n",
    "重新分发修改后的启动器时，一并提供与所分发版本对应的源码和构建说明。\n",
);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableLauncher {
    pub path: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableManifest {
    pub schema_version: u32,
    pub format: String,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub instance_name: String,
    pub payload: String,
    pub game_directory: String,
    pub data_directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<PortableLauncher>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personalization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableCustomTheme {
    pub hue: f64,
    pub saturation: f64,
    pub lightness: f64,
    pub titlebar_hue_shift: f64,
    pub decoration: String,
    pub decoration_opacity: f64,
}

impl Default for PortableCustomTheme {
    fn default() -> Self {
        Self {
            hue: 174.0,
            saturation: 39.0,
            lightness: 44.0,
            titlebar_hue_shift: 0.0,
            decoration: "lattice".into(),
            decoration_opacity: 0.35,
        }
    }
}

impl PortableCustomTheme {
    fn is_valid(&self) -> bool {
        [
            (self.hue, 0.0, 360.0),
            (self.saturation, 0.0, 100.0),
            (self.lightness, 0.0, 100.0),
            (self.titlebar_hue_shift, -180.0, 180.0),
            (self.decoration_opacity, 0.0, 1.0),
        ]
        .iter()
        .all(|(value, min, max)| value.is_finite() && (*min..=*max).contains(value))
            && THEME_DECORATIONS.contains(&self.decoration.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortablePersonalization {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_theme: Option<PortableCustomTheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titlebar_tint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_image: Option<String>,
}

#[derive(Debug)]
pub struct PortableBootstrap {
    pub manifest: PortableManifest,
    pub root: PathBuf,
    pub payload_path: PathBuf,
    pub game_directory: PathBuf,
    pub data_directory: PathBuf,
    pub personalization: Option<PortablePersonalization>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherAvailability {
    pub available: bool,
    pub detail: String,
    pub os: String,
    pub arch: String,
}

pub(crate) struct PortableSnapshot {
    pub root: PathBuf,
    pub manifest: PortableManifest,
    files: Vec<(String, String)>,
    inline_entries: Vec<(String, Vec<u8>)>,
}

fn data_path_for(data_directory: &str, file: &str) -> String {
    format!("{data_directory}/{file}")
}

fn data_path(file: &str) -> String {
    data_path_for(app_paths::DATA_DIRECTORY, file)
}

pub(crate) fn validate_manifest(manifest: &PortableManifest) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION || manifest.format != FORMAT {
        return Err("便携整合包格式无法识别，请使用支持此格式的 Nickel 发行版".into());
    }
    if manifest.package_id.len() != 32
        || !manifest.package_id.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("便携整合包身份无效，请重新导出".into());
    }
    named_profiles::validate_name(&manifest.instance_name)?;
    for value in [&manifest.name, &manifest.version] {
        if value.trim().is_empty()
            || value.len() > TEXT_MAX_BYTES
            || value.chars().any(char::is_control)
        {
            return Err("便携整合包名称或版本无效，请重新导出".into());
        }
    }
    // 固定包内布局，拒绝跨目录载荷或可执行文件。
    if manifest.data_directory != app_paths::DATA_DIRECTORY
        || manifest.payload != data_path_for(&manifest.data_directory, PAYLOAD_FILE)
        || manifest.game_directory != app_paths::GAME_DIRECTORY
        || manifest.personalization.as_ref().is_some_and(|path| {
            path != &data_path_for(&manifest.data_directory, PERSONALIZATION_FILE)
        })
    {
        return Err("便携整合包目录不符合约定，请完整解压原始 ZIP".into());
    }
    if let Some(launcher) = &manifest.launcher {
        let valid_path = match launcher.os.as_str() {
            "windows" => launcher.path == WINDOWS_LAUNCHER,
            "linux" => launcher.path == UNIX_LAUNCHER,
            _ => false,
        };
        if !valid_path
            || launcher.arch.is_empty()
            || launcher.arch.len() > 32
            || !launcher
                .arch
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("便携启动器平台信息无效，请重新导出".into());
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    instances::reject_links(path)?;
    let file = std::fs::File::open(path).map_err(|e| format!("读取便携文件失败：{e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("读取便携文件信息失败：{e}"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("便携文件无效或超过大小限制，请完整解压原始 ZIP".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取便携文件失败：{e}"))?;
    if bytes.len() as u64 > limit || bytes.len() as u64 != metadata.len() {
        return Err("便携文件已变化，请重新打开整合包".into());
    }
    Ok(bytes)
}

/// 路径选择只读取小型标记；标记永久保留，导入完成记录由首启流程另存。
pub fn read_manifest(root: &Path) -> Result<Option<PortableManifest>, String> {
    let path = root.join(MANIFEST_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取便携标记失败：{e}")),
        Ok(_) => {}
    }
    let manifest: PortableManifest =
        serde_json::from_slice(&read_bounded(&path, MANIFEST_MAX_BYTES)?)
            .map_err(|e| format!("便携标记无效，请完整解压原始 ZIP：{e}"))?;
    validate_manifest(&manifest)?;
    Ok(Some(manifest))
}

fn is_background_path(data_directory: &str, path: &str) -> bool {
    BACKGROUND_EXTENSIONS
        .iter()
        .any(|ext| path == data_path_for(data_directory, &format!("background.{ext}")))
}

impl PortablePersonalization {
    fn validate(&self, manifest: &PortableManifest) -> Result<(), String> {
        validate_manifest(manifest)?;
        if self
            .theme_mode
            .as_deref()
            .is_some_and(|v| !THEME_MODES.contains(&v))
            || self
                .accent
                .as_deref()
                .is_some_and(|v| !ACCENTS.contains(&v))
            || self.custom_theme.as_ref().is_some_and(|v| !v.is_valid())
            || self
                .background_blur
                .is_some_and(|v| !v.is_finite() || !(0.0..=40.0).contains(&v))
            || self
                .background_opacity
                .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
            || self
                .background_image
                .as_deref()
                .is_some_and(|v| !is_background_path(&manifest.data_directory, v))
        {
            return Err("便携个性化设置无效，请重新导出".into());
        }
        Ok(())
    }

    /// 显示值可解析为绝对路径；持久化仍保留同一 manifest 布局的相对路径。
    pub fn resolved_value(
        &self,
        root: &Path,
        manifest: &PortableManifest,
    ) -> Result<Value, String> {
        self.validate(manifest)?;
        let mut value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(background) = &self.background_image {
            let path = root.join(background);
            instances::reject_links(&path)?;
            value["backgroundImage"] = Value::String(path.to_string_lossy().into_owned());
        }
        Ok(value)
    }
}

/// 载荷和随包设置经同一 manifest 校验；安装及执行交给整合包事务入口。
pub fn load_bootstrap(root: &Path) -> Result<Option<PortableBootstrap>, String> {
    let Some(manifest) = read_manifest(root)? else {
        return Ok(None);
    };
    instances::reject_links(root)?;
    let root = std::fs::canonicalize(root).map_err(|e| format!("读取便携目录失败：{e}"))?;
    let payload_path = root.join(&manifest.payload);
    let (_, payload) = modpacks::local_manifest(&payload_path, &CancellationToken::new())?;
    if payload.name != manifest.name {
        return Err("便携标记与整合包内容不一致，请完整解压原始 ZIP".into());
    }
    let personalization = manifest
        .personalization
        .as_ref()
        .map(|path| {
            let settings: PortablePersonalization =
                serde_json::from_slice(&read_bounded(&root.join(path), MANIFEST_MAX_BYTES)?)
                    .map_err(|e| format!("读取便携个性化设置失败：{e}"))?;
            settings.validate(&manifest)?;
            if let Some(background) = &settings.background_image {
                validate_background(&root.join(background))?;
            }
            Ok::<_, String>(settings)
        })
        .transpose()?;
    let game_directory = root.join(&manifest.game_directory);
    let data_directory = root.join(&manifest.data_directory);
    instances::reject_links(&game_directory)?;
    instances::reject_links(&data_directory)?;
    crate::pack_export::instance_settings_export::read_file(
        &data_directory.join(INSTANCE_SETTINGS_FILE),
    )?;
    Ok(Some(PortableBootstrap {
        manifest,
        root,
        payload_path,
        game_directory,
        data_directory,
        personalization,
    }))
}

/// 标准本地导入路径不会经过 PortableBootstrap；仅当载荷位于有效便携根目录时读取固定设置文件。
pub(crate) fn instance_settings_for_payload(payload: &Path) -> Result<Option<Value>, String> {
    for candidate in payload.ancestors().skip(1) {
        let marker = candidate.join(MANIFEST_FILE);
        if std::fs::symlink_metadata(&marker).is_err() {
            continue;
        }
        let Some(manifest) = read_manifest(candidate)? else {
            continue;
        };
        let relative = payload
            .strip_prefix(candidate)
            .map_err(|_| "便携载荷不在标记目录内")?
            .to_string_lossy()
            .replace('\\', "/");
        if relative == manifest.payload {
            return crate::pack_export::instance_settings_export::read_file(
                &candidate
                    .join(&manifest.data_directory)
                    .join(INSTANCE_SETTINGS_FILE),
            );
        }
    }
    Ok(None)
}

fn launcher_path_for(source: &Path, development: bool, os: &str) -> Result<&'static str, String> {
    if source.ancestors().any(|path| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
    }) {
        return Err("macOS 请将完整 Nickel.app 放到解压目录中，与便携标记并列后运行".into());
    }
    if development {
        return Err(
            "当前为开发版，请使用 Nickel 发行版重新导出，或将发行版放到解压目录中运行".into(),
        );
    }
    match os {
        "windows" => Ok(WINDOWS_LAUNCHER),
        "linux" => Ok(UNIX_LAUNCHER),
        "macos" => Err("macOS 请将完整 Nickel.app 放到解压目录中，与便携标记并列后运行".into()),
        _ => Err("请将适用于接收方平台的 Nickel 发行版放到解压目录中运行".into()),
    }
}

fn current_launcher() -> Result<(PathBuf, &'static str), String> {
    let source = app_paths::launcher_executable()?;
    let name = launcher_path_for(
        &source,
        cfg!(debug_assertions) || tauri::is_dev(),
        std::env::consts::OS,
    )?;
    instances::reject_links(&source)?;
    let metadata = std::fs::metadata(&source).map_err(|e| format!("读取启动器失败：{e}"))?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > ArchiveLimits::default().max_entry_bytes
    {
        return Err("当前启动器文件无效或过大，请使用 Nickel 发行版重新导出".into());
    }
    Ok((source, name))
}

pub(crate) fn launcher_availability() -> LauncherAvailability {
    let result = current_launcher();
    let available = result.is_ok();
    let detail = match result {
        Ok(_) if cfg!(target_os = "windows") => format!(
            "附带当前 Windows {} 启动器；接收方需安装 WebView2",
            std::env::consts::ARCH
        ),
        Ok(_) => format!(
            "附带当前 Linux {} 启动器；解压后赋予执行权限，并安装 WebKitGTK 等系统依赖",
            std::env::consts::ARCH
        ),
        Err(error) => error,
    };
    LauncherAvailability {
        available,
        detail,
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
    }
}

pub(crate) fn check_launcher_request(include_launcher: bool) -> Result<(), String> {
    if include_launcher {
        current_launcher()?;
    }
    Ok(())
}

fn copy_snapshot(
    source: &Path,
    destination: &Path,
    limit: u64,
    cancel: &CancellationToken,
) -> Result<(), String> {
    cancel.check()?;
    instances::reject_links(source)?;
    instances::reject_links(destination)?;
    let mut input = std::fs::File::open(source).map_err(|e| format!("读取随包文件失败：{e}"))?;
    let metadata = input.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("随包文件无效或超过大小限制".into());
    }
    let parent = destination.parent().ok_or("随包文件缺少父目录")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建便携暂存目录失败：{e}"))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| format!("暂存随包文件失败：{e}"))?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        cancel.check()?;
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        copied = copied.checked_add(count as u64).ok_or("随包文件长度溢出")?;
        if copied > metadata.len() || copied > limit {
            return Err("随包文件已变化，请重新导出".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| format!("暂存随包文件失败：{e}"))?;
    }
    if copied != metadata.len() {
        return Err("随包文件已变化，请重新导出".into());
    }
    output
        .sync_all()
        .map_err(|e| format!("暂存随包文件失败：{e}"))?;
    cancel.check()
}

fn background_limit(extension: &str) -> u64 {
    if ["mp4", "m4v", "webm", "ogv"].contains(&extension) {
        crate::media::MAX_VIDEO_BYTES
    } else {
        BACKGROUND_MAX_BYTES
    }
}

fn validate_background(path: &Path) -> Result<(), String> {
    instances::reject_links(path)?;
    crate::media::validate_background_file(path)
}

// Project nested appearance fields instead of copying arbitrary settings JSON.
fn select_custom_theme(value: &Value) -> Option<PortableCustomTheme> {
    value.as_object()?;
    let defaults = PortableCustomTheme::default();
    let bounded = |key: &str, min: f64, max: f64, fallback: f64| {
        value[key]
            .as_f64()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(min, max))
            .unwrap_or(fallback)
    };
    Some(PortableCustomTheme {
        hue: bounded("hue", 0.0, 360.0, defaults.hue),
        saturation: bounded("saturation", 0.0, 100.0, defaults.saturation),
        lightness: bounded("lightness", 0.0, 100.0, defaults.lightness),
        titlebar_hue_shift: bounded("titlebarHueShift", -180.0, 180.0, defaults.titlebar_hue_shift),
        decoration: value["decoration"]
            .as_str()
            .filter(|v| THEME_DECORATIONS.contains(v))
            .unwrap_or(&defaults.decoration)
            .to_owned(),
        decoration_opacity: bounded("decorationOpacity", 0.0, 1.0, defaults.decoration_opacity),
    })
}

fn select_personalization(value: &Value) -> PortablePersonalization {
    PortablePersonalization {
        theme_mode: value["themeMode"]
            .as_str()
            .filter(|v| THEME_MODES.contains(v))
            .map(str::to_owned),
        accent: value["accent"]
            .as_str()
            .filter(|v| ACCENTS.contains(v))
            .map(str::to_owned),
        custom_theme: select_custom_theme(&value["customTheme"])
            .or_else(|| (value["accent"].as_str() == Some("custom")).then(PortableCustomTheme::default)),
        titlebar_tint: value["titlebarTint"].as_bool(),
        background_blur: value["backgroundBlur"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=40.0).contains(v)),
        background_opacity: value["backgroundOpacity"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v)),
        background_image: None,
    }
}

fn snapshot_personalization(
    app: &AppHandle,
    root: &Path,
    manifest: &PortableManifest,
    cancel: &CancellationToken,
) -> Result<PortablePersonalization, String> {
    cancel.check()?;
    validate_manifest(manifest)?;
    let value: Value = serde_json::from_str(&crate::settings_get(app.clone())?)
        .map_err(|e| format!("读取个性化设置失败：{e}"))?;
    let mut settings = select_personalization(&value);
    if let Some(background) = value["backgroundImage"].as_str().filter(|v| !v.is_empty()) {
        let source = PathBuf::from(background);
        let source = if source.is_absolute() {
            source
        } else {
            let (source_root, source_manifest) = app_paths::portable_root(app)?
                .filter(|(_, source_manifest)| {
                    is_background_path(&source_manifest.data_directory, background)
                })
                .ok_or("请重新选择本地背景图片或视频，或关闭「附带个性化设置」后导出")?;
            validate_manifest(&source_manifest)?;
            source_root.join(background)
        };
        let extension = source
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !BACKGROUND_EXTENSIONS.contains(&extension.as_str()) {
            return Err(
                "请选择支持的背景图片或 MP4、M4V、WebM、OGV 视频，或关闭「附带个性化设置」".into(),
            );
        }
        let relative = data_path_for(&manifest.data_directory, &format!("background.{extension}"));
        let destination = root.join(&relative);
        copy_snapshot(&source, &destination, background_limit(&extension), cancel)?;
        validate_background(&destination)?;
        settings.background_image = Some(relative);
    }
    settings.validate(manifest)?;
    Ok(settings)
}

pub(crate) fn prepare_snapshot(
    app: &AppHandle,
    request: &PackExportRequest,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<PortableSnapshot, String> {
    cancel.check()?;
    let root = workspace.join("portable-content");
    std::fs::create_dir(&root).map_err(|e| format!("创建便携暂存目录失败：{e}"))?;
    std::fs::create_dir(root.join(app_paths::DATA_DIRECTORY))
        .map_err(|e| format!("创建便携数据目录失败：{e}"))?;
    let mut manifest = PortableManifest {
        schema_version: SCHEMA_VERSION,
        format: FORMAT.into(),
        package_id: crate::store::random_token(),
        name: request.name.clone(),
        version: request.version.clone(),
        instance_name: request.version_id.clone(),
        payload: data_path(PAYLOAD_FILE),
        game_directory: app_paths::GAME_DIRECTORY.into(),
        data_directory: app_paths::DATA_DIRECTORY.into(),
        launcher: None,
        personalization: None,
    };
    let mut files = vec![(manifest.payload.clone(), manifest.payload.clone())];
    let mut inline_entries = Vec::new();
    if request.include_launcher {
        let (source, name) = current_launcher()?;
        copy_snapshot(
            &source,
            &root.join(name),
            ArchiveLimits::default().max_entry_bytes,
            cancel,
        )?;
        manifest.launcher = Some(PortableLauncher {
            path: name.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        });
        files.push((name.into(), name.into()));
    }
    let instance_settings = crate::pack_export::instance_settings_export::project_instance(
        Path::new(&request.dir),
        &request.version_id,
    )?;
    if instance_settings
        .settings
        .as_object()
        .is_some_and(|settings| !settings.is_empty())
    {
        inline_entries.push((
            data_path(INSTANCE_SETTINGS_FILE),
            crate::pack_export::instance_settings_export::serialize(&instance_settings.settings)?,
        ));
    }
    if request.include_personalization {
        let settings = snapshot_personalization(app, &root, &manifest, cancel)?;
        if let Some(path) = &settings.background_image {
            files.push((path.clone(), path.clone()));
        }
        let path = data_path(PERSONALIZATION_FILE);
        inline_entries.push((
            path.clone(),
            serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
        ));
        manifest.personalization = Some(path);
    }
    validate_manifest(&manifest)?;
    Ok(PortableSnapshot {
        root,
        manifest,
        files,
        inline_entries,
    })
}

impl PortableSnapshot {
    pub fn payload_path(&self) -> PathBuf {
        self.root.join(&self.manifest.payload)
    }

    pub fn write(
        &self,
        destination: &Path,
        cancel: &CancellationToken,
        progress: impl FnMut(usize, usize, &str),
    ) -> Result<(), String> {
        cancel.check()?;
        validate_manifest(&self.manifest)?;
        // 导出只消费已冻结载荷，原实例后续变化不参与这次写入。
        let (_, payload) = modpacks::local_manifest(&self.payload_path(), cancel)?;
        if payload.name != self.manifest.name {
            return Err("便携载荷名称不一致，请重新导出".into());
        }
        let launcher_step = match &self.manifest.launcher {
            Some(launcher) if launcher.os == "windows" => format!("运行 {}（Windows {}；需安装 WebView2）。", launcher.path, launcher.arch),
            Some(launcher) => format!("在 Linux {} 上为 {} 赋予执行权限后运行；需安装 WebKitGTK 等系统依赖。", launcher.arch, launcher.path),
            None => "将支持便携整合包的本平台 Nickel 发行版放在解压目录后运行；macOS 放置完整 Nickel.app，与便携标记并列。".into(),
        };
        let guide = format!(
            "{} · {}\n\n1. 将 ZIP 完整解压到可写目录，保持 {} 与 {} 文件夹相邻。\n2. {}\n3. 首次运行将导入 {}，应用包内实例启动设置，并下载缺少的 Minecraft、加载器和 Java 文件；在下载任务中查看进度。\n4. 导入完成后选择账号并启动；失败或取消时重新打开启动器重试。\n",
            self.manifest.name,
            self.manifest.version,
            MANIFEST_FILE,
            self.manifest.data_directory,
            launcher_step,
            self.manifest.payload
        );
        let mut inline = self.inline_entries.clone();
        if self.manifest.launcher.is_some() {
            inline.extend(
                LAUNCHER_LICENSE_ENTRIES
                    .iter()
                    .map(|(path, bytes)| ((*path).into(), bytes.to_vec())),
            );
            inline.push((SOURCE_FILE.into(), LAUNCHER_SOURCE_TEXT.as_bytes().to_vec()));
        }
        inline.push((
            MANIFEST_FILE.into(),
            serde_json::to_vec_pretty(&self.manifest).map_err(|e| e.to_string())?,
        ));
        inline.push((GUIDE_FILE.into(), guide.into_bytes()));
        let limits = ArchiveLimits {
            max_entry_bytes: PAYLOAD_MAX_BYTES,
            ..ArchiveLimits::default()
        };
        write_zip_files(
            destination,
            &self.root,
            &self.files,
            &inline,
            &limits,
            cancel,
            progress,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minecraft_java_rs_core::utils::archive::{extract_zip_subtrees, read_zip_entry};
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("nickel-portable-{}", crate::store::random_token()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_snapshot(root: &Path, launcher: bool, personalization: bool) -> PortableSnapshot {
        std::fs::create_dir_all(root.join(app_paths::DATA_DIRECTORY)).unwrap();
        let manifest = PortableManifest {
            schema_version: SCHEMA_VERSION,
            format: FORMAT.into(),
            package_id: crate::store::random_token(),
            name: "可移动整合包".into(),
            version: "1.0.0".into(),
            instance_name: "portable-instance".into(),
            payload: data_path(PAYLOAD_FILE),
            game_directory: app_paths::GAME_DIRECTORY.into(),
            data_directory: app_paths::DATA_DIRECTORY.into(),
            launcher: launcher.then(|| PortableLauncher {
                path: WINDOWS_LAUNCHER.into(),
                os: "windows".into(),
                arch: "x86_64".into(),
            }),
            personalization: personalization.then(|| data_path(PERSONALIZATION_FILE)),
        };
        let index = json!({
            "formatVersion": 1, "game": "minecraft", "versionId": manifest.version, "name": manifest.name,
            "files": [], "dependencies": { "minecraft": "1.20.1", "fabric-loader": "0.16.9" },
        });
        let cancel = CancellationToken::new();
        write_zip_files(
            &root.join(&manifest.payload),
            root,
            &[],
            &[
                (
                    "modrinth.index.json".into(),
                    serde_json::to_vec(&index).unwrap(),
                ),
                (
                    "overrides/mods/local.jar".into(),
                    b"tiny selected mod".to_vec(),
                ),
                (
                    "overrides/config/client.json".into(),
                    br#"{"quality":"high"}"#.to_vec(),
                ),
            ],
            &ArchiveLimits::default(),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        let mut snapshot = PortableSnapshot {
            root: root.into(),
            files: vec![(manifest.payload.clone(), manifest.payload.clone())],
            manifest,
            inline_entries: vec![],
        };
        if launcher {
            std::fs::write(root.join(WINDOWS_LAUNCHER), b"MZ tiny executable fixture").unwrap();
            snapshot
                .files
                .push((WINDOWS_LAUNCHER.into(), WINDOWS_LAUNCHER.into()));
        }
        if personalization {
            let mut settings = select_personalization(&json!({
                "themeMode": "system", "accent": "emerald", "backgroundBlur": 12, "backgroundOpacity": 0.4,
                "accessToken": "not-exported", "offlineUsername": "not-exported", "preLaunchCommand": "not-exported",
                "javaPath": "C:/private/java", "gameDir": "C:/private/game",
            }));
            let background = data_path("background.png");
            std::fs::write(root.join(&background), b"\x89PNG\r\n\x1a\nfixture").unwrap();
            settings.background_image = Some(background.clone());
            snapshot.files.push((background.clone(), background));
            snapshot.inline_entries.push((
                data_path(PERSONALIZATION_FILE),
                serde_json::to_vec(&settings).unwrap(),
            ));
        }
        snapshot.inline_entries.push((
            data_path(INSTANCE_SETTINGS_FILE),
            serde_json::to_vec(&json!({
                "windowTitle": "便携实例",
                "memoryMode": "custom",
                "memoryMb": 4096,
                "launcherVisibility": "hide",
                "serverAddress": "mc.example.com:25565",
                "jvmArgs": "-XX:+UseG1GC",
                "gameArgs": "--demo",
            }))
            .unwrap(),
        ));
        // 暂存目录中的账号、密钥和本机文件不参与导出。
        for name in [
            "accounts.json",
            "active_account.json",
            "credentials.key",
            "secrets.local.json",
            "settings.json",
        ] {
            std::fs::write(root.join(name), b"not-exported").unwrap();
            std::fs::write(root.join(data_path(name)), b"not-exported").unwrap();
        }
        // 即使本机有同名许可文件，仍应使用编译时的原文。
        for (path, _) in LAUNCHER_LICENSE_ENTRIES {
            let destination = root.join(path);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::write(destination, b"not-exported").unwrap();
        }
        std::fs::write(root.join(SOURCE_FILE), b"not-exported").unwrap();
        snapshot
    }

    fn assert_launcher_distribution(archive: &Path, launcher: bool, cancel: &CancellationToken) {
        let expected_paths = [
            "LICENSE",
            "LICENSE-ASSETS",
            "NOTICE.md",
            "LICENSES/dependencies-NOTICES.txt",
            "LICENSES/OFL-1.1.txt",
            "LICENSES/Apache-2.0.txt",
            "LICENSES/java-launch-wrapper-LICENCE.txt",
            "LICENSES/lwjgl-unsafe-agent-LICENSE.txt",
            "LICENSES/authlib-injector-COPYING.md",
            "LICENSES/AGPLv3-with-authlib-injector-exception.txt",
            "LICENSES/CC-BY-SA-4.0.txt",
            "src-tauri/vendor/minecraft-java-rs-core/LICENSE",
            "src/assets/fonts/NOTICE.md",
            "src/assets/credits/README.md",
        ];
        assert_eq!(LAUNCHER_LICENSE_ENTRIES.len(), expected_paths.len());
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for path in expected_paths {
            let embedded = LAUNCHER_LICENSE_ENTRIES
                .iter()
                .find_map(|(name, bytes)| (*name == path).then_some(*bytes))
                .expect(path);
            let bundle_input = std::fs::read(source_root.join(path)).unwrap();
            assert_eq!(embedded, bundle_input.as_slice(), "bundle/embedded {path}");
            let actual = read_zip_entry(archive, path, embedded.len() as u64, cancel);
            if launcher {
                let actual = actual.unwrap();
                assert_eq!(actual, embedded, "{path}");
                if path == "LICENSES/dependencies-NOTICES.txt" {
                    let text = std::str::from_utf8(&actual).unwrap();
                    assert!(text.contains("Copyright 2018 Robert F. Masen"));
                }
            } else {
                assert!(actual.is_err(), "{path}");
            }
        }
        let source = read_zip_entry(
            archive,
            SOURCE_FILE,
            LAUNCHER_SOURCE_TEXT.len() as u64,
            cancel,
        );
        if launcher {
            let source = source.unwrap();
            assert_eq!(source, LAUNCHER_SOURCE_TEXT.as_bytes());
            let text = std::str::from_utf8(&source).unwrap();
            assert!(text.contains("https://github.com/Nickel-Studio/Ni-Launcher"));
            assert!(text.contains(env!("CARGO_PKG_VERSION")));
            let normalized = text.to_ascii_lowercase();
            for forbidden in [
                "c:/",
                "d:/",
                "c:\\",
                "d:\\",
                "private",
                "file://",
                "credentials.key",
                "secrets.local.json",
            ] {
                assert!(!normalized.contains(forbidden), "{forbidden}");
            }
        } else {
            assert!(source.is_err());
        }
    }

    #[test]
    fn portable_zip_four_options_roundtrip_and_relocate_through_standard_payload() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        for launcher in [false, true] {
            for personalization in [false, true] {
                let case = format!("{launcher}-{personalization}");
                let snapshot = fixture_snapshot(&fixture.0.join(&case), launcher, personalization);
                let archive = fixture.0.join(format!("{case}.zip"));
                snapshot.write(&archive, &cancel, |_, _, _| {}).unwrap();
                let license_bytes = if launcher {
                    LAUNCHER_LICENSE_ENTRIES
                        .iter()
                        .map(|(_, bytes)| bytes.len() as u64)
                        .sum::<u64>()
                        + LAUNCHER_SOURCE_TEXT.len() as u64
                } else {
                    0
                };
                assert!(std::fs::metadata(&archive).unwrap().len() < 16 * 1024 + license_bytes);
                assert_launcher_distribution(&archive, launcher, &cancel);
                assert!(
                    read_zip_entry(&archive, "modrinth.index.json", MANIFEST_MAX_BYTES, &cancel)
                        .is_err()
                );
                for name in [
                    "accounts.json",
                    "active_account.json",
                    "credentials.key",
                    "secrets.local.json",
                    "settings.json",
                ] {
                    assert!(
                        read_zip_entry(&archive, name, MANIFEST_MAX_BYTES, &cancel).is_err(),
                        "{name}"
                    );
                    assert!(
                        read_zip_entry(&archive, &data_path(name), MANIFEST_MAX_BYTES, &cancel)
                            .is_err(),
                        "{name}"
                    );
                }
                assert!(
                    read_zip_entry(
                        &archive,
                        &data_path(INSTANCE_SETTINGS_FILE),
                        MANIFEST_MAX_BYTES,
                        &cancel
                    )
                    .is_ok()
                );
                assert_eq!(
                    read_zip_entry(&archive, WINDOWS_LAUNCHER, MANIFEST_MAX_BYTES, &cancel).is_ok(),
                    launcher
                );
                assert_eq!(
                    read_zip_entry(
                        &archive,
                        &data_path(PERSONALIZATION_FILE),
                        MANIFEST_MAX_BYTES,
                        &cancel
                    )
                    .is_ok(),
                    personalization
                );
                let relocated = fixture.0.join(format!("relocated-{case}"));
                std::fs::create_dir(&relocated).unwrap();
                std::fs::write(
                    relocated.join(MANIFEST_FILE),
                    read_zip_entry(&archive, MANIFEST_FILE, MANIFEST_MAX_BYTES, &cancel).unwrap(),
                )
                .unwrap();
                extract_zip_subtrees(
                    &archive,
                    &relocated.join(app_paths::DATA_DIRECTORY),
                    &[app_paths::DATA_DIRECTORY.into()],
                    &[],
                    &ArchiveLimits::default(),
                    &cancel,
                )
                .unwrap();
                let bootstrap = load_bootstrap(&relocated).unwrap().unwrap();
                assert_eq!(bootstrap.manifest.package_id, snapshot.manifest.package_id);
                assert_eq!(bootstrap.manifest.instance_name, "portable-instance");
                assert_eq!(bootstrap.manifest.data_directory, app_paths::DATA_DIRECTORY);
                assert_eq!(
                    bootstrap.game_directory,
                    std::fs::canonicalize(&relocated)
                        .unwrap()
                        .join(app_paths::GAME_DIRECTORY)
                );
                let (_, payload) =
                    modpacks::local_manifest(&bootstrap.payload_path, &cancel).unwrap();
                assert_eq!(payload.minecraft_version, "1.20.1");
                assert_eq!(payload.loader.unwrap().version, "0.16.9");
                let settings = instance_settings_for_payload(&bootstrap.payload_path)
                    .unwrap()
                    .unwrap();
                assert_eq!(settings["windowTitle"], "便携实例");
                assert_eq!(settings["memoryMb"], 4096);
                assert!(settings.get("javaPath").is_none());
                let imported = relocated.join("imported-content");
                extract_zip_subtrees(
                    &bootstrap.payload_path,
                    &imported,
                    &payload.overrides,
                    &[],
                    &ArchiveLimits::default(),
                    &cancel,
                )
                .unwrap();
                assert_eq!(
                    std::fs::read(imported.join("mods/local.jar")).unwrap(),
                    b"tiny selected mod"
                );
                assert!(!imported.join("gameData.json").exists());
                if let Some(settings) = bootstrap.personalization {
                    let text = serde_json::to_string(&settings).unwrap();
                    assert!(!text.contains("not-exported"));
                    assert!(!text.contains("C:/"));
                    let background = data_path("background.png");
                    assert_eq!(
                        settings.background_image.as_deref(),
                        Some(background.as_str())
                    );
                    let value = settings
                        .resolved_value(&bootstrap.root, &bootstrap.manifest)
                        .unwrap();
                    assert_eq!(
                        PathBuf::from(value["backgroundImage"].as_str().unwrap()),
                        bootstrap.root.join(background)
                    );
                }
            }
        }
    }

    #[test]
    fn launcher_zip_preserves_local_notice_links() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        let snapshot = fixture_snapshot(&fixture.0.join("source"), true, false);
        let archive = fixture.0.join("launcher.zip");
        snapshot.write(&archive, &cancel, |_, _, _| {}).unwrap();
        for path in [
            "NOTICE.md",
            "src/assets/fonts/NOTICE.md",
            "src/assets/credits/README.md",
        ] {
            let bytes = read_zip_entry(&archive, path, MANIFEST_MAX_BYTES, &cancel).unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            for link in text.split("](").skip(1) {
                let target = link.split(')').next().unwrap();
                if target.starts_with("https://")
                    || target.starts_with("http://")
                    || target.starts_with('#')
                {
                    continue;
                }
                let mut components: Vec<_> = path.split('/').collect();
                let _ = components.pop();
                for component in target.split('/') {
                    match component {
                        ".." => {
                            assert!(components.pop().is_some(), "{path} -> {target}");
                        }
                        "." => {}
                        part => components.push(part),
                    }
                }
                let resolved = components.join("/");
                assert!(
                    read_zip_entry(
                        &archive,
                        &resolved,
                        ArchiveLimits::default().max_entry_bytes,
                        &cancel
                    )
                    .is_ok(),
                    "{path} -> {target}"
                );
            }
        }
    }

    #[test]
    fn launcher_licenses_follow_windows_and_linux_layouts() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        let directory = app_paths::DATA_DIRECTORY;
        for (os, name) in [("windows", WINDOWS_LAUNCHER), ("linux", UNIX_LAUNCHER)] {
            let case = format!("{directory}-{os}");
            let root = fixture.0.join(&case);
            let mut snapshot = fixture_snapshot(&root, false, false);
            snapshot.manifest.data_directory = directory.into();
            snapshot.manifest.payload = data_path_for(directory, PAYLOAD_FILE);
            snapshot.manifest.launcher = Some(PortableLauncher {
                path: name.into(),
                os: os.into(),
                arch: "x86_64".into(),
            });
            std::fs::write(root.join(name), b"tiny executable fixture").unwrap();
            snapshot.files = vec![
                (
                    snapshot.manifest.payload.clone(),
                    snapshot.manifest.payload.clone(),
                ),
                (name.into(), name.into()),
            ];
            for (path, _) in &mut snapshot.inline_entries {
                *path = data_path_for(directory, INSTANCE_SETTINGS_FILE);
            }
            let archive = fixture.0.join(format!("{case}.zip"));
            snapshot.write(&archive, &cancel, |_, _, _| {}).unwrap();
            assert_launcher_distribution(&archive, true, &cancel);
            let manifest: PortableManifest = serde_json::from_slice(
                &read_zip_entry(&archive, MANIFEST_FILE, MANIFEST_MAX_BYTES, &cancel).unwrap(),
            )
            .unwrap();
            assert_eq!(manifest, snapshot.manifest);
            assert!(
                read_zip_entry(&archive, &manifest.payload, MANIFEST_MAX_BYTES, &cancel)
                    .is_ok()
            );
            assert_eq!(
                read_zip_entry(&archive, name, MANIFEST_MAX_BYTES, &cancel).unwrap(),
                b"tiny executable fixture"
            );
            assert!(
                read_zip_entry(
                    &archive,
                    &data_path_for(directory, INSTANCE_SETTINGS_FILE),
                    MANIFEST_MAX_BYTES,
                    &cancel
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn launcher_license_path_conflicts_create_no_zip() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        for inline in [false, true] {
            let case = format!("conflict-{inline}");
            let mut snapshot = fixture_snapshot(&fixture.0.join(&case), true, false);
            if inline {
                snapshot
                    .inline_entries
                    .push(("LICENSE".into(), b"not-exported".to_vec()));
            } else {
                snapshot.files.push(("LICENSE".into(), "LICENSE".into()));
            }
            let archive = fixture.0.join(format!("{case}.zip"));
            assert!(snapshot.write(&archive, &cancel, |_, _, _| {}).is_err());
            assert!(!archive.exists());
        }
    }

    #[test]
    fn bootstrap_rejects_escape_unknown_fields_missing_payload_and_invalid_identity() {
        let fixture = Fixture::new();
        let root = fixture.0.join("source");
        let snapshot = fixture_snapshot(&root, false, false);
        let good = serde_json::to_value(&snapshot.manifest).unwrap();
        for (key, value) in [
            ("payload", json!("../escape.mrpack")),
            ("gameDirectory", json!("C:/private/game")),
            ("dataDirectory", json!("../data")),
            ("instanceName", json!("../instance")),
            ("packageId", json!("")),
            ("schemaVersion", json!(2)),
            ("preLaunchCommand", json!("not-allowed")),
            ("personalization", json!("C:/private/settings.json")),
        ] {
            let mut invalid = good.clone();
            invalid[key] = value;
            std::fs::write(
                root.join(MANIFEST_FILE),
                serde_json::to_vec(&invalid).unwrap(),
            )
            .unwrap();
            assert!(read_manifest(&root).is_err(), "{key}");
        }
        std::fs::write(root.join(MANIFEST_FILE), serde_json::to_vec(&good).unwrap()).unwrap();
        let settings_path = root.join(data_path(INSTANCE_SETTINGS_FILE));
        std::fs::write(&settings_path, br#"{"unknownFutureKey":true}"#).unwrap();
        assert!(load_bootstrap(&root).is_err());
        std::fs::remove_file(&settings_path).unwrap();
        std::fs::remove_file(root.join(data_path(PAYLOAD_FILE))).unwrap();
        assert!(read_manifest(&root).unwrap().is_some());
        assert!(load_bootstrap(&root).is_err());
        assert!(read_manifest(&fixture.0).unwrap().is_none());
    }

    fn manifest_for_layout(data_directory: &str) -> PortableManifest {
        PortableManifest {
            schema_version: SCHEMA_VERSION,
            format: FORMAT.into(),
            package_id: "a".repeat(32),
            name: "Layout fixture".into(),
            version: "1.0".into(),
            instance_name: "fixture-instance".into(),
            payload: data_path_for(data_directory, PAYLOAD_FILE),
            game_directory: app_paths::GAME_DIRECTORY.into(),
            data_directory: data_directory.into(),
            launcher: None,
            personalization: Some(data_path_for(data_directory, PERSONALIZATION_FILE)),
        }
    }

    #[test]
    fn manifest_requires_the_canonical_layout() {
        let manifest = manifest_for_layout(app_paths::DATA_DIRECTORY);
        validate_manifest(&manifest).unwrap();
        for (os, path) in [("windows", WINDOWS_LAUNCHER), ("linux", UNIX_LAUNCHER)] {
            let mut value = manifest.clone();
            value.launcher = Some(PortableLauncher { path: path.into(), os: os.into(), arch: "x86_64".into() });
            validate_manifest(&value).unwrap();
        }
        for directory in ["Other", "../Nickel", "Nickel/../Other", "Nickel/", "C:/Nickel", "nickel"] {
            let mut value = manifest_for_layout(directory);
            assert!(validate_manifest(&value).is_err());
            value.data_directory = app_paths::DATA_DIRECTORY.into();
            assert!(validate_manifest(&value).is_err());
        }
        for path in ["Other.exe", "../Nickel.exe", "Nickel/../Nickel.exe", "C:/Nickel.exe", "Nickel.exe:stream"] {
            let mut value = manifest.clone();
            value.launcher = Some(PortableLauncher { path: path.into(), os: "windows".into(), arch: "x86_64".into() });
            assert!(validate_manifest(&value).is_err(), "{path}");
        }
    }

    #[test]
    fn personalization_only_allows_appearance_and_matching_relative_raster_background() {
        let selected = select_personalization(&json!({
            "themeMode": "dark", "accent": "mono", "backgroundBlur": 0, "backgroundOpacity": 0.5,
            "backgroundImage": "C:/Users/private/wallpaper.png", "accounts": [], "accessToken": "credential",
            "gameDir": "/home/private/game", "javaPath": "/home/private/java", "preLaunchCommand": "run",
        }));
        assert_eq!(
            serde_json::to_value(&selected).unwrap(),
            json!({ "themeMode": "dark", "accent": "mono", "backgroundBlur": 0.0, "backgroundOpacity": 0.5 })
        );
        let directory = app_paths::DATA_DIRECTORY;
        let manifest = manifest_for_layout(directory);
        for value in [
            json!({"accessToken": "credential"}),
            json!({"preLaunchCommand": "run"}),
            json!({"backgroundImage": "C:/Users/private/image.png"}),
            json!({"backgroundImage": "../image.png"}),
            json!({"backgroundImage": data_path_for(directory, "background.svg")}),
            json!({"backgroundImage": data_path_for("Other", "background.png")}),
            json!({"backgroundImage": format!("{directory}/../background.png")}),
            json!({"backgroundOpacity": 2}),
            json!({"accent": "not-a-color"}),
        ] {
            assert!(
                serde_json::from_value::<PortablePersonalization>(value)
                    .map(|v| v.validate(&manifest).is_err())
                    .unwrap_or(true)
            );
        }
        let settings: PortablePersonalization = serde_json::from_value(
            json!({"backgroundImage": data_path_for(directory, "background.png")}),
        )
        .unwrap();
        settings.validate(&manifest).unwrap();
    }


    #[test]
    fn personalization_theme_roundtrip_preserves_all_accents_and_custom_values() {
        let custom = json!({
            "hue": 312.5, "saturation": 82.0, "lightness": 46.5,
            "titlebarHueShift": -72.0, "decoration": "folds", "decorationOpacity": 0.72,
        });
        let directory = app_paths::DATA_DIRECTORY;
        let manifest = manifest_for_layout(directory);
        for accent in ACCENTS {
            let selected = select_personalization(&json!({
                "themeMode": "light", "accent": accent, "customTheme": custom,
                "titlebarTint": true,
            }));
            let encoded = serde_json::to_value(&selected).unwrap();
            assert_eq!(encoded["accent"], *accent);
            assert_eq!(encoded["customTheme"], custom);
            assert_eq!(encoded["titlebarTint"], true);
            let decoded: PortablePersonalization = serde_json::from_value(encoded.clone()).unwrap();
            decoded.validate(&manifest).unwrap();
            assert_eq!(decoded.resolved_value(Path::new("."), &manifest).unwrap(), encoded);
            assert_eq!(decoded, selected);
        }
    }

    #[test]
    fn personalization_theme_projection_bounds_values_and_drops_nested_private_state() {
        let projected = select_personalization(&json!({
            "accent": "custom", "titlebarTint": false,
            "customTheme": {
                "hue": 999, "saturation": -1, "lightness": 150,
                "titlebarHueShift": -999, "decoration": "not-a-decoration", "decorationOpacity": 3,
                "accessToken": "credential", "eulaAccepted": true, "changelogSeen": "future",
            },
            "eulaAccepted": true, "changelogSeen": "future", "accounts": [],
            "accessToken": "credential", "javaPath": "private", "preLaunchCommand": "run",
        }));
        assert_eq!(serde_json::to_value(&projected).unwrap(), json!({
            "accent": "custom", "titlebarTint": false,
            "customTheme": {
                "hue": 360.0, "saturation": 0.0, "lightness": 100.0,
                "titlebarHueShift": -180.0, "decoration": "lattice", "decorationOpacity": 1.0,
            },
        }));
        projected.validate(&manifest_for_layout(app_paths::DATA_DIRECTORY)).unwrap();
        for malformed in [Value::Null, json!(false), json!([]), json!("invalid")] {
            let selected = select_personalization(&json!({"accent": "custom", "customTheme": malformed}));
            assert_eq!(selected.custom_theme, Some(PortableCustomTheme::default()));
        }
        let selected = select_personalization(&json!({"accent": "custom", "customTheme": {
            "hue": "42", "saturation": null, "lightness": false,
            "titlebarHueShift": [], "decorationOpacity": {},
        }}));
        assert_eq!(selected.custom_theme, Some(PortableCustomTheme::default()));
    }

    #[test]
    fn personalization_theme_import_rejects_invalid_or_unknown_fields() {
        let manifest = manifest_for_layout(app_paths::DATA_DIRECTORY);
        for (key, invalid) in [
            ("hue", json!(-0.01)), ("hue", json!(360.01)),
            ("saturation", json!(100.01)), ("lightness", json!(-0.01)),
            ("titlebarHueShift", json!(-180.01)), ("titlebarHueShift", json!(180.01)),
            ("decorationOpacity", json!(-0.01)), ("decorationOpacity", json!(1.01)),
            ("decoration", json!("external-image")),
            ("accessToken", json!("credential")), ("eulaAccepted", json!(true)),
            ("changelogSeen", json!("future")),
        ] {
            let mut custom = serde_json::to_value(PortableCustomTheme::default()).unwrap();
            custom[key] = invalid;
            assert!(serde_json::from_value::<PortablePersonalization>(json!({
                "accent": "custom", "customTheme": custom,
            })).map(|v| v.validate(&manifest).is_err()).unwrap_or(true), "accepted invalid {key}");
        }
        for value in [
            json!({"eulaAccepted": true}), json!({"changelogSeen": "future"}),
            json!({"titlebarTint": "true"}), json!({"customTheme": {"hue": 42}}),
        ] {
            assert!(serde_json::from_value::<PortablePersonalization>(value).is_err());
        }
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!PortableCustomTheme { hue: invalid, ..PortableCustomTheme::default() }.is_valid());
        }
    }



    #[test]
    fn personalization_theme_zip_roundtrip_exports_only_appearance() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        let whitelist = [
            "themeMode", "accent", "customTheme", "titlebarTint",
            "backgroundBlur", "backgroundOpacity", "backgroundImage",
        ];
        for (index, accent) in ACCENTS.iter().enumerate() {
            let source = fixture.0.join(format!("theme-{index}"));
            let mut snapshot = fixture_snapshot(&source, false, false);
            let custom = json!({
                "hue": 312.5, "saturation": 82.0, "lightness": 46.5,
                "titlebarHueShift": -72.0, "decoration": "folds", "decorationOpacity": 0.72,
            });
            let mut private_custom = custom.clone();
            private_custom["accessToken"] = json!("private-credential");
            private_custom["eulaAccepted"] = json!(true);
            let selected = select_personalization(&json!({
                "themeMode": "light", "accent": accent, "customTheme": private_custom,
                "titlebarTint": index % 2 == 0, "backgroundBlur": 12, "backgroundOpacity": 0.4,
                "eulaAccepted": true, "eulaVersion": "private-version",
                "accounts": [{"accessToken": "private-credential"}],
                "activeAccount": "private-account", "changelogSeen": "private-update",
                "updateHistory": ["private-update"], "onboardingComplete": true,
            }));
            selected.validate(&snapshot.manifest).unwrap();
            let relative = data_path(PERSONALIZATION_FILE);
            snapshot.manifest.personalization = Some(relative.clone());
            snapshot.inline_entries.push((relative.clone(), serde_json::to_vec(&selected).unwrap()));
            let archive = fixture.0.join(format!("theme-{index}.zip"));
            snapshot.write(&archive, &cancel, |_, _, _| {}).unwrap();
            let bytes = read_zip_entry(&archive, &relative, MANIFEST_MAX_BYTES, &cancel).unwrap();
            let encoded: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(encoded.as_object().unwrap().keys().all(|key| whitelist.contains(&key.as_str())));
            assert_eq!(encoded["accent"], *accent);
            assert_eq!(encoded["customTheme"], custom);
            assert_eq!(encoded["titlebarTint"], index % 2 == 0);
            assert!(!String::from_utf8(bytes.clone()).unwrap().contains("private-"));
            for forbidden in ["accounts.json", "active_account.json", "settings.json", "eula.json", "update-history.json"] {
                assert!(read_zip_entry(&archive, forbidden, MANIFEST_MAX_BYTES, &cancel).is_err());
                assert!(read_zip_entry(&archive, &data_path(forbidden), MANIFEST_MAX_BYTES, &cancel).is_err());
            }
            let relocated = fixture.0.join(format!("theme-relocated-{index}"));
            std::fs::create_dir(&relocated).unwrap();
            std::fs::write(
                relocated.join(MANIFEST_FILE),
                read_zip_entry(&archive, MANIFEST_FILE, MANIFEST_MAX_BYTES, &cancel).unwrap(),
            ).unwrap();
            extract_zip_subtrees(
                &archive, &relocated.join(app_paths::DATA_DIRECTORY),
                &[app_paths::DATA_DIRECTORY.into()], &[], &ArchiveLimits::default(), &cancel,
            ).unwrap();
            let bootstrap = load_bootstrap(&relocated).unwrap().unwrap();
            let decoded = bootstrap.personalization.unwrap();
            assert_eq!(decoded, selected);
            assert_eq!(decoded.resolved_value(&bootstrap.root, &bootstrap.manifest).unwrap(), encoded);
        }
    }




    #[test]
    fn sidecar_snapshot_does_not_follow_source_changes_and_cancellation_creates_no_zip() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.png");
        let copied = fixture.0.join("copy.png");
        let cancel = CancellationToken::new();
        std::fs::write(&source, b"\x89PNG\r\n\x1a\nselected").unwrap();
        copy_snapshot(&source, &copied, BACKGROUND_MAX_BYTES, &cancel).unwrap();
        std::fs::write(&source, b"changed to another file").unwrap();
        validate_background(&copied).unwrap();
        assert_eq!(
            std::fs::read(&copied).unwrap(),
            b"\x89PNG\r\n\x1a\nselected"
        );
        assert!(validate_background(&source).is_err());
        let snapshot = fixture_snapshot(&fixture.0.join("portable"), false, false);
        let destination = fixture.0.join("cancelled.zip");
        cancel.cancel();
        assert!(snapshot.write(&destination, &cancel, |_, _, _| {}).is_err());
        assert!(!destination.exists());
        assert!(
            copy_snapshot(
                &source,
                &fixture.0.join("cancelled.png"),
                BACKGROUND_MAX_BYTES,
                &cancel
            )
            .is_err()
        );
        assert!(!fixture.0.join("cancelled.png").exists());
    }

    #[test]
    fn debug_and_macos_bundle_are_not_presented_as_standalone_executables() {
        assert!(
            launcher_path_for(Path::new("build/debug/ni-launcher.exe"), true, "windows")
                .unwrap_err()
                .contains("发行版")
        );
        for bundle in [
            "Applications/Nickel.app/Contents/MacOS/ni-launcher",
            "Applications/Nickel.app/Contents/MacOS/ni-launcher",
        ] {
            assert!(
                launcher_path_for(Path::new(bundle), false, "macos")
                    .unwrap_err()
                    .contains("完整 Nickel.app")
            );
        }
        assert_eq!(
            launcher_path_for(Path::new("build/release/ni-launcher.exe"), false, "windows")
                .unwrap(),
            WINDOWS_LAUNCHER
        );
        assert_eq!(
            launcher_path_for(Path::new("build/release/ni-launcher"), false, "linux").unwrap(),
            UNIX_LAUNCHER
        );
    }

    #[test]
    fn preexisting_zip_is_preserved_on_export_failure() {
        let fixture = Fixture::new();
        let snapshot = fixture_snapshot(&fixture.0.join("source"), false, false);
        let destination = fixture.0.join("existing.zip");
        std::fs::write(&destination, b"existing archive").unwrap();
        assert!(
            snapshot
                .write(&destination, &CancellationToken::new(), |_, _, _| {})
                .is_err()
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing archive");
    }
    #[test]
    fn personalization_video_zip_roundtrip_relocates_only_validated_media() {
        let fixture = Fixture::new();
        let cancel = CancellationToken::new();
        for (extension, bytes) in [
            ("mp4", b"\0\0\0\x18ftypisom\0\0\0\0isommp42video-fixture".as_slice()),
            ("m4v", b"\0\0\0\x18ftypisom\0\0\0\0isommp42video-fixture".as_slice()),
            ("webm", b"\x1a\x45\xdf\xa3\x42\x82\x84webmfixture".as_slice()),
            ("ogv", b"OggSfixture\x80theora".as_slice()),
        ] {
            let mut snapshot = fixture_snapshot(&fixture.0.join(format!("video-{extension}")), false, false);
            let relative = data_path(&format!("background.{extension}"));
            let path = snapshot.root.join(&relative);
            std::fs::write(&path, bytes).unwrap();
            validate_background(&path).unwrap();
            let mut settings = select_personalization(&json!({"accent": "custom", "accounts": ["private-fixture"]}));
            settings.background_image = Some(relative.clone());
            settings.validate(&snapshot.manifest).unwrap();
            let personalization = data_path(PERSONALIZATION_FILE);
            snapshot.manifest.personalization = Some(personalization.clone());
            snapshot.files.push((relative.clone(), relative.clone()));
            snapshot.inline_entries.push((personalization.clone(), serde_json::to_vec(&settings).unwrap()));
            let archive = fixture.0.join(format!("video-{extension}.zip"));
            snapshot.write(&archive, &cancel, |_, _, _| {}).unwrap();
            assert_eq!(read_zip_entry(&archive, &relative, crate::media::MAX_VIDEO_BYTES, &cancel).unwrap(), bytes);
            let relocated = fixture.0.join(format!("video-relocated-{extension}"));
            std::fs::create_dir(&relocated).unwrap();
            std::fs::write(relocated.join(MANIFEST_FILE), read_zip_entry(&archive, MANIFEST_FILE, MANIFEST_MAX_BYTES, &cancel).unwrap()).unwrap();
            extract_zip_subtrees(&archive, &relocated.join(app_paths::DATA_DIRECTORY), &[app_paths::DATA_DIRECTORY.into()], &[], &ArchiveLimits::default(), &cancel).unwrap();
            let bootstrap = load_bootstrap(&relocated).unwrap().unwrap();
            let decoded = bootstrap.personalization.unwrap();
            assert_eq!(decoded, settings);
            let resolved = decoded.resolved_value(&bootstrap.root, &bootstrap.manifest).unwrap();
            assert_eq!(PathBuf::from(resolved["backgroundImage"].as_str().unwrap()), bootstrap.root.join(relative));
            assert!(!serde_json::to_string(&resolved).unwrap().contains("private-fixture"));
        }
        assert_eq!(background_limit("mp4"), crate::media::MAX_VIDEO_BYTES);
        assert_eq!(background_limit("png"), BACKGROUND_MAX_BYTES);
        // Export and import limits must move together, or a video could export but fail to import.
        assert!(crate::media::MAX_VIDEO_BYTES <= ArchiveLimits::default().max_entry_bytes);
    }

}
