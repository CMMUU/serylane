# Serylane 官网

基于已确认的三段磨砂玻璃设计实现，原生 HTML / CSS / JavaScript，无框架、第三方字体、分析脚本或客户端构建依赖。Cloudflare Workers Static Assets 托管；上传 `public/` 和公开发布查询 Worker，不上传桌面程序源码、用户配置或密钥。

官网：<https://serylane.cmmuu.com/>。已于 **2026-09-08** 部署至 Cloudflare Workers Static Assets，首页与四篇文档 HTTPS 200、未知路径 404、`.html` 归一化 307、robots 和 sitemap 均已通过线上请求验收。首次边缘访问曾短暂超时，复核后上述页面均正常；本记录不代表持续可用性监控或搜索引擎已收录。

首次上线版本 ID：`3fdb1fbf-ace8-43dc-8fec-5e9aa1c96c4b`。先上传未绑定域名的新 Worker，再核对域名变更预检只包含该 hostname 的一个新增项、无更新/移除/冲突，最后以三个覆盖开关均为 `false` 绑定域名。回读确认指向 `serylane-website`，未替换已有 DNS 或其他网站。域名绑定会新增 Cloudflare 管理的 DNS 与证书。

历史部署记录（下述当时的渠道行为已由当前单一下载中心契约替代）：v0.7.6 官网更新已于 **2026-09-08** 上线，版本 ID：`ab83f410-fa93-422a-9394-dfff107e4707`，100% 流量；通过版本上传／部署完成，未重建或覆盖域名与 DNS。线上主页、`site.js`、`/docs/` 实际 GET 成功，v0.7.6 下载链接、国内缺失回退与唯一订阅路径均已核对；静态与下载交互离线检查通过。线上 IAB 复核连续超时，不将其记录为本次视觉验收通过。

## 本地检查

需要 Node.js 24、pnpm 11.19.0。

```sh
cd website
pnpm install --frozen-lockfile
pnpm test
pnpm dev
```

开发服务只监听 `127.0.0.1:4177`。它不会打开浏览器，也不会调用桌面客户端、本地代理或 Codex。可设置本进程的 `SERYLANE_PREVIEW_PORT` 改用其他空闲端口。

Cloudflare 的真实资源路由与安全头预览：

```sh
pnpm exec wrangler dev --local --ip 127.0.0.1 --port 4178 --show-interactive-dev-session=false
pnpm deploy:check
```

只需网页测试，无须启动、退出或切换桌面代理软件。`scripts/serve.mjs` 是便于审阅的静态开发服务；生产行为以 Wrangler / Cloudflare 为准。

## 上线

1. `pnpm exec wrangler whoami --json` 检查账号。尚未登录时，可执行 `pnpm exec wrangler login --device --browser=false`，由账号所有者在官方页面授权；不要在聊天、Git 或站点文件中放入 Token。
2. 在 Cloudflare 中确认 `cmmuu.com` 为正确账号的有效 zone。核对 `serylane.cmmuu.com` 的**准确 DNS 记录、Workers 自定义域名绑定和现有用途**。公共 DNS 返回 Cloudflare 地址也可能来自泛解析，不代表该子域名空闲。
3. 提交并推送已审阅的源码至 GitHub `main` 后，运行 `pnpm run deploy`（必须显式写 `run`，避免误用 pnpm 的同名工作区命令）。脚本仅上传版本并切换现有 Worker 的流量；不会修改根域名、其他网站或 DNS。
4. 验收主页与四篇文档 HTTPS 200，未知路径 404、`.html` 别名归一化、robots、sitemap、canonical 和下载链接。成功后才更新仓库 About 的 Homepage、README 的部署状态。

注意：Wrangler 的直接 `deploy` 命令可能处理并覆盖自定义域名绑定，因此 `pnpm run deploy` 已封装为严格的版本上传／部署脚本。不要绕开脚本直接执行 `wrangler deploy` 或 `triggers deploy`。首次域名绑定不属于普通页面发布流程；CLI 升级后须重新核查行为。

