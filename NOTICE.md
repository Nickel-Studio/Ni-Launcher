# 版权与第三方资源

Nickel 由 ItzArona、ItzPlana 与项目贡献者共同维护。

- 原创程序代码与项目文档：GNU Affero General Public License v3.0，见 [LICENSE](LICENSE)。
- 下表列明的原创美术：Creative Commons Attribution-ShareAlike 4.0 International，见 [LICENSE-ASSETS](LICENSE-ASSETS)。
- 第三方代码、字体、纹理与标识按各自的许可和使用条款保留。

## 原创美术与署名

| 资源                        | 文件范围                                                        | 署名                                                       |
| --------------------------- | --------------------------------------------------------------- | ---------------------------------------------------------- |
| Nickel 品牌图形、横幅与字标 | `src/assets/logo.png`、`src/assets/brand/` 下的 PNG             | ItzArona、ItzPlana / Nickel Studio                         |
| 应用图标与网站图标          | `src-tauri/icons/`、`public/logo-32.png`、`public/logo-256.png` | ItzArona、ItzPlana / Nickel Studio；由 Nickel 品牌图形导出 |

复制或改编上述美术时，请标明作者、[项目地址](https://github.com/Nickel-Studio/Ni-Launcher)、CC-BY-SA-4.0 和修改情况，并以相同许可分享改编作品。

## 第三方代码

### 编译与前端依赖

React、React DOM、scheduler、Tauri 与其他锁定依赖保留各自的版权及许可。版本、来源、原文和 MPL 对应源码入口见 [完整依赖声明](LICENSES/dependencies-NOTICES.txt)。这份声明随安装包及附带启动器的便携 ZIP 一并提供；第三方许可不由 Nickel 的 AGPL 或美术许可替换。

### minecraft-java-rs-core

`src-tauri/vendor/minecraft-java-rs-core/` 基于 Fitz 的 0.4.1 版本，保留 MIT 许可与版权声明。Nickel 在本地维护网络来源、加载器解析、归档校验与启动参数处理。

- [上游项目](https://github.com/Fitzxel/minecraft-java-rs-core)
- [随附 MIT 许可](src-tauri/vendor/minecraft-java-rs-core/LICENSE)

### PCL 与 PCL-CE

- [PCL](https://github.com/Meloong-Git/PCL) 由龙腾猫跃开发，采用《PCL 分发有限许可》及《PCL 存储库合理使用指南》。随附 [上游原文](LICENSES/PCL-LICENCE.txt)，正式分发条款见其原文中的链接。
- [PCL-CE](https://github.com/PCL-Community/PCL-CE) 保留 `Copyright 2025 PCL Community`。[上游根许可](https://github.com/PCL-Community/PCL-CE/blob/dev/LICENSE)规定 `Plain Craft Launcher 2/` 使用该目录的[自定义条款](https://github.com/PCL-Community/PCL-CE/blob/dev/Plain%20Craft%20Launcher%202/LICENCE)，其余目录使用 Apache-2.0；不能按贡献作者或来源替换这一目录边界。
- 两个项目的标识用于「特别鸣谢」中的项目识别。相关权利属于对应作者，不纳入 Nickel 原创代码或原创美术授权。

### 按需取得的 Java 组件

| 组件                | 版本             | 许可                                        | 上游                                                                            |
| ------------------- | ---------------- | ------------------------------------------- | ------------------------------------------------------------------------------- |
| authlib-injector    | 1.2.8 / build 56 | AGPL-3.0-or-later，附 AUTHLIB-INJECTOR 例外 | [yushijinhun/authlib-injector](https://github.com/yushijinhun/authlib-injector) |
| Java Launch Wrapper | 1.4.4            | MIT                                         | [00ll00/java_launch_wrapper](https://github.com/00ll00/java_launch_wrapper)     |
| LWJGL Unsafe Agent  | 2.0              | Apache-2.0                                  | [HMCL-dev/lwjgl-unsafe-agent](https://github.com/HMCL-dev/lwjgl-unsafe-agent)   |

这些组件按游戏版本、Java 和认证方式取得。Java Launch Wrapper 保留 `Copyright (c) 2022 00ll00`，LWJGL Unsafe Agent 保留 `Copyright 2026 Glavo`。完整许可随附在 `LICENSES/`，其中 authlib-injector 的例外原文为 [AGPLv3-with-authlib-injector-exception.txt](LICENSES/AGPLv3-with-authlib-injector-exception.txt)。重新分发组件时一并保留相应许可、版权与 NOTICE，并提供许可要求的对应源码。

## 字体

`src/assets/fonts/nowhere-line.woff2` 是 Noto Sans SC 的字符子集，保留 Adobe 的版权声明与 SIL Open Font License 1.1。详见 [字体说明](src/assets/fonts/NOTICE.md) 与 [完整 OFL](LICENSES/OFL-1.1.txt)。品牌横幅中的文字已栅格化，字体文件不随横幅分发。

## 游戏纹理与外部标识

- `src/assets/blocks/` 的实例、加载器与发布状态图标，以及 `src/assets/mod-placeholder.png`，保留既有功能资源和分配映射。游戏纹理与对应项目标识的权利属于各自权利人；品牌更名不替换这些功能图标，也不将其重新授予 Nickel 原创美术许可。
- `src/assets/skins/` 的 Steve / Alex 头像裁自 Minecraft 默认皮肤，相关纹理归 Mojang / Microsoft；见 [Minecraft Usage Guidelines](https://www.minecraft.net/en-us/usage-guidelines)。
- `src/assets/credits/` 的 GitHub 头像属于对应用户或组织；LinuxDO 标识取自其官方公布的图形。来源列表见 [鸣谢图标](src/assets/credits/README.md)。

以上第三方资源不列入 Nickel 原创美术授权范围。
