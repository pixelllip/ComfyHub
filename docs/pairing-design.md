# 配对设计：桌面与手机怎么互相认出对方（扫码 + 滚动验证码）

> 缘起（2026-09-24）用户问："**你如何让电脑端和手机端完成配对？手机扫码 + 输入实时生成的验证码行吗？**"
>
> 这是**设计**，不是已完成的东西。上游是 [`inbound-auth-design.md`](inbound-auth-design.md)
> （令牌闸门本身，D1~D12）与 [`sync-api-contract.md`](sync-api-contract.md) §0（令牌怎么上请求）。
> 本文只补"**人怎么把令牌交到手机手里**"这一段，**不推翻**上面两份的任何决策；
> 凡是细化了 D-x 的地方都显式标注。
>
> 每条"现状"都标了代码出处，每条决策都写了代价 —— 与 `inbound-auth-design.md` §4 同一套写法。

---

## 1. 一句话结论

**行，而且这就是我们要的形态** —— 但它的价值被高估的地方和低估的地方都写清楚：

| 问题 | 答案 |
| --- | --- |
| 扫码 + 输 6 位码能完成配对吗 | **能**。二维码带 `urls[] + ticket`，6 位码由桌面**实时滚动**，两者都对上才发长期令牌 |
| 6 位码是 SMS / TOTP 那类东西吗 | **不是**。它不证明"你是账号主人"，只证明"**拿手机的人此刻看得到这台桌面的屏幕**"。这正是配对需要的那一件事 |
| 比"手抄 43 字符令牌"好在哪 | 手抄要在两个屏幕之间来回看一分钟、必然出错；扫码把 `baseUrl` 也一起解决了（**手抄时代手机不知道该连哪个 IP**） |
| 它买到了什么 | ① 手机自动知道连哪（多网卡不用猜）；② 长期令牌**一次都不出现在人的视野里**；③ 二维码被拍照转发后**仍然配不成**（码在滚） |
| 它**没有**买到什么 | 明文 HTTP 上的中间人、攻击者已经能读库、用户把手机和码一起交给别人 —— 这三件**扫码 + 输码一件都防不住**（§3.3 与 `inbound-auth-design.md` §9 逐条对齐） |

> 一句话：**扫码解决"去哪、传什么"，6 位码解决"你是不是在旁边"。**
> 前者是体验，后者是那唯一一点真实的安全增量。

---

## 2. 现状：今天这件事一件都还做不了

不是"配对方式不好"，是**链路上每一环都缺**：

| 环节 | 现状 | 出处 |
| --- | --- | --- |
| 后端有配对/鉴权端点吗 | ❌ **没有**。Kotlin 侧 `route("/api")` 只挂了 prompt/tag/media/capture/ai 六族，**没有 `authRoutes()`**；Rust 侧 40 条路由里也**没有一条 `/api/auth`** | `server/src/main/kotlin/com/comfyhub/Application.kt:605-631`、`server-rs/src/web/routes.rs:165-205` |
| 后端认令牌吗 | ❌ 一次都没出现过。`X-ComfyHub-Token` / `InboundAuth` / `AuthStore` 在 `server/src/main/kotlin` 里 **0 命中**（只在文档里） | 全仓 grep 的结论 |
| 局域网可达吗 | ⚠ 默认只听 `127.0.0.1`（`allowRemote=false` → `host=127.0.0.1`），必须 `COMFYHUB_ALLOW_REMOTE=1` 才听 `0.0.0.0` | `Config.kt:29`、`Config.kt:66-70`、`Config.kt:54` |
| 手机知道连哪吗 | ⚠ 安卓默认 `http://10.0.2.2:8080`（**那是模拟器打宿主机的地址**，真机上它什么也不是）；桌面自己则是从 `.run\api.json` 读**自动挑的端口** | `lib/core/settings_store.dart:60-75`（`:67` 安卓分支、`:74` 桌面分支） |
| 手机带令牌吗 | ❌ **参数有、值永远没有**。`RunKitClient(transport, baseUrl, token)` 和 `SyncPushClient.push(baseUrl, token)` 都会把 `X-ComfyHub-Token` 放进头，桥也把 `token` 透下来了 —— 但 Dart 侧**一处都没有传**：`_source.ensureRunKit(baseUrl: …)`、`_source.syncNow(baseUrl: …)` 只给了地址，`token` 恒 `null` | `android/harness/.../kit/RunKitClient.kt:22-23`、`android/harness/.../sync/SyncPush.kt:99-107`、`.../bridge/HarnessBridge.kt:248/444`、`lib/state/ai_workspace_store.dart:1085/1104` |
| 手机存得下密钥吗 | ✅ **存得下，而且已经写好了**。`KeystoreCredentials` 用 AndroidKeyStore 生成 AES/GCM 密钥、密文 base64 落 SharedPreferences（`iv:ciphertext`）—— 这是"M6 那一层"，**今天就在生产代码里**，不是计划 | `android/app/src/main/kotlin/com/comfyhub/harness/bridge/KeystoreCredentials.kt:34-58` |
| 手机有设备号吗 | ✅ `KvKey.DEVICE_ID = "sync.deviceId"`，首次上行时生成一次并落库（**生成两次 = 桌面当成两台设备**，那条纪律已经写在注释里） | `android/harness/.../store/Models.kt:125-141`、`.../sync/SyncPush.kt:181-187` |
| 桌面设置页有这张卡片吗 | ❌ 没有。`AdaptiveColumns` 里的 7 个 section 是：本地服务 / AI 模型与凭据 / NSFW 内容 / 后端服务 / ComfyUI 自动捕获 / 库统计 / 关于 —— **没有"远程访问与配对"** | `lib/pages/settings_page.dart:491-617` |

**结论**：配对不是"给现有链路加个二维码"，而是 `inbound-auth-design.md` P0（令牌闸门）**做完之后**的 P2。
在 P0 之前做配对，等于给一扇没有锁的门配一把好看的钥匙。

> ⚠ 顺带一条**现状与文档的偏差**：`inbound-auth-design.md` §2 引的 `Application.kt:312-392`
> 插件链、`:305-310` 的 warn、`:337-364` 的 CORS 白名单**行号都已过时**（现在是 `:512-566` 与 `:552-554`）。
> 结论不变（**没有 Authentication**），但实施时按现文件的行号找。

---

## 3. 完整的配对流程

### 3.1 桌面：点「允许移动端连接」

