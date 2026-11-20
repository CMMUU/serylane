# Serylane 应用内多语言门禁

## 目标与批准

- 用户于 2026-11-19（Asia/Shanghai）明确确认创建本门禁并实施首版应用内多语言。语言按用户给出的顺序展示母语名称。每次选择立即刷新当前窗口并自动保存。主窗口、应用自有菜单、日期数字和操作反馈采用生效语种。页面、光标、草稿、运行状态和在途任务用于切换回归比对。
- 源码基线为 GitHub main b7c01c3，树 e779b71c39206b1b8a3a4b56f087c654e7bb2773。分支 codex/app-i18n-v1 在独立工作树实施。基线 npm run test:ui 通过 180 项。
- 语言顺序为 zh-Hans 简体中文、zh-Hant 繁體中文、en English、ja 日本語、ko 한국어、de Deutsch、fr Français、es Español、pt-BR Português (Brasil)、ru Русский、ar العربية。已有 zh-CN 解析至简中。新配置跟随系统，无法匹配的系统语言回退英语。阿拉伯语使用 RTL，地址及代码标识保持 LTR。
- 用户于 2026-11-19（Asia/Shanghai）明确确认两项共同实施及验收。应用内更新按 GitHub 直连、经已运行的 Serylane 本地代理访问 GitHub、下载中心检查与下载。在自动检查开启时每次启动后台检查，只对可用的新版本显示可关闭的右下角卡片。点击直达设置的软件更新面板。卡片使用当前语言。重新使用其他路径时核对冻结的版本、平台、架构、大小、SHA-256 与签名。
- 更新工作以用户确认的应用内入口、传输、卡片、测试和规则更新为范围。发布与文档使用现有可信流水线。签名工件及用户已安装应用是验收边界。

## 已批准写入路径

- 新增 project-gate.md、locales/manifest.json、locales/{zh-Hans,zh-Hant,en,ja,ko,de,fr,es,pt-BR,ru,ar}.ftl、src/i18n.ts、src-tauri/src/i18n.rs、tests/i18n.test.mjs。
- 修改 index.html、src/{main,settings-view,ui,theme,api,types}.ts、src/{styles,desktop-theme}.css、src-tauri/src/{lib,models,storage,user_rules,macos_menu_panel,traffic_monitor}.rs。
- 修改 src/{subscription-cards,subscription-editor,subscription-import,subscription-task,proxy-state,connection-feedback,domain-routing,diagnostics,log-view,rule-manager,node-selection,node-test,manual-outbound,openai-costs,ai-health,session-resume,app-update,local-routing}.ts、src-tauri/src/{error,subscription,subscription_task,proxy_control,domain_routing,diagnostics,connection_feedback,session_resume,app_update}.rs。
- 修改 package.json、package-lock.json、src-tauri/{Cargo.toml,Cargo.lock,build.rs}、tests/{ui,theme,subscriptions,subscription-task,proxy-state,connection-feedback,diagnostics,domain-routing,session-resume,update}.test.mjs。
- 更新功能修改 src-tauri/src/app_update.rs、src-tauri/src/app_update/{transport,contract_tests,native_tests,recovery_tests}.rs、src/{main,settings-view,api,types,app-update}.ts、src/{styles,desktop-theme}.css、tests/{update,ui}.test.mjs 及上述语言资源。同步受影响的 AGENTS.md、README.md、docs/当前实现与业务契约.md、scripts/product-docs.mjs、官网双语使用说明和集中项目契约。
- 用户指定 CMMUU/project-docs 的 serylane/ 为集中项目文档位置。发布后文档与公开验证按项目 AGENTS.md 的适用要求处理。
- 验收接入 src-tauri/tests/macos_tray_native.rs，使用同一语言模块在一次性状态栏项目上测试。同步 scripts/check-branding.mjs 和官网双语 docs/verification.html 中原有的单路径断言与说明。
- 发行候选沿用版本流水线，更新 CHANGELOG.md、src-tauri/tauri.conf.json、现有版本字段和新版本发布说明 docs/发布说明-v0.7.44.md。集中验收记录由 serylane/README.md 链接，并随实际发行状态同步当前业务契约。

