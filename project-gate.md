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
- L1 Chrome/Playwright 隔离 IPC 验收覆盖 11 种语言原文及顺序、每项即时重绘与保存、重开恢复、焦点与端口草稿、阿拉伯语方向、新版卡片定位、同版本去重、最新版和失败时静默。另对英、德、韩、阿四种语言逐页检查了 11 个空态与诊断视图的残留中文。
- L1 `npm run test:core` 的隔离 Mihomo 配置通过。官网 `npm test` 通过 11 页、372 个链接与静态资源、16 项 Worker 和部署脚本测试。Python 3.12 发行脚本测试通过 151 项，Node 版本标签与恢复测试通过 9 项。
- 本轮保留新版发行的项目更新签名。macOS 沿用上一版兼容发行方式，Developer ID、Apple 公证、用户主机登录与重启验收单独记录。语言资源采用机器辅助翻译，主流程词条经过人工编辑，长尾文案的母语审校待补。