| # | 谁 | 做什么 | 走哪个请求 / 传什么 | 失败怎么办 |
| --- | --- | --- | --- | --- |
| 1 | 用户 | 设置页「远程访问与配对」卡片里点开关 | `PUT /api/auth` `{enabled:true}` | 后端读 `app_settings` 失败 → 回 `{ok:false, reason}`，界面**不许**把开关画成已打开 |
| 2 | 后端 | 打开总开关；**若还没有令牌就顺手生成一个长期令牌**（D10 的既有语义，本文不改） | `SecureRandom` 32 字节 → base64url 43 字符（D11） | 生成失败 = 起不来，不是"降级成无令牌" |
| 3 | 用户 | 点「配对新手机」 | `POST /api/auth/pair/start` | 限速命中 → `429`，界面提示"等一会儿再点" |
| 4 | 后端 | 生成**一次性 ticket** 并把它留在内存 | 32 字节随机 → base64url；TTL **120 秒**；**单次使用**；**同时只保留最新一个**（再点一次 = 旧 ticket 立刻作废） | 内存里没有 ticket 时 `claim` 一律 `404`，**不许**用"过期的也能兑"来兜底 |
| 5 | 后端 | 枚举所有可达地址，连同 ticket / 过期时间 / **当前 6 位码**回给界面 | 见 §4.1 的响应；`urls[]` 为什么是多个见 §4.4 | 一个网卡都枚举不出来（离线 / 只有回环）→ 把 `urls` 里只放回环地址并**在界面上说明"手机连不上这个地址"**，不要给一个空的二维码 |
| 6 | 界面 | 同一张卡片上画两样东西：**二维码**（`urls[] + ticket + 版本 + 过期时间`）+ **6 位验证码**（每 60 秒重新显示） | 二维码内容见 §4.5 | 二维码生成失败 → 退化成"把地址和码念给手机"的文本行（不能让这条路径完全断掉） |

### 3.2 手机：扫码 → 输码 → 换令牌

| # | 谁 | 做什么 | 走哪个请求 / 传什么 | 失败怎么办 |
| --- | --- | --- | --- | --- |
| 7 | 手机 | 扫码，得到 `urls[]` 与 `ticket` | 本地解析，**不发请求** | 二维码不是我们的格式 → 如实说"这不是 ComfyHub 的配对码"，不许猜 |
| 8 | 用户 | 看着桌面屏幕，把 6 位码敲进手机 | 纯 UI | 超时（>120 秒）→ 提示"配对码已过期，在电脑上重新点一次" |
| 9 | 手机 | 逐个试 `urls[]` 直到有一个通 | `GET /api/health`（**回环豁免 / 无令牌也能通**，`Application.kt:606`） | 全部不通 → 把"试过哪几个地址、各自错在哪"逐条说出来（网段不同是常见原因，见 §4.4） |
| 10 | 手机 | 提交兑换 | `POST /api/auth/pair/claim`，**body** 里带 `{ticket, code, deviceId, deviceName, devicePublicKey?}`；**不需要已配对令牌**，但**必须限速** | `400` 码错 → 允许重试但计数；`410` ticket 过期/已用 → 回第 6 步重来；`429` → 告诉用户等多久 |
| 11 | 后端 | 校验 ticket（在不在、过没过期、用过没有）+ 校验 code（当前窗口 / 前一窗口） | 见 §4.2 的响应 | 失败 5 次 → **ticket 当场作废**（不是"锁 5 分钟"：作废更简单且判据更硬） |
| 12 | 后端 | 生成**该设备专属**的长期令牌，**在响应体里回一次** | `deviceId` 一起回；**绝不进 URL、绝不进日志**（D5/RSK-B） | 写库失败 → 不发明文令牌，回 `500`，手机上什么都不存 |
| 13 | 手机 | 存进密钥库并记下 `deviceId` | 复用 `KeystoreCredentials.put(ref="auth.pairing", value)`；`deviceId` 进 `mobile_kv` | 存失败 → **当场把令牌丢掉并要求重配**（存不下去的令牌 = 每次启动都要重扫） |

**之后**：所有请求带 `X-ComfyHub-Token`（`sync-api-contract.md` §0）；
媒体字节走 D5 的短期签名 URL（`?t=<HMAC>&e=<exp>`，长期令牌永不进 URL）；
SSE 按 D6 双认（header 优先）。

### 3.3 撤销与轮换

| 动作 | 走哪个请求 | 语义 |
| --- | --- | --- |
| 看已配对设备 | `GET /api/auth/devices` | 每台一行：设备名 / 最后活跃 / 最后 IP / 配对时间。**只回事实，不回令牌**（连前缀都不回） |
| 吊销单台 | `POST /api/auth/devices/{id}/revoke` | 该设备的令牌**立刻** `401`；别的设备不受影响 |
| 吊销全部 | `POST /api/auth/rotate`（D10 既有） | 轮换全局令牌 → **所有**设备立刻 `401`（RSK-E：无宽限期，界面要写明"手机上要重新配对"） |

> **本文细化 D10/D11（一机一令牌）**：`inbound-auth-design.md` P2 把"每设备独立令牌 + 单独吊销"
> 列为体验项，本文把它提到**配对时就落地**。理由：配对是唯一能自然拿到 `deviceId` 的时刻，
> 事后再补"这台是谁"要么重配一次、要么永远显示 `unknown`。
> **代价**：令牌从"一个"变成"一行一个"，`auth.pairing` 的形状要扩（§4.6），
> 且 `lastUsedAt` 的写入变成"每设备一行"—— 仍然按 D3 的纪律**攒在内存里最多 1 分钟落一次**。

---

## 4. 验证码的设计（这条是问题的核心）

### 4.1 它是什么、不是什么

```
code = 6 位十进制 = HMAC-SHA256(key = ticket, msg = str(floor(unix / 60))) mod 10^6
窗口 = 60 秒；接受当前与**前一个**窗口（±1）以容忍时钟边界
```

| 判据 | 定值 | 为什么是它 |
| --- | --- | --- |
| 密钥 | **ticket 本身** | 不需要额外存东西（同 D5 的思路）：二维码里已经有 ticket，桌面内存里也有，两边天然同步 |
| 6 位 | 是 | 这是**人念、人敲**的码。位数越多越安全，但敲错的概率涨得比安全快；6 位在"5 次作废"下暴力概率是 `5/10^6` |
| 窗口 60 秒 | 是 | 人从看屏幕到敲完 6 位数字大约 5~15 秒；60 秒够用且**滚得够快**，拍照转发的人追不上 |
| 接受 ±1 窗口 | 是 | 手机与桌面时钟不同步、以及"在窗口最后一秒敲完"是常态。只接受当前窗口会让用户偶尔莫名其妙地失败 |
| **失败 5 次作废 ticket** | 是 | 6 位码空间只有 10^6；不限次的话一个同 LAN 的攻击者可以在 120 秒里试很多次。作废比"锁定"更好实现，判据也更硬 |
| **只存在内存里** | 是 | 码**不落库、不进日志、不进任何响应给非本机请求**。它是"当下这一刻"的事实，落库只会制造一份可被读走的旧码 |
| 不落库的代价 | — | 后端重启 = 正在进行的那次配对作废。这是可接受的（用户在旁边，再点一次） |

### 4.2 "扫码"和"输码"各自解决什么

| 威胁 | 光扫码 | 扫码 + 输码 | 为什么 |
| --- | --- | --- | --- |
| **同 LAN 的陌生设备抢 ticket** | 挡不住（他能连上后端、能猜 `/api/auth/pair/claim`） | ✅ | ticket 只出现在桌面屏幕上；claim 还要一个"只在屏幕上"的码 |
| **二维码被拍照 / 转发**（发到群里、"帮我看看"） | ❌ 挡不住：转发即完整凭据 | ✅ | 码 60 秒滚一次，转发出去那一刻的码大概率已过期；即使卡在窗口内，对方也得**恰好**在你还在时看到 |
| **用户自己点错机器**（家里两台电脑都开了配对） | ❌ | ✅ | 你在 A 电脑屏幕上看到的码，兑不到 B 电脑的 ticket（ticket 不同 → HMAC 密钥不同） |
| **明文 HTTP 上的中间人** | ❌ | ❌ | 码和 ticket 都在明文里飞。这条只有 TLS 能解（`remote-access-security.md` §4 第②层），配对**一件都不解决** |
| **攻击者已经能读库**（`app_settings` 是明文） | ❌ | ❌ | 他能直接拿长期令牌，**根本不需要配对**。D3 的代价已经在那边写过了 |
| **用户把手机和码一起交给别人** | ❌ | ❌ | 社交工程。任何"人在旁边"的证明都防不住"人主动配合" |

