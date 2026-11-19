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