GitHub Actions 的 `Website checks and deployment` 对官网源码推送／PR 执行检查；受信任 `main` 的官网改动推送、成功的本仓库正式构建工作流完成，以及主分支手动触发可部署。源码推送与正式发行后的部署不依赖旧镜像同步成功。兼容的旧同步／展示元数据工作流也可调用官网部署；生产部署串行并只检出最新受信任 `main`，PR 检查不使用生产凭据。账户专用 `CLOUDFLARE_API_TOKEN` 存入该仓库的 Actions Secrets，仅需目标账户的 Workers Scripts 编辑权限，不授予 DNS／域名路由编辑权限。缺失凭据会明确失败，不能把 dry-run 或源码同步成功当成网站已上线。

部署生成不入库的 `/build-info.json`，记录源码提交、website 树及公共页面摘要。上传后再次确认 `main` 未变化，再激活准确版本；回读线上标记和实际 HTML／JS／CSS／文档字节，并验收六个架构的最新下载跳转。校验失败会阻止报告同步成功，但不会擅自回滚或重写域名。若上传期间 `main` 已更新，停止激活，由新一轮同步部署最新源码。

每次功能变更还需人工／开发代理同步审阅官网功能文案、使用文档、README 和发布说明；自动部署负责发布已提交的内容，不会自动推断或编写新功能介绍。持续约束见根目录 `AGENTS.md`。

对于已经绑定正确域名的官网，更新静态资源优先使用 `wrangler versions upload --keep-vars`，再将返回的准确版本 ID 以 `wrangler versions deploy <VERSION_ID>@100` 上线；先执行 `versions upload --dry-run`。已核对 Wrangler 4.129.0：该路径不运行域名／路由触发器部署，不需要再次绑定域名。不要使用 `triggers deploy` 或覆盖选项来发布普通页面更新。新部署后再次核对公共页面和下载链接，记录版本 ID 以便回滚。

## 动态下载与发布契约

以下描述当前源码；是否已部署以线上 `build-info.json` 和实际六个下载入口的验收结果为准。页面主体为静态资源，只有 `/api/*` 与 `/download/*` 进入 Worker。无数据库、登录、分析脚本、代理探测或桌面接管。

- 安装包和版本信息**仅使用下载中心**。官网主按钮、无 JavaScript 链接及文档均使用同源 `/download/{target}`，覆盖 Windows/macOS/Linux × x64/arm64。Mac 明确区分 Intel 与 Apple 芯片（M 系列）。
- `GET /api/releases/latest` 每次从固定地址 `https://downloads.cmmuu.com/api/projects/serylane/releases/latest` 获取清单；不从其他发行站点重新解析、不读取旧缓存。要求 schemaVersion 1、项目 serylane、正式版本、60 秒内 checkedAt、完整六架构、准确文件名、正整数大小（最多 2 GiB）和 SHA-256。
- `GET /download/{target}` 在重新获取清单后，对所选安装包进行 HEAD 校验：状态 200、大小匹配、非文本/JSON/HTML 类型、无重定向。只有校验成功才 302 到下载中心固定文件域 `https://files.cmmuu.com/releases/serylane/{version}/{filename}`。前端使用 `GET /api/download/{target}` 执行同一过程并显示结果，随后交给浏览器下载。
- 下载中心协议保留内部 `channel: "hk"` 与可用性字段；页面统一称为“下载中心”。发布来源的 `source` 字段可以记录源码发行来源，但安装包的 channel 与 fileUrl 必须属于下载中心。拒绝备用渠道、其他主机、带凭据/参数/片段的 URL、错误版本和错误架构。
- 旧 `?channel=github`、`?channel=gitee` 及其他查询参数返回 400，不进行上游请求。未知目标返回 404，非 GET/HEAD 返回 405；HEAD 响应没有正文。
- 中心查询或文件校验失败返回 503 和 `Retry-After: 30`，页面只提示稍后重试。没有渠道切换、旧版本回退或备用下载入口。所有动态响应均为 `no-store`，每次点击重新核对，长期开着的页面不会固定旧包。
- 元数据最多 256 KiB，查询超时 8 秒，文件 HEAD 超时 5 秒。仅访问固定中心 API 和精确不可变文件 URL，禁止跟随重定向。日志仅包含固定内部诊断代码，不包含访问者数据、上游正文或凭据。
- 清单中的摘要与包身份来自下载中心已核验的发行契约；网站不重新下载完整包计算哈希。发布同步负责逐包哈希验证，客户端更新仍独立验签、核对大小和 SHA-256。浏览器开始传输后，官网不能保证持续连通或接管续传。
- “最新”指下载中心本次返回且通过全部校验的正式发行版。源码提交不等于安装包发布；其他仓库有新版本但中心未就绪时不绕开中心。