> 这四条**不解决的**与 `inbound-auth-design.md` §9 的 RSK-A / RSK-B / RSK-D / RSK-F 是同一批风险，
> 本文不另起一套说法：**配对解决"你是谁"，不解决"这条链路会不会被人截、这台机器会不会被人打"**
> （那句原话在 `remote-access-security.md` §1 的"最大的误解"一行）。

### 4.3 替代方案对比

| 方案 | 一句实话 | 取舍 |
| --- | --- | --- |
| **① 只扫码不输码** | 二维码 = 完整凭据，转发即配对 | 实现最省（`claim` 只要 ticket），但**拍照转发就完蛋** —— 不采用 |
| **② 扫码 + 手抄 43 字符长期令牌** | 最安全也最没人用 | 二维码只解决 `baseUrl`；43 字符抄一分钟、必错一次。它不算备选，算"什么都不做时的现状下限" |
| **③ 扫码 + 输 6 位码**（推荐） | 体验与安全的平衡点 | 成本 ≈ 150 行 Kotlin（两个端点 + 滚动码）；换来"转发无效"+"用户点错机器也能发现" |
| **④ mDNS 自动发现 + 人工确认** | 不用扫码，但要多播 | `NsdManager` + Bonjour 两端都写；同 LAN 多台开了配对时用户还要在列表里挑，**那一步本质仍要"证明你在旁边"** —— 留到 ③ 之后（`android-client-plan.md` §4 第 5 步） |
| **⑤ 无配对**（今天的行为） | 什么都不用做，也没有任何保护 | 一开 `COMFYHUB_ALLOW_REMOTE=1`，同 Wi-Fi 下**任何设备**都能读媒体原件、驱动 ComfyUI、花 Provider 的钱（资产表见 `inbound-auth-design.md` §2）—— 明确不做 |

### 4.4 为什么二维码里是 `urls[]`（多个地址）

手机与桌面**不一定在同一个网段**：桌面可能同时有 Wi-Fi 的 `192.168.x.x`、虚拟网卡的 `10.x.x.x`、
Tailscale 的 `100.x.x.x`（`remote-access-security.md` §3.1），而手机只走其中一条。
桌面没法知道手机走哪条 —— 所以**把所有可达地址都编进二维码，让手机逐个试**（第 9 步）。

- 地址怎么算：遍历网卡取"非回环、非 link-local、已启用"的地址；端口用后端**实际监听**的那个
  （端口是自动挑的、记在 `.run\api.json` 里，`Config.kt:82-90`）—— **不许硬编码 8080**。
- 逐个试的代价：多几次 `GET /api/health`（回环豁免、无令牌也通的最轻端点）。
- **副作用**：二维码含多个内网 IP + ticket，等于把内网拓扑给"看到二维码的人"看一眼。
  可接受（他就站在你旁边，那台机器本来就监听在局域网上），但**别把它投到大屏上**。

### 4.5 二维码内容（版本化）

```
comfyhub://pair?v=1&t=<ticket>&e=<expiresAt>&u=<base64url(urls 用 \n 连接)>
```

用自造 scheme 而不是 `http://`：将来手机真的做了 deep link 时不会和"打开网页"混淆。
`v=1` 是**必须**的：没有版本号，将来改字段就会让"旧手机扫新码"变成一条看不懂的错误。

### 4.6 状态形状（在 D3 / §5 的基础上扩一层）

```jsonc
// app_settings.k = 'auth.pairing'   （形状不兼容旧版 → schema 升位）
{
  "schema": "comfyhub-inbound-auth/2",
  "enabled": true,
  "token": "…43 字符 base64url…",     // 全局令牌（D10 的 rotate 作用在它身上；也是"还没有设备时"的兜底钥匙）
  "createdAt": "2026-09-24T21:00:00+08:00",
  "rotatedAt": null,
  "devices": [                        // ★ 本文新增（细化 D10/D11）
    { "id": "phone-7f3a…", "name": "OnePlus 8T", "token": "…43 字符…",
      "publicKey": null, "pairedAt": "…", "lastSeenAt": "…", "lastIp": "192.168.1.23" }
  ]
}
```

- `schema` 从 `/1` 升到 `/2`：旧版 JSON 里没有 `devices`，**读到时当作空数组**（不是"坏 JSON"）——
  这样"先装了旧版再升级"不会把用户的所有配对打掉。
- ticket 与滚动码**不在这里**：它们只活在内存（§4.1）。
- 与 `sync-api-contract.md` §3 的关系：`app_settings` **本来就不同步**（`/api/sync/bootstrap` 对它回 `400`），
  所以 `devices[].token` **不会**被手机的第一条同步请求顺走。**这条要在实现时保持**。

---

## 5. 接口形状（D10 的风格：只回事实、不回秘密）

放在既有的 `/api/auth` 下（D10），新建 `AuthRoutes.kt`，**不塞进 `AiRoutes.kt`**。

### 5.1 `POST /api/auth/pair/start`（桌面 UI 触发；回环来源才允许）

```json
{
  "ticket": "9f2c…43 字符…",
  "expiresAt": "2026-09-24T21:03:20+08:00",
  "code": "418302",
  "codeExpiresAt": "2026-09-24T21:01:00+08:00",
  "urls": ["http://192.168.1.23:51837", "http://100.101.102.103:51837"]
}
```

| 字段 | 谁用 | 规矩 |
| --- | --- | --- |
| `ticket` | 编进二维码 | 32 字节随机 base64url；**只在回环请求的响应里回**（承 D4：非回环拿不到它） |
| `expiresAt` | 界面倒计时 | TTL **120 秒**（**本文修正 `inbound-auth-design.md:274` 的"5 分钟"**，理由见 §6.3） |
| `code` | 界面显示给人看 | 6 位十进制；**这个响应是它唯一一次出现**，不进日志 |
| `codeExpiresAt` | 界面 | 界面据此滚动重画（下一个窗口换一个新码） |
| `urls` | 编进二维码 | 见 §4.4；**至少一个**，没有可达地址时回空数组 + 让界面如实说 |

### 5.2 `POST /api/auth/pair/claim`（手机调用，**不需要已配对令牌**，但必须限速）

```json
// 请求
{
  "ticket": "9f2c…",
  "code": "418302",
  "deviceId": "phone-7f3a…",
  "deviceName": "OnePlus 8T",
  "devicePublicKey": null
}
```

```json
// 200
{
  "deviceId": "phone-7f3a…",
  "token": "…43 字符 base64url…",
  "schemaVersion": 12,
  "serverTime": "2026-09-24T21:00:42Z"
}
```

