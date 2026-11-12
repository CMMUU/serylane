# Serylane 官网改版门禁

## 已批准目标与范围
- 用户于 2026-11-12（Asia/Shanghai）选择 B / Precision Routes，并确认「方案二同时需要考虑到 seo 优化」。本阶段授权隔离本地实现和验收；生产上线、推送和合并仍须另行确认。
- 基线：1717defdd0a503b6da728cc7ffe46460119deece；隔离分支 codex/website-precision-20261112，工作树 /opt/workspace/codex/worktrees/serylane/website-precision-20261112。另一个工作树的客户端改动不复制、不回退、不发布。
- 已批准 26 文件：修改 website/public/index.html、styles.css、site.js、404.html、sitemap.xml；website/public/docs/index.html、routing.html、verification.html、privacy.html；website/src/worker.ts（仅非 API 503 HTML 双语说明）；website/scripts/check-site.mjs、test-downloads.mjs、deploy-site.mjs、test-deploy-site.mjs、test-worker.mjs；website/README.md、QA.md、package.json。新增本门禁、website/public/theme.js、website/public/zh/index.html、website/public/zh/docs/index.html、routing.html、verification.html、privacy.html、website/scripts/test-presentation.mjs。
- 公共样式重写为 B 风格。英文根路径和中文 /zh/ 全静态对应；文档锚点兼容。导航、下载状态、辅助标签完整本地化，GitHub 指向 CMMUU/serylane 源码，不作安装包备用源。
- 两主题：默认深色，支持浅色和跟随系统；仅允许一个本地主题偏好键 serylane-theme（dark/light/system），不保存语言、配置、用户信息，不增加其他存储。读取/写入被禁时安全降级。隐私说明和全脚本安全检查同步更新。
- 分流动效使用 SVG/CSS，无依赖；仅作能力示意，不代表实时流量。支持暂停、减少动态效果、离屏/后台暂停，不启动或联系桌面代理。
- SEO：各语种自引用 canonical、双向 en/zh-CN/x-default hreflang、静态正文、独立标题/描述、结构化数据、OG/Twitter、完整 sitemap、404 noindex、旧锚点。无虚构评分、用户量、测速或排名保证。
- 不改变 Worker 动态协议、六架构下载路径、校验和渠道、CSP、DNS、CI、依赖或 lockfile。仅 package.json 接入新增测试。明确源码不等于最新可下载安装包，域名路由默认关闭，TUN/停用能力边界不变。

## 验证与交付
- 主验收：pnpm test、pnpm typecheck；双语六下载路径、错误/重试、无 JavaScript、安全/CSP、SEO 互链和静态核验。
- 浏览器：320/390/768/1440 × 中英 × 深浅 16 组无横溢；主题首绘/刷新/跨页/存储禁用；动效确实流动、暂停/离屏/后台/减少动态效果；正文对比度目标 >=4.5:1、控件/大字 >=3:1；GitHub 链接正确，无控制台异常或越界请求。
- 证据目录已批准：/opt/workspace/codex/artifacts/serylane/2026-11-12/官网改版/。测试日志与截图可保存该目录，不包含密钥、个人配置或订阅。
- 先交付本地可交互预览。生产发布必须再确认，使用现有版本上传/切换流程，不能因本地验收通过声称已上线或已收录。

## 时间盒与止损
- 从 2026-11-12 15:10 +08:00 开始，本地实现/验收时间盒 120 分钟。到 18:10 仍未满足主验收则暂停并汇报。CI/生产发布不在本轮。
- 连续三轮主指标无改善或同一问题第二次修复失败则停止当前路径，重新评审。不得越过批准文件范围；新增路径先确认。
- 当前实施迭代 1；浏览器缺陷修复 1 次（小屏装饰网格越界已通过容器内裁切解决）；同问题失败修复 0，连续无改善 0。恢复仅针对本隔离工作树的本轮文件，保留其他会话改动。

