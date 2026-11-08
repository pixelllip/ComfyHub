# 跨网络连接：没有第三台设备时怎么做（2026-11-08 修订版）

> **缘起**：目标是移动端 ↔ 桌面端**跨网络**连接。上一版按"自建中继 + 共享 Rust 核心"设计，
> 前提是"有一台自己的服务器"。用户明确：**没有第三台设备建中继**。本版按这个约束重写。
>
> 结论先说：**中继是一台服务器，不一定是你的设备**。Tailscale 的 DERP 中继就是"别人的服务器、
> 免费给你用"；你的代码已经认识它（`100.64.0.0/10`）。真正缺的不是中继，是
> **"配对后记住多个地址 + 换网络时重挑"** —— 这是一段 Dart 代码，不是一台服务器。
>
> 身份沿用 [`pairing-design.md`](pairing-design.md)，安全四层沿用
> [`remote-access-security.md`](remote-access-security.md)。
>
> **2026-11-08 结论：暂缓。** 电脑是移动设备、常在校园 CERNET2；商用 5G 到 CERNET2
> 没有直连路径（`--direct-only` 对公网 IPv4 与桌面 IPv6 都超时），而"无外部依赖"又排除了中继。
> 移动电脑也无法自己当中继。所以**跨网络互联不作为 v1 目标**；同网（Wi-Fi）直连照旧。
> 重新评估的条件见 §10。

---

## 1. 结论先行

| 问题 | 结论 |
| --- | --- |
| 没有第三台设备，还能跨网络吗 | **能**。有公网 IPv6 就直连；没有就用 Tailscale（免费、无需你的设备）；再不行 Cloudflare Quick Tunnel。三者都只是"给你一个可达地址" |
| 要自建中继吗 | **现在不要**。自建中继只在"你有一台 VPS"时才有意义；没有就先不写 |
| 最该改的是什么 | **配对后把全部地址落库 + 换网重挑**（P0 实现已按用户要求回退；改走 IPv6，见 §5.1） |
| 共享 Rust 核心还要吗 | **暂缓**。没有中继就没有"两端对称的隧道客户端"这个前提；现在搬进 Rust 只加 FFI 成本，不解决"连不上" |
| 公网明文行不行 | **不行**。Tailscale / Cloudflare 自带加密；IPv6 / UPnP 直连必须给桌面加 TLS |

> 一句话：**你缺的不是服务器，是一条"从手机到桌面"的可达地址；Tailscale 免费给你这条地址，
> 你的 App 已经会用它，只差把地址记住并在换网时重挑。**

---

## 2. 约束三角：三者最多选二

| 你想要 | 代价 |
| --- | --- |
| 无第三方服务器 + 无第三台设备 + CGNAT 下可用 | **不可能**。CGNAT 下没有公网入口，必须有人替你中转 |
| 无第三方 + 无设备 | 只能赌 **IPv6 直连**（且要加密：WireGuard 或 TLS），运营商/路由器不给就无解 |
| 无设备 + CGNAT 可用 | 必须接受**第三方协调/中继**（Tailscale、Cloudflare）或**租一台 VPS** |
| 无第三方 + CGNAT 可用 | 必须有设备：VPS，或支持 Tailscale/ZeroTier 的路由器 |

**所以你的选择不是"建不建中继"，而是"用谁的中继"**：

- 用 Tailscale 的（免费，加密，装个 App 就行）；
- 用 Cloudflare 的（免费，TLS，不用账号，但地址每次重启会变）；
- 用自己的（租 VPS，约 ¥30/月，可控但要运维）；
- 都不用（赌 IPv6，且自己给桌面加 TLS）。

---

## 3. 现状盘点：其实已经完成 80%

| 能力 | 现状 | 出处 |
| --- | --- | --- |
| 桌面枚举可达地址（回环 / 局域网 / Tailscale / 其他） | ✅ 有 | `server-rs/src/mobile/net_urls.rs:36` `classify()`、`:94` `assemble()`（IPv6 自动加方括号） |
| `GET /api/net/urls` 报告自己的地址 | ✅ 有 | `net_urls.rs:164` `urls_json()` |
| 配对二维码带**多个**地址（局域网优先、Tailscale 次之） | ✅ 有 | `lib/core/pairing_payload.dart:46` `preferredUrl`、`:80` `encode()` |
| 手机端候选分类（含 Tailscale `100.64/10`） | ✅ 有 | `lib/core/sync_endpoints.dart:47` `classifyEndpoint()` |
| 探测 / 自动挑一台能连的桌面 | ✅ 有 | `sync_endpoints.dart:233` `probeSyncEndpoint()`、`:377` `autoPickBaseUrl()` |
| 启动时自动挑地址 | ✅ 有（仅 Android） | `lib/app.dart:507` `_pickDesktopAutomatically()` |
| 局域网 UDP 广播发现 | ✅ 有 | `lib/app.dart:533` `_startDiscovery()`、`server-rs/src/mobile/discovery.rs` |
| 从桌面拉"它还有哪些地址" | ⚠ 写好了但没人调（P0 实现已回退） | `sync_endpoints.dart` 的 `discoveredEndpoints()` |
| 配对后保存**全部**地址 | ❌ 只存首选那条（P0 实现已回退） | `pairing_section.dart` |
| 换网络 / 回前台时重挑地址 | ❌ 只在冷启动挑（P0 实现已回退） | `app.dart` |