| 规矩 | 为什么 |
| --- | --- |
| `ticket` / `code` **只在 body**，**绝不出现在 URL** | URL 会进日志、历史、中间代理（RSK-B / `remote-access-security.md` §5 第 1 条） |
| 响应里 `token` **只此一次** | 之后任何接口（含 `GET /api/auth/devices`）都不再回它 |
| 失败一律**不区分**"ticket 不对"与"code 不对" | 区分等于告诉攻击者"ticket 猜对了"。统一回 `400 {error:"配对失败"}` |
| 限速按**来源地址 + ticket** 双口径 | 来源地址防"扫一个网段"，ticket 防"换 IP 接着猜" |
| `devicePublicKey` 现在留 `null` | 它是"将来做请求签名"的位置（比"D9 的常数时间比较 + 长期令牌"更硬），**本文只占位，不实现** —— 不许占着位置假装已经安全了 |

### 5.3 `GET /api/auth/devices` / `POST /api/auth/devices/{id}/revoke`

```json
{ "devices": [
  { "id": "phone-7f3a…", "name": "OnePlus 8T",
    "pairedAt": "2026-09-24T21:00:42Z", "lastSeenAt": "2026-09-24T21:40:11Z",
    "lastIp": "192.168.1.23" }
] }
```

- 非回环请求**必须已带令牌**才看得到这张表（它本身是"这台机器上谁有钥匙"的情报）。
- 吊销是**立刻**生效（内存里那份设备表与库同步更新），不是"下次启动时清理"。
- 吊销不删 `sync_clients` 那一行：游标是同步状态，与身份是两件事（`desync` 时才该动它）。

### 5.4 落在哪个后端

**两个后端都还没有 `/api/auth`**（§2 已核）。实施时按"手机连的是谁"决定：

| 后端 | 有什么 | 缺什么 |
| --- | --- | --- |
| Kotlin `:8080` | `/api/ai/run-kit`（手机取规则的来源之一） | `/api/sync/push` 等四条同步端点 |
| Rust `:8081` | 四条 `/api/sync/*` **+ `/api/ai/run-kit{,/status}`**（2026-09-24 第 22 步搬完） | 会话写侧 / Run 与 SSE / 工具层与审批 / 附件 |

> 📌 **2026-09-24 更新**：写这份设计时 `/api/ai/run-kit` 只在 Kotlin 上（所以手机"取规则"与
> "推数据"各需要一台后端）。**现在 Rust 侧两条都有**（`ai/run_kit.rs`，
> 2026-09-24 的 P4 第 22 步 —— 那段流水账 2026-09-30 已从总纲删掉，要看用 `git log`）
> —— 配对落地时**直接做在 Rust 上**，手机只连这一个后端。

配对端点在**手机实际连的那个后端**上才生效。第一步的验收**只验"闸门与端点本身"**
（它不依赖 `/api/sync/push` 或 `run-kit`），但第 3 步的真机验收要在 Rust 上做。

---

## 6. 落地顺序（三步，每步都能独立验收）

### 6.1 第 1 步：后端 auth 闸门 + `auth.pairing` 状态（= `inbound-auth-design.md` 的 P0，**未做**）

- `Config.kt` 加 `authLoopbackExempt`（环境变量，默认 true）；
- 新 `InboundAuth.kt`：`decide(remoteHost, method, path, headerToken, queryToken) → Allow | Deny(reason)`，**纯函数**；
- `Application.kt:605` 之前装插件；CORS 的 `allowHeader` 加 `X-ComfyHub-Token`（D8）；
- 新 `AuthRoutes.kt` + `AuthStore`（读写 `app_settings` 的 `auth.pairing`，走 `SettingsRepo.get/put`，`SettingsRepo.kt:21-32`）。
  ⚠ CORS 的 `allowHeader` 现在在 `Application.kt:552-554`（`inbound-auth-design.md` §D8 引的 `:337-364` 是旧行号）。

**验收**：`InboundAuthTest`（纯函数，零新依赖）+ 新 `scripts/e2e-auth-test.ps1`（**今天只有 3 条 e2e，没有这条**）。
非回环在本机的造法沿用 D8 那条：绑 `0.0.0.0` 然后打**本机网卡 IP**。

### 6.2 第 2 步：配对端点 + 桌面设置页卡片

- `POST /api/auth/pair/start`（§5.1）、`POST /api/auth/pair/claim`（§5.2）、`GET /api/auth/devices`、`POST /api/auth/devices/{id}/revoke`；
- 桌面设置页新增 section「远程访问与配对」，照 `_nsfwCard`（`settings_page.dart:763-836`）的卡片写法与 `_SwitchRow`：
  开关 / 二维码 / **滚动 6 位码** / `urls[]` 文本 / 已配对设备列表（含吊销按钮）；
- 二维码怎么画：**后端出 PNG**（少一个 Flutter 依赖）或引一个纯 Dart 二维码包（本仓 `pubspec.yaml` 现在**一个都没有**）。

**验收**：`e2e-auth-test.ps1` 扩到"扫码配一次 → 带令牌 200 / 不带 401 → 吊销后立刻 401"，
全部用 HTTP 请求驱动（**不需要真的手机**）。

### 6.3 第 3 步：手机侧配对 UI（扫码 + 输码 + 存密钥库）

- 扫码用平台 API / 插件（安卓 `CameraX` 或现成扫码插件）；解析 §4.5 的 URI；
- `KeystoreCredentials` 复用现成实现（`KeystoreCredentials.kt:52-58`），ref 用 `auth.pairing`；
- 给 Dart 侧**补上一直没传的那个参数**：`SettingsStore` 存/读令牌 → `ensureRunKit(token:)` / `syncNow(token:)`
  （`lib/state/ai_workspace_store.dart:1085/1104` 就是两个调用点）；
- **真机验一次**：这是唯一能抓住"平台约束"的地方 —— M3 的三条阻断级问题（明文 HTTP 策略、
  主线程网络、`@UiThread`）**全部只在真机上出现**，单测全绿也照样抓不到。

**验收**：真机上"扫一次 → 之后桌面重启也不用再扫"，且 `adb reverse` 关掉后走局域网 IP 仍然通。

### 6.4 阻塞点（**2026-09-24 当天已解**，留档说明为什么当初要写这一节）

写这份设计时：手机的 `run-kit` 只存在于 **Kotlin** 后端，而 `/api/sync/push` 只存在于
**Rust** 后端 —— 手机"发得出去"与"推得回去"各需要一台不同的后端，配对完的下一步
（取规则 / 推数据）必然撞上它。

**这条已经解了**：`GET /api/ai/run-kit{,/status}` 在 2026-09-24 搬到了 Rust
（`ai/run_kit.rs`，P4 第 22 步），真库副本上两支后端 canon 零差异。
所以配对三步现在都做在 **Rust** 上，手机只连一个后端。

> ⚠ 保留这一节是因为它记着一条**通用纪律**：**"配对通了" ≠ "配完能用"**。
> 验收第 3 步时别只看到"手机存下了令牌"，要走到"取到规则 + 推上去一条数据"为止。

---

## 7. 防线（"删了以后出错还能不能被测试抓住"是唯一标准）

