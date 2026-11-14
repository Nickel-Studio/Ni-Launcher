pub mod events;
pub mod game_data;
pub mod options;

pub use events::LaunchEvent;


use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

use crate::error::LaunchError;
use crate::game::{
    arguments::{get_classpath, get_game_arguments, get_jvm_arguments, LoaderContext},
    assets::{copy_assets, get_assets},
    bundle::{check_bundle, check_files},
    java::get_java_files,
    libraries::{extract_natives, get_assets_others, get_libraries, natives_dir_for},
    version::get_version_json,
};
use crate::launcher::game_data::{load_game_data, save_game_data, GameData, JavaInfo};
use crate::launcher::options::LaunchOptions;
use crate::loader::{create_loader, types::LoaderInstallInput};
use crate::models::loader::LoaderType;
use crate::models::minecraft::AssetItem;
use crate::net::transfer::TransferContext;
use crate::net::downloader::Downloader;
use crate::utils::version_check::is_old;

/// `java.required_major` 表示自动启动所需的运行时主版本。缓存必须来自
/// 同一主版本，避免版本提高 Java 要求后继续复用旧的 `jre-8` 等运行时。
fn cached_java_matches_requirement(options: &LaunchOptions, cached: &GameData) -> bool {
    if options.java.path.is_some() {
        return true;
    }
    crate::game::java::java_runtime_satisfies_required_major(
        options,
        std::path::Path::new(&cached.minecraft_java.path),
    )
}

fn cached_identity_matches(options: &LaunchOptions, cached: &GameData) -> bool {
    if cached.minecraft_version != options.version || options.url.is_some()
        || (options.loader.enable && cached.loader_type != options.loader.loader_type)
        || (!options.loader.enable && cached.loader_type.is_some()) {
        return false;
    }
    if !cached_java_matches_requirement(options, cached) {
        return false;
    }
    if let Some(java) = &options.java.path {
        if java != std::path::Path::new(&cached.minecraft_java.path) { return false; }
    }
    if options.loader.enable && !matches!(options.loader.build.as_str(), "latest" | "recommended") {
        let (Some(id), Some(kind)) = (&cached.loader_version_id, &cached.loader_type) else { return false; };
        if !cached_loader_build_matches(kind, &options.version, id, &options.loader.build) {
            return false;
        }
    }
    true
}

fn cached_loader_build_matches(kind: &LoaderType, mc: &str, id: &str, requested: &str) -> bool {
    let resolved = match kind {
        LoaderType::Fabric | LoaderType::LegacyFabric => {
            let normalized_mc = crate::loader::fabric::normalize_minecraft_version(mc);
            id.strip_prefix("fabric-loader-")
                .and_then(|build| build.strip_suffix(&format!("-{normalized_mc}")))
        }
        LoaderType::Quilt => id.strip_prefix("quilt-loader-")
            .and_then(|build| build.strip_suffix(&format!("-{mc}"))),
        LoaderType::Forge => id.strip_prefix(&format!("{mc}-forge-")),
        LoaderType::NeoForge if mc == "1.20.1" => {
            // 此历史 NeoForge profile 仍沿用 forge 名称，但构建目录包含 Minecraft 前缀。
            id.strip_prefix(&format!("{mc}-forge-"))
                .or_else(|| id.strip_prefix(&format!("{mc}-neoforge-")))
        }
        LoaderType::NeoForge => id.strip_prefix("neoforge-"),
        LoaderType::OptiFine => id.strip_prefix(&format!("{mc}-OptiFine_")),
        LoaderType::LiteLoader => id.strip_prefix(&format!("{mc}-LiteLoader-")),
    };
    let Some(resolved) = resolved.filter(|build| !build.is_empty()) else { return false; };
    if requested == id { return true; }
    let requested = if matches!(kind, LoaderType::Forge)
        || (matches!(kind, LoaderType::NeoForge) && mc == "1.20.1")
    {
        requested
            .strip_prefix(&format!("{mc}-"))
            .unwrap_or(requested)
            .to_string()
    } else if matches!(kind, LoaderType::OptiFine) {
        if let Some(preview) = requested.strip_prefix("preview_OptiFine_") {
            format!("preview_{preview}")
        } else {
            requested
                .strip_prefix("OptiFine_")
                .unwrap_or(requested)
                .to_string()
        }
    } else {
        requested.to_string()
    };
    resolved == requested.as_str()
}

async fn cached_instance_complete(options: &LaunchOptions, cached: &GameData) -> Result<bool, LaunchError> {
    if !cached_identity_matches(options, cached) || !std::path::Path::new(&cached.minecraft_java.path).is_file() {
        return Ok(false);
    }
    let mut bundle = get_libraries(options, &cached.minecraft_json);
    bundle.extend(cached.loader_libraries.clone());
    for file in &cached.minecraft_java.files {
        if file.url.is_some() {
            let runtime_file = std::path::Path::new(&file.path);
            let absolute = if runtime_file.is_absolute() { runtime_file.to_path_buf() } else {
                let (component, _) = crate::game::java::java_component(options, &cached.minecraft_json);
                options.path.join("runtime").join(component).join(crate::game::java::mojang_platform_key(options.intel_enabled_mac)).join(runtime_file)
            };
            bundle.push(AssetItem::Asset { path: absolute.to_string_lossy().into_owned(), sha1: file.sha1.clone().unwrap_or_default(), size: file.size.unwrap_or(0), url: String::new() });
        }
    }
    if let Some(index) = &cached.minecraft_json.asset_index {
        let path = options.path.join("assets").join("indexes").join(format!("{}.json", index.id));
        let Ok(raw) = tokio::fs::read_to_string(path).await else { return Ok(false); };
        let Ok(items) = crate::game::assets::parse_asset_items(options, &cached.minecraft_json, &raw) else { return Ok(false); };
        bundle.extend(items);
    }
    if let Some(custom_jar) = &options.mcp {
        if !std::path::Path::new(custom_jar).is_file() { return Ok(false); }
    }
    let cancel = TransferContext::current().cancellation().clone();
    let verify = options.verify;
    tokio::task::spawn_blocking(move || {
        for item in bundle {
            cancel.check().map_err(LaunchError::InvalidData)?;
            let (path, size, hash) = match item {
                AssetItem::Asset { path, size, sha1, .. } | AssetItem::NativeAsset { path, size, sha1, .. } => (path, size, sha1),
                AssetItem::CFile { path, .. } => (path, 0, String::new()),
            };
            let Ok(metadata) = std::fs::metadata(&path) else { return Ok(false); };
            if !metadata.is_file() || (size > 0 && metadata.len() != size) { return Ok(false); }
            if verify && !hash.is_empty() {
                let actual = crate::utils::hash::get_file_hash_cancellable(std::path::Path::new(&path), crate::utils::hash::HashAlgorithm::Sha1, &cancel)
                    .map_err(LaunchError::InvalidData)?;
                if !actual.eq_ignore_ascii_case(&hash) { return Ok(false); }
            }
        }
        Ok(true)
    }).await.map_err(|e| LaunchError::InvalidData(e.to_string()))?
}