## 本地交付结果 · 2026-11-12
- 已实现 B / Precision Routes 全新内容和布局；英文及中文十个静态内容页、404、两主题与系统模式、单键外观记忆、可暂停 SVG 分流光段。无第三方资源或动画依赖，GitHub 仅指向源码。
- SEO 已落实并验证：唯一标题/描述与单H1、自引用 canonical、en/zh-CN/x-default、静态正文、OG/Twitter、真实结构化数据、十页 sitemap/30个语言互链、旧文档ID 62/62保留、别名归一化和真实 noindex 404。不代表已经收录或取得排名。
- L1：pnpm test 退出0；11个 HTML、370个站内链接/资源、双语下载和主题/动效检查通过，Node子测试16/16、失败及跳过0。pnpm typecheck 退出0；保留现有工具建议，未新增依赖。
- L1：最终 Chromium / 本地 Cloudflare runtime 浏览器验收54/54：16首页布局+32文档布局+6交互/无JS/SEO场景。正文/按钮对比度>=4.5:1，关键控件/连线>=3:1。未预期浏览器异常0，站外请求0；动效确实移动，暂停/减少动态效果/离屏有效。
- 浏览器有意模拟24次中心不可用响应检查错误和重试；不能将截图中的状态或合成测试当作线上下载中心故障/可用性结论。没有下载安装包正文，也未操作用户应用或代理。
- 独立只读复核无阻塞代码缺陷；README/QA已更新并保留旧验收为历史。范围审计为26个批准路径（18修改+8新增），Worker仅非API503 HTML一行变化；客户端、lock、依赖、CSP、配置、CI均不变。
- 原始测试/类型检查/浏览器日志、JSON、16份实际页面截图、首次失败证据、范围审计与完整补丁保存在批准的证据目录；可复核入口为 [QA.md](QA.md)。本地预览 http://127.0.0.1:4187/ 与 /zh/，由托管后台任务提供。
- 待用户审阅本地设计并明确确认上线；尚未提交/推送/合并或部署。生产必须重新核对可信main与发布状态，按原版本上传/切换流程验收十页字节和六架构下载。真实设备/Safari/辅助技术、生产性能及Search Console/索引尚未验证。

## 用户追加调整 · 主题按钮化
1. 用户明确要求「将黑色/浅色模式切换为按钮级别的切换」。目标：取消下拉框，三个原生图标按钮直接选择深色、浅色和跟随系统；保留原外观记忆与无障碍语义。属于已批准主题能力与26文件范围内的UI细化，不扩大生产/下载/客户端权限。
2. 基线证据：11个HTML当前均为select[data-theme-select]，theme.js使用select change；既有L1结果见上。改为button[data-theme-value]的click，aria-pressed标示偏好，系统模式不因实际亮暗变化而丢失选中。标题/aria-label中英对应，原生Tab/Enter/Space可用。
3. 本轮仅改原范围中的11个公共HTML、public/styles.css、public/theme.js、scripts/test-presentation.mjs和README/QA/本门禁（17路径）；原始浏览器验收脚本与新按钮专项证据保存在已批准证据目录。无需新源码文件或依赖，SEO/下载/API/CSP/动效不改变。
4. 验证：pnpm test、pnpm typecheck；浏览器检查11页、320/390/768/1440布局、按钮点击/键盘/aria同步、主题跨页/刷新/中英文与系统偏好、拒绝存储降级、没有新增请求；原有动效/下载回归继续通过。
5. 从2026-11-12 16:06 +08:00开始，时间盒30分钟，超过16:51未达标则暂停汇报。仅回退本轮按钮化差异，保留已通过的官网改版。实施迭代1、同问题失败修复0；仍仅本地预览，不提交/推送/合并/上线。
6. 按钮追加验收L1：11页原生按钮已替换，pnpm test与typecheck退出0，独立展示套件四组通过，实际Chromium54/54通过（含Enter/Space/Tab、320px布局、跨语种/文档/刷新、系统主题和存储拒绝）。无范围外源码、额外请求或生产操作。原始证据见QA新增「主题按钮追加验收」；旧下拉框阶段证据保留。