## 证据与恢复

- L1 主门禁覆盖 11 个原文选项及顺序、缺失词条与变量、运行中重绘、设置读写与重开。执行 node --test tests/i18n.test.mjs、npm run check、cargo test --manifest-path src-tauri/Cargo.toml。三端验收最小窗口、德语长文案、阿语混排、代理与任务状态。
- 基础链路时间盒为一个工作日。当前实施迭代 1，同一问题失败修复 0，连续无改善 0。分批回退隔离分支的对应差异，复用旧配置的语言字符串。

- 更新 L1 覆盖三路径顺序与去重、锁定相同工件、哈希和签名、禁止降级、失败与超时、发现新版才提示及点击定位。更新批次通过 npm run test:update、cargo test --manifest-path src-tauri/Cargo.toml --lib app_update 后与语言批次联验。

## v0.7.44 候选验收

- L1 `npm run check` 通过。本地 Node 379 项通过，含 183 项 UI 和 20 项更新测试。TypeScript、Vite 构建及 Rust 检查通过。11 个词库各有 1,884 条一致的消息，参数完整。
- L1 `cargo test --manifest-path src-tauri/Cargo.toml` 通过 464 项，5 项按平台或发行工件前置条件跳过。AppKit 原生菜单运行英文与阿拉伯文文本和方向断言。Rust fmt 和 Clippy 零告警门禁均通过。
- L1 Chrome/Playwright 隔离 IPC 验收覆盖 11 种语言原文及顺序、每项即时重绘与保存、重开恢复、焦点与端口草稿、阿拉伯语方向、新版卡片定位、同版本去重、最新版和失败时静默。英文、韩文、德文、法文、西班牙文、巴西葡萄牙文、俄文和阿拉伯文的 88 个空态与诊断视图组合通过残留中文检查。
- L1 `npm run test:core` 的隔离 Mihomo 配置通过。官网 `npm test` 通过 11 页、372 个链接与静态资源、16 项 Worker 和部署脚本测试。Python 3.12 发行脚本测试通过 151 项，Node 版本标签与恢复测试通过 9 项。
- 本轮保留新版发行的项目更新签名。macOS 沿用上一版兼容发行方式，Developer ID、Apple 公证、用户主机登录与重启验收单独记录。语言资源采用机器辅助翻译，主流程词条经过人工编辑，长尾文案的母语审校待补。
- L1 对已发行的 v0.7.43 核对 GitHub 与下载中心清单。隔离 Mihomo 实例通过本地 HTTP 代理读取 GitHub 发行 API 和固定版本清单。两来源的 10 个更新目标大小、SHA-256 与签名一致。

## v0.7.45：macOS TUN 签名发布门禁

- 用户于 2026-11-19（Asia/Shanghai）确认四文件修复及正式发布范围。生产代码仅限 `src-tauri/src/tun_service/{admin,launchd}.rs`、`src-tauri/src/{macos_service,macos_upgrade}.rs` 及其中的测试。发布另同步五个版本文件、README、CHANGELOG、本门禁、文档索引、业务契约、新发布说明、官网双语的首页／使用说明／路由／验证页，以及集中项目三份文档。现有工作流、签名校验和平台范围不变。
- L1 候选基于 GitHub main `673f7db` 与修复 `936e3cb`，已通过 Developer ID、Hardened Runtime、公证票据检查。隔离 macOS 27.2.1（26B131，ARM64）中重现旧登记 `78: EX_CONFIG`，按系统提示重新批准后台服务，未跳过系统授权；重新打开后确认 v0.7.45 Helper 就绪。未指定 HTTP 代理的 TCP 与外部 IPv4 HTTPS 经 `utun4`；停止后核心退出，路由和系统代理与基线相同。虚拟机关停，宿主代理与默认路由未改变。
- L1 候选前的标准 `npm run check` 覆盖 379 个 Node 测试；本机串行 Rust 测试 472 通过、5 项前置条件未满足而跳过。额外全量旧 Node 测试入口的 13 项错误与未修改的 v0.7.44 基线一致，不作为本次修复引入的回归。
- 发布顺序：现有 CI／站点检查 → 受审 main → GitHub 正式签名发布 → 下载中心归档 → 官网六架构入口与集中项目文档。上一公开版本 v0.7.44 的兼容包和标签保留。Intel 运行时、休眠唤醒及 IPv6 不用签名验收替代。

