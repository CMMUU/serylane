# Serylane 官网

基于已确认的三段磨砂玻璃设计实现，原生 HTML / CSS / JavaScript，无框架、第三方字体、分析脚本或客户端构建依赖。Cloudflare Workers Static Assets 托管；上传 `public/` 和公开发布查询 Worker，不上传桌面程序源码、用户配置或密钥。

官网：<https://serylane.cmmuu.com/>。已于 **2026-09-08** 部署至 Cloudflare Workers Static Assets，首页与四篇文档 HTTPS 200、未知路径 404、`.html` 归一化 307、robots 和 sitemap 均已通过线上请求验收。首次边缘访问曾短暂超时，复核后上述页面均正常；本记录不代表持续可用性监控或搜索引擎已收录。

首次上线版本 ID：`3fdb1fbf-ace8-43dc-8fec-5e9aa1c96c4b`。先上传未绑定域名的新 Worker，再核对域名变更预检只包含该 hostname 的一个新增项、无更新/移除/冲突，最后以三个覆盖开关均为 `false` 绑定域名。回读确认指向 `serylane-website`，未替换已有 DNS 或其他网站。域名绑定会新增 Cloudflare 管理的 DNS 与证书。

v0.7.6 官网更新已于 **2026-09-08** 上线，版本 ID：`ab83f410-fa93-422a-9394-dfff107e4707`，100% 流量；通过版本上传／部署完成，未重建或覆盖域名与 DNS。线上主页、`site.js`、`/docs/` 实际 GET 成功，v0.7.6 下载链接、国内缺失回退与唯一订阅路径均已核对；静态与下载交互离线检查通过。线上 IAB 复核连续超时，不将其记录为本次视觉验收通过。

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

GitHub Actions 中的 `Sync GitHub to Gitee` 在每次成功的非预演同步后，调用官网检查、部署和线上验收。官网 PR／push 检查本身不使用生产凭据；正式部署只检出受信任的最新 `main`，并串行执行。手动运行 `Website checks and deployment` 也可部署。账户专用 `CLOUDFLARE_API_TOKEN` 存入该仓库的 Actions Secrets，仅需目标账户的 Workers Scripts 编辑权限，不授予 DNS／域名路由编辑权限。缺失凭据会明确失败，不能把 dry-run 或源码同步成功当成网站已上线。

部署生成不入库的 `/build-info.json`，记录源码提交、website 树及公共页面摘要。上传后再次确认 `main` 未变化，再激活准确版本；回读线上标记和实际 HTML／JS／CSS／文档字节，并验收六个架构的最新下载跳转。校验失败会阻止报告同步成功，但不会擅自回滚或重写域名。若上传期间 `main` 已更新，停止激活，由新一轮同步部署最新源码。

每次功能变更还需人工／开发代理同步审阅官网功能文案、使用文档、README 和发布说明；自动部署负责发布已提交的内容，不会自动推断或编写新功能介绍。持续约束见根目录 `AGENTS.md`。

对于已经绑定正确域名的官网，更新静态资源优先使用 `wrangler versions upload --keep-vars`，再将返回的准确版本 ID 以 `wrangler versions deploy <VERSION_ID>@100` 上线；先执行 `versions upload --dry-run`。已核对 Wrangler 4.129.0：该路径不运行域名／路由触发器部署，不需要再次绑定域名。不要使用 `triggers deploy` 或覆盖选项来发布普通页面更新。新部署后再次核对公共页面和下载链接，记录版本 ID 以便回滚。

## 动态下载与发布契约

已于 **2026-09-10** 使用版本上传／部署上线，当前 Worker 版本 `e032654d-5513-40f7-96db-6abbac00b55b`（100%），未更改域名或 DNS。线上动态查询、Gitee 主下载、同版本 GitHub 回退及独立 GitHub 入口已经实测；桌面 1440px、手机 390px、无 JavaScript 及模拟查询失败／重试完成检查。上游在验收期间存在间歇性超时，不能将一次渠道验证当作长期可用保证；有限诊断代码通过 `X-Serylane-Error` / `X-Serylane-Mirror` 返回，不包含请求内容或签名下载 URL。

