# 代理质量验收

使用 Node.js 20 或更新版本。工具只计算提供的脱敏证据。采集、故障注入和出口核验在隔离 Windows 镜像及受控目标完成。

```powershell
node --test tests/proxy-quality.test.mjs
node scripts/proxy-quality.mjs --fixture
node scripts/proxy-quality.mjs evaluate path/to/trial.ndjson
node scripts/proxy-quality.mjs evaluate path/to/trial.ndjson --require-excellent
```

`--fixture` 使用合成观测验证十项计算，整份结果固定标为 `fixture-only`。`evaluate` 对每个指标输出 `meets`、`fails`、`insufficient` 或 `missing`。默认输出报告即可成功退出。`--require-excellent` 仅在输入为 `observed` 且十项均有足够证据并达到目标时退出 0，其他有效报告退出 1，无效输入退出 2。阈值由 `proxy-quality.mjs` 中的 `excellent` 导出。

## 隔离测试口径

一份文件只包含同一网络模式的观测。系统代理与 TUN 分开出报告。真实模型流、受控协议流和合成数据分别记录。对照比较固定系统镜像、订阅、规则、目标、模型、传输、出口和负载。测首文本开销时用同出口的受控流定时发送，候选组与对照组交替测试。比较桌面客户端时先比较默认配置，再比较同核心和同策略配置。

| 指标 | 优秀线与证据 |
| --- | --- |
| 启动就绪 | 至少 20 次。发出启动指令到控制器可用且经预期出口成功，P95 ≤2 秒。未就绪或未到预期出口记失败。 |
| 恢复 | 正常停止、启动失败、崩溃后恢复、第三方接管全部在 3 秒内符合预期。以显式停止或恢复器开始为时间起点，核对系统配置及原所有者。 |
| 分流 | 域名、进程、直连、代理、阻断、IPv6、DNS、规则冲突、故障场景均正确，禁止直连的流量零绕行。规则链与受控服务看到的出口要能交叉核验。 |
| WSS | 新连接首次尝试在 10 秒内完成协议握手的比例 ≥99.9%。HTTP/1.1 记录 `101`，HTTP/2 记录成功的 Extended CONNECT。由采集端验证协议细节。 |
| 模型流 | 按 SSE 和 WSS 分别统计。有效首事件之后，匹配响应编号的 `response.completed` 比例 ≥99.9%。主动取消与首事件前失败另报。重试单独记录。 |
| 首文本 | 各传输至少 20 个完整配对。两组均测首个非空文本增量，取首次尝试的候选组与对照组差值，负值取零，再算 P95，要求 ≤100 ms。 |
| 意外断开 | 各传输每百连接小时 ≤0.5 次，分别至少记录 600 连接小时并检查统计证据。按实际已建立连接去重。主动或计划关闭单列。 |
| 故障恢复 | 至少 20 次。故障后新建的响应须在 180 秒观察时限内经核实的指定备用出口完整完成。故障至首事件 P95 ≤30 秒，未恢复记失败。 |
| 误切 | 连续至少 72 小时的完整健康观测内，自动切换已核实仍健康的出口共 0 次。手动、规则和预算动作独立记录。 |
| 折算流量 | 同订阅、同负载与同计量层，两组至少各 20 个成功逻辑任务。所有尝试的上传与下载乘已核实倍率后求和，除以成功任务数。候选组每任务折算量 ≤对照组的 1.10 倍。后台探针单独观测。 |

