# 入站配对令牌鉴权：设计（开远程的前置条件）

> 2026-09-21 用户要求："设计入站配对令牌鉴权（开远程的前置条件）"。
> 这是**设计**，不是已完成的东西 —— 每条"现状"都标了代码出处，每条决策都写了代价。
>
> 上游背景：`docs/android-client-plan.md` §4 第 1 步已写下要点（`X-ComfyHub-Token`、默认关闭、覆盖 SSE）；
> 需求侧缺口记在 `docs/ai-home-requirements-audit.md:61/103/119`（AIH-016 收紧监听与 CORS、AIK-003 预留远程：
> 两条都只差"**认证**"这一块）。本文把它补全到可实施的程度。

---

## 1. 一句话

后端加一个**配对令牌**：默认**完全关闭**（回环 + 无鉴权，行为与今天逐字一致）；
用户在设置里打开"允许移动端连接"时生成一个随机令牌，此后**非回环来源**的请求必须带令牌，否则 `401`。

```
移动端 / 别的设备 ──── 局域网 (非回环) ────▶ 必须带令牌 ──▶ 后端 :8080
桌面 App / PS 脚本 / ComfyUI 节点 ── 回环 ──▶ 默认豁免 ──▶ 后端 :8080
```

---

## 2. 为什么这是"开远程的前置条件"，而不是洁癖

后端持有的是**能花钱、能读写磁盘、能驱动 ComfyUI** 的能力，不是一份只读数据：

| 资产 | 出处 | 落到别人手里的后果 |
| --- | --- | --- |
| AI Provider API Key（DPAPI 加密落盘） | `ai/CredentialService.kt:29`，`<storage>\ai\credentials.dpapi.json` | 后端能代替用户**调用上游、花用户的钱** |
| 文件读写（`read_file` / `write_file` / `list_dir`） | `ai/tools/ToolRegistry.kt`，写只允许 `<根>\comfyui` | 读用户目录里的工作流；写只在白名单内（这条是好的） |
| ComfyUI 提交与产物入库 | `ComfySubmitter.kt:69/352`（在 `com/comfyhub/` 根下，不在 `ai/` 子包里）、`capture.captureRun()` | 白烧显卡、往画廊里塞东西 |
| 官方 comfy-mcp 的 39 个工具 | `ai/mcp/McpToolBridge.kt` | 与上一条同类，面更大 |
| 媒体原件（含 NSFW 标记的内容） | `MediaRoutes.kt:230/239/259`、`AiRoutes.kt:995/1009` | 用户的私人生成物被读取 |

而今天**整条 API 一个 token 都没有**：`Application.kt:312-392` 装的插件是
DefaultHeaders → CallLogging → ContentNegotiation → Compression → PartialContent → CORS → StatusPages，
**没有 Authentication**；`server/build.gradle.kts:33-64` 里也没有 `ktor-server-auth`
（构建产物 61 个 jar 中 0 个含 auth）。全仓 grep `authenticate(|Principal` 零命中，
命中的 `Authorization` 全是**出网**（`ai/AiUpstream.kt:115`、`ai/HarnessRunner.kt:1104`、`ai/tools/CivitaiSearch.kt:330`）。

默认只绑 `127.0.0.1`（`Config.kt:57`）挡住了大部分风险，但 `COMFYHUB_ALLOW_REMOTE=1`
一开就是 `0.0.0.0`（`Config.kt:55-57`）—— 同一个 Wi-Fi 下**任何设备**（包括别人的手机）都能直接调。
`Application.kt:305-310` 现在对非回环监听只打一句 warn，**不拦请求**。

---

## 3. 设计约束（从代码事实推出来，不是喜好）

这四条决定了方案长什么样，**改设计前先读这一节**：

### C1. 桌面 App 有 11 处主动请求 + 11 处被动取字节，其中一部分**根本带不上头**

- 主动请求 11 处：`ApiClient` 5（`lib/core/api_client.dart` 的 `_get:71`、`_send:79`、`mediaPrompt:240`、`uploadMedia:284`、`_workflowText:370`）、
  `AiApiClient` 5（`ai_api_client.dart` 的 `_get:46`、`_send:50`、`uploadAttachments:285`、`promptWorkflow:319`、`runEvents:435`）、
  `backend_launcher.dart:116`（健康探测，裸 `http.get`）。