页面主体继续使用静态资源；只有 `/api/*` 与 `/download/*` 先进入 Worker，其他路径由 Static Assets 处理。没有数据库、登录、分析脚本、代理探测或桌面接管。

- `GET /api/releases/latest`：查询当前最新正式版、六个主包的名称/大小/SHA-256、各包香港/Gitee 渠道的元数据与 HEAD 检查结果（hkAvailable、giteeAvailable 独立，domesticAvailable 为任一可用）。页面展示可能早于下一次发布，因此只用于说明。
- `GET /download/{windows|macos|linux}-{x64|arm64}`：**每次点击重新解析** GitHub 官方 `releases/latest` 的正式版跳转，固定该标签，核对发布清单和校验清单。香港下载中心与 Gitee 同标签清单分别核验且所选包 HEAD 大小匹配才可 302，优先香港；均未就绪才检查并转向同版本 GitHub 包。
- 加上精确参数 `?channel=github` 可跳过国内渠道。无 JavaScript 时仍可使用 Windows x64 的动态入口。
- 查询、跳转、错误响应均为 `no-store`；不读旧版本缓存，不把旧包冒充最新版。未知目标/参数被拒绝，不接受用户传入下载 URL。
- 元数据最多 256 KiB；固定 HTTPS 上游/发行 CDN 白名单、最多五跳；查询总超时 12 秒，香港/Gitee 并行检查各 4 秒，GitHub 包 HEAD 5 秒。安装包交由浏览器从官方渠道下载，Worker 不下载或缓存大包。
- GitHub 最新标签通过公开跳转解析，不依赖匿名 REST API 的共享配额；仍受上游网络、服务可用性及其自身发布元数据传播影响。“最新”指本次查询时 GitHub 指定的正式发行版，未发布源码不计入。
- 渠道检查是版本/清单字节/文件大小检查，不是每次点击重下安装包验算哈希；镜像流水线负责逐包哈希验证。浏览器已经跳转、开始传输后，不能跨渠道无缝续传；可返回选择 GitHub 备用。

### 新版本自动接入

`scripts/publish_github_release.py` 从 v0.7.7 起，在主包齐全且更新签名校验通过后生成 `downloads.json`（schemaVersion、v 前缀版本、六个目标的 filename/size/sha256）。该清单纳入 `SHA256SUMS.txt`；全部附件上传并验证后才将 draft 发布为正式版。官网不需要修改版本号或重新部署。

macOS 页面和文档明确区分 Intel 芯片（x64）与 Apple 芯片（M 系列，ARM64），无 JavaScript 时也有各自的动态下载链接。Releases 标题及安装包 `label` 使用 Serylane；历史二进制真实 `name`、URL、摘要和签名不变。需要整理既有发布记录时运行 `Release display metadata` 工作流，默认预演；确认应用后仅更新 GitHub/Gitee 的展示元数据，不创建、删除或重新上传附件。Gitee 不支持独立附件显示标签，历史真实文件名仍保留以兼容更新。

Gitee 同步把 `downloads.json` 与五份更新清单放在所有普通附件验证完成后的屏障后上传。清理旧附件时先撤下清单。镜像没同步完时官网使用 GitHub **同版本**，不使用旧的 Gitee 最新版。此变更不修改保留历史的范围或触发新应用版本发布。

已正式发布的 v0.7.6 没有新清单，因此兼容读取其原始 `latest.json` 和 `SHA256SUMS.txt`；DMG 大小通过 GitHub HEAD 补齐。不改历史附件、文件名或签名。从下一版起缺少新清单视为未就绪，返回 503。

### 测试

