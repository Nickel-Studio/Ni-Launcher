# 架构与兼容边界

## 模块分工

| 目录或模块 | 职责 |
| --- | --- |
| `src/components/`、`src/pages/` | 界面、页面与交互 |
| `src/lib/` | 前端状态、导航与跨页逻辑 |
| `src/styles/tokens.css` | 颜色、字体、尺寸与动效令牌 |
| `src-tauri/src/lib.rs` | IPC 注册、窗口与启动编排 |
| `app_paths.rs`、`store.rs`、`vault.rs` | 配置根、账号与凭据存储 |
| `instances.rs`、`version_settings.rs`、`instance_paths.rs` | 实例身份、继承设置与有效资源目录 |
| `app_tasks.rs`、`named_profiles.rs` | 下载生命周期与实例事务 |
| `loader_catalog.rs`、`loader_artifact.rs` | 完整构建清单与安装器解析 |
| `runtime_settings.rs`、`java_selection.rs`、`memory_plan.rs` | 游戏启动参数、Java 与内存 |
| `portable_pack.rs`、`portable_bootstrap.rs` | 便携布局、设置投影与首启导入 |
| `vendor/minecraft-java-rs-core/` | 游戏元数据、传输、校验、加载器与进程启动 |

## 前后端契约

`lib.rs` 的 `generate_handler!` 是唯一 IPC 注册表。跨端字段用 camelCase；启动事件为 `launcher-event`，下载事件为 `install-event`。新增命令同步注册、调用、返回类型和测试。

账号公开模型与后端存储模型分离。账号令牌、密码和 API Key 不进入公开模型；仅状态和必要的公共客户端配置可发送到界面。

## 持久化与更名

`app_paths` 在便携布局、新品牌配置和旧配置中选择完整根目录。账号、加密密钥和缓存必须来自同一根；损坏配置或独立密钥也属于已有数据，不能据此选择空目录。

新安装使用程序旁 `Nickel`，应用标识为 `studio.nickel.launcher`；旧 `com.youtu.nowhere` 和程序旁 `NoWheRE` 继续识别。已有实例、存档和包含绝对路径的缓存原位保留。

`nowhere.*` 实例文件、JSON 扩展字段、便携格式与前端缓存键属于兼容协议。产品可见名称使用 Nickel，兼容协议变更须独立设计迁移。

外部实例设置按只读来源解析。Minecraft 版本以有效档案、`clientVersion`、继承链和 `VersionVanillaName` 识别；展示名与内部数值编码不作为版本依据。保存实例设置只写 Nickel 的文件。

## 来源与任务

版本、文件和社区 API 各自持有来源快照；一次操作不临时改写全局来源。共享传输预算约束全部文件下载；认证与皮肤接口不经过下载镜像。

实例目录占用与任务生命周期统一互斥。取消先进入收尾，最终提交与取消互斥；失败清理只处理本事务预留的目录。进度使用当前阶段的实际计数。

## 便携布局

新导出使用 `Nickel` 数据目录；旧 `NoWheRE` 布局继续读取。载荷、个性化、背景和启动器文件名必须来自同一已验证布局。相对路径保留到落盘，发送界面时才解析为绝对路径。

实例设置通过白名单投影；账号凭据、本机 Java 路径、缓存路径和启动前命令不参与迁移。

## 界面约束

首页保留左侧启动卡与右侧背景留白。全局状态入口单一，页面复用 `PagerLayout`、`Collapse`、`Modal` 等组件。明暗模式与颜色主题分离；动效降级和键盘焦点均由共享组件管理。

关于页只展示 Nickel 简介、版本、徽标、许可、开发者和鸣谢。品牌与外链常量集中在 `src/lib/brand.ts`，鸣谢头像随应用打包。