- 被动取字节 11 处，**没有任何注入头的地方**：
  `Image.network` 5 处（`media_thumb.dart:227`、`ai_home_page.dart:2243/2247/3536/3591`，它有 `headers:` 参数但都没传）、
  `NetworkImage` 2 处（`zoomable_image_view.dart:100/110`，**这个类不接受 headers**）、
  视频 2 处（`win_video_view.dart:53/226`，`networkUrl` 无 header 入口）、
  交给系统外部程序打开 2 处（`media_detail_page.dart:149-151`、`ai_home_page.dart:3616-3618`，**连 URL 之外的通道都没有**）。

→ **推论**：如果"所有请求都要令牌"，媒体（图片/视频/原件下载）会**一部分坏、一部分没法修**。
所以令牌的**强制范围必须与"来源"绑定**，并且媒体要有"能把凭据放进 URL"的通道（见 D5）。

### C2. 本机脚本与第三方客户端**一个头都不带**

- 探活：`autorun-app.ps1:62`、`server.ps1:177`、`comfyhub.ps1:231` 都是裸 `Invoke-RestMethod .../api/health`。
- 三条 e2e（`e2e-capture-test.ps1:55-64`、`e2e-submit-test.ps1:79-84`、`e2e-ai-tools-test.ps1:136-145`）
  的 `Invoke-Api` 只设 `Content-Type`；SSE 客户端只设 `Accept`（`e2e-ai-tools-test.ps1:188`）。
- ComfyUI 自定义节点 `comfyui/comfyhub_capture/capture_core.py:359-378` 只用 `urllib` + `Content-Type`。

→ **推论**：这些调用方今天**全部来自回环**（脚本在本机跑、节点装在用户的 ComfyUI 上）。
"回环豁免"能让它们**零改动**；而一旦用户的 ComfyUI 在**另一台机器**（跨机捕获），
它就成了非回环调用方，必须能配令牌（见 D11 与 §7 的连带影响）。

### C3. SSE **不是** `EventSource`（这点与 `android-client-plan.md` 的假设不同）

`lib/core/ai_api_client.dart:434-471` 是 `http.Request` + `..headers['Accept'] = 'text/event-stream'` + `_client.send(req)`，
再手工按行分帧 —— **完全可以自定义请求头**，而且 URL 里已经有 `?after=` 这种查询参数的先例。
后端那一个 SSE 端点是 `AiRoutes.kt:551`（`call.respondTextWriter(ContentType.Text.EventStream)`，15s 心跳 `:569-574`），
全仓只此一个。

→ **推论**：SSE 用 header 就能覆盖，`?token=` 只是"给未来的浏览器端/curl 留的门"，不是必需。

### C4. 现有的"秘密"设施**故意不让你读回明文**

`CredentialService`（`ai/CredentialService.kt:29`）的 `describe()` 只回 `configured/source/writable`（`:47-55`），
唯一能读回明文的 `resolve()` 被注释限定为"仅后端内部使用"（`:19-20`、`:79`）。
而本需求明确要求令牌**在设置页可见、可轮换**。

→ **推论**：配对令牌**不能**塞进 `CredentialService`（除非破坏它那条纪律），要走 `app_settings`（见 D3）。

---

## 4. 决策（每条都带代价）

### D1. 令牌只在**非回环来源**上强制；回环默认豁免 —— 这是本设计的支点

判据用 `call.request.origin.remoteHost ∈ {127.0.0.1, ::1, 0:0:0:0:0:0:0:1, localhost}`。
Ktor 没装 `XForwardedHeaders`，所以 `origin.remoteHost` 是**socket 对端地址**，不会被 `X-Forwarded-For` 伪造（这点必须保持）。

- **换来什么**：桌面 App（C1）、本机脚本（C2）、ComfyUI 节点（C2）**一行都不用改**；
  默认关闭时行为与今天逐字一致 —— 正是 `android-client-plan.md:82` 要的"保证桌面 App 不受影响"。
- **代价（必须写出来）**：任何能读库的人都能读明文令牌，但能读库的人本来就能读 `credentials.dpapi.json` 的密文、
  能改 `memory.md`、能跑脚本 —— 只保护回环访问对这类攻击者没有意义。
- **代价（更要写出来）**：**端口转发会伪装成回环** —— `netsh portproxy`、`adb reverse tcp:8080 tcp:8080`、
  同机反向代理，都会让远程流量以 `127.0.0.1` 出现在后端眼里，从而**绕过令牌**。
  所以配一个开关 `COMFYHUB_AUTH_LOOPBACK_EXEMPT=0`（默认 `1`）：关掉后**回环也要求令牌**。
  这时 C1/C2 都要改（桌面 App 加头、脚本 `-Headers`、媒体走签名 URL），
  是"更严但要动更多地方"的选项，**不设为默认**。