| 层 | 用例 | 为什么必须是这一条 |
| --- | --- | --- |
| JVM 纯函数 | ticket **单次使用**：同一个 ticket 兑第二次 → `410` | 这是"一次性"这个词的全部含义 |
| JVM 纯函数 | ticket **过期**：`now > expiresAt` → `410`（边界值：恰好等于时也算过期） | 边界不定死，就会有人写出"过期 1 秒内还能用" |
| JVM 纯函数 | 码错 **5 次 → ticket 作废**（第 6 次即使码对也失败） | 6 位码的空间只有 10^6，不限次等于没有 |
| JVM 纯函数 | **码窗口边界**：`floor(unix/60)` 在当前窗口与前一个窗口都通过，再往前一个不通过 | ±1 的容差写错就会变成"±1 小时" |
| JVM 单测 | 长期令牌**只在 `claim` 的响应体里出现一次**；`GET /api/auth/devices`、日志、`/api/auth` 的其它响应里都搜不到它 | 这是 D4/D5 的核心纪律，靠人眼 review 抓不住 |
| JVM 单测 | **轮换后旧令牌 `401`**、新令牌 `200` | D10；无宽限期是有意的（RSK-E） |
| e2e | **非回环无令牌 → `403`**（不是 `401`）；带令牌 → `200`；回环不带 → `200` | D7 的默认拒绝；`401`/`403` 的语义不能混 |
| e2e | **设备吊销后立刻 `401`**，且**别的设备仍然 `200`** | "一机一令牌"的全部价值；只测"吊销生效"会漏掉"误伤全体" |
| Dart / 后端单测 | **桌面 UI 的二维码内容里不含长期令牌**（字符串层面断言） | 这条是"不把长期令牌编进二维码"（§8）唯一的机器判据 |
| e2e | `claim` 的失败限速：超过阈值 → `429`，且失败响应里**不回显 ticket / code** | 6 位码 + 无限次 = 可暴力；日志回显 = 凭据泄漏 |
| 手机单测 | 令牌**不进任何 URL**、不进 `debugPrint` / logcat 的字符串 | 手机上最容易顺手 `print(url)` 把令牌打出来 |

**反向回归**（与 `inbound-auth-design.md` §8 同一条）：三条既有 e2e
（`e2e-capture-test.ps1` / `e2e-submit-test.ps1` / `e2e-ai-tools-test.ps1`）**不带任何头**，
必须在"令牌默认关闭"下全绿 —— 它们一红就说明默认行为被改了。
另加一条**跨文档一致性用例**：`auth.pairing` 的 `devices[].token` **不许**出现在
`/api/sync/bootstrap` 的任何一页里（今天靠"`app_settings` 不在同步范围内"这个白名单，
白名单是可以被后来的人改掉的，所以要有用例钉着）。

---

## 8. 明确不做的事

- **不做账号体系 / 登录页 / 找回密码**：单用户桌面应用，配对令牌就是全部身份（承 D10 与 `android-client-plan.md` §5）。
- **不做 SMS / 邮件验证码**：那要求手机号或邮箱、要求一个能发信的服务器 —— 与"便携发布包 + 局域网直连"正面冲突，而且**它证明的是"你能收短信"，不是"你在这台电脑旁边"**。
- **不做"永久二维码"**（把长期令牌直接编进二维码）：那等于把 D5 的"长期令牌永不进 URL/不落纸面"整条作废；二维码会被截图、被存在相册、被同步到云端。
- **不做公网暴露**：配对只解决"同一个 Wi-Fi / 同一个组网内可信"（承 `inbound-auth-design.md` §10 与 `remote-access-security.md` §6）。公网要么先有 TLS，要么走组网 / Tunnel。
- **不做"配对过就当操作审批过"**：令牌是**连接鉴权**，不是**操作审批**（`sync-api-contract.md` §0 那条警告）。
  配对之后 `comfy_submit` / `write_file` 该要批准仍然要批准。
- **本轮不做 `devicePublicKey` 的请求签名**：字段占位，但**实现它的那一天要 bump 一次 `schema`**，
  别让"字段存在"被当成"已经防住了"。也不做设备指纹 / 人脸 / 指纹解锁（那会与"harness 要在前台服务里后台跑"冲突，
  `KeystoreCredentials.kt:45-47` 已经为同一理由刻意不要求用户认证）。

---

## 9. 判断留痕（哪些是我拍的板，供评审推翻）

| # | 拍板 | 备选与代价 |
| --- | --- | --- |
| 1 | TTL **120 秒**（修正 `inbound-auth-design.md:274` 的 5 分钟） | 5 分钟更宽容，但二维码被拍走后可用的窗口长 2.5 倍；`remote-access-security.md` §5 本来就建议 120 秒，本文按它对齐 |
| 2 | 码的 HMAC **以 ticket 为密钥**，不另存 secret | 另存一个 secret 多一处状态与一处泄漏面；以 ticket 为密钥则"作废 ticket"自动作废码 |
| 3 | 手机密钥库**复用 `KeystoreCredentials`**（Kotlin 侧 AES/GCM + SharedPreferences），而不是 `flutter_secure_storage` | 总纲 §1 的 M6 行写的是 `flutter_secure_storage`。选复用是因为它**已经在生产代码里**、且 Dart 侧不需要再碰密钥；代价是"自己写的加密"要在评审时单独看一遍。若评审更信任成熟库，改成 `flutter_secure_storage` 只影响第 3 步 |
| 4 | 一机一令牌**在配对时就落地**（而不是 D10 的"以后再说"） | 代价是状态形状要升 `/2`；收益是吊销从第一天起就是"按设备"的 |
| 5 | ✅ **已拍板（2026-09-27）：二维码由 Dart 侧画**（`qr_flutter`，纯 Dart 包），后端**只回事实**（`ticket` / `urls[]` / `code` / 两个 `expiresAt`） | 备选是后端出 PNG，或后端直接回 `qrPayload` 字符串 —— **两种都否掉**：出 PNG 让后端多一条渲染路径；回 `qrPayload` 会把"格式"变成**两份实现**（Rust 编、Dart 解），而 §4.5 那串 URI 的字段（`v`/`t`/`e`/`u`）正是要靠**一份**实现守住的。现在编与解都在 `lib/core/pairing_payload.dart`，`PairingQrImage` 只负责画（白底那条纪律也在它里面）。渲染失败时**必须**退化成"把地址和 6 位码念给手机"的文本行（§3.1 第 6 步） |
| 6 | ✅ **已拍板（2026-09-27）：手机扫码用 `mobile_scanner`，且相机不可用不算失败** | 相机是"锦上添花"的那条路：插件缺失 / 权限被拒 / 不是 ComfyHub 的码，一律**当场退回手敲 6 位码那条已经跑通的路**（B 通道），并说清原因。**不许**因为扫码不可用就让配对整条不可用 |

---

## 10. 参考

- [`inbound-auth-design.md`](inbound-auth-design.md) —— 令牌闸门本身（D1~D12、§6 的 P0/P1/P2、§8 防线、§9 风险登记）
- [`remote-access-security.md`](remote-access-security.md) §5（配对与令牌的具体要求）、§4（四层防护）、§6（红线）
- [`sync-api-contract.md`](sync-api-contract.md) §0（`X-ComfyHub-Token` 与"连接鉴权 ≠ 操作审批"）
- [`implementation-master-plan.md`](implementation-master-plan.md) §1（桌面唯一真源与当前状态）、🔒 [`run-kit-contract.md`](run-kit-contract.md)（工件下发）
- [`android-client-plan.md`](android-client-plan.md) §4（第 1 步鉴权、第 5 步局域网发现）

