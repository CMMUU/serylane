# Serylane — 开源 Mihomo 桌面代理客户端

Serylane 基于 **Mihomo、Rust 与 Tauri 2**，为 Windows、macOS 和 Linux 提供中文订阅、配置、节点、分流规则及系统代理管理。AI 工具域名路由默认关闭。

**Serylane is an open-source Mihomo desktop proxy client for Windows, macOS and Linux.** Optional AI domain routing runs inside Mihomo and is off by default. TUN availability differs by platform; see the compatibility guide.

这是独立客户端项目，并非 Mihomo、Clash 或 OpenAI 的官方产品。项目不提供代理节点或订阅，需要使用自己的订阅与配置。

## 下载与更新

- [官网](https://serylane.cmmuu.com/) · **[下载中心](https://downloads.cmmuu.com/projects/serylane)** · [源码仓库](https://github.com/CMMUU/serylane)
- 官网和应用内更新只使用下载中心。中心不可用、目标安装包缺失或校验失败时提示重试，不切换其他下载来源，不提供旧版本或其他架构替代包。
- Windows、macOS、Linux 各提供 x64 / ARM64 入口；macOS 请区分 **Intel 芯片（x64）** 与 **Apple 芯片（M 系列，ARM64）**。
- 每次检查或点击下载查询中心已核验的稳定发行。源码版本不等于已发布版本，是否可下载以中心实时结果为准。
- 下载有界重试，可取消；签名和 SHA-256 通过后才允许确认安装。检查与下载不停止代理，确认安装和重启会短暂中断连接。Linux 内置更新适用于 AppImage。
- **v0.7.24–v0.7.27 内置安装失败的用户**：首次从下载中心获取已正式发布的 v0.7.28 或更高修复版完整安装包，退出旧版后覆盖原位置并保留配置目录。旧更新器不会先执行新版修复代码。

## 当前功能

- **订阅与配置**：「订阅 → 添加订阅」是唯一新增入口，概览引导前往同一入口；支持添加后选用、配置校验、版本回滚、套餐用量与到期信息。配置更新与用量查询分别记录。
- **选点**：自选节点与 AI 代理互斥。自选模式保留订阅策略组与规则；AI 代理筛选合格候选并生成基础 Fallback，支持流量倍率预算。未知倍率不等于 1×。
- **节点详情**：概览显示所选策略组的实际出口、链路、协议与最近有效延迟。规则模式不存在唯一全局出口；切换影响后续连接，不恢复已断开的数据流。
- **域名路由**：选择工具域名和出口、保存草稿后独立确认开启。规则写入 Mihomo，并回读核对是否生效；不新增 Codex 接入，不代理或重放模型请求正文。
- **启动与状态恢复**：可选择不随系统启动、登录后后台恢复或显示窗口恢复。恢复上次配置和网络模式；上次停止则保持停止。主启动按钮使用已保存模式。
- **日志与界面**：应用日志默认 3 天，可设 1–90 天，正文有 256 KiB / 2,000 条上限；磨砂玻璃主题、隐藏滚动条、托盘与流量监控。长列表按批加载，读取结果按当前操作归属提交。

<!-- product-contract:start -->
| 源码事实 | 当前值 |
| --- | --- |
| 源码版本 | 0.7.34（不代表已经发行） |
| 产品下载来源 | 仅下载中心；失败提示重试 |
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
| macOS | 已有特权辅助服务代码与租约；兼容发行包缺少满足 TUN 要求的 Developer ID 身份，不应视为 TUN 已可用。 |
| Linux | 当前未实现 TUN 网络接管，入口不可用。 |

[启用与兼容性](docs/TUN启用与兼容性.md) · [三端稳定化方案](docs/TUN三端稳定化方案-2026-10-30.md)。稳定化方案是下一阶段设计，不能把构建、模拟或单元测试通过当成三端网络接管验收完成。

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

- [文档索引](docs/index.md) · [当前实现与业务契约](docs/当前实现与业务契约.md) · [更新日志](CHANGELOG.md)
- [当前应用图标](assets/brand/图标说明.md) · [集中项目文档](https://github.com/CMMUU/project-docs/tree/main/serylane)
- 改动功能时同时更新当前说明、源码事实表和对应测试；执行 `node scripts/product-docs.mjs --write` 刷新事实表，`node scripts/product-docs.mjs` 验证同步。
- 六个平台的构建、原生安装升级门禁及签名验收通过后才发布。GitHub 用于源码、构建和不可变发行产物归档；不再作为应用或官网的下载备用源。历史标签、签名、兼容身份与附件不重写。
- 官网从受信任 main 独立部署，不依赖旧镜像任务是否成功。发行后核对下载中心版本、文件完整性及官网六个架构入口；未归档到中心时不能报告用户交付完成。
- macOS 兼容包的项目更新签名不等于 Developer ID 或 Apple 公证。TUN 正式可用还需要签名、服务安装与真实网络矩阵验收。

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