### D2. 不引入 `ktor-server-auth` 依赖

我们的校验是"一个固定令牌 + 常数时间比较 + 两个取 token 的位置（header / query）"，
用 Ktor 的 `Authentication` 插件要多一个依赖 + 4 个 jar，收益是零。
实现：一个 `InboundAuth.kt`，在 `Application.kt` 的 `install` 链末尾、`routing { }` 之前装上
（`Application.kt:392` 与 `:394` 之间）：

```kotlin
install(createInboundAuthPlugin(cfg, authStore))   // 伪代码：内部 intercept(ApplicationCallPipeline.Plugins) { ... }
```

放在 `StatusPages` 之后是有意的：401 也走统一错误形状；
放在 `routing` 之前，才能在所有路由（含 `respondFile` 的媒体端点）之前短路。

### D3. 令牌存 `app_settings`（明文），**不进** `CredentialService`

键：`auth.pairing`，值 JSON（形状见 §5）—— 与 `capture.config` / `ai.nsfw` / `ai.tools.policy`
同一个套路（`SettingsRepo.get:21` / `put:24`，表 `app_settings(k,v)`：`db/schema.sql:159-165` + `Migrate.kt:63-74`）。

**为什么破例存明文**（对应 C4）：

1. 需求是"界面可见" —— 人要能抄下来填进手机；
2. 令牌的价值**仅限**"局域网内访问本机后端"，它**不是上游 API Key**，不能拿去花钱；
3. DPAPI 防的是"文件被整个拷走后在别的机器上解密"，对本场景防的是同一类攻击者中的一个子集。

**代价**：任何能读 MySQL 的人能看到它。缓解：界面/日志/接口都只给**分段显示**（如 `a1b2-…-9f0e`），
只有"点显示"或"复制"时才回完整值；`GET` 明文接口本身要过 D4。

### D4. 明文令牌**只回给可信请求**

否则 `GET /api/auth` 等于把钥匙挂在门上给所有能连上的人看。
规则：只有在**回环**、或**已带正确令牌**的请求上，响应里才含 `token` 字段；否则该字段为 `null`。

### D5. 媒体字节走"**URL 里的短期签名**"，不是把长期令牌塞进查询串

对应 C1 里那些带不上头的取字节点（`NetworkImage`、视频、外部程序打开）。三个选项：

| 方案 | 图片 | 视频 | 外部打开 | 代价 |
| --- | --- | --- | --- | --- |
| 全都要求 header | ✗（`NetworkImage` 不支持） | ✗ | ✗ | 不可行 |
| 给 `Image.network` 加 `headers`，其余豁免 | ✓ | ✗ | ✗ | 视频与原件仍裸奔 |
| **签名查询串 `?t=<HMAC>&e=<exp>`** | ✓ | ✓ | ✓ | URL 会进日志/历史 |

选第三个，但**不是把令牌直接放 URL**，而是放一个**无状态签名**：

```
t = base64url(HMAC-SHA256(secret = 令牌, msg = "<path>|<exp>"))     e = <exp>（unix 秒）
校验：签名匹配 && now < exp（TTL 建议 5 分钟）&& path 与签名时那个一致
```

- `secret` 就是配对令牌本身（**不需要额外存东西**，也不需要 DB 会话表）；
- 前端在渲染前向后端要一次签名 URL（或后端在返回 `mediaIds` 的 DTO 里**直接给带签名的 URL**——更省事：
  `MediaRepo.kt:50-51` 已经在后端算 `fileUrl`/`thumbUrl` 了，只要在"非回环 + 令牌启用"时把签名拼上）；
- 长期令牌**永不进 URL**；过期 URL 即使被日志记住也没用。

**代价**：多一个"URL 里带签名"的概念（约 40 行 Kotlin）；`MediaRepo` 现在是无状态拼字符串，
要变成"知道当前令牌状态"（注入一个 `UrlSigner`）。
**P0 先不做**（P0 只保回环 + 令牌闸门），**P1 做** —— 因为只有移动端开始看图时才真的需要。

### D6. SSE 用 header（C3），但**同时接受** `?token=`

`HttpClient`/`Invoke-WebRequest` 类客户端带 header；未来若有浏览器端 `EventSource`（它不能设头），
查询串是**唯一**出路。两个都收，不冲突。

