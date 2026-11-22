# 开发与构建

## 环境

Node.js 22+、pnpm 10+、Rust stable，以及 [Tauri v2 对应平台依赖](https://v2.tauri.app/start/prerequisites/)。前端使用 React 19 / TypeScript / Vite，桌面与业务后端使用 Tauri v2 / Rust。

```sh
git clone https://github.com/Nickel-Studio/Ni-Launcher.git
cd Ni-Launcher
pnpm install --frozen-lockfile
pnpm tauri dev
```

## 常用命令

```sh
pnpm dev
pnpm build
pnpm tauri build
cargo check --locked --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

`pnpm dev` 提供前端开发服务器，默认端口 1420。`pnpm tauri dev` 同时运行桌面后端。`pnpm tauri build` 打包当前平台发行文件。

应用名称为 `Nickel`，Rust 包与二进制名为 `ni-launcher`，库名为 `ni_launcher_lib`，应用标识为 `studio.nickel.launcher`。品牌图标源文件为 `src/assets/brand/Nickel.png`。

## API 配置与构建注入

本地开发可以不配置 API，但正式发布必须同时具备可用的 CurseForge Key 和 Microsoft 公共客户端配置，不能用缺少功能的构建替代。`secrets.local.json` 是私有输入，不提交、不随归档分发，也不输出到构建日志。

### Microsoft

在 Azure / Entra 注册支持个人 Microsoft 账号的公共客户端，启用公共客户端流，将 Application (client) ID 填入 `microsoft.clientId`。当前设备码链路固定使用 `consumers` 端点与 `XboxLive.signin offline_access` scope，随后完成 Xbox Live、XSTS 和 Minecraft 档案验证；不使用 client secret。`clientId` 是公共 OAuth 元数据，不是账号凭据。

仅当本地配置尚不存在时复制示例，再在本机编辑：

```powershell
if (-not (Test-Path -LiteralPath secrets.local.json)) {
    Copy-Item secrets.local.example.json secrets.local.json
}
```

运行时读取顺序为 `NICKEL_SECRETS` 指定文件、选定配置目录、程序目录，最后回退到内嵌配置；仅调试构建会在内嵌配置前尝试开发根目录。AppImage 的程序目录按经过验证的外部 AppImage 定位，不使用临时挂载目录。

### CurseForge

在 [CurseForge Console](https://console.curseforge.com) 为启动器取得已授权的 API Key，填入同一文件的 `curseforge.apiKey`。Key 只由后端构造精确官方 API 的 `x-api-key` 请求头，不注册为前端 IPC，不发送给镜像；API 请求不跟随重定向，文件 CDN 下载仍使用独立客户端。

### 注入边界

构建只允许四个字段：`microsoft.clientId`、`microsoft.authority`、`microsoft.redirectUri`、`curseforge.apiKey`。账号 access/refresh token、密码、client secret、tenant/object ID 等不进入构建投影。

默认输入为项目根的 `secrets.local.json`；`NICKEL_BUILD_SECRETS` 可指定另一个已授权文件。非空的 `NICKEL_MSA_CLIENT_ID`、`NICKEL_MSA_AUTHORITY`、`NICKEL_MSA_REDIRECT_URI` 和 `NICKEL_CURSEFORGE_API_KEY` 优先覆盖对应字段。显式输入不可读或格式错误时构建失败，不输出原文。

输出为每次构建随机化的 AES-256-GCM 四字段封装 `nickel-embedded-config.bin`，不再嵌入明文 JSON。CF 句柄仅保存加密字节，构造请求头时短暂解封；受控临时缓冲区和支持的密码内部状态在释放时清除，build/runtime 使用同一特性并有 `ZeroizeOnDrop` 编译期约束。

这能提高直接 `strings` 扫描和静态提取的成本，但可执行文件仍包含解码材料，无法阻止有本地控制权的逆向、调试、hook 或内存提取。清除不保证覆盖寄存器、操作系统副本或 HTTP 库的头部存储。不要把混淆或 CSP 当成不可提取保证；真正不随客户端分发的 CF Key 需要自有后端代理，并另行评估服务运营和 API 授权约束。

## 构建通道

`NICKEL_CHANNEL` 在编译时选择 `development` / `production`。普通开发与测试默认 `development`；打包工作流指定 `production`。开发通道未配置 CurseForge Key 时使用社区镜像；生产通道向官方 API 发请求并按真实响应处理错误，不用测试值冒充可用凭据。

正式发布启用配置门禁，并先请求官方服务验收：

```powershell
$env:NICKEL_CHANNEL = "production"
$env:NICKEL_REQUIRE_RELEASE_SECRETS = "1"
node scripts/release-secrets.mjs --remote
if ($LASTEXITCODE -ne 0) { throw "Official release configuration preflight failed" }
pnpm tauri build -- --locked
```

门禁拒绝缺失字段、全零 Microsoft client ID 和 `NICKEL_TEST_CURSEFORGE_KEY`。本地排障如显式关闭 `NICKEL_REQUIRE_RELEASE_SECRETS`，只能获得未通过发布验收的构建；不能据此宣称正式 CF 可用。官方设备码预检只证明公共客户端被接受，不证明完整账号登录或游戏所有权。

Windows 窗口排障可设置 `NICKEL_DIAG=1`：

```powershell
$env:NICKEL_DIAG = "1"
pnpm tauri dev
```

## CSP 与本地背景媒体

生产 CSP 保留必要的 Tauri IPC；脚本不允许 `unsafe-inline` / `unsafe-eval`，浏览器连接仅允许 IPC，禁用 frame/object/form 等入口。它用于减少脚本注入和前端外传风险，不用于隐藏已经随客户端分发的 Key。内联样式仍是现有界面必需；HTTP/HTTPS 远程图片仍可用，不代表允许浏览器 `fetch` 这些地址。开发 CSP 单独允许 HMR 和 WebSocket。

通用 `asset:` 文件协议及全盘 `**` scope 已关闭。本地图片使用 `nickel-image` 协议，共用绝对路径、敏感文件名、文件类型和 32 MiB 限制检查，只接受 PNG/JPEG/GIF/BMP/WebP/AVIF 等栅格头部，不支持 SVG 或最终文件符号链接；这不是完整图片解码验证。`image_data_url` IPC 走同一套校验，只在 `<img>` 实际加载失败后作为一次性回退，不做预先探测，避免每张背景被完整读取两次。

本地视频（MP4/M4V/WebM/OGV）使用 `nickel-video` 协议：扩展名与容器头一致、512 MiB 文件上限、每次最多返回 4 MiB，支持 HEAD、206 和 416。不带 `Range` 的普通 GET 在文件超过 4 MiB 时同样返回 206 和 `Content-Range`，这是有意偏离 RFC 9110，保证任何请求都不会把整个视频读进 IPC 内存，不要“修正”成返回完整文件。`MAX_VIDEO_BYTES` 不得超过便携包导入的 `ArchiveLimits::default().max_entry_bytes`，单测锁定了这一约束。

图片和视频的文件头都会读满（读到所需长度或 EOF），避免过滤驱动的短读造成误拒。Windows 上只接受本地磁盘：盘符路径和 `\\?\` 盘符路径，且盘符不能是映射网络盘（`GetDriveTypeW` 返回 `DRIVE_REMOTE`）；UNC 共享（`\\server\share\...`、`\\?\UNC\...`、WebDAV 形式的 `\\host@80\...`）和 `\\.\` 等设备命名空间一律拒绝。这些检查是纯路径判断，在任何文件系统访问之前完成。原因是打开网络路径时 Windows 会自动用当前用户凭据做 NTLM 认证，而 WebView 中任何 `nickel-image:` / `nickel-video:` 地址（包括社区资源图标、他人分享实例中的图标路径）都可能指向攻击者的主机，造成凭据哈希泄露；不要为了支持 NAS 壁纸放开这条限制。网络路径被拒时返回“请先复制到本地磁盘”的明确提示，其余情况按媒体类型返回图片或视频各自的提示文案。响应设置 `nosniff` 和 `no-store`，不得为了让测试通过恢复任意文件读取或放宽脚本 CSP。

浏览器预览模式（非 Tauri）把背景读成 data URL 并写入 `localStorage`，因此上限为 3 MiB，避免超出配额后被静默丢弃。

## 测试

`cargo test --locked` 运行单元测试。依赖网络、现有 Java、系统 UI 或外部实例的用例使用 `#[ignore]`，按测试要求单独运行。

```powershell
pnpm build
node --test tests/window-corners/overlays.test.mjs
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib --test build_config --test dependency_notices
node --test scripts/release-secrets.test.mjs scripts/audit-release.test.mjs tests/release-hardening/security.test.mjs
python -m unittest discover -s scripts -p 'test_*.py' -v
python -m unittest discover -s tests/startup-acceptance -p test_native_safety.py -v
python -m unittest discover -s tests/release-hardening -p 'test_*.py' -v
node tests/embedded-config/verify.mjs --fixture
node tests/embedded-config/verify.mjs --placeholder
node tests/embedded-config/verify.mjs
```

窗口是透明无边框的，四个圆角只由 `.ni-shell` 的 CSS 画出。挂在 `body` 下、`position: fixed` 且铺满整窗的浮层不受它裁剪，必须自己声明 `border-radius: var(--shell-radius)`，`tests/window-corners/overlays.test.mjs` 会扫描 `src/**/*.css` 和 `index.html` 检查这一点。会贴近窗口边缘的小浮层（如 Tooltip）要在定位时避开四角。

Detached Release 验证器复制实际 build/runtime 密码特性，并验证白名单、随机化、UTF-8/UTF-16LE 明文扫描、门禁及移除临时源码后的运行。合成凭据情景不证明实际 Tauri 包或官方登录成功。`audit-release.mjs` 审计真实二进制与当前构建封装；严格模式同时拒绝历史 `nickel-embedded-secrets.json` 明文输出，不擅自删除已有文件。`--allow-incomplete-local` 只用于局部检查，`releaseReadyForPublication` 始终为 false。

原生包验收入口为 `tests/release-hardening/native_security.py`（Windows portable、CSP/图片/背景，可选 `--devicecode`）和 `tests/release-hardening/appimage_startup.py`（Linux portable，使用 `xvfb-run -a dbus-run-session`）。先运行 `--help`，提供重新计算的实际归档 SHA-256 和全新的证据路径。设备码测试不记录代码或完成账号授权；Linux 启动检查不等同 Linux CSP UI 验收。任何重新构建或重新打包后的产物，都必须重新绑定哈希，不沿用旧原生报告。

在 `pnpm dev` 后打开以下回归页：

- `/tests/loader-picker/regression.html`：提供方清单、兼容组合、重试、旧响应隔离。
- `/tests/ui-foundation/regression.html`：基础组件、键盘焦点、页面切换与减少动态效果。
- `/tests/version-instances/regression.html`：实例选择、收藏、删除与指南入口。
- `/tests/ui-remaining/regression.html`：社区检索、详情与下载任务。
- `/tests/mods-scan/regression.html`：加载、扫描、失败恢复与旧实例隔离。
- `/tests/community-version/regression.html`：真实游戏版本的筛选与安装上下文。
- `/tests/loading-animation/regression.html`：加载布局与错误态动画。
- `/tests/settings-regression/regression.html`：设置与内存轮询。
- `/tests/brand/regression.html`：Nickel 名称、关于页、本地徽标与外链。

浏览器测试使用本机已有 Chrome、Edge 或已缓存浏览器。测试输出写入本地临时目录。

## CI 与发行

[CI](../.github/workflows/ci.yml) 在 `main` 推送和 PR 上运行前端构建、production 通道 Rust 测试及发布安全/portable 门禁的合成回归。[Build](../.github/workflows/build.yml) 支持手动打包与 `v*` 标签，覆盖 Windows x64、macOS arm64 / x64 和 Linux x64；四目标全部通过收集门禁后才向 Release 草稿上传产物。

GitHub Secrets 必须配置 `NICKEL_MSA_CLIENT_ID` 和 `NICKEL_CURSEFORGE_API_KEY`；可选 Variables 为 `NICKEL_MSA_AUTHORITY`、`NICKEL_MSA_REDIRECT_URI`。构建使用独立 `src-tauri/target/release-build`，禁止缓存含可恢复配置的 target 输出。只上传经过 `prepare_release.py` 审计的目录，不上传构建缓存、秘密源文件或运行数据。

发布前保持 `package.json`、`src-tauri/Cargo.toml`、根包的 `Cargo.lock` 条目与 `tauri.conf.json` 版本一致，标签使用 `v<版本>`。构建固定使用锁文件，不在发行时更新依赖。

MSI 只接受 `主.次.修订[.构建]` 的纯数字版本，带 `-beta` 等预发布标识时必须在 `tauri.conf.json` 的 `bundle.windows.wix.version` 单独指定（1.0.3-beta 用 `1.0.3.1`）；回到纯数字版本时删除该字段，MSI 版本随 `version` 推导。

### 每目标必须包含 portable

| 目标        | 安装包                 | Portable                                                   |
| ----------- | ---------------------- | ---------------------------------------------------------- |
| Windows x64 | MSI、NSIS              | `Nickel_<版本>_windows-x64_portable.zip`                   |
| macOS arm64 | DMG                    | 完整 `.app` 的 `Nickel_<版本>_macos-arm64_portable.tar.gz` |
| macOS x64   | DMG                    | 完整 `.app` 的 `Nickel_<版本>_macos-x64_portable.tar.gz`   |
| Linux x64   | DEB、优化后的 AppImage | `Nickel_<版本>_linux-x64_portable.tar.gz`                  |

必须完整解压到可写目录，保留启动器旁的 `nickel.launcher-portable.json`；`Nickel/` 存设置和加密账号，`.minecraft/` 存默认游戏数据。macOS 数据位于完整 `.app` 旁，不修改签名包内部。此标记与整合包的 `nickel.portable.json` 独立，不能触发整合包导入；发布归档不包含用户账号、凭据密钥文件、配置原文、Java 或游戏数据，但可包含已授权的四字段内嵌 API 配置。Windows portable 仍依赖 WebView2，Linux 仍依赖相应 GTK/WebKit 系统库，不宣称完全静态或无需平台依赖。

`package_portable.py` 可单独打包/检查，但只有正式 `prepare_release.py` 同时通过官方 CF 请求、Microsoft 设备码预检、最终原生二进制注入审计、许可及归档校验后才能形成上传集。`collect_release.py` 要求四个完整目标、每目标 portable 与安装包，并重新核对清单、哈希及启动器来源；缺任意目标或证据即阻断全部 Release。

### AppImage 无损优化

Linux 发布在生成签名之前运行 `optimize_appimage.py`。它比较原始与重压缩 SquashFS 的文件内容、权限、链接、属主、时间和 xattrs，不删除库、资源或许可，也不替换 AppImage runtime。每个候选还必须通过发行图像自带的 `--appimage-extract`；系统 `unsquashfs` 支持某种算法，并不表示自带 runtime 支持。只选择更小且兼容的结果，否则保留原始图像。已经签名的图像或 updater `.sig` 输入被拒绝，不能先签名再优化。

最终以 `appimage-compression.json` 的字节数、SHA-256、内部 launcher 哈希与 `runtimeExtractionVerified` 为准；压缩率随依赖/资源变化，不能沿用旧构建数字。更小但无法自解包的 xz 候选不允许发布。启动验收可用 `APPIMAGE_EXTRACT_AND_RUN=1`，仍需检查实际窗口和 portable 相邻路径。

### 公开前验收

质量检查通过后由维护者推送标签，等待四个真实目标的安装包和 portable 全部完成。验收图标、名称、安装/启动、CSP 与图片、真实 CF 功能、用户明确授权的完整 Microsoft → Xbox → Minecraft 登录及游戏所有权；设备码预检和合成归档测试不能替代这些验收。

随包附带 AGPL、美术许可、第三方声明和 `SOURCE.txt`，在公开版本前确认其 `v<版本>` 对应源码可获取，包含本地维护的 vendor 源码、锁文件和开发构建说明，不含秘密。核对 `SHA256SUMS.txt` 后再公开草稿；工作流拒绝修改已公开的 Release 或覆盖已有草稿附件。

## 结构与约定

模块分工、持久化和兼容边界见 [架构说明](architecture.md)。参与开发前阅读 [贡献指南](../CONTRIBUTING.md)。第三方代码和资源的来源、版本与许可列在 [NOTICE.md](../NOTICE.md)。