## 用户已授权上线 · 2026-11-12
1. 用户明确要求「现在将这个进行上线，流动效果那部分的速率调的稍微快一点」，并于17:33/18:07要求继续。此前仅本地/待确认限制在本阶段已由明确上线授权替代；不授权客户端发布或其他网站/域名操作。
2. 目标：官网B改版、中英静态/SEO/主题按钮真实上线；SVG周期5s→3.5s（快约43%），分支相位同比例改为-1.05s/-2.1s，暂停/减少动态效果不变。现网sourceCommit与远端main仍1717def；旧Worker版本ae732bb4-441d-4283-b5b6-4a5b4962c3fd。
3. 源码仍仅原26路径；新增调整只涉及styles.css、test-presentation.mjs与README/QA/本门禁。授权对官网分支提交、推送、创建/合并通过检查的PR，以及现有Worker版本上传/切换。GitHub主分支保护/并行PR已只读检查，不夹带未合并的客户端修复。不得改CI/CSP/DNS/下载校验/依赖/其他会话文件。
4. 验证：本地pnpm test/typecheck、3.5s及错峰断言、实际浏览器暂停/减少动态效果；独立源码审阅；PR静态检查通过后合并，现有网站CI发布。验收线上build-info、十个内容页/17资源字节、真实noindex404、主题按钮、动效周期与SEO互链、六个实时正式版下载入口（HEAD，无包正文）。不把源码发布等同新桌面安装包发布。
5. 恢复后准备/提交时间盒60分钟，CI/边缘验证另20分钟；连续三轮无改善或同问题第二修复失败则停止路线重新评审。失败不重写tag、不降级校验、不直接运行域名部署命令、不重试上传写入；先核对现网状态，必要回退仅限已记录Worker版本且先报告。
6. Git首次HTTPS失败已定位：命令使用显式进程级现有代理后读取远端成功，没有改网络或代理设置；GitHub与Cloudflare账户已有合法授权。当前阶段实施迭代1；此前待提交状态现已由以下真实交付证据替代。
7. 上线前L1已通过：产品文档源码事实、pnpm test/typecheck、versions upload --dry-run全部退出0，实际Chromium54/54（含16组合计算样式3.5s）通过。26路径已独立审查并提交官网PR，通过官网和六平台回归检查后合并与发布；原始证据见QA。

## 正式交付结果 · 2026-11-12
- 已上线：英文https://serylane.cmmuu.com/，中文/zh/；分流3.5s、三主题按钮、双语文档与SEO均包含。本轮仅网站，应用正式版仍由下载中心实时解析为v0.7.39，没有新发安装包或操作客户端。
- [PR #36](https://github.com/CMMUU/serylane/pull/36)代码f07fcf3，合并main76ed5cd737d7685f826a9f70e00e730e525309d8。源码仅批准26路径，客户端/CI/CSP/DNS/配置/依赖/下载校验不变。[发布作业43200982269](https://github.com/CMMUU/serylane/actions/runs/43200982269)成功，首轮Worker305da7cc-8ea0-47a0-a743-a31dd617c0b6为100%。
- 生产L1：build-info/websiteTree与可信main匹配；十页/17资源实际GET逐一匹配项目源码SHA-256（robots保留托管前缀并提取项目段）；两个未知路径真实noindex404；六架构当前正式版HEAD302及废弃渠道400通过，无安装包正文；生产浏览器8/8（含3.5s/按钮/暂停/减少动态效果），真实发布目录8/8为200，异常及站外请求0。
- 旧Gitee镜像private字段契约不符导致独立sync失败，已公开记录；故障来自未改动的旧main，发生于同步写入前。按既有AGENTS契约不作为官网部署前提，未修改镜像可见性/CI/密钥/渠道。不得宣称镜像同步成功。
- 证据与残余限制见[QA.md](QA.md)。最后同步README/QA/本门禁三篇关闭记录，网页/脚本/Worker字节不改，受同一可信main网站流程验证；最终源码身份以实时build-info为准。Safari/真机/辅助技术、性能及索引不作为已验收事项。