### D7. 绑了 `0.0.0.0` 但没启用令牌 → **默认拒绝非回环请求**（不是"裸奔放行"）

这条把 `Config.kt:55-57` 的"显式开关"升级成真正的安全默认：
`COMFYHUB_ALLOW_REMOTE=1` 只负责**监听**；**能不能被非回环访问**由令牌状态决定。
没启用令牌时，非回环请求一律 `403`（`401` 保留给"有令牌但不对"，`403` 给"根本不允许远程"），
启动日志把 `Application.kt:305-310` 的 warn 改成明确的"已监听 0.0.0.0，但未启用配对令牌 → 非回环请求会被拒绝"。

**代价**：现在有人靠 `COMFYHUB_ALLOW_REMOTE=1` 裸用（例如临时用手机浏览器看画廊）会**变红**——
这正是要发生的事；文档与 doctor 都要给出"那就去设置里开令牌"的一步指引。

### D8. CORS 白名单要加 `X-ComfyHub-Token`

`Application.kt:337-364` 现在 `allowHeader` 只有 `Content-Type` / `Authorization` / `Range`。
浏览器场景（P2 的二维码页/调试页）不带这个头就会被预检拦掉。**一行改动，但漏了就"接口明明对却通不过"**。

### D9. 常数时间比较 + 失败限速

- 比较用 `MessageDigest.isEqual(a, b)`（字节数组），**不要**用 `==`/`equals`；
- 失败计数：同源地址每分钟 > 20 次错误令牌 → `429`（内存计数器即可）。
  32 字节随机令牌本来就暴力不了，这条主要是**防日志刷屏**与"存在感扫描"。

### D10. 接口形状（挂在 `/api/auth`，独立于 `/api/ai`）

放 `AuthRoutes.kt`，在 `Application.kt:394-432` 的 `route("/api") { ... }` 里加一行 ——
"入站鉴权"不是 AI 功能，别塞进 `AiRoutes.kt`（那个文件已经 56 个路由了）。

| 方法 | 路径 | 语义 |
| --- | --- | --- |
| `GET` | `/api/auth` | `{enabled, loopbackExempt, createdAt, rotatedAt, lastUsedAt, token?}`（`token` 仅按 D4 回） |
| `PUT` | `/api/auth` | `{enabled: bool}` —— 打开时**若还没有令牌就顺手生成一个**；关闭不清除令牌（下次打开还是它） |
| `POST` | `/api/auth/rotate` | 生成新令牌，**旧令牌立即失效**（无宽限期，见 RSK-E） |

沿用现有 DTO 纪律：**只回事实、不回秘密**（除了 D4 允许的那一个字段）。

### D11. 令牌格式与生成

- 生成：`SecureRandom` 32 字节 → `base64url`（43 字符，无 `+/=`，可直接进 URL/二维码）。
- 展示：界面按 4 字符分段（`a1b2-c3d4-…`），**复制时给完整值**；
- **只生成、不允许手改**：给了输入框，用户迟早会设 `123456`；
- 轮换按钮 = `POST /api/auth/rotate`，界面上要写明"手机上要重新填"。

### D12. 跨机 ComfyUI 节点（连带影响，必须落到文档）

用户把 ComfyUI 装在**另一台机器**时，`/api/ingest/comfyui`（`CaptureRoutes.kt:147`）来自非回环 → 需要令牌。
节点侧现在只认 `COMFYHUB_URL` / `config.json` 的 `comfyhubUrl`（`capture_core.py:128-134`）——
**要加一个 `comfyhubToken` / `COMFYHUB_TOKEN`**（一个请求头，三行 Python）。
这条在 P0 里只是"文档里写清楚"，P1 动 `comfyui/comfyhub_capture/`。

---

## 5. 状态与数据形状

```jsonc
// app_settings.k = 'auth.pairing'
{
  "schema": "comfyhub-inbound-auth/1",
  "enabled": false,          // 总开关；false = 回环照旧、非回环一律 403（D7）
  "token": "…43 字符 base64url…",
  "createdAt": "2026-09-21T22:00:00+08:00",
  "rotatedAt": null,
  "lastUsedAt": null         // 收到正确令牌时更新（每次写库太频繁 → 内存里攒、最多 1 分钟落一次）
}
```

- `loopbackExempt` **不存库**，它来自环境变量 `COMFYHUB_AUTH_LOOPBACK_EXEMPT`（默认开）——
  这是"这台机器怎么跑"的事，不是"用户设置"。