---

## 11. 实施补充（2026-09-24 落地时冻结的**实际形状**）

> 这一节是**实现契约**：§1~§9 是设计与理由，形状以本节为准（两端 + 脚本 + 用例都照它写）。
> 落点在 **Rust 后端**（§5.4：手机只连一个后端），Kotlin 侧**不加** `/api/auth`。

### 11.1 ★ 两条配对通道，本轮只做 B（**当场拍板**）

| | A. 扫码通道（**本轮未做**，形状已留） | B. 手机发起通道（**本轮落地**） |
| --- | --- | --- |
| 谁先动 | 桌面 `pair/start` 出 ticket + 二维码 | 手机 `pair/request` 先举一次手 |
| 手机怎么知道 ticket | 扫二维码 | **不需要 ticket**（服务端留在自己手里） |
| 桌面怎么把码给人 | 屏幕上直接显示 | 屏幕上的**待配对列表**里显示 |
| 卡在哪 | 手机要相机扫码（一个 Flutter 插件 + 相机权限）；桌面要画二维码（另一个包） | 零新依赖 |
| 安全属性 | 人必须看到屏幕上的 6 位码 | **同一条**：码只出现在桌面屏幕上 |

**为什么 B 不与 §3 冲突**：§3 的安全增量只有一句 —— **"拿手机的人此刻看得到这台桌面的屏幕"**。
B 通道把这个属性完整保留（码由桌面算出并显示，手机只能提交、不能计算）；
它让掉的只是"手机自动知道连哪"（用户手填一次桌面地址，这件事在 §2 的手机侧本来就要做）。
A 通道留到"真的要做扫码"时再加，**到时候不需要动本节任何已冻结的形状**（只是多一条 `pair/start`）。

> ⚠ **一条绝对不能破的实现纪律**：B 通道里 `requestId` **必须与 ticket 相互独立**。
> 若把 `requestId` 就当成 ticket 发给手机，手机自己就能算出 6 位码 ——
> "必须看到屏幕"这件事**当场失效**，而且**任何测试都不会红**（流程照样跑通）。
> 形状上体现为：`pair/request` 的响应里**只有 `requestId`，没有 `ticket`**。

### 11.2 端点形状（全部在 Rust 后端）

```
GET  /api/auth                            仅回环 → {enabled, hasToken, schema, devices[], pending[]}
PUT  /api/auth                            {enabled}  仅回环 → 同上（开开关时若没有令牌就顺手生成一个）
POST /api/auth/rotate                     仅回环 → {ok, hasToken}
GET  /api/auth/devices                    回环 或 带令牌 → {devices:[{id,name,pairedAt,lastSeenAt,lastIp,allowCredentials}]}
POST /api/auth/devices/{id}/revoke        仅回环 → {ok, revoked}
POST /api/auth/devices/{id}/credentials   {allow}  仅回环 → {ok, id, allowCredentials}

POST /api/auth/pair/request               匿名（限速）{deviceId, deviceName} → {requestId, expiresAt, codeExpiresAt, serverTime}
GET  /api/auth/pair/pending               仅回环 → {pending:[{requestId, ip, deviceName, code, codeExpiresAt, expiresAt}]}
POST /api/auth/pair/claim                 匿名（限速）{requestId|ticket, code, deviceId, deviceName}
                                          → {deviceId, token, schemaVersion, serverTime}

GET  /api/ai/credentials?refs=a,b         回环 或 **已配对且 allowCredentials=true 的设备**
                                          → {credentials:{a:"…"}}   （未授权的 ref 不出现，不报错）
```

**错误码（不能混）**：

| 场景 | 状态码 | body |
| --- | --- | --- |
| 非回环、闸门开着、**没带**令牌 | `403` | `{"error":"需要配对令牌（X-ComfyHub-Token）"}` |
| 带了令牌但**不对/已吊销** | `401` | `{"error":"令牌无效或已被吊销"}` |
| 配对失败（ticket/requestId 不在、码错） | `400` | `{"error":"配对失败"}` —— **不区分**是哪一个错（§5.2） |
| 会话过期 / 已用过 / 试错 5 次作废 | `410` | `{"error":"配对会话已失效，请在电脑上重新发起"}` |
| 限速命中 | `429` | `{"error":"请求太频繁，请稍后再试"}` |
| 凭据端点：非回环**且没带令牌** | `403` | `{"error":"需要配对令牌（X-ComfyHub-Token）"}` |
| 凭据端点：带了令牌但**不是"已授权设备"**（含全局令牌） | `403` | `{"error":"这台设备没有被允许读取密钥"}`（**与闸门开关无关**） |
| 非回环打**仅回环**的管理端点（`GET/PUT /api/auth`、`pair/pending`、`devices/{id}/revoke`、`.../credentials`、`rotate`） | `403` | `{"error":"<动作> 只允许在本机（127.0.0.1）调用"}` |

### 11.3 状态：`app_settings.k = 'auth.pairing'`（承 §4.6）

```jsonc
{ "schema": "comfyhub-inbound-auth/2", "enabled": false, "token": "…43…",
  "createdAt": "…", "rotatedAt": null,
  "devices": [ { "id": "phone-…", "name": "OnePlus 8T", "token": "…43…",
                 "pairedAt": "…", "lastSeenAt": "…", "lastIp": "10.44.33.71",
                 "allowCredentials": false } ] }
```

- `devices` / `allowCredentials` 缺失一律按空数组 / `false` 读（**不是**坏 JSON）。
- **`allowCredentials` 是"密钥下行"的唯一开关**，默认 `false`：手机配对成功 ≠ 能拿密钥。
- ticket / 滚动码 / pending 请求**只在内存**，永不落库、永不进日志。

### 11.4 闸门的判定顺序（纯函数，一处实现）

1. `/api/auth/pair/request` 与 `/api/auth/pair/claim` → **放行**（它们本来就是给未配对设备用的，内部限速）；
2. 来源是回环 → **放行**（桌面 App 与脚本都走这条）；
3. `/api/ai/credentials` → 只认"已配对 + 已授权"的**设备**令牌（`enabled` 关着也一样，**永不对匿名开放**）；
4. `enabled == false` → **放行**（今天的行为；三条既有 e2e 不带任何头，必须全绿）；
5. 有令牌 → 放行并记 `lastSeenAt` / `lastIp`；没令牌 → `403`；令牌不对 → `401`。

### 11.5 本轮**不做**的（诚实清单，别当成已完成）

- 二维码（`pair/start`）与手机扫码：见 §11.1 的取舍；
- `devicePublicKey` 请求签名（§8 已声明不做）；
- HTTPS / 公网暴露（§8）；
- 手机侧的前台服务与后台长时间 Run（归总纲 §6 风险表，与本轮无关）。

---

## 12. 真机怎么验（可复现的操作手册）

> 这一节是**给下一个人照做的**：不写"我验过了"，写"怎么验、看什么才算通过"。
> 桌面与手机在同一台 Wi-Fi 下（本机实测：PC `10.44.130.232` / 手机 `10.44.33.71`）。

### 12.1 前置：谁在跑

