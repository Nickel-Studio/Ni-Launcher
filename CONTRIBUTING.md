# 参与 Nickel

欢迎报告问题、改进交互、补充翻译或贡献代码。

## 提交问题

先搜索 [已有 Issue](https://github.com/Nickel-Studio/Ni-Launcher/issues)。遇到故障时，使用「故障反馈」模板提供 Nickel 版本、系统、游戏版本、加载器、复现步骤和相关日志；希望改进产品时，使用「功能建议」描述使用场景与预期操作。

上传日志前移除令牌、密码、邮箱和本机用户名。涉及账号认证、凭据泄露或任意文件读写的漏洞，按 [安全报告](SECURITY.md) 提交。

## 本地开发

参照 [开发与构建](docs/development.md) 准备环境。修改前先阅读所在模块和直接调用方；复用已有组件、状态入口、HTTP 客户端及设计令牌。

```sh
pnpm install --frozen-lockfile
pnpm build
cargo check --locked --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

- TypeScript 保持严格检查，Rust 错误返回沿用现有 `Result` 边界。
- 界面文案用中文；明暗模式与颜色主题分别表述。
- 交互保留键盘焦点，动画尊重系统减少动态效果设置。
- 注释解释约束、需求、边界和注意点；可直接读懂的代码不重复翻译。
- 接口、持久化或行为变更同时更新文档与相关回归测试。
- 保留依赖锁定；依赖变化在 Pull Request 中说明原因。

## Pull Request

从 `main` 创建用途明确的分支，使用 `feat:`、`fix:`、`docs:`、`refactor:` 等提交前缀。PR 说明改动、复现或操作路径、验证结果；涉及界面时附明暗两种模式的截图。

原创代码沿用 [AGPL-3.0-only](LICENSE)，原创美术沿用 [CC-BY-SA-4.0](LICENSE-ASSETS)。引入第三方内容时补充来源和许可，更新 [NOTICE.md](NOTICE.md)。