- 读失败 / 键不存在 / JSON 坏 → **回落 `enabled=false` 且不抛**（与 `NsfwConfig.load():38-43` 同一纪律：
  配置坏了不能让后端起不来）。

---

## 6. 落地的三个阶段

### P0 —— 闸门本身（**开远程的必要条件**）

1. `Config.kt`：加 `authLoopbackExempt`（环境变量，默认 true）。
2. 新 `InboundAuth.kt`：`AuthState`（enabled/token）+ **纯函数** `decide(remoteHost, method, path, headerToken, queryToken) → Allow | Deny(reason)`。
   **把决策抽成纯函数是关键**：项目现在**没有任何 HTTP 层测试**（`server/src/test` 里 0 个 `testApplication`、
   没有 `ktor-server-test-host` 依赖、没有一处构造过 `AppConfig(`），
   引测试宿主是一笔新依赖；纯函数让 P0 的防线**零依赖**就能写满。
3. `Application.kt`：在 `:392`/`:394` 之间装插件；`:337-364` 的 `allowHeader` 加 `X-ComfyHub-Token`（D8）；
   启动 warn 改文案（D7）。
4. 新 `AuthRoutes.kt` + `AuthStore`（读写 `auth.pairing`）。
5. Flutter：设置页新卡片"**远程访问与配对**"（照抄 `settings_page.dart:740-813` 的 `_nsfwCard` 模板 +
   `_SwitchRow:1293-1330`）：开关 / 令牌显示与复制 / 轮换按钮 / 一句"手机上打开 App 时填这个"。
   —— 桌面 App 自己走回环，**不需要**在请求里带令牌（D1）。
6. 防线（见 §8）。

### P1 —— 真的能拿手机看图

- 签名 URL（D5）+ `MediaRepo` 注入 `UrlSigner`；
- CallLogging 打码（`Application.kt:316` 的 `format` 里把 `token`/`t` 查询参数换成 `***`）—— **上 P1 前必须先做**，否则日志里全是凭据；
- 移动端设置页（填 baseURL + 令牌）；
- ComfyUI 节点加 `comfyhubToken`（D12）。

### P2 —— 体验与加固

- 二维码配对：一次性 ticket（TTL 5 分钟，`POST /api/auth/pair` 换长期令牌）—— 省掉"手抄 43 字符"；
- 局域网发现（`android-client-plan.md` §4 第 5 步）；
- 每设备独立令牌 + 单独吊销（现在是一把钥匙全设备通用）；
- 审计：谁在什么时候配对成功（`auth.pairing.log`）。

---

## 7. 与既有需求条目的对应

| 条目 | 现状（审计原文） | 本文补上什么 |
| --- | --- | --- |
| **AIH-016** 收紧监听与 CORS | `◐ 部分实现`，"**没有任何 Authentication / 会话令牌拦截**"（`ai-home-requirements-audit.md:61`） | D2/D7/D8：拦截器 + 默认拒绝 + CORS 头 |
| **AIK-003** 预留 Android / 远程 | `◐`，只有"可配 Base URL + 显式开对外监听 + Android 默认地址"（`:103`） | D1/D10/D11 + P1 的移动端接入 |
| `android-client-plan.md` §4 第 1 步 | 要点已写（`:78-97`） | 补齐：回环豁免、媒体签名、默认拒绝、轮换语义、纯函数防线；**修正**它那句"`EventSource` 不能自定义 header"（C3：我们用 `http.Request`，能带头） |
| README `:1188-1190`"单用户、无鉴权（回环）" | 现状陈述 | 实施后改成"回环免鉴权、非回环需配对令牌" |

---

## 8. 防线（删了以后能不能抓住回归，是唯一标准）