**结论**：跨网络不是"从零建一套通信"，而是**把已经写好的候选管理接上**：

1. 配对成功时，把二维码里**所有**非回环地址写进 `baseUrlHistory`；
2. 配对成功后调一次 `discoveredEndpoints()`，把桌面自报的地址（Tailscale / IPv6）补进来；
3. 回前台 / 冷启动时，若当前地址不通，跑一次 `autoPickBaseUrl()` 重挑。

这三步做完，Tailscale / IPv6 地址就能被自动用上，**零新服务器、零 Rust**。

---

## 4. 五条路对比（按"没有第三台设备"筛过）

| 方案 | 需要你的设备 | 需要第三方 | CGNAT 可用 | 链路加密 | 改动量 | 备注 |
| --- | --- | --- | --- | --- | --- | --- |
| **Tailscale**（无 IPv6 时推荐） | 否 | 是（协调 + DERP，免费） | 是 | 是（WireGuard） | 小（接线 + 引导） | 手机装 App、开 VPN；地址 `100.x` 稳定，家里/外面都能用。⚠ 国内网络下 DERP 可能不稳，可换 ZeroTier / 国内组网工具（同类：都是"别人的中继"） |
| **Cloudflare Quick Tunnel** | 否 | 是（Cloudflare） | 是 | 是（到 CF 为 TLS） | 小（PC 跑 `cloudflared` + 二维码带 URL） | 不用账号/域名；URL 每次重启会变；CF 看得到明文。⚠ 若反代连的是 `127.0.0.1`，会打穿回环豁免（§7） |
| **IPv6 直连** | 否 | 否 | 视运营商 | **否**（WireGuard 或 TLS） | 中（WireGuard 零代码 / TLS 要证书） | 双方都有公网 IPv6 且路由器放行才成立；加密走 WireGuard 或 TLS |
| **UPnP 公网 IPv4** | 否 | 否 | **否**（CGNAT 无解） | **否**（要自己加 TLS） | 中 | 有公网 IPv4 才行；端口暴露给全网扫描 |
| **自建 VPS 中继** | 是（租的） | 否 | 是 | 是 | 大 | 唯一能"产品级零设置"的路，但要钱要运维 |
| P2P 打洞 | 否 | 部分（STUN 免费，TURN 要服务器） | 蜂窝网常失败 | 否 | 大 | 对称 NAT 下仍需中继兜底，不建议 v1 |

**推荐顺序（按你的网络，不是按"哪条最省事"）**：
有公网 IPv6 → IPv6 + WireGuard 或 TLS（零第三方、零月租）；否则 Tailscale；再否则 Cloudflare；
最后 VPS。§5.0 的三项体检就是用来判断你落在哪一档。

> ⚠ **P2P 不是"没有服务器"**：打洞要先交换地址（信令），失败还要 TURN 中继。
> 手机蜂窝网大量是对称 NAT，打洞成功率低 —— 这条路比 Tailscale 更贵、更不可靠。

---

## 5. 推荐路线

### 5.0 先体检：你有没有现成的路（5 分钟）

| 查什么 | 怎么查 | 结论 |
| --- | --- | --- |
| 桌面有没有公网 IPv6 | `Get-NetIPAddress -AddressFamily IPv6 \| Where-Object { $_.IPAddress -notlike 'fe80*' -and $_.IPAddress -ne '::1' }` | 有 `2xxx:` / `240x:` 开头的地址 = 有 |
| 手机蜂窝网有没有 IPv6 | 手机浏览器打开 `test-ipv6.com` | 显示 IPv6 地址 = 有 |
| 家里有没有公网 IPv4 | 路由器 WAN IP 与 `curl ifconfig.me` 对比 | 相同 = 公网；`100.64.x.x` 开头 = CGNAT |