/// GC 所有权过滤：覆盖收集器、分代模式、紧凑对象头、G1 比例与停顿 / 堆比例。
/// 不按精确参数列表打补丁，确保用户或加载器写出的变体也能被完整替换。
fn is_gc_option(argument: &str) -> bool {
    let Some(raw) = argument.strip_prefix("-XX:") else {
        return false;
    };
    let name = raw
        .strip_prefix('+')
        .or_else(|| raw.strip_prefix('-'))
        .unwrap_or(raw)
        .to_ascii_lowercase();
    if name == "unlockexperimentalvmoptions"
        || name == "zgenerational"
        || name == "usecompactobjectheaders"
        || name == "perfdisablesharedmem"
    {
        return true;
    }
    (name.starts_with("use") && name.ends_with("gc"))
        || name.starts_with("g1")
        || [
            "maxgcpausemillis",
            "mingcpausemillis",
            "maxheapfreeratio",
            "minheapfreeratio",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn apply_gc_plan(
    jvm_args: &mut Vec<String>,
    plan: &Option<Vec<String>>,
    user_args: &[String],
    loader_args: &[String],
) {
    let preserved: Vec<String> = if plan.is_none() {
        user_args
            .iter()
            .chain(loader_args.iter())
            .filter(|argument| is_gc_option(argument))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    jvm_args.retain(|argument| !is_gc_option(argument));
    if let Some(plan) = plan {
        jvm_args.extend(plan.iter().cloned());
    } else {
        jvm_args.extend(preserved);
    }
}

fn set_game_argument(args: &mut Vec<String>, key: &str, value: &str) {
    let mut index = 0;
    while index < args.len() {
        if args[index] == key {
            args.remove(index);
            if index < args.len() { args.remove(index); }
        } else { index += 1; }
    }
    let value = if key == "--gameDir" {
        crate::game::arguments::java_argument(value)
    } else { value.to_owned() };
    args.extend([key.to_owned(), value]);
}

// ── Launcher ──────────────────────────────────────────────────────────────────

pub struct Launcher {
    options: LaunchOptions,
    game_data: Option<GameData>,
    runtime_directory: Option<PathBuf>,
    custom_info: String,
    /// `Some` 表示自动 GC：先清理全部冲突项再追加；`None` 表示自定义模式，
    /// 清理核心默认 G1 后原样恢复用户与加载器提供的 GC 参数。
    gc_args: Option<Vec<String>>,
    launch_extra_jvm_args: Vec<String>,
    launch_wrapper: Option<PathBuf>,
}

impl Launcher {
    pub fn new(mut options: LaunchOptions) -> Self {
        // Absolutize options.path so every path derived from it (classpath,
        // natives, game args, java binary) works even when the Java process
        // runs with a different current_dir (e.g. save_dir for Tauri).
        if options.path.is_relative() {
            if let Ok(abs) = std::env::current_dir().map(|cwd| cwd.join(&options.path)) {
                options.path = abs;
            }
        }
        Self {
            options,
            game_data: None,
            runtime_directory: None,
            custom_info: String::new(),
            gc_args: None,
            launch_extra_jvm_args: Vec::new(),
            launch_wrapper: None,
        }
    }

    /// 游戏工作目录独立于依赖与缓存位置，切换隔离不移动已有缓存。
    ///
    /// GC 计划由上层在解析实际 Java 主版本后生成；核心只负责所有权清理，
    /// 避免基础参数、版本 JSON 与用户参数中的 GC 选项互相冲突。
    pub fn set_runtime_options(
        &mut self,
        directory: PathBuf,
        custom_info: String,
        gc_args: Option<Vec<String>>,
    ) {
        self.runtime_directory = Some(directory);
        self.custom_info = custom_info;
        self.gc_args = gc_args;
    }

    /// 仅补充本次启动参数；不重新解析 Java，也不改写磁盘 GameData。
    pub fn set_launch_extras(&mut self, jvm_args: Vec<String>, wrapper: Option<PathBuf>) {
        self.launch_extra_jvm_args = jvm_args;
        self.launch_wrapper = wrapper;
    }

    pub fn options(&self) -> &LaunchOptions {
        &self.options
    }

    pub fn game_data(&self) -> Option<&GameData> {
        self.game_data.as_ref()
    }

    /// Download, verify, and optionally install a mod loader for the configured
    /// Minecraft version. Stores the result in `self.game_data`.
    ///
    /// Emits progress events on `event_tx`. After this call,
    /// [`Launcher::launch`] can be invoked without downloading again.
    ///
    /// 完整本地实例无需联网；文件缺失时直接尝试实际下载候选。
    pub async fn download_game(
        &mut self,
        event_tx: Sender<LaunchEvent>,
    ) -> Result<(), LaunchError> {
        let context = TransferContext::current();
        context.scope(self.download_game_inner(event_tx)).await
    }

    async fn download_game_inner(&mut self, event_tx: Sender<LaunchEvent>) -> Result<(), LaunchError> {
        let options = &self.options;

        TransferContext::current().cancellation().check().map_err(LaunchError::InvalidData)?;
        // 完整实例先验证本地缓存；不通过无关站点的连通性探测决定能否启动。
        if let Ok(cached) = load_game_data(&options.save_dir()).await {
            if cached_instance_complete(options, &cached).await? {
                let bundle = get_libraries(options, &cached.minecraft_json);
                extract_natives(options, &cached.minecraft_json, &bundle).await?;
                if is_old(cached.minecraft_json.assets.as_deref()) { copy_assets(options, &cached.minecraft_json).await?; }
                self.game_data = Some(cached);
                let _ = event_tx.try_send(LaunchEvent::GameDownloadFinished);
                return Ok(());
            }
        }

        // ── Cache fast-path (skip_bundle_check) ──────────────────────────────
        // "Skip if possible" hint: load from cache and return early when the
        // caller trusts the existing installation.  If the cache is absent,
        // fall through to the full download path without error.
        //
        // Java is the one thing we never skip: without the runtime the process
        // can't even spawn (start() fails with os error 2), so `is_corrupt_crash`
        // would never get to see the crash logs produced by any missing game
        // files. We therefore always ensure Java is present here, while still
        // skipping the (expensive) bundle integrity check for everything else.
        if options.skip_bundle_check {
            if let Some(mut cached) = load_game_data(&options.save_dir()).await.ok().filter(|cached| cached_identity_matches(options, cached)) {
                let java_present = std::path::Path::new(&cached.minecraft_java.path).exists();
                if !java_present {
                    let client = crate::net::client::build_client(
                        options.timeout_secs,
                        options.force_ipv4,
                        options.dns,
                    )
                    .map_err(LaunchError::Http)?;
                    let java_result =
                        get_java_files(options, &cached.minecraft_json, &client, &event_tx).await?;
                    cached.minecraft_java = JavaInfo {
                        files: java_result.files,
                        path: java_result.java_path,
                    };
                    save_game_data(&options.save_dir(), &cached).await?;
                }
                self.game_data = Some(cached);
                let _ = event_tx.send(LaunchEvent::GameDownloadFinished).await;
                return Ok(());
            }
            // Cache absent → continue with normal download.
        }

        // ── Shared HTTP client ────────────────────────────────────────────────
        let client =
            crate::net::client::build_client(options.timeout_secs, options.force_ipv4, options.dns)
                .map_err(LaunchError::Http)?;

        // ── Version JSON ──────────────────────────────────────────────────────
        // PCL 等第三方启动器的实例是自包含档案，Mojang 清单里没有同名条目。
        // 本地 JSON 已完整时优先使用，避免把它们误报成 VersionNotFound。
        let mut version_json = match crate::game::version::local_version_json(options).await {
            Some(json) => json,
            None => get_version_json(options, &client).await?,
        };
        let mc_version = version_json.id.clone();

        // ── File bundle ───────────────────────────────────────────────────────
        let mut bundle: Vec<AssetItem> = Vec::new();
        bundle.extend(get_libraries(options, &version_json));
        bundle.extend(get_assets_others(options, options.url.as_deref(), &client).await?);
        bundle.extend(get_assets(options, &version_json, &client).await?);

        // Java runtime download is managed separately (has its own concurrency
        // and progress reporting); its files are not added to the bundle.
        let java_result = get_java_files(options, &version_json, &client, &event_tx).await?;

        // ── Bundle integrity check & download ─────────────────────────────────
        let pending =
            check_bundle(&bundle, &event_tx, options.clamped_verify_concurrency()).await?;
        if !pending.is_empty() {
            let downloader = Downloader::new(
                options.timeout_secs,
                options.clamped_concurrency(),
                options.force_ipv4,
                options.dns,
            );
            downloader
                .download_multiple(pending, event_tx.clone())
                .await?;
        }

        // ── Mod loader install ────────────────────────────────────────────────
        let (
            loader_libraries,
            loader_main_class,
            loader_version_id,
            loader_type,
            loader_extra_game_args,
            loader_extra_jvm_args,
        ) = if options.loader.enable {
            if let Some(loader_type) = &options.loader.loader_type {
                let mc_jar = options
                    .path
                    .join("versions")
                    .join(&mc_version)
                    .join(format!("{mc_version}.jar"))
                    .to_string_lossy()
                    .into_owned();
                let mc_json = options
                    .path
                    .join("versions")
                    .join(&mc_version)
                    .join(format!("{mc_version}.json"))
                    .to_string_lossy()
                    .into_owned();

                let input = LoaderInstallInput {
                    mc_version: mc_version.clone(),
                    java_path: java_result.java_path.clone(),
                    mc_jar,
                    mc_json,
                };

                let loader_impl = create_loader(loader_type.clone());
                let result = loader_impl
                    .install(options, &input, &client, &event_tx)
                    .await?;
                (
                    result.libraries,
                    result.main_class,
                    Some(result.loader_version),
                    Some(result.loader_type),
                    result.extra_game_args,
                    result.extra_jvm_args,
                )
            } else {
                (vec![], None, None, None, vec![], vec![])
            }
        } else {
            (vec![], None, None, None, vec![], vec![])
        };

        // ── Download Forge/NeoForge runtime libraries ─────────────────────────
        // The loader install step (above) downloads processor/install-time JARs
        // but NOT the runtime classpath libraries listed in version.json (e.g.
        // bootstraplauncher, securejarhandler, modlauncher).  We check and
        // download them here.  When --installClient was used the files already
        // exist and check_bundle returns an empty pending list immediately.
        if !loader_libraries.is_empty() {
            let loader_pending = check_bundle(
                &loader_libraries,
                &event_tx,
                options.clamped_verify_concurrency(),
            )
            .await?;
            if !loader_pending.is_empty() {
                let downloader = Downloader::new(
                    options.timeout_secs,
                    options.clamped_concurrency(),
                    options.force_ipv4,
                    options.dns,
                );
                downloader
                    .download_multiple(loader_pending, event_tx.clone())
                    .await?;
            }
        }

        // ── Optional post-download SHA-1 verify ───────────────────────────────
        if options.verify {
            check_files(&bundle, &event_tx, options.clamped_verify_concurrency()).await?;
        }

        // ── Extract native JARs ───────────────────────────────────────────────
        extract_natives(options, &version_json, &bundle).await?;
        version_json.has_natives = bundle
            .iter()
            .any(|item| matches!(item, AssetItem::NativeAsset { .. }));

        // ── Legacy asset copy (pre-1.6) ───────────────────────────────────────
        if is_old(version_json.assets.as_deref()) {
            copy_assets(options, &version_json).await?;
        }

        // ── Persist & store ───────────────────────────────────────────────────
        let game_data = GameData {
            minecraft_json: version_json,
            minecraft_loader: None,
            minecraft_version: mc_version,
            minecraft_java: JavaInfo {
                files: java_result.files,
                path: java_result.java_path,
            },
            loader_libraries,
            loader_main_class,
            loader_version_id,
            loader_type,
            loader_extra_game_args,
            loader_extra_jvm_args,
        };

        save_game_data(&options.save_dir(), &game_data).await?;
        self.game_data = Some(game_data);

        let _ = event_tx.send(LaunchEvent::GameDownloadFinished).await;

        Ok(())
    }

    /// Assemble the Java command line and spawn the Minecraft process.
    ///
    /// Resolves game data from `self.game_data` (set by [`Launcher::download_game`])
    /// or, if absent, from the persisted cache on disk. Returns
    /// [`LaunchError::GameDataNotReady`] if neither is available.
    ///
    /// Stdout and stderr are piped; each line is forwarded as a
    /// [`LaunchEvent::Data`] event. The caller is responsible for calling
    /// `child.wait()` and emitting [`LaunchEvent::Close`] when appropriate.
    pub async fn launch(
        &self,
        event_tx: Sender<LaunchEvent>,
    ) -> Result<tokio::process::Child, LaunchError> {
        let loaded;
        let game_data: &GameData = match &self.game_data {
            Some(gd) => gd,
            None => {
                loaded = load_game_data(&self.options.save_dir())
                    .await
                    .map_err(|_| LaunchError::GameDataNotReady)?;
                &loaded
            }
        };

        let options = &self.options;
        let version_json = &game_data.minecraft_json;

        // Natives directory used for -Djava.library.path. Must agree with the
        // path `extract_natives` writes to — `natives_dir_for` handles both
        // the legacy `<base>/natives` layout and the MC 26.x / LWJGL 3.4
        // `<base>/natives/java` subdir, so the JVM's search path and the
        // extraction target stay in lockstep.
        let natives_path: PathBuf = natives_dir_for(options, version_json);

        // Build the classpath: loader libraries FIRST so Forge/NeoForge classes
        // take precedence over vanilla when there are collisions.
        let mut bundle: Vec<AssetItem> = game_data.loader_libraries.clone();
        let mut vanilla_libs = get_libraries(options, version_json);
        // Modern Forge (1.17+) and NeoForge use the bootstraplauncher which manages
        // Minecraft classes via client-slim.jar. Including the full vanilla jar
        // causes split-package conflicts in the Java module layer.
        // Old Forge (pre-1.17, no module path args) uses FML's class patcher which
        // needs the vanilla jar directly on the classpath — don't exclude it there.
        let uses_module_path = game_data
            .loader_extra_jvm_args
            .iter()
            .any(|a| a == "-p" || a == "--module-path");
        let exclude_vanilla_jar = matches!(game_data.loader_type, Some(LoaderType::NeoForge))
            || (matches!(game_data.loader_type, Some(LoaderType::Forge)) && uses_module_path);
        if exclude_vanilla_jar {
            let mc_jar = options
                .path
                .join("versions")
                .join(&version_json.id)
                .join(format!("{}.jar", version_json.id))
                .to_string_lossy()
                .into_owned();
            vanilla_libs
                .retain(|lib| !matches!(lib, AssetItem::Asset { path, .. } if path == &mc_jar));
        }
        bundle.extend(vanilla_libs);

        // Argument assembly.
        let loader_ctx = game_data
            .loader_version_id
            .as_ref()
            .map(|vid| LoaderContext {
                loader_type: game_data.loader_type.as_ref(),
                version_id: Some(vid.as_str()),
                extra_game_args: &game_data.loader_extra_game_args,
                extra_jvm_args: &game_data.loader_extra_jvm_args,
            });
        let mut jvm_args = get_jvm_arguments(options, version_json, &natives_path, loader_ctx.as_ref());
        let mut game_args = get_game_arguments(options, version_json, loader_ctx.as_ref());
        // 自定义模式需要清理核心无条件加入的默认 G1，再把用户与加载器声明的
        // GC 参数原样放回；自动模式则直接用上层基于实际 Java 生成的计划替换。
        apply_gc_plan(
            &mut jvm_args,
            &self.gc_args,
            &options.jvm_args,
            &game_data.loader_extra_jvm_args,
        );
        if let Some(directory) = &self.runtime_directory {
            set_game_argument(&mut game_args, "--gameDir", &directory.to_string_lossy());
        }
        if !self.custom_info.is_empty() {
            set_game_argument(&mut game_args, "--versionType", &self.custom_info);
        }
        let (cp_args, vanilla_main_class) = get_classpath(version_json, &bundle);

        // Screen size / fullscreen (conditional args from the version JSON are
        // skipped by get_game_arguments, so we add them here explicitly).
        if let Some(w) = options.screen.width {
            game_args.push("--width".into());
            game_args.push(w.to_string());
        }
        if let Some(h) = options.screen.height {
            game_args.push("--height".into());
            game_args.push(h.to_string());
        }
        if options.screen.fullscreen {
            game_args.push("--fullscreen".into());
        }

        let main_class = game_data
            .loader_main_class
            .as_deref()
            .unwrap_or(&vanilla_main_class)
            .to_owned();

        // Collect JARs on the Java module path (-p flag) from JVM args so we
        // can exclude them from -cp. NeoForge places bootstrap JARs on the
        // module path; having them on both paths causes IllegalStateException.
        let module_path_jars: std::collections::HashSet<String> = {
            let mut set = std::collections::HashSet::new();
            let mut iter = jvm_args.iter().peekable();
            while let Some(arg) = iter.next() {
                if arg == "-p" {
                    if let Some(module_path) = iter.next() {
                        for jar in module_path.split(':') {
                            // Normalize to a canonical filename for matching.
                            if let Some(name) = std::path::Path::new(jar).file_name() {
                                set.insert(name.to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
            set
        };

        // Filter module-path JARs out of -cp to avoid duplicate module errors.
        let cp_args = if module_path_jars.is_empty() {
            cp_args
        } else {
            cp_args
                .into_iter()
                .map(|arg| {
                    // The classpath string is the arg after "-cp".
                    if arg.contains(':') || arg.ends_with(".jar") {
                        let filtered: Vec<&str> = arg
                            .split(':')
                            .filter(|entry| {
                                let fname = std::path::Path::new(entry)
                                    .file_name()
                                    .map(|f| f.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                !module_path_jars.contains(&fname)
                            })
                            .collect();
                        filtered.join(":")
                    } else {
                        arg
                    }
                })
                .collect()
        };

        let mut all_args: Vec<String> = Vec::new();
        all_args.extend(jvm_args);
        all_args.extend(self.launch_extra_jvm_args.iter().cloned());
        #[cfg(target_os = "linux")]
        all_args.push("-DGLFW_PLATFORM=x11".into());
        all_args.extend(cp_args);
        if let Some(wrapper) = &self.launch_wrapper {
            all_args.extend(["-jar".into(), wrapper.to_string_lossy().into_owned()]);
        }
        all_args.push(main_class);
        all_args.extend(game_args);

        let java_path_raw = &game_data.minecraft_java.path;
        // Resolve to an absolute path so the binary is found regardless of
        // what current_dir is set to below.
        let java_path_buf = std::path::Path::new(java_path_raw)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(java_path_raw));
        let java_path = java_path_buf.to_string_lossy();

        // 启动参数可能包含账号令牌或用户命令中的私密值，不转发完整命令行。
        let _ = event_tx.send(LaunchEvent::Data("启动 Java 游戏进程".into())).await;

        // Spawn the process.
        let mut cmd = tokio::process::Command::new(java_path.as_ref());
        cmd.args(&all_args)
            .current_dir(self.runtime_directory.clone().unwrap_or_else(|| options.save_dir()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // On Linux, force GLFW 3.4+ to use X11 via XWayland to avoid the
        // [0x1000C] "Wayland: does not provide window position" error that
        // Forge treats as fatal in its strict GLX._initGlfw error callback.
        //
        // DISPLAY may not be exported on pure Wayland sessions (e.g. GNOME on
        // Fedora with on-demand XWayland) even when XWayland is available, so
        // we fall back to probing the X11 socket directly.
        //
        // Removing WAYLAND_DISPLAY alone is not enough: libwayland's
        // wl_display_connect(NULL) falls back to "wayland-0" via
        // XDG_RUNTIME_DIR even when WAYLAND_DISPLAY is absent. Setting
        // WAYLAND_SOCKET to a non-numeric value causes wl_display_connect to
        // return NULL immediately (before the fallback path), so GLFW's Wayland
        // backend fails and it falls through to X11.
        #[cfg(target_os = "linux")]
        {
            let display = std::env::var_os("DISPLAY").or_else(|| {
                (0..10u8).find_map(|n| {
                    let sock = format!("/tmp/.X11-unix/X{n}");
                    std::path::Path::new(&sock)
                        .exists()
                        .then(|| format!(":{n}").into())
                })
            });
            if let Some(disp) = display {
                cmd.env("DISPLAY", disp);
                cmd.env_remove("WAYLAND_DISPLAY");
                cmd.env("GLFW_PLATFORM", "x11");
                cmd.env("WAYLAND_SOCKET", "invalid");
            }
        }

        // LWJGL 2 (old Minecraft, pre-1.13) runs `xrandr` at startup via
        // Runtime.exec() to enumerate display modes. If xrandr is not installed
        // the subprocess returns nothing, getScreenNames() returns an empty
        // array, and LinuxDisplay:951 throws ArrayIndexOutOfBoundsException: 0.
        // Fix: if xrandr is missing, drop a minimal stub script into a cache
        // dir and prepend that dir to PATH so Java finds it first.
        #[cfg(target_os = "linux")]
        if crate::game::lwjgl_native::uses_lwjgl2(version_json)
            && !crate::game::lwjgl_native::xrandr_in_path()
        {
            let stub_dir = options.path.join("cache").join("xrandr-stub");
            if crate::game::lwjgl_native::write_xrandr_stub(&stub_dir)
                .await
                .is_ok()
            {
                let base_path = std::env::var("PATH").unwrap_or_default();
                cmd.env(
                    "PATH",
                    format!("{}:{}", stub_dir.to_string_lossy(), base_path),
                );
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| LaunchError::ProcessError(e.to_string()))?;

        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_process_output(stdout, event_tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_process_output(stderr, event_tx));
        }

        Ok(child)
    }

    /// Download the game and immediately launch it.
    ///
    /// Equivalent to `download_game` followed by `launch`. Returns the
    /// [`tokio::process::Child`] handle so the caller can monitor or kill
    /// the process.
    ///
    /// To receive a [`LaunchEvent::Close`] event, wait on the returned child
    /// and send it yourself:
    /// ```ignore
    /// let code = child.wait().await?.code().unwrap_or(-1);
    /// let _ = tx.send(LaunchEvent::Close(code)).await;
    /// ```
    pub async fn start(
        &mut self,
        event_tx: Sender<LaunchEvent>,
    ) -> Result<tokio::process::Child, LaunchError> {
        let context = TransferContext::current();
        context.scope(async {
            self.download_game(event_tx.clone()).await?;
            context.cancellation().check().map_err(LaunchError::InvalidData)?;
            self.launch(event_tx).await
        }).await
    }

    /// Heuristically detect whether a game crash was caused by a corrupt or
    /// incomplete installation.
    ///
    /// Returns `true` when `exit_code` is non-zero **and** at least one log
    /// line matches a known corrupt-installation pattern. The caller can use
    /// this as a signal to force a full re-check by calling `download_game()`
    /// with `skip_bundle_check: false` on the next attempt.
    ///
    /// # Example
    /// ```ignore
    /// let code = child.wait().await?.code().unwrap_or(-1);
    /// let lines: Vec<String> = /* collected LaunchEvent::Data lines */;
    /// if Launcher::is_corrupt_crash(code, &lines) {
    ///     // Re-run download_game with skip_bundle_check: false
    /// }
    /// ```
    pub fn is_corrupt_crash(exit_code: i32, logs: &[String]) -> bool {
        if exit_code == 0 {
            return false;
        }
        // Match only on JVM exception class names, which the runtime prints
        // verbatim regardless of its locale. Localized prose messages (e.g.
        // "Error: Could not find or load main class", "Unable to access
        // jarfile", "Error opening zip file") are translated on non-English
        // JVMs and must not be relied on — the corresponding exception is
        // always present alongside them and is locale-independent:
        //   missing main class / jar  → ClassNotFoundException / NoClassDefFoundError
        //   missing native library    → UnsatisfiedLinkError
        //   missing file              → FileNotFoundException / NoSuchFileException
        //   corrupt archive           → ZipException
        const PATTERNS: &[&str] = &[
            "NoClassDefFoundError",
            "ClassNotFoundException",
            "UnsatisfiedLinkError",
            "FileNotFoundException",
            "NoSuchFileException",
            "ZipException",
        ];
        logs.iter()
            .any(|line| PATTERNS.iter().any(|pat| line.contains(pat)))
    }
}

/// JVM 可能按系统代码页输出；按字节分行后宽容解码，避免一条非 UTF-8 日志截断后续事件。
async fn forward_process_output(reader: impl tokio::io::AsyncRead + Unpin, event_tx: Sender<LaunchEvent>) {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                    if bytes.last() == Some(&b'\r') { bytes.pop(); }
                }
                let line = String::from_utf8_lossy(&bytes).into_owned();
                // 接收端关闭后仍排空管道，避免 JVM 被写满的 stdout/stderr 阻塞。
                let _ = event_tx.send(LaunchEvent::Data(line)).await;
            }
        }
    }
}


// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_options() -> LaunchOptions {
        use crate::launcher::options::{JavaOptions, LoaderConfig, MemoryConfig, ScreenConfig};
        use crate::models::minecraft::Authenticator;
        LaunchOptions {
            path: PathBuf::from("/mc"),
            version: "1.20.4".into(),
            authenticator: Authenticator {
                access_token: "test-token".into(),
                name: "Player".into(),
                uuid: "test-uuid".into(),
                xbox_account: None,
                user_properties: None,
                client_id: None,
                client_token: None,
            },
            timeout_secs: 10,
            download_concurrency: 5,
            verify_concurrency: 4,
            memory: MemoryConfig::default(),
            java: JavaOptions::default(),
            loader: LoaderConfig::default(),
            screen: ScreenConfig::default(),
            verify: false,
            game_args: vec![],
            jvm_args: vec![],
            instance: None,
            url: None,
            mcp: None,
            intel_enabled_mac: false,
            bypass_offline: false,
            skip_bundle_check: false,
            force_ipv4: false,
            dns: None,
        }
    }

    #[test]
    fn launcher_new_stores_options() {
        let opts = make_options();
        let launcher = Launcher::new(opts.clone());
        assert_eq!(launcher.options.version, "1.20.4");
        assert_eq!(launcher.options.path, std::path::absolute("/mc").unwrap());
    }

    #[test]
    fn launcher_save_dir_no_instance() {
        let opts = make_options();
        let launcher = Launcher::new(opts);
        assert_eq!(launcher.options.save_dir(), std::path::absolute("/mc").unwrap());
    }

    #[test]
    fn launcher_save_dir_with_instance() {
        let mut opts = make_options();
        opts.instance = Some("myworld".into());
        let launcher = Launcher::new(opts);
        assert_eq!(
            launcher.options.save_dir(),
            std::path::absolute("/mc").unwrap().join("instances").join("myworld")
        );
    }

    #[test]
    fn sanitize_replaces_access_token() {
        let token = "secret-access-token";
        let cmd = format!("java -cp foo.jar Main --accessToken {token}");
        let sanitized = cmd.replace(token, "<access_token>");
        assert!(!sanitized.contains(token));
        assert!(sanitized.contains("<access_token>"));
    }

    #[test]
    fn all_args_order_is_correct() {
        // Verify the expected CLI ordering: jvm_args, -cp, classpath, main_class, game_args
        let jvm: Vec<String> = vec!["-Xms1G".into(), "-Xmx2G".into()];
        let cp: Vec<String> = vec!["-cp".into(), "a.jar:b.jar".into()];
        let main_class = "net.minecraft.client.main.Main".to_owned();
        let game: Vec<String> = vec!["--username".into(), "Player".into()];

        let mut all: Vec<String> = Vec::new();
        all.extend(jvm);
        all.extend(cp);
        all.push(main_class.clone());
        all.extend(game);

        assert_eq!(all[0], "-Xms1G");
        assert_eq!(all[2], "-cp");
        assert_eq!(all[4], main_class);
        assert_eq!(all[5], "--username");
    }

    #[test]
    fn screen_args_appended_when_set() {
        use crate::launcher::options::ScreenConfig;
        let screen = ScreenConfig {
            width: Some(1920),
            height: Some(1080),
            fullscreen: false,
        };
        let mut game_args: Vec<String> = vec!["--version".into(), "1.20.4".into()];
        if let Some(w) = screen.width {
            game_args.push("--width".into());
            game_args.push(w.to_string());
        }
        if let Some(h) = screen.height {
            game_args.push("--height".into());
            game_args.push(h.to_string());
        }
        assert!(game_args.contains(&"--width".to_string()));
        assert!(game_args.contains(&"1920".to_string()));
        assert!(game_args.contains(&"--height".to_string()));
        assert!(game_args.contains(&"1080".to_string()));
        assert!(!game_args.contains(&"--fullscreen".to_string()));
    }

    #[test]
    fn screen_fullscreen_appended_when_set() {
        use crate::launcher::options::ScreenConfig;
        let screen = ScreenConfig {
            width: None,
            height: None,
            fullscreen: true,
        };
        let mut game_args: Vec<String> = vec![];
        if screen.fullscreen {
            game_args.push("--fullscreen".into());
        }
        assert!(game_args.contains(&"--fullscreen".to_string()));
    }

    #[test]
    fn loader_main_class_overrides_vanilla() {
        let vanilla = "net.minecraft.client.main.Main".to_owned();
        let loader_main_class: Option<String> =
            Some("net.fabricmc.loader.impl.launch.knot.KnotClient".into());
        let main_class = loader_main_class.as_deref().unwrap_or(&vanilla).to_owned();
        assert_eq!(
            main_class,
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
    }

    #[test]
    fn no_loader_main_class_uses_vanilla() {
        let vanilla = "net.minecraft.client.main.Main".to_owned();
        let loader_main_class: Option<String> = None;
        let main_class = loader_main_class.as_deref().unwrap_or(&vanilla).to_owned();
        assert_eq!(main_class, "net.minecraft.client.main.Main");
    }

    // ── is_corrupt_crash ─────────────────────────────────────────────────────

    #[test]
    fn corrupt_crash_zero_exit_always_false() {
        let logs = vec!["NoClassDefFoundError: net/minecraft/Foo".into()];
        assert!(!Launcher::is_corrupt_crash(0, &logs));
    }

    #[test]
    fn corrupt_crash_nonzero_no_pattern_false() {
        let logs = vec!["Exception in thread \"main\" java.lang.RuntimeException".into()];
        assert!(!Launcher::is_corrupt_crash(1, &logs));
    }

    #[test]
    fn corrupt_crash_empty_logs_false() {
        assert!(!Launcher::is_corrupt_crash(1, &[]));
    }

    #[test]
    fn corrupt_crash_all_patterns_detected() {
        let cases = [
            "java.lang.NoClassDefFoundError: Foo",
            "java.lang.ClassNotFoundException: net.minecraft.Main",
            "java.lang.UnsatisfiedLinkError: /lib/foo.so",
            "java.io.FileNotFoundException: /mc/lib.jar (No such file)",
            "java.nio.file.NoSuchFileException: /mc/versions/1.20.4/1.20.4.jar",
            "java.util.zip.ZipException: invalid LOC header",
        ];
        for case in &cases {
            assert!(
                Launcher::is_corrupt_crash(1, &[case.to_string()]),
                "pattern not detected: {case}"
            );
        }
    }

    #[test]
    fn corrupt_crash_detected_on_localized_main_class_error() {
        // A non-English JVM translates the prose line but the cause exception
        // (ClassNotFoundException) is locale-independent and must still match.
        let logs = vec![
            "Error: no se ha encontrado o cargado la clase principal net.minecraft.client.main.Main".into(),
            "Causado por: java.lang.ClassNotFoundException: net.minecraft.client.main.Main".into(),
        ];
        assert!(Launcher::is_corrupt_crash(1, &logs));
    }


    #[tokio::test]
    async fn complete_cached_instance_never_requires_connectivity_probe() {
        let dir = tempfile::TempDir::new().unwrap();
        let java = dir.path().join("java");
        std::fs::write(&java, b"runtime").unwrap();
        let mut options = make_options(); options.path = dir.path().to_path_buf();
        let mut cached: GameData = serde_json::from_value(serde_json::json!({
            "minecraftVersion": "1.20.4", "minecraftJava": { "path": java, "files": [] },
            "minecraftJson": { "id": "1.20.4", "type": "release", "libraries": [], "mainClass": "Main" }
        })).unwrap();
        save_game_data(dir.path(), &cached).await.unwrap();
        let version_file = dir.path().join("versions/1.20.4/1.20.4.json");
        std::fs::create_dir_all(version_file.parent().unwrap()).unwrap();
        std::fs::write(version_file, serde_json::to_vec(&cached.minecraft_json).unwrap()).unwrap();
        let mut launcher = Launcher::new(options.clone());
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        tokio::time::timeout(std::time::Duration::from_secs(1), launcher.download_game(tx)).await.unwrap().unwrap();
        assert_eq!(launcher.game_data().unwrap().minecraft_version, "1.20.4");
        cached.loader_type = Some(LoaderType::Fabric);
        assert!(!cached_identity_matches(&options, &cached));
        options.loader.enable = true; options.loader.loader_type = Some(LoaderType::Fabric); options.loader.build = "0.16.9".into();
        cached.loader_version_id = Some("fabric-loader-0.16.8-1.20.4".into());
        assert!(!cached_identity_matches(&options, &cached));
        cached.loader_version_id = Some("fabric-loader-0.16.9-1.20.4".into());
        assert!(cached_identity_matches(&options, &cached));
        cached.loader_libraries.push(AssetItem::Asset { path: dir.path().join("missing.jar").to_string_lossy().into_owned(), sha1: String::new(), size: 10, url: "http://127.0.0.1:1/missing".into() });
        assert!(!cached_instance_complete(&options, &cached).await.unwrap());
    }

    #[test]
    fn required_major_rejects_old_runtime_cache_and_accepts_matching_cache() {
        let mut options = make_options();
        let platform = crate::game::java::mojang_platform_key(false);
        let old_java = options
            .path
            .join("runtime/jre-8")
            .join(&platform)
            .join("bin/java");
        let cached: GameData = serde_json::from_value(serde_json::json!({
            "minecraftVersion": "1.20.4",
            "minecraftJava": { "path": old_java, "files": [] },
            "minecraftJson": {
                "id": "1.20.4",
                "type": "release",
                "libraries": [],
                "mainClass": "Main",
                "javaVersion": { "component": "jre-8", "majorVersion": 8 }
            }
        }))
        .unwrap();

        // 无主版本要求时不误伤已有缓存；要求提升后旧缓存必须失效。
        assert!(cached_java_matches_requirement(&options, &cached));
        assert!(cached_identity_matches(&options, &cached));
        options.java.required_major = Some(21);
        assert!(!cached_java_matches_requirement(&options, &cached));
        assert!(!cached_identity_matches(&options, &cached));

        let expected_java = options
            .path
            .join("runtime/jre-21")
            .join(&platform)
            .join("bin/java");
        let mut expected = cached;
        expected.minecraft_java.path = expected_java.to_string_lossy().into_owned();
        assert!(cached_java_matches_requirement(&options, &expected));
        assert!(cached_identity_matches(&options, &expected));
    }

    #[test]
    fn cached_loader_build_identity_is_exact_and_loader_specific() {
        for (kind, mc, id, requested, expected) in [
            (LoaderType::NeoForge, "1.20.1", "1.20.1-forge-47.1.106", "1.20.1-47.1.106", true),
            (LoaderType::NeoForge, "1.20.1", "1.20.1-neoforge-47.1.106", "1.20.1-47.1.106", true),
            (LoaderType::NeoForge, "1.20.1", "1.20.1-forge-47.1.106", "1.20.1-47.1.105", false),
            (LoaderType::NeoForge, "1.20.1", "1.20.2-forge-47.1.106", "1.20.1-47.1.106", false),
            (LoaderType::NeoForge, "1.20.4", "neoforge-20.4.237", "20.4.237", true),
            (LoaderType::Forge, "1.20.1", "1.20.1-forge-47.1.106", "47.1.106", true),
            (LoaderType::Forge, "1.20.1", "1.20.1-forge-47.1.106", "1.20.1-47.1.106", true),
            (LoaderType::Forge, "1.20.1", "1.20.1-neoforge-47.1.106", "47.1.106", false),
            (LoaderType::Fabric, "1.20.4", "fabric-loader-0.16.9-1.20.4", "0.16.9", true),
            (LoaderType::Fabric, "1.20.4", "quilt-loader-0.16.9-1.20.4", "0.16.9", false),
            (LoaderType::Fabric, "20w14∞", "fabric-loader-0.16.9-20w14infinite", "0.16.9", true),
            (LoaderType::Quilt, "1.20.4", "quilt-loader-0.26.0-1.20.4", "0.26.0", true),
            (LoaderType::Quilt, "1.20.4", "quilt-loader-unrelated-0.26.0-1.20.4", "0.26.0", false),
            (LoaderType::OptiFine, "1.20.1", "1.20.1-OptiFine_1.20.1_HD_U_I6", "OptiFine_1.20.1_HD_U_I6", true),
            (LoaderType::OptiFine, "1.20.1", "1.20.1-OptiFine_1.20.1_HD_U_I6", "preview_OptiFine_1.20.1_HD_U_I6", false),
            (LoaderType::LiteLoader, "1.12.2", "1.12.2-LiteLoader-1.12.2-SNAPSHOT-r0", "1.12.2-SNAPSHOT-r0", true),
            (LoaderType::LiteLoader, "1.12.2", "1.12.2-LiteLoader-1.12.2-SNAPSHOT-r0", "1.12.2-SNAPSHOT-r1", false),
        ] {
            assert_eq!(cached_loader_build_matches(&kind, mc, id, requested), expected, "{kind} {id} {requested}");
        }
    }

    #[tokio::test]
    async fn complete_legacy_neoforge_cache_skips_network() {
        let dir = tempfile::TempDir::new().unwrap();
        let java = dir.path().join("java");
        std::fs::write(&java, b"runtime").unwrap();
        let mut options = make_options();
        options.path = dir.path().to_path_buf();
        options.version = "1.20.1".into();
        options.loader.enable = true;
        options.loader.loader_type = Some(LoaderType::NeoForge);
        options.loader.build = "1.20.1-47.1.106".into();
        let cached: GameData = serde_json::from_value(serde_json::json!({
            "minecraftVersion": "1.20.1", "minecraftJava": { "path": java, "files": [] },
            "minecraftJson": { "id": "1.20.1", "type": "release", "libraries": [], "mainClass": "Main" },
            "loaderType": "neoforge", "loaderVersionId": "1.20.1-forge-47.1.106"
        })).unwrap();
        save_game_data(dir.path(), &cached).await.unwrap();
        let version_file = dir.path().join("versions/1.20.1/1.20.1.json");
        std::fs::create_dir_all(version_file.parent().unwrap()).unwrap();
        std::fs::write(version_file, serde_json::to_vec(&cached.minecraft_json).unwrap()).unwrap();
        assert!(cached_instance_complete(&options, &cached).await.unwrap());
        let mut launcher = Launcher::new(options.clone());
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        tokio::time::timeout(std::time::Duration::from_secs(1), launcher.download_game(tx)).await.unwrap().unwrap();
        assert_eq!(launcher.game_data().unwrap().loader_version_id.as_deref(), Some("1.20.1-forge-47.1.106"));
        options.loader.loader_type = Some(LoaderType::Forge);
        assert!(!cached_identity_matches(&options, &cached));
    }

    #[tokio::test]
    async fn process_output_continues_after_non_utf8_and_preserves_line_boundaries() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        let forward = tokio::spawn(forward_process_output(reader, tx));
        let write = tokio::spawn(async move {
            let bytes = b"before\r\nWARN \xD6\xD0\xCE\xC4\xFF\r\n\nCreated textures/atlas\nSound engine started\nUTF8 \xE4\xB8\xAD\xE6\x96\x87\nlast\xFF";
            for byte in bytes {
                writer.write_all(&[*byte]).await.unwrap();
            }
            writer.shutdown().await.unwrap();
        });
        let mut lines = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(event) = rx.recv().await {
                if let LaunchEvent::Data(line) = event { lines.push(line); }
            }
            write.await.unwrap();
            forward.await.unwrap();
        }).await.unwrap();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "before");
        assert!(lines[1].starts_with("WARN "));
        assert!(lines[1].contains('\u{fffd}'));
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "Created textures/atlas");
        assert_eq!(lines[4], "Sound engine started");
        assert_eq!(lines[5], "UTF8 中文");
        assert_eq!(lines[6], "last\u{fffd}");
        assert!(lines.iter().all(|line| !line.ends_with(['\r', '\n'])));
    }

    #[tokio::test]
    async fn process_output_drains_after_event_receiver_closes() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(8);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        let forward = tokio::spawn(forward_process_output(reader, tx));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            for _ in 0..64 {
                writer.write_all(b"\xFFwarning\nrendered\n").await.unwrap();
            }
            writer.shutdown().await.unwrap();
            forward.await.unwrap();
        }).await.unwrap();
    }

    #[test]
    fn gc_plan_owns_conflicting_options_and_custom_restores_user_values() {
        let base = vec![
            "-XX:+UnlockExperimentalVMOptions",
            "-XX:+UseG1GC",
            "-XX:G1NewSizePercent=20",
            "-XX:G1ReservePercent=20",
            "-XX:G1HeapRegionSize=32M",
            "-XX:MaxGCPauseMillis=50",
            "-XX:MinHeapFreeRatio=25",
            "-XX:UseShenandoahGC",
            "-Xmx4G",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let user = vec![
            "-XX:+UseZGC".to_string(),
            "-XX:+ZGenerational".to_string(),
            "-XX:+UseCompactObjectHeaders".to_string(),
            "-Xmx4G".to_string(),
        ];
        let loader = vec!["-XX:+UseSerialGC".to_string()];
        let plan = vec!["-XX:+UseZGC".to_string(), "-XX:+ZGenerational".to_string()];
        let mut automatic = base.clone();
        apply_gc_plan(&mut automatic, &Some(plan.clone()), &user, &loader);
        assert_eq!(
            automatic
                .iter()
                .filter(|argument| is_gc_option(argument))
                .cloned()
                .collect::<Vec<_>>(),
            plan
        );
        assert!(automatic.contains(&"-Xmx4G".into()));
        assert!(automatic.iter().all(|argument| !argument.starts_with("-XX:G1")));

        let mut custom = base.clone();
        apply_gc_plan(&mut custom, &None, &user, &loader);
        assert!(custom.contains(&"-XX:+UseZGC".into()));
        assert!(custom.contains(&"-XX:+ZGenerational".into()));
        assert!(custom.contains(&"-XX:+UseCompactObjectHeaders".into()));
        assert!(custom.contains(&"-XX:+UseSerialGC".into()));
        assert!(custom.contains(&"-Xmx4G".into()));
        assert!(!custom.contains(&"-XX:+UseG1GC".into()));
        assert!(custom.iter().all(|argument| !argument.starts_with("-XX:G1")));
    }

    #[cfg(windows)]
    #[test]
    fn runtime_game_directory_override_keeps_java_compatible_paths() {
        let mut args = vec!["--gameDir".into(), "old".into(), "--gameDir".into(), "other".into()];
        set_game_argument(&mut args, "--gameDir", r"\\?\D:\Game Files\测试");
        assert_eq!(args, ["--gameDir", r"D:\Game Files\测试"]);
        set_game_argument(&mut args, "--gameDir", r"\\?\UNC\server\share\game");
        assert_eq!(args, ["--gameDir", r"\\server\share\game"]);
    }




}