`pnpm test` 覆盖六个系统/架构组合、键盘选择、异步加载/失败/重试、无 JavaScript 链接；Worker 测试覆盖发布切换、镜像滞后/部分可用/超时/清单不一致、错误大小、缺包、预发布、恶意跳转和无缓存响应。`pnpm typecheck` 生成 Cloudflare 运行时类型并检查 TypeScript。Python 发布与镜像测试验证清单生成和上传屏障。

本地静态预览不提供动态接口，会显示查询失败；真实接口测试用 Wrangler。本任务不安装或重启桌面应用，不切换系统代理。

## SEO 与安全

- 首页和四篇文档拥有独立标题、描述与精确 canonical；真实语义 HTML 可直接读取，不依赖 JavaScript 生成正文。
- 包含 robots、sitemap、Open Graph、Twitter 摘要卡及 SoftwareApplication JSON-LD，无虚构评分、下载量或稳定性指标。
- 404 返回真实错误状态并使用 noindex；`.html` 文档路径由 Cloudflare 归一化到无扩展 canonical。
- 严格 CSP：浏览器仅允许同源发布查询，不连接外部 API 或本地代理，不执行内联脚本或第三方脚本；网站没有登录、上传、支付或接管程序功能。
- 正文即时重新验证缓存，PNG 标志缓存一天；不对未带内容哈希的文件设置 immutable。
- 可被抓取不等于已被 Google 收录。上线后仍需域名所有者在 Search Console 验证站点并提交 sitemap；不得声称已提交或保证排名。
- 线上 robots 会叠加当前 Cloudflare 托管内容信号：普通搜索允许抓取，部分 AI 爬虫被阻止，项目 sitemap 仍保留。本次未修改这些区域级安全/爬虫设置。

## 设计与素材

延续批准稿的冰白底色、蓝紫环境光、磨砂玻璃、深色中文、统一线性图标与细圆角滚动条。下载版本/渠道提示及准确安全文案是相对概念稿的有意调整。页面中的路由界面是原生代码构建的**只读示意**，不代表实时连接状态。

`public/assets/serylane-mark.png` 是仓库 `assets/brand/serylane-icon.png` 经 Tauri 官方图标转换生成的 256×256 PNG；`public/favicon.png` 为同源 32×32 PNG。新品牌源图来自内置 image_gen 对批准标志的提取，完整来源及提示词见 `assets/品牌素材来源.md`。没有使用整张设计图充当网页，也没有把生成资产留在外部临时路径供生产依赖。

## 香港下载中心接入（本轮源码，部署验收单独记录）

香港文件源固定为 `https://files.cmmuu.com/releases/serylane/{tag}/{filename}`，管理和文件 Cookie 隔离。网站仅发小型清单 GET 和安装包 HEAD，再返回 302，不代理大文件。`downloads.json` 必须与 GitHub 权威当前标签的原始字节一致，文件大小也必须匹配；未归档、部分归档或历史版本不自动作为最新版。API 响应分别保留 `X-Serylane-HK` 与 `X-Serylane-Mirror` 诊断，HK 正常不代表 Gitee 已同步。

桌面更新使用 `https://downloads.cmmuu.com/api/releases/serylane/latest` 的 Tauri 静态格式清单，仅当前权威发行已完整归档才返回 200，待同步或上游当前版本不确定返回 503。新发行 `latest-serylane-hk.json` 与另外四份清单一起生成：版本、摘要、大小及签名相同，只有 URL 渠道不同。历史导入不回写 GitHub 原附件。自动更新按最高稳定版本选择、同版优先香港，实际下载回退仍要求版本/摘要/大小/签名全部一致；手动指定渠道不悄悄改用另一个渠道。

此源码接入不等于 HK 已部署，也不解除 GitHub/Gitee/官网完整发布验收要求。域名创建、文件导入、完整性/断点/六架构公网验收由下载中心部署流程记录。