## v0.7.46 全屏菜单、主窗口恢复和更新门禁

- 用户已确认方案二、正式发布及集中验收同步。基线为已发布 GitHub main `feb001252bcbe391d35dd9525d2381316868fd30`，隔离工作树为 `codex/fullscreen-window-update-v0746`。以当前屏幕的全屏菜单可见且可操作、首次 WebKit 内容进程退出后能显示应用、已核对新版可从第二路下载为首要验收条件。
- 检查及下载顺序为 GitHub 直连 → GitHub 加速代理 → 经 Serylane 本地代理访问 GitHub → 下载中心。加速代理使用现有官网 Worker 和固定的 Serylane 正式版路径。固定版本工件继续校验平台、架构、大小、SHA-256 和签名。
- 自动检查在进程内调度。确认可用新版时，主窗口可见后显示右下角卡片。同一进程内关闭或打开卡片后，相同版本不再次弹出，侧栏版本红点继续显示。点击卡片或版本号进入软件更新并定位可用的下载或安装操作。
- 已批准修改 `src-tauri/src/{lib,macos_menu_panel,app_update}.rs`、`src-tauri/tests/macos_tray_native.rs`、`src-tauri/src/app_update/{transport,contract_tests,recovery_tests}.rs`、`src/{main,api,types,app-update,settings-view}.ts`、`src/desktop-theme.css`、`tests/update.test.mjs` 和 11 个现有语言包。新增 `src-tauri/src/macos_window_recovery.rs`、`website/src/github-updates.ts`，修改 `website/src/worker.ts` 和 `website/scripts/{test-worker,deploy-site,test-deploy-site}.mjs`。
- 文档及发行范围包括本门禁、`AGENTS.md`、`README.md`、`CHANGELOG.md`、`scripts/product-docs.mjs`、`docs/{index,当前实现与业务契约,自动更新与发版,发布说明-v0.7.46}.md`、`website/public/{docs,zh/docs}/{index,verification}.html` 和五个版本文件。发行验收进入 `CMMUU/project-docs` 的 `serylane/{README,当前实现与业务契约,发布说明-v0.7.46}.md`。
- 使用真实全屏空间、带流量宽度的状态项、隔离候选的 WebKit 首次加载中断以及四路失败注入和公开下载验收。签名、归档及官网沿现有发布流程。本地联验时间盒为一个工作日，失败可回退本工作树的具体批次。
- 用户追加确认使用 AppKit 非激活 NSPanel，保留现有菜单命令及 11 种语言。完整应用须在其他应用全屏时实际显示菜单，并从菜单打开、显示常规和遭遇首次 WebKit 中断后的主窗口。再核对四路更新、启动卡片与版本号红点，通过后发布 v0.7.46 并同步集中验收。

### 候选本地验收

- Apple 芯片 macOS 27.2.1 隔离虚拟机中，完整候选的 64pt 双行流量状态项在另一应用的真实全屏空间响应左键与右键。可再次点图标、点外部或按 Esc 收起。选「打开主窗口」切换到 Serylane 主窗口所在的桌面。
- 虚拟机的 WebKit WebContent 受到系统 IOSurface 权限限制，已发布 v0.7.45 也复现图层空白。改在实体 Mac 上以独立应用标识和配置运行完整候选。让首个文档保持尚未提交，结束该隔离 WebContent 进程，再调用 AppKit 面板的原生打开命令。新进程加载应用入口并挂载 11 个导航项，实际画面显示完整首页。
- 软件更新在冷进程实测发现 Rustls 尚未设置加密提供程序。GitHub 发行查询现在先初始化和更新插件一致的提供程序，再构建 HTTPS 客户端。独立运行的回归测试覆盖先于插件的 GitHub 两个入口。