### 新版本接入与上线检查

发布流程在主包齐全和更新签名校验通过后生成 `downloads.json`，包括六个目标的 filename/size/sha256，清单纳入 `SHA256SUMS.txt`。下载中心同步验收后才提供最新正式版。官网不硬编码发行版本，不需要为每次版本号单独改页面。

历史安装包真实文件名、URL、摘要和签名不改写；历史品牌文件名仅在精确身份计算中保留，不作为页面文案。仓库链接用于查看源码、变更记录和提交问题，不是另一个下载渠道。

`scripts/deploy-site.mjs` 验收最新清单、六个同源中心专用架构入口的版本/来源/目标 URL，以及已停用渠道参数返回 400。验收使用 HEAD，不下载安装包正文；失败不能报告已完成上线。中心项目页是独立服务，其代码及部署不属于此官网目录，官网自身不依赖中心页面的跳转策略。

### 测试

`pnpm test` 覆盖六个架构、Mac 芯片标签、键盘选择、加载/失败/重试、无 JavaScript 链接，以及点击时重新核对和拒绝其他源。Worker 测试覆盖更新切换、过期/不完整/超大元数据、中心不可用、错误版本/架构/摘要/大小、外部地址、重定向、HEAD、错误隐私和无缓存。部署检查测试验证六个中心专用入口和旧参数拒绝。

`pnpm typecheck` 生成 Cloudflare 运行时类型并检查 TypeScript。渲染变更还需本地桌面与手机浏览器检查，记录错误、交互和截图；模拟成功不等于线上中心可用性验证。本地静态预览不提供动态接口，会显示可重试错误，真实 Worker 可用 Wrangler 本地预览。无需启动、重启或切换桌面代理应用。

## SEO 与安全

- 首页和四篇文档拥有独立标题、描述与精确 canonical；真实语义 HTML 可直接读取，不依赖 JavaScript 生成正文。
- 包含 robots、sitemap、Open Graph、Twitter 摘要卡及 SoftwareApplication JSON-LD，无虚构评分、下载量或稳定性指标。
- 404 返回真实错误状态并使用 noindex；`.html` 文档路径由 Cloudflare 归一化到无扩展 canonical。
- 严格 CSP：浏览器仅允许同源发布查询，不连接外部 API 或本地代理，不执行内联脚本或第三方脚本；网站没有登录、上传、支付或接管程序功能。
- 正文即时重新验证缓存，PNG 标志缓存一天；不对未带内容哈希的文件设置 immutable。
- 可被抓取不等于已被 Google 收录。上线后仍需域名所有者在 Search Console 验证站点并提交 sitemap；不得声称已提交或保证排名。
- 线上 robots 会叠加当前 Cloudflare 托管内容信号：普通搜索允许抓取，部分 AI 爬虫被阻止，项目 sitemap 仍保留。本次未修改这些区域级安全/爬虫设置。

## 设计与素材

延续批准稿的冰白底色、蓝紫环境光、磨砂玻璃、深色中文和统一线性图标。下载版本/中心状态提示及准确安全文案是相对概念稿的有意调整。页面中的路由界面是原生代码构建的**只读示意**，不代表实时连接状态。

`public/assets/serylane-mark.png` 是仓库 `assets/brand/serylane-icon.png` 经 Tauri 官方图标转换生成的 256×256 PNG；`public/favicon.png` 为同源 32×32 PNG。新品牌源图来自内置 image_gen 对批准标志的提取，完整来源及提示词见 `assets/品牌素材来源.md`。没有使用整张设计图充当网页，也没有把生成资产留在外部临时路径供生产依赖。