| 层 | 写什么 | 为什么必须是这条 |
| --- | --- | --- |
| JVM 纯函数单测 | `InboundAuthTest`：回环豁免 / 非回环无令牌 401 / 令牌正确放行 / 令牌错误 401 / `?token=` 与 header 等价 / 关闭时非回环 403 / `loopbackExempt=0` 时回环也要令牌 | 这张表就是"安全边界"本身；**零新依赖** |
| JVM 单测 | `AuthStoreTest`：坏 JSON 回落 `enabled=false` 不抛；轮换后旧令牌不再匹配 | 配置损坏不能让服务起不来（承 `NsfwConfig` 纪律） |
| e2e | 新 `scripts/e2e-auth-test.ps1`：① 不带令牌的非回环请求 401；② 带令牌 200；③ 回环不带令牌 200；④ 轮换后旧令牌 401、新令牌 200；⑤ 关闭后非回环 403 | **非回环怎么在本机造**：`COMFYHUB_ALLOW_REMOTE=1` 绑 `0.0.0.0`，然后打**本机网卡 IP**（不是 `127.0.0.1`）→ 源地址就是网卡 IP，走非回环分支。这是本机能实测的手法 |
| 反向回归 | 三条既有 e2e（capture / submit / ai-tools）**不带任何头**，必须在"令牌默认关闭"下**全绿** | 它们就是 C2 的化身；一红说明默认行为变了 |
| doctor | 报一行"入站鉴权：关闭 / 已启用（回环豁免开）/ 监听 0.0.0.0 但未启用 → 非回环会被拒" | 用户"开了远程却没开令牌"的现场，一眼可见 |

---

## 9. 风险登记

| # | 风险 | 现状/触发 | 处置 |
| --- | --- | --- | --- |
| RSK-A | **端口转发伪装回环**，绕过令牌 | `netsh portproxy` / `adb reverse` / 同机反代 | 提供 `COMFYHUB_AUTH_LOOPBACK_EXEMPT=0`；文档写明；P2 可考虑对回环请求也要求"本机进程凭据" |
| RSK-B | 签名/令牌进日志、URL 历史、外部程序命令行 | CallLogging 会记 path（`Application.kt:316`）；`?t=` 进 URL | P1 先给 CallLogging 打码；签名 TTL 5 分钟；长期令牌永不进 URL（D5） |
| RSK-C | 用户绑 `0.0.0.0` 却忘开令牌 → 裸奔 | 今天就是这样 | D7 默认拒绝 + doctor 明确报 |
| RSK-D | 令牌明文落库 | D3 的取舍 | 只回给可信请求（D4）；界面分段显示；文档写明它**不是**上游密钥 |
| RSK-E | 轮换后手机突然全断 | 无宽限期 | 轮换按钮旁写明"手机上要重新填"；P2 若要做宽限期，就要引入"上一把令牌 + 过期时间" |
| RSK-F | 令牌经 HTTP 明文传输（局域网内可被嗅探） | 后端是纯 HTTP，没有 TLS | 局域网内可接受（与用户家 Wi-Fi 的信任级别一致）；**公网暴露明确不做**（README `:1188-1190` 的立场不变），真要做就得先有 TLS |

---

## 10. 明确不做的事

- **不做用户账户 / 登录页 / 会话 cookie**：单用户桌面应用，令牌就是全部。（`android-client-plan.md` §5）
- **不做 Basic Auth / mTLS / OAuth**：前者要在每个 URL 里带凭据（比 header 更糟），后两者与"本机单用户"不匹配。
- **不引 `ktor-server-auth`**（D2）。
- **不把令牌写进 `.env` / 脚本**：它是运行期状态，真源是库（`app_settings`）。
- **不动 `CredentialService` 的"永不回读"纪律**（C4/D3）。
- **不做公网暴露**：本设计的目标是"同一个 Wi-Fi 内可信"，不是"暴露到互联网"。

---

## 11. 验收判据（DoD）

1. 令牌关闭时：桌面 App、三条 e2e、`comfyhub.ps1 doctor`、探活脚本 **行为与今天逐字一致**（无一处需要改）。
2. 令牌开启 + `COMFYHUB_ALLOW_REMOTE=1`：从本机网卡 IP 打 → 无令牌 `401`，带令牌 `200`；从 `127.0.0.1` 打 → `200`。
3. 关闭令牌但绑 `0.0.0.0`：非回环 `403`，且启动日志/doctor 都说明原因。
4. `POST /api/auth/rotate` 之后：旧令牌 `401`、新令牌 `200`。
5. `GET /api/auth` 在**非回环且未带令牌**时不回明文令牌。
6. `flutter analyze` + `flutter test` 全绿（设置页新卡片不能碰 `test/localization_test.dart` 盯的 `locale`/delegate/落地页）。

---

## 12. 参考

- `docs/android-client-plan.md` §4 第 1 步（本设计的上游要点）
- `docs/ai-home-requirements-audit.md:61/103/119`（AIH-016、AIK-003 的缺口原文）
- `README.md:1188-1190`（"单用户、无鉴权（回环）"的现状陈述）、`:1195/1208`（§14 后续清单里的"鉴权与限流"）
- 端到端模板：`docs/comfyui-capture.md`、三条 `scripts\e2e-*-test.ps1`
