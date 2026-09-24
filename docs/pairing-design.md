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
| 后端有配对/鉴权端点吗 | ❌ **没有**。Kotlin 侧 `route("/api")` 只挂了 prompt/tag/media/capture/ai 六族，**没有 `authRoutes()`**；Rust 侧 40 条路由里也**没有一条 `/api/auth`** | `server/src/main/kotlin/com/comfyhub/Application.kt:605-631`、`server-rs/src/routes.rs:165-205` |
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
> "推数据"各需要一台后端）。**现在 Rust 侧两条都有**（`docs/implementation-master-plan.md` §8
> "P4 第 22 步"）—— 配对落地时**直接做在 Rust 上**，手机只连这一个后端。

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
（`docs/implementation-master-plan.md` §8 "P4 第 22 步"），真库副本上两支后端 canon 零差异。
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
| 3 | 手机密钥库**复用 `KeystoreCredentials`**（Kotlin 侧 AES/GCM + SharedPreferences），而不是 `flutter_secure_storage` | 总纲 M6 行（`implementation-master-plan.md:201`）写的是 `flutter_secure_storage`。选复用是因为它**已经在生产代码里**、且 Dart 侧不需要再碰密钥；代价是"自己写的加密"要在评审时单独看一遍。若评审更信任成熟库，改成 `flutter_secure_storage` 只影响第 3 步 |
| 4 | 一机一令牌**在配对时就落地**（而不是 D10 的"以后再说"） | 代价是状态形状要升 `/2`；收益是吊销从第一天起就是"按设备"的 |
| 5 | 二维码**由后端出 PNG** 还是引 Dart 包 —— 本文**不定**，留给第 2 步当场拍 | 后端出图少一个 Flutter 依赖、但后端多一条渲染路径；引包则前端自足。这条不影响任何接口形状 |

---

## 10. 参考

- [`inbound-auth-design.md`](inbound-auth-design.md) —— 令牌闸门本身（D1~D12、§6 的 P0/P1/P2、§8 防线、§9 风险登记）
- [`remote-access-security.md`](remote-access-security.md) §5（配对与令牌的具体要求）、§4（四层防护）、§6（红线）
- [`sync-api-contract.md`](sync-api-contract.md) §0（`X-ComfyHub-Token` 与"连接鉴权 ≠ 操作审批"）
- [`implementation-master-plan.md`](implementation-master-plan.md) §1（桌面唯一真源）、§3.4（run-kit）、`:748-753`（两端分居的阻塞点）
- [`android-client-plan.md`](android-client-plan.md) §4（第 1 步鉴权、第 5 步局域网发现）