配对端点**只在 Rust 后端上**（§5.4）。所以要把 Rust 后端**听在局域网上**：

```powershell
.\server-rs\target\debug\comfyhub-server-rs.exe --db .db\comfyhub.db --storage storage --host 0.0.0.0 --port 8081
curl.exe http://127.0.0.1:8081/api/health          # → {"database":"ok","impl":"rust",…}
```

**两个容易卡住的点**（都实测过）：

1. **Windows 防火墙**：非交互式启动的未知程序会被系统**自动建一条 Block 规则**
   （实测 `comfyhub-server-rs.exe` 上有两条 `Inbound/Block/Public`，而 WLAN 正是 Public 档）。
   防火墙里 **Block 优先于 Allow**，所以只加一条放行规则是没用的，得先把 Block 删掉：
   ```powershell
   Get-NetFirewallRule -DisplayName 'comfyhub-server-rs.exe' | Remove-NetFirewallRule
   New-NetFirewallRule -DisplayName 'ComfyHub 手机配对' -Direction Inbound -Action Allow `
     -Protocol TCP -LocalPort 8081 -RemoteAddress 10.44.33.71 -Profile Any
   ```
   **验完把它删掉**（只留这一条、且只允许那一台手机的地址）。
2. **手机能不能连上**：安卓有 `toybox`，用它发一个原始 HTTP 请求最省事：
   ```powershell
   adb shell "echo GET /api/health | toybox nc 10.44.130.232 8081"
   # 回 HTTP/1.1 400 Bad Request 就说明**通了**（400 是因为没写请求路径，不是失败）
   ```

### 12.2 手机侧

```powershell
flutter build apk --debug
adb install -r build\app\outputs\flutter-apk\app-debug.apk
adb shell svc power stayon usb      # 大包传输中途设备别掉线
```

手机上：**设置 → 远程访问与配对** → 把「桌面地址」填成 `http://10.44.130.232:8081` → 点「配对新电脑」。

### 12.3 桌面侧（两种都行）

- **有界面**：桌面 App 的地址也要指向 Rust（灰度期默认连的是 Kotlin 的 8080，
  而 `/api/auth` 只在 Rust 上）→ 设置 → 远程访问与配对 → 打开「允许移动端连接」→
  展开「配对新手机」→ 卡片上会出现 **6 位码**；
- **没界面**（脚本 / 远程）：全是回环端点，本机 curl 即可：
  ```powershell
  curl.exe -X PUT http://127.0.0.1:8081/api/auth -H 'content-type: application/json' -d '{\"enabled\":true}'
  curl.exe http://127.0.0.1:8081/api/auth/pair/pending        # → pending[].code 就是屏幕上那组码
  ```

把码敲进手机 → 配对成功。**通过判据**（少一条都不算过）：

1. `GET /api/auth/devices`（回环）里多出那台设备，`allowCredentials=false`；
2. 手机上再点一次发送不再被拦（说明令牌真的进了请求头）；
3. **把防火墙那条规则删掉之后**，手机上报 `403`（而不是"连不上"）——
   那证明它连的是后端、只是没带令牌。

### 12.4 密钥下行（用户问的"API Key 设置互传"）

1. 桌面上勾那台设备的「允许把密钥发给这台设备」（默认关，**这是唯一的开关**）；
2. 手机上点「从桌面同步密钥」；
3. 通过判据：手机上该 Provider 的凭据状态变成 `keystore`（不是 `none`），
   而且**发送一条消息能收到真回复**（上游 401 会当场露馅）。

> ⚠ 桌面上没勾就点同步：手机拿到的是"未授权"，界面要如实说"这把还得自己填" ——
> 那不是失败，是默认就该这样（§11.3）。

### 12.5 ✅ 实测记录（2026-09-24，OnePlus 8T / KB2000 + Windows 本机）

**结论：电脑 → 手机（provider 配置 + API Key）与 手机 → 电脑（会话 / 消息 / 有序块）
两个方向都通了**，而且是在**闸门打开**的状态下走的（不是"局域网裸奔"那条路）。

| # | 验的是什么 | 实测证据 |
| --- | --- | --- |
| 1 | 手机连得上电脑（局域网 + 防火墙） | `adb shell "echo GET /api/health \| toybox nc 10.44.130.232 8081"` → `HTTP/1.1 400 Bad Request`（400 是因为没写请求路径） |
| 2 | 闸门开着时局域网匿名被挡 | 手机上没带令牌打 `/api/prompts` → **`403`**（不是连不上） |
| 3 | 手机举手 → 桌面出码 → 兑码 | `pair/pending` 里出现 `requestId=7d930b8a…`、`ip=10.44.33.71`、`deviceName=KB2000`、`code=575340`；手机敲完码提示"已配对" |
| 4 | 设备令牌**落库**且是专属于这台设备的 | `app_settings.auth.pairing`：`schema=/2`、全局令牌 43 字符、`devices[0].token` **43 字符**、`allowCredentials=false` |
| 5 | 配对之后手机**真的在带令牌** | `lastSeenAt` 在配对后立刻跳动（没有令牌的请求不会被记这一笔） |
| 6 | **API Key 下行** | 手机 `shared_prefs/comfyhub_credentials.xml` 里出现 `auth.pairing` / `COMMAND_CODE_GOAT` / `DEEPSEEK_API_KEY` 三条（值都是 AES/GCM 密文）；界面提示"取回了几把密钥" |
| 7 | **用下行来的 Key 真的聊通一轮** | 手机上收到真回复（上游就是 `api.commandcode.ai`，用的是桌面上那一把 key） |
| 8 | **数据回传到桌面** | `_sync_log` 在 `14:41:29` / `14:42:02` 记下手机的 `ai_messages` + `ai_message_parts` 上行；桌面库里那条会话是 `provider_id=command-code-goat`、`model_id=deepseek/deepseek-v4.1-flash`（**正是手机上选的那个**，桌面 App 自己记的是"假模型"，而且它当时根本没开） |

> 📌 **这一步抓住的坑（都已修，见 §13）**：`adb install -r` 会卡死（要 `push` + `pm install`）、
> `input tap` 坐标与截图对不上（最后是**请用户点**）、「AI 模型与凭据」页自己造了个不带令牌的客户端
> （真机上一打开就是红横幅 `需要配对令牌`，而配对明明是成功的）、以及手机上两处 `RenderFlex` 溢出。

### 12.6 手机与桌面的地址不一致（灰度期的事实，别当 bug）

手机上只需填**一个**地址，而它必须指向**同时有 `/api/auth` 与 `/api/sync/push` 的那个后端** ——
今天就是 **Rust（8081）**。桌面 App 默认连的是 Kotlin（8080，它没有 `/api/auth`），
所以：

* 想用桌面上的**配对卡片**，得把桌面 App 的地址也改成 Rust 的 8081（设置 → 远程访问与配对）；
* 不改也行 —— 桌面那几个管理端点全是**回环**的，本机 `curl` 一样能开闸门、看码、勾授权（§12.3）。

这也是 P4 收口时要消掉的那条缝：等 Rust 把 `/api/capture/*` 与 `/api/ai/*` 剩下的搬完，
两端就都只连一个后端了。

---

## 13. 这一轮真机验收改掉的东西（都在 `docs/pitfalls.md` 有台账）

