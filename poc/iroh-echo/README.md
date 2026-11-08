# iroh echo POC（**不属于主工程**）

目的：在**不碰主工程**的前提下，验证 iroh 能否满足"一次配对、随时切网"。
主工程代码一行没改；这里只有两个独立 crate（`server` / `client`）。

## 结论（2026-11-08）

### Wi-Fi（OnePlus 8T / KB2000 + Windows，同一 Wi-Fi）

| 场景 | 结果 | RTT |
| --- | --- | --- |
| Wi-Fi 直连（IPv4 `10.44.130.232`） | ✅ | 5–100 ms |
| Wi-Fi 直连（IPv6 `2001:da8:…:230e:97fa:98a:1026`） | ✅ | 6–100 ms |
| **只给 EndpointId（不给任何 IP）** | ✅ 自动发现并直连 | 5–85 ms |
| 强制中继（`--relay-only`） | ✅ | 195–280 ms |
| Wi-Fi 关 6 秒再开，**直连** | 连接没断；恢复后继续（一次 8–11 s 卡顿） | — |
| 同上，**中继模式** | ❌ 连续 6 次重连超时，需重启 endpoint | — |

> **"只给 EndpointId"那条是关键**：手机端一个 IP 都没传，iroh 靠 DNS/Pkarr 发现找到了桌面
> 并直连成功。这对应"配对只交换公钥、之后 IP 变了也能找到"——也就是你要的"一次配对"。

### 蜂窝 5G（PJX110，中国移动 + 中国电信双卡，NR_SA）

| 场景 | 结果 | RTT |
| --- | --- | --- |
| Wi-Fi → 5G（关 Wi-Fi） | ✅ 连接没断，16.5 s 后恢复 | 244–18018 ms，频繁 5–10 s 尖峰 |
| 5G 强制中继（`--relay-only`） | ✅ 能连 | 277–9792 ms |
| 5G 只直连（`--direct-only`，公网 IPv4 `183.236.19.190`） | ❌ 超时 | — |
| 5G 只直连（`--direct-only`，桌面 IPv6 `2001:da8:…`） | ❌ 超时 | — |

**为什么 5G 也这么慢（根因，不是 iroh 的锅）**：

- **5G 空口不是瓶颈**：手机 `ping6` AliDNS（`2400:3200::1`）**20 ms**，`ping` AliDNS **20–51 ms**。
- 桌面在 **CERNET2**（`2001:da8:…`），手机蜂窝在 **中国电信 / 移动 5G SA**（`240e` / `2409`）。
- 手机 `ping6` 桌面：**100% 丢包**；`ping` 桌面公网 IPv4 `183.236.19.190`：**100% 丢包**。
- 用 `--direct-only`（清掉中继传输）分别试公网 IPv4 与桌面 IPv6：**都超时**
  → 确认不是 iroh 选错路，是根本没有直连路径。
- 也就是：**5G 下没有直连路径**，只能回落 iroh 的公共中继（`aps1`，境外），
  所以延迟 300–2000 ms 且抖动大 —— 这是运营商到 CERNET2 的路由/策略问题，
  换 5G、换手机都一样。

**结论**：在这台桌面所在的网络（校园 CERNET2）里，5G 直连不可用。
如果桌面最终放在**家庭宽带**（电信 / 联通 / 移动的 `240e` / `2409`），手机同运营商 5G
到家里 IPv6 大概率能直连（20–50 ms 量级）——但这次没条件验证。
如果家里也是 CERNET2 / 运营商封锁，就必须有一台**国内中继**（VPS），公共 iroh 中继不够用。

## 跑法

### 1. 桌面 echo 服务

```powershell
cargo build --release --manifest-path poc\iroh-echo\Cargo.toml
poc\iroh-echo\target\release\server.exe
```

启动后打印：

```
ENDPOINT_ID=...        # 手机端拨号用（公钥，不随 IP 变）
DIRECT=ip:port         # 直连候选（IPv4 / IPv6 都有）
RELAY=https://...      # 中继地址
BOUND=0.0.0.0:...      # 实际绑定端口
BOUND=[::]:...
READY
```

### 2. 桌面客户端（对照）

```powershell
poc\iroh-echo\target\release\client.exe <ENDPOINT_ID> <ip:port>
# 强制中继：
poc\iroh-echo\target\release\client.exe <ENDPOINT_ID> relay:https://aps1-1.relay.n0.iroh.link./ --relay-only
```

### 3. 手机客户端（Rust CLI，adb）

```powershell
$env:ANDROID_HOME="$env:LOCALAPPDATA\Android\Sdk"
$env:ANDROID_NDK_HOME="$env:LOCALAPPDATA\Android\Sdk\ndk\28.2.13676358"
$env:ANDROID_NDK_ROOT=$env:ANDROID_NDK_HOME
# ⚠ TMP/TEMP 必须指到工作区内，否则 clang 写临时文件会被拒（"unable to make temporary file"）
$env:TMP="D:\myProject\FlutterProject\viewer\poc\iroh-echo\.tmp"
$env:TEMP=$env:TMP
# 在 poc\iroh-echo 目录下执行：
cargo ndk -t arm64-v8a build --release -p client
adb push target\aarch64-linux-android\release\client /data/local/tmp/iroh-client
adb shell chmod 755 /data/local/tmp/iroh-client
adb shell /data/local/tmp/iroh-client <ENDPOINT_ID> 10.44.130.232:<port>
```

## 已知问题（都是"接进 App 前"要处理的）

1. **裸 CLI 在 Android 上会 panic**：`ndk-context ... android context was not initialized`。
   它发生在后台任务，主循环还能跑；Kotlin 绑定会初始化 context，正式接法不会这样。
2. **`--relay-only` 在 Wi-Fi 断/开后重连失败**（连续超时）。正常模式（直连优先、中继兜底）
   没复现；Android 上 iroh 官方建议前后台切换时 `shutdown()` + 重新 `bind()`，App 里要照做。
3. **中继 / DNS 默认走 iroh 公共基础设施**（`aps1-1.relay.n0.iroh.link`）。
   要完全自持，得自建 `iroh-relay` + `iroh-dns-server`（需要一台服务器）。
4. **dev profile 在 MSVC 下编 `ring` 会失败**（临时文件权限）；POC 一律用 `--release`。

## 下一步

- 5G 已测：**校园 CERNET2 下直连不可用**，只能走境外中继（不可用）；
- 换到**家庭宽带**再测一次 5G 直连（这是唯一能判断"iroh 够不够用"的现场）；
- 如果家里也是 CERNET2 / 运营商封锁：先解决"国内中继"（一台 VPS），再谈 Kotlin 绑定；
- 直连能通再做 **Kotlin 绑定 + 本地代理**（仍然先放 `poc/`，不碰主工程）。
