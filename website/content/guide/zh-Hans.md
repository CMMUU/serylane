# Serylane 使用指南

从下载安装、添加自己的订阅或本地 YAML，到选择网络模式、配置 AI 域名分流、检查更新和排查常见故障。

## 下载与安装
<!-- anchor: install -->

从[官网下载区](https://serylane.cmmuu.com/zh/#download)或[下载中心](https://downloads.cmmuu.com/projects/serylane)选择正式版。Serylane 本身不提供节点、订阅或 AI 服务账号。

- Windows：Intel / AMD 64 位电脑选 x64；Windows on ARM 选 ARM64。
- macOS：M 系列选 Apple 芯片，Intel Mac 选 Intel 芯片。打开 DMG，把 Serylane 拖进「应用程序」。需要 macOS 13 或更新版本。
- Linux：按架构选择 AppImage，赋予文件执行权限后运行。

从 v0.7.51 起，新用户在进入主页前选择界面语言。官网 Windows 安装包在安装过程中提供选择。macOS、Linux 或使用 MSI 安装的用户在首次打开时选择。系统匹配的语种会预先选中，也可以选择跟随系统。升级继续使用已保存的语言。

## 五分钟开始使用
<!-- anchor: quick-start -->

1. 有订阅时，在「订阅 → 添加订阅」填写名称和 Mihomo / Clash Meta 订阅地址，点「验证并添加」。有本地 YAML 时，在「配置」打开 YAML，先检查再创建并激活。
2. 确认目标订阅显示「已选用」。已有配置时，只有勾选「添加后选用」或手动选用才替换它；选用配置本身不会启动代理。
3. 先试顶部「系统代理」。状态显示运行后，打开平时使用的网站，并在「连接」检查请求是否进入核心、是否命中预期规则。需要时再到「诊断」定位问题。

## 选择代理模式
<!-- anchor: proxy-modes -->

- **系统代理**：适合遵循系统代理的应用。Linux 的系统代理依赖 GNOME 的 gsettings；其他桌面环境可使用本地端口。
- **TUN**：适用于不读系统代理的应用。macOS 先使用签名、公证的完整安装包，再按提示授权辅助服务，在「系统设置 → 通用 → 登录项与扩展」确认后台许可。Windows 需管理员权限，仍属实验性；Linux 未提供 TUN。
- **本地端口**：在需要代理的应用中手动填写 `127.0.0.1` 和 Serylane 设置里的混合端口，按应用支持选择 HTTP 或 SOCKS5。不要填控制端口。

辅助服务就绪不等于流量已接管。开启 TUN 后检查顶部状态和实际连接；用完点「停止」。网络模式决定流量如何进入核心；规则、全局、直连决定进入后如何转发。

## 管理订阅
<!-- anchor: subscriptions -->

通过卡片「刷新」或「刷新全部」更新。配置和用量独立刷新，失败时保留上一份可用配置。卡片的「更多」或右键菜单可编辑；改地址后手动刷新。导入失败时先确认链接是 Mihomo / Clash Meta 订阅而非账户网页，再核对订阅下载路径及错误阶段；选择的代理路径失败不会静默改成直连。

## 自选节点与 AI 代理
<!-- anchor: nodes -->

两种选点方式二选一。自选节点按订阅策略组选点，可单项或一键测速。AI 代理可用 1–10 个合格节点，只有一个时没有备用节点。手动选点后点「应用节点」，等到显示「已应用」；需要恢复自动选点时点「恢复自动」。更换节点只影响后续连接，不会续接中断的模型输出。

## AI 工具、域名路由与 Codex
<!-- anchor: ai-tools -->

域名路由默认关闭，只处理已进入核心、处于规则模式的流量。勾选工具或添加域名、选择出口、保存草稿，再确认开启或应用。`example.com` 是根域，`*.example.com` 只匹配子域。其他流量保留原规则；详见[域名路由](https://serylane.cmmuu.com/zh/docs/routing)。

**可选：Codex 跟随系统代理。** macOS 或 Windows 上，支持此选项的 Codex CLI 可用 `codex --enable respect_system_proxy` 启动。它只让本次 Codex CLI 读取已有系统代理，不开启 PAC、不改系统设置。系统已有 PAC 时可按其选路，但这不是故障切换，也不会让同一条线路更稳定。查看[OpenAI 官方说明](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests)，用一次完整的真实请求确认。

## 检查与安装更新
<!-- anchor: updates -->

进入「设置 → 软件更新」，检查版本和实际来源。应用依次尝试 GitHub 直连、GitHub 加速、经运行中的 Serylane 代理和下载中心；对同一版本、架构和签名校验后才允许安装。旧版仍更新失败时，从下载中心下载同系统同架构正式完整包覆盖安装，保留用户配置。[查看校验说明](https://serylane.cmmuu.com/zh/docs/verification)。

## 常见故障
<!-- anchor: troubleshooting -->

- 显示运行但某应用不经过代理：确认系统代理或 TUN 已接管，而不是仅本地端口运行；检查「连接」。
- macOS TUN 待授权或需重新关联：检查登录项与扩展中的许可。明确需重关联时先停代理，再到设置的 TUN 权限服务重关联并等就绪。
- 切回系统代理报端口占用：查看「技术详情」的端口和失败阶段，刚停 TUN 可等旧连接释放后重试。显示「代理保持停止」并不代表已恢复运行。
- AI 接口返回 HTTP 401：只证明接口可达，分别验证登录和真实模型输出。

反馈请附版本、系统与架构、网络模式、时间和脱敏错误，不附订阅链接、Token 或密钥。