## v0.7.47 软件更新面板

- 2026-11-20（Asia/Shanghai），用户反馈 v0.7.44 检查更新卡住，确认移除软件更新偏好区列出固定顺序的「更新来源」行。基线为已发布 v0.7.46 源码 `2f057714091c3dc0ba83153f2ea6bb4bf4fa4b50`。检查结果仍显示实际来源。
- v0.7.44 正式标签的 GitHub 查询先构建 HTTPS 客户端。使用同版 Reqwest 和构建调用在独立进程复现缺少默认 TLS 加密提供程序。v0.7.46 已初始化该提供程序，旧版可用正式完整包覆盖升级。
- 改动限于 `src/settings-view.ts`、`tests/update.test.mjs`、版本字段和相关发布及产品文档。验收实际设置页、更新结果和按钮焦点，执行更新、TLS、网站及发布检查。按既定流程发布签名补丁版并同步集中验收，保留既有正式版标签和工件。若验收失败，隔离工作树可回退。
- L1 `npm run check` 的 379 项 Node 测试、构建和 Rust 检查通过。冷进程 TLS 回归单独运行通过。官网 19 项测试与类型检查通过。
- L1 用 Chrome 与 Playwright 渲染构建产物，Browser 插件未安装。隔离状态验证 11 种语言的偏好行、实际来源、检查中与失败显示，以及卡片和版本红点到下载或安装操作的焦点。

## v0.7.48 macOS TUN 覆盖升级恢复

- 2026-11-20（Asia/Shanghai），用户确认方案二。先在隔离 macOS 实测有、无恢复记录的覆盖升级，以及用户批准后自动继续。通过后才修复宿主的 Serylane TUN 旧登记，实测接管和停止后的网络恢复，最后同步集中验收。
- 基线为正式 v0.7.47 `648209d4e106fbf26bfcc46123bdcd856632aa9f`。宿主的当前安装包和辅助程序通过签名校验，launchd 仍记录 v0.7.42 和 `78: EX_CONFIG`。旧恢复记录版本为 0.7.43 → 0.7.44。启动试图恢复，约 225 毫秒后失败。macOS 同次启动记录服务有效许可为 `disallowed`。
- 业务改动范围为 `src-tauri/src/{macos_upgrade,macos_service,lib,session_resume}.rs`、TUN 管理层 `src-tauri/src/tun_service/{admin,launchd}.rs` 和 `src/main.ts` 的状态刷新。复用同一服务的登记、移除确认和验签握手，保留原生许可、互斥、运行时及停止意图约束。发行包含版本字段、本门禁、现有发布与产品文档及受影响的官网说明。
- 隔离真机标准以有无标记两种覆盖升级进入同一恢复流程、许可变化后原进程自动就绪、旧登记被新版替代为准。宿主验收记录实包身份、实际流量路径、停止前后路由和系统代理。测试失败保留路径、阶段和系统错误码，必要时在隔离工作树回退。实施时间盒为一个工作日。
- 第一次签名候选实测中，系统许可为启用，旧登记已报告 `78: EX_CONFIG`，原有逻辑仍等待 XPC 到上限。第二轮以重新读回的旧登记和许可作为先行恢复条件。仍以新版认证握手和移除确认作为就绪条件。
- L1 隔离验收在两台 macOS 27.2.1 ARM64 虚拟机运行通过。SIP 与 Gatekeeper 已启用，完整候选通过 Developer ID 签名、公证及签名校验。两组启动前的 launchd 旧登记均为 v0.7.42、未运行、退出码 78。
- 有记录的一组读取旧的 v0.7.43 → v0.7.44 恢复记录。无记录的一组通过 Finder 替换安装，首次启动识别旧登记并写入记录。两组都先等系统许可，批准并返回应用后在原进程继续。各自只移除旧登记一次，随后 launchd 运行 v0.7.48，新 Helper 通过协议 2 的身份核对，恢复记录清除。隔离原生菜单启动 TUN 后，未走 HTTP 代理的 TCP 探针经 `utun4` 命中测试端点。停止后，测试路由和系统代理快照一致。

## v0.7.48 原生菜单与 TUN 模式切换

