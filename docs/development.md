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

## 可选 API 配置

### Microsoft

在 Azure / Entra 注册公共客户端，允许公共客户端流，账号范围包含组织与个人 Microsoft 账号，委托权限使用 `openid`、`profile`、`offline_access`。

复制示例文件，将 Application (client) ID 填入 `microsoft.clientId`：

```powershell
Copy-Item secrets.local.example.json secrets.local.json
```

运行时读取顺序为 `NICKEL_SECRETS` 指定文件、选定配置目录、程序目录，再使用内嵌配置；仅调试构建会在内嵌配置前尝试开发根目录。Microsoft 设备码认证使用公共客户端，不需要 client secret。

### CurseForge

在 [CurseForge Console](https://console.curseforge.com) 的 API Tokens 页生成开发者 Key，填入同一文件的 `curseforge.apiKey`。Key 由后端用于官方 API 请求头。

## 构建通道

`NICKEL_CHANNEL` 在编译时选择 `development` / `production`。普通开发与测试默认 `development`；打包工作流指定 `production`。

开发通道未配置 CurseForge Key 时使用社区镜像；生产通道向官方 API 发请求并按真实响应处理错误。

```powershell
$env:NICKEL_CHANNEL = "production"
pnpm tauri build
```

Windows 窗口排障可设置 `NICKEL_DIAG=1`：

```powershell
$env:NICKEL_DIAG = "1"
pnpm tauri dev
```

## 测试

`cargo test --locked` 运行单元测试。依赖网络、现有 Java、系统 UI 或外部实例的用例使用 `#[ignore]`，按测试要求单独运行。

在 `pnpm dev` 后打开以下回归页：

- `/tests/loader-picker/regression.html`：提供方清单、兼容组合、重试、旧响应隔离。
- `/tests/ui-foundation/regression.html`：基础组件、键盘焦点、页面切换与减少动态效果。
- `/tests/version-instances/regression.html`：实例选择、收藏、删除与指南入口。
- `/tests/ui-remaining/regression.html`：社区检索、详情与下载任务。
- `/tests/mods-scan/regression.html`：渐进扫描、失败恢复与旧实例隔离。
- `/tests/community-version/regression.html`：真实游戏版本的筛选与安装上下文。
- `/tests/loading-animation/regression.html`：加载布局与错误态动画。
- `/tests/settings-regression/regression.html`：设置与内存轮询。
- `/tests/brand/regression.html`：Nickel 名称、关于页、本地徽标与外链。

浏览器测试使用本机已有 Chrome、Edge 或已缓存浏览器。测试输出写入本地临时目录。

## CI 与发行

[CI](../.github/workflows/ci.yml) 在 `main` 推送和 PR 上运行前端构建与 production 通道的 Rust 测试；本地可分别设置 development / production 验证两种请求策略。[Build](../.github/workflows/build.yml) 支持手动打包与 `v*` 标签，覆盖 Windows x64、macOS arm64 / x64 和 Linux x64。标签触发时创建 Release 草稿。

发布前保持 `package.json`、`src-tauri/Cargo.toml`、根包的 `Cargo.lock` 条目与 `tauri.conf.json` 版本一致，标签使用 `v<版本>`。构建固定使用锁文件，不在发行时更新依赖。

质量检查通过后推送版本标签，等待四个平台的安装包构建完成。验收图标、名称、随包许可和源码归档，附上 `SHA256SUMS.txt`，再将 Release 草稿公开。打包附带 AGPL、美术许可与第三方声明。

## 结构与约定

模块分工、持久化和兼容边界见 [架构说明](architecture.md)。参与开发前阅读 [贡献指南](../CONTRIBUTING.md)。第三方代码和资源的来源、版本与许可列在 [NOTICE.md](../NOTICE.md)。