P95 取升序排列的第 `ceil(0.95 × n)` 项。WSS 和模型流成功率同时检查单侧 95% 二项证据。以 `independentId` 合并同一个故障窗口内相关的记录，任一记录失败即记为该试验单元失败。没有失败时，至少需要 2,995 个独立单元才能支撑 99.9% 的目标。意外断开使用单侧泊松计数检验，实际使用须核对观测条件和独立性。工具校验记录结构，独立性和受控观测条件由采集者确认。 [Google SRE 的 SLO 方法](https://sre.google/workbook/implementing-slos/)

## NDJSON 数据格式

每行一个 JSON 对象，首行必须为 `run`。其余记录均带 `id` 和 `arm`，`arm` 取 `candidate` 或 `control`。编号使用最多 96 个英数字及 `_.-`，首字符为英数字。同一类型、同一组的编号须唯一。所有绝对时间使用同一单调毫秒时基。`?` 表示字段可省略或填 `null`。最大输入 16 MiB。

```json
{"type":"run","id":"trial-a","schemaVersion":1,"source":"observed","networkMode":"system-proxy","comparisonId":"same-workload-and-meter"}
{"type":"startup","id":"cold-1","arm":"candidate","durationMs":1400,"controllerReady":true,"expectedEgress":true}
```

| `type` | 额外字段 |
| --- | --- |
| `run` | `id`、`schemaVersion: 1`、`source: fixture 或 observed`、`networkMode: system-proxy 或 tun`、`comparisonId?`。这行没有 `arm`。 |
| `startup` | `durationMs`、`controllerReady`、`expectedEgress`。后两者为布尔值。 |
| `restore` | `durationMs`、`stateMatches`、`ownerPreserved`、`scenario`。场景为 `stop`、`start-failure`、`crash-recovery`、`third-party-owner`。 |
| `route` | `scenario: domain/process/direct/proxy/block/ipv6/dns/conflict/fault`、`expected`、`actual?`、`proofId?`、`forbiddenDirect`。出口或阻断结果使用约定代号。 |
| `wss` | `sessionId`、`attemptNumber`、`reused`、`durationMs`、`protocol: http1 或 http2`、`status?`、`negotiated`、`independentId?`。 |
| `attempt` | `taskId`、`transport: sse 或 wss`、`startMs`、`endMs`、`firstEventMs?`、`firstTextMs?`、`outcome`、`responseId?`、`terminalResponseId?`、`terminalEvent?`、`independentId?`、`pairId?`、`comparisonId?`、`egress?`、`egressProof?`、`faultId?`、`txBytes?`、`rxBytes?`、`multiplier?`。 |
| `task` | `outcome: completed/failed/cancelled/unknown`、`comparisonId?`。编号对应同组请求的 `taskId`。 |
| `connection` | `transport: sse 或 wss`、`durationMs`、`outcome: normal/unexpected/planned/cancelled/unknown`。编号对应已经建立的独立连接。 |
| `fault` | `atMs`、`observedUntilMs`、`failedEgress`、`standbyEgress`、`injectionProof`。同组请求通过 `faultId` 关联。 |
| `switchWindow` | `startMs`、`endMs`、`complete`。只在窗口内的健康和切换观测完整时置为真。 |
| `switch` | `atMs`、`windowId`、`cause: automatic/manual/rule/budget`、`previousHealth: healthy/unhealthy/unknown`、`healthProof?`。 |

`attempt.outcome` 可取 `completed`、`interrupted`、`timed-out`、`cancelled`、`service-error`、`request-failed`。成功要求 `terminalEvent: response.completed`、`terminalResponseId` 与 `responseId` 匹配，并有首事件时间。其他可用终态为 `response.failed`、`response.incomplete` 和 `error`。 [OpenAI 流式转发要求](https://learn.chatgpt.com/docs/enterprise/gateway-compatibility#streaming)

- `negotiated` 由握手采集端给出。HTTP/1.1 要验证 Upgrade、Connection 和 Sec-WebSocket-Accept，HTTP/2 要验证 Extended CONNECT 成功。只有新连接的 `attemptNumber: 1` 进入首次成功率。 [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html#section-4.1) · [RFC 8441](https://www.rfc-editor.org/rfc/rfc8441.html#section-5)
- `egress` 与 `egressProof` 只在连接标识和受控出口都可以核对时同时填写。同一出口证据只对应一个独立试验单元。时间相近的 Mihomo 连接快照与 Codex 日志不足以归因，留空即可。报告中的分出口完成量只汇总带有效证据的记录。 [Mihomo 连接 API](https://wiki.metacubex.one/en/api/#connections)
- 两组的首文本尝试用相同 `pairId`。首文本与折算流量的行使用首行声明的 `comparisonId`，该代号标记同负载、同计量层和同订阅。记录各次尝试的上下行字节和已核实倍率。任务终态、字节、倍率或对照不全时，成本不计为已达标。
- 故障记录在观察窗口结束或已得到完整恢复结果后提交。备用响应必须在故障后新建，命中指定出口并有证据，截止前完成。一个故障仍未恢复会阻止该指标达标。
- 文件只存匿名代号与计时、计数、结果。采集前移除请求内容、认证、订阅地址和真实连接标识。额外字段会被拒绝。