- 2026-11-20（Asia/Shanghai），用户确认恢复 v0.7.45 的 AppKit `NSMenu`，并采用方案二处理 TUN 切换系统代理的端口收尾。基线为 GitHub main `04d9c2a5ece10ba6a8be6f8527fe287836d01d2a`。独立工作树 `codex/native-menu-tun-handoff-v0748` 分别提交两项源码变更，与辅助服务修复合并发布。
- 菜单展示与触发改动限于 `src-tauri/src/macos_menu_panel.rs` 和 `src-tauri/tests/macos_tray_native.rs`。标准状态项挂接原生菜单，沿用现有命令、读回、语言、流量和主窗口调用。
- 端口处理限于 `src-tauri/src/{runtime,proxy_control}.rs` 及其中的回归测试。实体 Mac 日志两次记录混合端口 7895 返回 `EADDRINUSE`。隔离机在已确认退出的 TUN 内核端口上观察到 `TIME_WAIT`，且无监听进程。清理后系统代理使用同一端口启动。
- 网络和覆盖实测均使用隔离 macOS。内置屏菜单使用独立候选，在可以专用于验收的时段核对 28 pt 图标、64 pt 流量、左右键和普通桌面及原生全屏。标准菜单外观、点外部或 Esc 收起、长时间保持及确切的一次动作都需有记录。全屏失败先记录复现并暂停发布。本机正式应用及代理不参与测试。
- 相关说明、发行门禁及 `CMMUU/project-docs` 随 v0.7.48 同步。时间盒为一个工作日。任何一项验收未通过都保留独立提交和实测记录。

## v0.7.48 AppKit 菜单独立进程

- 2026-11-20（Asia/Shanghai），用户确认先完成菜单方案。实体内置屏双版本对照记录在 PR #49，Codex 全屏时常规 AppKit 应用创建了菜单窗口，系统窗口列表标为不可见。同一可执行文件的独立菜单进程原型通过隔离全屏及主窗口派发验证。
- 本轮改动范围包括 `src-tauri/src/{lib,macos_menu_panel,traffic_monitor}.rs`、`src-tauri/tests/macos_tray_native.rs` 和本门禁。主应用以常规 AppKit 进程保有 Dock、控制器和主窗口，独立辅助进程持有唯一的 `NSStatusItem` 与系统 `NSMenu`。通过匿名管道同步语言、已核对状态和流量，按操作标识识别拒绝反馈。父进程保管子进程生命周期和命令派发。
- L1 `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` 及 `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` 通过。在隔离 macOS 执行 `macos_tray_native`，通过原生绘制、语言、状态、旧操作拒绝、准确派发、独立进程启动、崩溃重启和退出。
- L1 Apple 芯片 macOS 27.2.1（26B131）安装 Developer ID 签名并公证的完整候选，Gatekeeper 与 SIP 均开启。SHA-256 为 `47309ed8f56b4af21d29a5cdffbe1280747c0df2f073bc52c13074b99fe21b09`。普通桌面与真实 TextEdit 全屏，两种宽度的八次左右键点击均由系统创建可见菜单，TextEdit 保持前台。菜单保持可见约八秒，期间流量行更新。Esc 与点击外部按原生行为收起，系统浅色和深色均显示原生外观。
- L1 从全屏菜单选择「打开主窗口」显示并切到原应用进程。普通桌面点击 Dock 也切到同一个常规主进程。试验中手动模式菜单启动只出现一个 Mihomo 进程，系统代理菜单显示标准勾选并成功接管隔离机。停止后系统代理和默认路由与基线一致。结束菜单进程后，同一主进程重建菜单。通过菜单退出及在跟踪菜单时结束主进程，均核对两个进程退出。
- 主窗口完整网页画面仍需实体内置屏复验。隔离 macOS 中，本次候选首次显示窗口及从菜单打开后均观察到空白 WebView，先前隔离候选也复现。下一轮采用准备好的独立标识、独立数据的界面候选，核对两种宽度、主窗口渲染、Dock 与外观。真机门槛通过后才推进发行及集中验收同步。