- **两边都有 IPv6** → 走 P2（IPv6 + WireGuard 或 TLS），零第三方、零月租；
- **没有 IPv6，但有公网 IPv4** → 端口映射 + TLS（UPnP 或手动），零第三方；
- **CGNAT 且没有 IPv6** → 只能选 Tailscale（免费）或租 VPS（约 ¥30/月）。

### P0：把地址管理接上（1~2 天，Dart）

> ⚠ **2026-11-08 这版实现已按用户要求回退**（用户不装 Tailscale，改走 IPv6）。
> 下面这段仍是"地址管理"该做的事，但优先级排在"IPv6 可达 + 加密"之后（§5.1）。

- `pairing_section.dart`：配对成功后把 `payload.urls` 里所有非回环地址写进历史，
  再 best-effort 拉一次 `/api/net/urls`（`_rememberReachableUrls` / `_rememberDiscovered`）；
- `settings_store.dart`：新增 `rememberBaseUrls()`，只记候选、不切当前地址、不改"用户手填"；
- `app.dart`：新增 `_recoverDesktopOnResume()`，回前台 / 冷启动（已配对但地址不是自动）时，
  当前地址探测失败就在历史候选里重挑，成功后 `reloadPairingToken()` + `refreshAll()`；
- **验收**：手机在 4G 下冷启动 / 回前台，自动连上 IPv6 地址；回到 Wi-Fi 自动切回局域网。

### 5.1 IPv6 直连：先过三道关（当前主线）

用户 2026-11-08 定：**不装 Tailscale，网络有 IPv6**。IPv6 解决"可达"，**不解决"加密"**——
直接 `http://[240e:…]:8080` 等于把配对令牌明文发到公网。三道关都要过：

| 关卡 | 现状 | 要做的事 |
| --- | --- | --- |
| 服务端听不听 IPv6 | ❌ 开远程时绑 `0.0.0.0`（IPv4 only，`main.rs:126`） | 改 `[::]`（或双 listener）；`parse_addr` 认方括号 |
| 防火墙放不放行 | ❓ 未验证 | Windows 入站规则 + 路由器 IPv6 防火墙；手机侧确认有公网 IPv6 |
| 链路加不加密 | ❌ 后端只有明文 HTTP | 下面三选一；**直接 HTTP 不做** |

加密三选一：

- **WireGuard（推荐，零代码）**：PC 与手机各装 WireGuard，用 IPv6 做 endpoint 直连；
  App 连隧道内的 IPv4 地址（如 `10.9.0.2`），**后端监听一个字都不用改**。
  没有账号、没有协调服务器 —— 它不是 Tailscale。
- **真证书 + 域名**：桌面跑 Caddy/nginx 终止 TLS，域名 AAAA 指到家里 IPv6（DDNS）；
  手机零安装，但要有域名，且 IPv6 前缀变了要更新 DDNS。
- **自签 CA + 手机信任**：桌面生成 CA/证书，手机装 CA，并在 App 的 network security config
  里信任用户证书；不依赖第三方，但要动手机的信任库（安全面更大）。

> ⚠ **不要把"有 IPv6"当成"可以裸奔"**：`remote-access-security.md` §4 的②层（TLS）是硬要求。
> 真要在公网直接听 HTTP，至少先读 §6 的红线；本项目的默认答案是"不"。

### P0.5：把"为什么连不上"说清楚

设置页「同步与连接」显示：当前走哪条（回环 / 局域网 / IPv6 / 其他）、试过哪些候选、
各自错在哪。没有可达地址时，直接给下一步："检查 IPv6（含路由器防火墙）" / "装 WireGuard" / "手填地址"。

### P1：IPv6 体检与引导（半天）

- 设置页加一段"跨网络体检"：本机有没有公网 IPv6（`net_urls` 已枚举）、端口通不通；
- 说清两条路：WireGuard（推荐，两端装、用 IPv6 直连）或 TLS（要域名）；
- 不做自动安装、不引导注册 —— 只把路指清楚。

### P2：IPv6 直连 + 加密（二选一）

- WireGuard：桌面 / 手机各一份配置，App 连隧道内地址；不需要改代码，先做这个；
- TLS：桌面终止 TLS（Caddy/nginx 或自签 CA），手机信任；要域名或装 CA；
- 只有 P0/P1 不够用时再做。

### P3（可选）：VPS 中继

- 只有你有了 VPS 才做；届时把上一版的隧道协议、`via_relay` 修法、共享 Rust 核心
  原样拿回来（它们仍然是对的，只是前提没到）。

