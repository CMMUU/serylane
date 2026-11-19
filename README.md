# Serylane — 开源 Mihomo 桌面代理客户端

Serylane 基于 **Mihomo、Rust 与 Tauri 2**，为 Windows、macOS 和 Linux 提供订阅、配置、节点、分流规则及系统代理管理。AI 工具域名路由默认关闭。

**Serylane is an open-source Mihomo desktop proxy client for Windows, macOS and Linux.** Optional AI domain routing runs inside Mihomo and is off by default. TUN availability differs by platform; see the compatibility guide.

这是独立客户端项目，并非 Mihomo、Clash 或 OpenAI 的官方产品。项目不提供代理节点或订阅，需要使用自己的订阅与配置。

## 下载与更新

- [官网](https://serylane.cmmuu.com/) · **[下载中心](https://downloads.cmmuu.com/projects/serylane)** · [源码仓库](https://github.com/CMMUU/serylane)
- 官网安装包由下载中心提供。应用内按 GitHub 直连、经已运行的 Serylane 本地代理访问 GitHub、下载中心的顺序检查正式版。
- Windows、macOS、Linux 各提供 x64 / ARM64 入口；macOS 请区分 **Intel 芯片（x64）** 与 **Apple 芯片（M 系列，ARM64）**。
- 自动检查开启时，应用在启动后检查。发现新版本时，点击右下角卡片进入「设置 → 软件更新」。官网每次点击安装包都会核对下载中心的当前发行。
- 下载有界重试，可取消；签名和 SHA-256 通过后才允许确认安装。检查与下载不停止代理，确认安装和重启会短暂中断连接。Linux 内置更新适用于 AppImage。
- **v0.7.24–v0.7.27 内置安装失败的用户**：首次从下载中心获取已正式发布的 v0.7.28 或更高修复版完整安装包，退出旧版后覆盖原位置并保留配置目录。旧更新器不会先执行新版修复代码。

## 当前功能

- **语言**：设置中可选择 11 种语言。选项显示各自的母语名称，切换在当前窗口生效并自动保存。

- **订阅与配置**使用「订阅 → 添加订阅」在弹窗中填写。点弹窗外或按 Esc 收起，重新打开可继续当前会话的草稿；已提交的导入在后台继续。订阅卡对齐展示选用状态、套餐用量、到期与配置更新，展开可查看用量采样和异常详情。支持配置校验和版本回滚。
- **选点**：自选节点与 AI 代理互斥。自选模式保留订阅策略组与规则。AI 代理重新筛选后，按连接表现、倍率和延迟推荐候选，显示编号及当前出口。当前节点健康时继续使用，确认故障后按排名选择新连接的出口；仍可手动选点：点击「应用节点」后，只有核心确认所选节点为手动固定的实际出口，按钮才变灰并显示「已应用」。切换只影响命中该组的新连接。订阅推荐编号供手动选用。
- **macOS 菜单栏**：左键或右键点击图标打开 macOS 标准原生菜单，可在其他应用全屏时使用。菜单显示全局流量，与主窗口共用系统代理、TUN、路由、刷新、启动和停止的控制与状态核对。选中命令后按系统习惯收起，执行在后台继续；也可点击菜单外或按 Esc 收起。选择「打开主窗口」保留原页面。
- **节点详情**：概览显示所选策略组的实际出口、链路、协议与最近有效延迟。规则模式不存在唯一全局出口；切换影响后续连接，不恢复已断开的数据流。
- **域名路由**：选择工具域名和出口、保存草稿后独立确认开启。规则写入 Mihomo，并回读核对是否生效；不新增 Codex 接入，不代理或重放模型请求正文。
- **启动与状态恢复**：可选择不随系统启动、登录后后台恢复或显示窗口恢复。恢复上次配置和网络模式；上次停止则保持停止。主启动按钮使用已保存模式。
- **日志与界面**：应用日志默认 3 天，可设 1–90 天，正文有 256 KiB / 2,000 条上限；磨砂玻璃主题、隐藏滚动条、托盘与流量监控。长列表按批加载，读取结果按当前操作归属提交。
- **逐项诊断**：检查按真实执行进度逐条出现，当前项显示检测中；已完成结果保留，单项失败继续后续检查。核心停止时明确跳过网络项，不启动代理或切换出口。

<!-- product-contract:start -->
| 源码事实 | 当前值 |
| --- | --- |
| 源码版本 | 0.7.45（不代表已经发行） |
| 官网安装包 | 下载中心 |
| 应用内更新 | GitHub 直连 → Serylane 本地代理 → 下载中心 |
| AI 合格节点范围 | 1–10 个；单节点没有备用 |
| 程序代理 | 暂时停用 |
| 增强稳定灾备 | 暂时停用 |
| 新增 Codex 接入 | 暂时停用 |
<!-- product-contract:end -->

上表由实际源码生成并在测试中核对。更多实现事实、默认值和局限见[当前实现与业务契约](docs/当前实现与业务契约.md)。历史发布说明只记录当时版本，不替代当前文档。

## 快速开始

1. 从下载中心选择对应系统与芯片的安装包。
2. 在「订阅 → 添加订阅」输入 Clash Meta / Mihomo YAML 地址，按需勾选「添加后选用」；本地 YAML 在「配置」导入。
3. 选用配置，点击「启动」使用保存的网络模式。需要切换时明确选择系统代理或 TUN，按平台权限流程确认。
4. 需要 AI 工具域名分流时，再单独配置并启用「路由」。基础联网、特定网站可达与真实模型流健康是不同状态。

## TUN 平台边界

| 平台 | 当前实际状态 |
| --- | --- |
| Windows 10/11 | 实验性，需要整个应用具有管理员权限；尚未完成真实路由、DNS、异常退出恢复的完整验收。 |
| macOS | TUN 使用 `utun` 与特权辅助服务。请选择带 Developer ID 签名和 Apple 公证的完整安装包；v0.7.45 修复部分升级后的旧服务登记问题。v0.7.44 等临时签名兼容包不满足 TUN 要求。 |
| Linux | 当前未实现 TUN 网络接管，入口不可用。 |

[v0.7.45 的安装、授权与验收范围](docs/发布说明-v0.7.45.md) · [三端稳定化方案](docs/TUN三端稳定化方案-2026-10-30.md)。已在 Apple 芯片上的 macOS 27.2.1 隔离虚拟机验证升级恢复、实际接管和停止后的路由恢复；Intel 的发行工件验证与运行时实测分别记录。

## 开发与验证

Windows 使用对应 MSVC、C++ Build Tools、Windows SDK 和 WebView2；Node.js/npm、Rust 及各平台 Tauri 依赖也需准备。

```bash
npm ci
npm run core:prepare
npm run tauri dev
npm run check
npm run test:rust
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

`core:prepare` 按当前 Rust target 下载固定版本 Mihomo 并验证 SHA-256；`core:prepare:all` 准备全部目标。应用运行前先执行 Mihomo 原生配置校验。

## 文档与发行维护

- [文档索引](docs/index.md) · [当前实现与业务契约](docs/当前实现与业务契约.md) · [代理质量验收](docs/代理质量验收.md) · [更新日志](CHANGELOG.md)
- [当前应用图标](assets/brand/图标说明.md) · [集中项目文档](https://github.com/CMMUU/project-docs/tree/main/serylane)
- 改动功能时同时更新当前说明、源码事实表和对应测试；执行 `node scripts/product-docs.mjs --write` 刷新事实表，`node scripts/product-docs.mjs` 验证同步。
- 六个平台的构建、原生安装升级门禁及签名验收通过后才发布。GitHub 用于源码、构建和不可变发行产物归档，也是应用内更新路径之一；官网安装入口仍统一使用下载中心。历史标签、签名、兼容身份与附件不重写。
- 官网从受信任 main 独立部署，不依赖旧镜像任务是否成功。发行后核对下载中心版本、文件完整性及官网六个架构入口；未归档到中心时不能报告用户交付完成。
- 项目更新签名、macOS Developer ID、Apple 公证和系统后台服务许可分别核验。临时签名兼容包的更新签名不会补齐 TUN 服务所需的开发者身份。

## 许可证

源码使用 [GNU GPL v3（GPL-3.0-only）](LICENSE)，第三方依赖见[第三方声明](THIRD_PARTY_NOTICES.md)。每个发行按其锁文件生成许可证清单和 SBOM，并随签名产物归档。`private: true` 仅防止误发布到 npm，不限制 GPL 权利。

## Star 增长趋势

<a href="https://www.star-history.com/?repos=CMMUU%2Fserylane&amp;type=date&amp;legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;theme=dark&amp;legend=top-left" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;legend=top-left" />
    <img alt="Serylane 的 GitHub Star 增长趋势" src="https://api.star-history.com/chart?repos=CMMUU/serylane&amp;type=date&amp;legend=top-left" width="800" />
  </picture>
</a>

图表由 Star History 提供，可能存在缓存延迟。