| 改动 | 为什么非改不可 |
| --- | --- |
| **令牌只留一份，且由客户端默认带上**（`lib/core/pairing_token.dart`） | 真机上「AI 模型与凭据」页一条红横幅 `需要配对令牌`，而同一个 App 的 AI 工作台跑得好好的 —— 因为那个页面自己 `new` 了一个不带令牌的客户端。"哪个页面忘了接线"这种漏法不可能靠人眼守住，所以做成客户端的默认值 |
| 配对**兑码也按来源地址限速**（20/分） | 设计 §5.2 要的是"来源地址 + ticket"双口径；只有"错 5 次作废"的话，攻击者换个 ticket 就又有 5 次 |
| 「AI 模型与凭据」页在**手机上**：两张维护卡折叠、列表高度按可用高度取四成、两处溢出修掉 | 用户原话"手机端这个排版的实用性极差"：那两张卡是**电脑上的维护活**（补能力声明 / 真发请求实测校准，后者花钱），手机上只会挡住 Provider 与模型列表 |
| `adb install -r` → `push` + `pm install`；debug 包只带 `android-arm64` | streamed install 在这台机器上会卡住（几十秒能做完的事拖成几分钟） |

---

## 14. 密钥上行：手机 → 桌面（2026-09-27 用户拍板"双向"）

> 用户原话：**"手机完全可以独立配置 provider，有密钥的也同步到手机端。"**
> 这一条**改掉了 AIH-012 的口径**（原先是"清单不含密钥"，只做下行）；现在是**双向**：
> 手机能独立配、也能把 key 推回桌面。下面是把这件事做对需要定的形状。

### 14.1 两条通道，两个开关（别合成一个）

| 方向 | 端点 | 谁允许 | 默认 |
| --- | --- | --- | --- |
| 下行（桌面 → 手机） | `GET /api/ai/credentials?refs=a,b` | 已配对设备 + **`allowCredentials=true`** | **关** |
| 上行（手机 → 桌面） | `PUT /api/ai/credentials` | 已配对设备 + **`allowCredentialsUpload=true`**（新字段，与上一条**分开**） | **关** |

**为什么必须两个开关**：读和写不是同一种权力。允许"这台手机能用我的 key"与
允许"这台手机能改我的 key"，用户心里的答案经常不一样；合成一个开关的后果是
用户为了图方便把读打开，**顺手也把写打开了** —— 而写能做的事包括"把 key 换成
一个攻击者控制的端点上的 key"（下一次桌面自己发请求就用它）。回环来源（桌面自己）两个都免闸门。

### 14.2 形状

```jsonc
PUT /api/ai/credentials
{ "credentials": { "DEEPSEEK_API_KEY": "sk-…" }, "rev": 12 }   // rev 见 §14.3
→ { "applied": ["DEEPSEEK_API_KEY"], "rejected": [ { "ref": "X", "code": "STALE_IGNORED" } ], "rev": 13 }
```

- **值只在请求体里出现一次**，不写日志、不回显（`GET` 才回，且要过下行那道闸门）。
- 写盘落在**桌面托管凭据文件**（`<storage>\ai\credentials.dpapi.json`，DPAPI 加密）——
  与桌面上手动填 key 是**同一个存储**，不许另起一份"来自手机的 key"。
- 拒的码：`FORBIDDEN`（开关没开/设备没授权，**永久性**）/ `STALE_IGNORED`（桌面那份更新，见下）/
  `BAD_PAYLOAD`（ref 形状不合法）。

### 14.3 冲突：给凭据也加一个 `rev`

凭据是"只写不读"的，没有天然的 `updated_at` 可比 —— 所以**每次写入把
`app_settings` 的 `ai.credentials.rev` 加一**，上行请求带上它读到的 `rev`：

- 手机 `rev` **小于**桌面 → `STALE_IGNORED`（桌面那份更新，手机上要如实提示"桌面上改过，先同步"）；
- 相等 → 应用，`rev` 加一；
- 大于（桌面被回滚过）→ 也只有相等才应用：**宁可让用户重试一次，也不许悄悄覆盖**；
- **请求里不带 `rev` = 强制覆盖**（`rev` 加一）。这条是给"用户刚刚显式点了『推给桌面』"用的：
  他手里没有 `rev` 的读数（界面没做"上次同步到哪一版"的展示），而这时候拿旧 `rev` 去比
  反而会把**用户刚刚的意图**判成过期。**"不带 rev" 只能是显式动作**，不许自动同步用它 ——
  自动路径必须带 `rev`，否则它就变成了"无脑覆盖"。

### 14.4 手机侧的"独立配置"落在哪

`android/harness` 的本地库里 `ai_providers` / `ai_models` 本来就是**本机表**
（不进镜像、不进同步白名单，见 `MobileSchema.kt` 的清单）—— 所以"手机上新建一个 provider"
不需要动同步契约。两端各有一份清单，交叉点是**凭据 ref**：

- 手机上配的 provider 若用桌面也有的 ref → 值来自手机自己那份（`keystore`）；
- 想让它俩一致 → 走上行/下行任一条（都由用户显式点）；
- **不做自动合并**：清单不同步、key 自动互推都是"静默改用户凭据"，这一版明确不做。

> ⚠ **上行要求手机侧"把密钥读出来"—— 这是全仓唯一一处允许这么做的地方，得写清楚。**
> `lib/core/credential_store.dart` 现在**刻意没有任何读值方法**（"值只写不读"，AIH-012），
> `DeviceCredentialStore` 只有 `hasCredential` / `putCredential` / `removeCredential`
> 三个桥动作。而上行必须把明文交出去，所以这一批要新增：
>
> | 层 | 新增 | 纪律 |
> | --- | --- | --- |
> | Kotlin 桥 | `getCredential(ref)` | **只被「推给桌面」这一个动作调用**；不许进日志、不许进事件、不许回显 |
> | Dart | `CredentialStore.reveal(provider)` | 名字刻意不叫 `read`/`get`：它一出现就说明"有东西要出本机"。远端实现**直接抛**（桌面那份不需要上行） |
> | UI | 「把本机密钥推给桌面」按钮 | 点之前要问一次（这把 key 会离开本机）；结果如实报 applied / rejected |
>
> 防线：一条用例断言"没有任何别的路径调用 `reveal`"（grep 生产源码，
> 与 R13 那条扫描同款做法）—— 否则这个口子会在某次"顺手读一下"里被用滥。

### 14.5 验收

| # | 验什么 | 判据 |
| --- | --- | --- |
| 1 | 开关默认关 | 新配对设备的 `allowCredentials` 与 `allowCredentialsUpload` 都是 `false`；不带它们打上行 → `403 FORBIDDEN` |
| 2 | 开了才生效 | 打开 upload → 手机推送 → 桌面上该 ref **真的能发请求**（上游 401 会当场露馅） |
| 3 | 冲突不静默 | 桌面先改一把 → 手机用旧 `rev` 推 → `STALE_IGNORED`，且桌面那把你**没被覆盖**（读出来比对） |
| 4 | 值不泄漏 | 上行/下行都不写日志；`GET /api/ai/tools`、`run-kit`、`/api/ai/providers` 里**永远只有 `credential:{configured,source,writable}`**（无值） |