---

## 6. 为什么共享 Rust 核心暂缓

上一版采纳它，前提是"中继架构下两端都是隧道客户端"（对称：都要出站 WSS、重连、心跳、帧编解码）。
**没有中继，这个前提就不成立**：手机是客户端、桌面是服务端，角色不对称。

- 现在真正要做的是**地址候选管理**，这份逻辑已经在 `sync_endpoints.dart` 里，Dart 改几十行就够；
- 搬进 Rust 要付 NDK / 4 ABI / FFI 边界 / 体积的代价，**换不来"连得上"**；
- 等 P3（有 VPS）或要做 E2E 时，再按上一版的最小形态（`comfyhub-tunnel-protocol`）做。

**触发条件**：有 VPS 要上中继 / 要做 E2E / 要加 iOS。

---

## 7. 安全底线（跨网络不放松）

- **公网明文 HTTP 禁止**：令牌在明文链路上等于公开。Tailscale / Cloudflare 自带加密；
  IPv6 / UPnP 直连必须给桌面加 TLS（自签 + 固定指纹，或真证书）。
- 令牌、审批闸门、写白名单**一个字都不改**：跨网络不产生"远程专用"的放宽分支。
- **任何"本机反代"都会打穿回环豁免**：中继隧道端、Cloudflare / frp / nginx 若从 `127.0.0.1`
  连进来，`decide()` 会把公网请求当成本机（匿名放行、仅回环管理端点敞开）。
  修法只有一条：让请求带一个**可信的**"经反代"标记（`via_relay`），`decide()` 据此跳过回环豁免；
  或让反代从非回环地址连入（这样会要求令牌）。Tailscale 没有这个问题（来源是 `100.x`，天然非回环）。

---

## 8. 验收

1. 手机**关 Wi-Fi 用 4G** 打开 App：画廊能看、视频能拖（Range）、聊天能跑 SSE；
2. 回到 Wi-Fi：自动切回局域网（或继续 Tailscale，都算通过）；
3. 两条路都不通时：诊断页说清"试了哪几条、各自错在哪、下一步装什么"，**不假装连上**；
4. 没有 Tailscale、没有 IPv6 的用户：界面明确告诉他"需要一条可达路径"，而不是转圈。

---

## 9. 待拍板

1. IPv6 直连的加密方式：**WireGuard**（推荐，零代码）/ 真证书 + 域名 / 自签 CA + 手机信任；
2. 是否现在就把服务端改成双栈监听（`[::]`，IPv4 + IPv6 都收）—— 这是"直接 IPv6"的前提，
   WireGuard 路线不需要；
3. 手机与桌面是否都有公网 IPv6（先按 §5.0 体检，尤其路由器 IPv6 防火墙）。

---

## 10. 暂缓与重新评估（2026-11-08）

**为什么暂缓**：

- 电脑是移动设备、常在校园 **CERNET2**；手机商用 5G（电信 / 移动）到 CERNET2
  **没有直连路径**：`--direct-only` 对公网 IPv4 与桌面 IPv6 都超时，`ping6` 100% 丢包
  （而 `ping6` AliDNS 20ms）。
- 电脑是移动的，**没法自己当中继**（它到哪都在 NAT 后面，地址还会变）。
- "无外部依赖" + "移动电脑" + "跨网络" 三者不能同时成立；中继或 VPN 必须选一个。

**v1 保留什么**：

- 同网（Wi-Fi）配对 + 局域网发现 + 断线重连（现有能力；POC 也验证过 iroh 直连可用）；
- 地址候选 / `baseUrl` 的抽象保留（以后加中继不用改上层）；
- **不引入 iroh / WireGuard / 中继代码**，POC 留在 `poc/iroh-echo` 作证据。

**什么时候重新评估**（任一成立）：

1. 有一台**固定公网 IP** 的机器（VPS / 家里宽带 + 端口映射）——自建中继或 VPN 端点；
2. 电脑的主要位置变成**家庭宽带**（商用 `240e` / `2409` IPv6），手机同运营商 5G 可直连；
3. 接受 **WireGuard / Tailscale** 这类 VPN（用别人的中继，换零运维）；
4. 出现**必须远程**的硬需求（远程审批 / 远程跑 Run），且能接受中继成本。

> POC 结论见 [`poc/iroh-echo/README.md`](../poc/iroh-echo/README.md)：Wi-Fi / IPv6 直连、
> 按 EndpointId 发现、切网不断线都成立；唯一过不去的是"校园 CERNET2 ↔ 商用 5G"这条网络路径。
