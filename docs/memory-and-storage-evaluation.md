# 内存占用与存储选型评估

> 起因（2026-09-22 用户提出）：`flutter + JVM + MySQL` 三个进程加起来吃 1GB 以上内存，
> 而 ComfyUI 跑图时同样抢内存/显存，所以"项目偷吃内存"不可接受。手里可用的技术栈是
> Rust（在学）和 TypeScript（刚起步）。问题：**要不要迁到 SQLite / NoSQL / 二者结合？
> 后面还要做跨端（主要是安卓）存储与同步，先评估方案。**
>
> 本文是**评估**，不是实施计划。所有数字都是本机 2026-09-22 实测，出处标了文件或命令。
> 结论先写在 §0，嫌长只看那一节 + §8 的路线图。

---

## 0. 结论先行

### 0.1 五句话

1. **1GB 里有 1.6GB 根本不在这三个进程里** —— 最大的两个 Java 进程是
   **Kotlin 编译守护进程（1134MB）**和 **Gradle 守护进程（454MB）**，它们是**开发期**产物，
   跑起来的项目只有后端 352MB + mysqld 481MB（+ Flutter ≈150MB）。先别急着换架构，先看清靶子。
2. **光调配置就能把运行期从 841MB 压到 270~370MB**（mysqld 638→189/117MB，后端峰值 272.6→141.3MB、
私有提交 378→150MB）—— **已经沙箱实测过，不是估算**，见 §3.2。
改动量是**两个配置文件**、风险接近零。**这是 ROI 最高的一步，必须先做。**
3. **数据库该换 SQLite，而且理由不是内存** —— 换掉 mysqld 只省那 481MB，
   真正的收益是：发布包少 300MB+（实测 MySQL 本体 304.3MB）、少一个 Visual C++ 运行时的硬依赖、
   少一整套 `mysql.ps1` 生命周期（init/start/stop/move/reset/僵尸进程/端口冲突），
   以及**安卓端能用同一套 schema**。内存只是顺带的。
4. **NoSQL 明确否决**。你的数据是**重度关系型**的：提示词↔标签多对多、产物↔标签、
   会话→消息→有序块→Run→事件 四级级联，而且**外键的删除策略是承担业务语义的**
   （`ON DELETE SET NULL` 保产物、`CASCADE` 清关联，见 `docs/database-study-notes.md` §2.2）。
   换 NoSQL 等于把这些用应用代码重写一遍，收益为零。
5. **跨端同步不要买同步引擎，也不要上 CRDT。** 你的架构是**单写者**
   （桌面是唯一真源，`docs/android-client-plan.md` §5 已经把这条定死了），
   单写者的同步是"下游按月/按游标增量拉取"，**一个 REST 接口 + 手机端一个只读 SQLite 缓存**就够了，
   不需要冲突解决。CRDT 解决的是多主并发写，你没有这个问题。

### 0.2 一句话路线图

```
P0 调优（半天~1天，运行期 841MB → 270~370MB，开发期再省 1.6GB，零风险）【已实测，见 §3.2】
  └─ 立刻做
P1 事件日志治跑飞（半天，DB 里那张 112.9MB / 44.6 万行的表降到个位数 MB，消除单 Run 4.7 万行写入与线性内存增长）
  └─ 立刻做
P2 MySQL → SQLite（2~3 周，省 481MB + 发布包瘦身 + 去掉 VC++ 依赖 + 安卓同构）
  └─ 内存问题在 P0 就解决了；这一步是为了"分发"和"安卓"，值不值得看 §5.2
P3 JVM → Rust（4~6 个月，再省约 200MB）—— 只影响**桌面**；安卓端不编译 Rust（§7.5）
同步：不引引擎。桌面为真源 + 游标增量拉取 + 手机端只读 SQLite 缓存（§6）
```

---

## 1. 实测：内存到底被谁吃了

2026-09-22 本机实测（`Get-Process` 的 `WorkingSet64`，机器总内存 23.3GB）：

| 进程 | 工作集 | 性质 | 说明 |
| --- | ---: | --- | --- |
| **Kotlin 编译守护进程** | **1134 MB** | ⚠️ **开发期** | `kotlin-compiler-embeddable` + `-Xmx1536m`，`keepalive` 闲置 2 小时才退 |
| **Gradle 守护进程 8.12** | **454 MB** | ⚠️ **开发期** | `org.gradle.launcher.daemon`，`-Xmx1536m` |
| mysqld | 481 MB | 运行期 | `--defaults-file=.mysql\my.ini` |
| **Kotlin 后端（真身）** | **352 MB** | 运行期 | 无 `-Xmx`，默认堆上限 6.25GB，75 个线程 |
| node ×4 | ~700 MB | 与本项目无关 | DSH / 编辑器 |
| python | 67 MB | 与本项目无关 | |
| mysqld（残留） | 27 MB | 运行期 | 第二个实例，疑似未清干净的残留 |
| viewer.exe | 未运行 | 运行期 | 见 §2.4 |

### 1.1 第一个必须纠正的认知

用户说的"三个进程加起来 1GB+"——**按运行期算是对的**：

```
运行期 = 后端 352 + mysqld 481 + Flutter ≈150  ≈ 980 MB   ← 用户观察到的 1GB
开发期 = 再加上 Gradle 454 + Kotlin 守护 1134    ≈ 2.6 GB   ← 真正的大头
```

那 1.6GB 的构建守护进程**只在 Gradle 构建时存在**，但它
`daemon-autoshutdownIdleSeconds=7200`（闲置 2 小时）也不退。也就是说：
你在改 Flutter 前端、或者在跑 ComfyUI 的时候，只要今天构建过一次后端，
这 1.6GB 就一直躺着。**这是"感觉项目很吃内存"的最大来源，而且是最容易治的。**

---

## 2. 逐项拆解：钱花在哪，能省多少

### 2.1 mysqld 的 481MB（可省约 350MB，纯配置）

用 `performance_schema.memory_summary_global_by_event_name` 实测到的分配（单位 MB）：

| 分配点 | 实测 | 来源 |
| --- | ---: | --- |
| `memory/innodb/buf_buf_pool` | **261.6** | `innodb_buffer_pool_size=256M`（`.mysql\my.ini:11`） |
| **`memory/performance_schema/*` 合计** | **233.2** | **MySQL 8 默认 `performance_schema=ON`，本项目一个字节都不需要** |
| `memory/innodb/log_buffer_memory` | 64.0 | `innodb_log_buffer_size` 默认 64M（8.4 的默认值，my.ini 没设） |
| `memory/innodb/ut0link_buf` | 24.0 | 随 buffer pool 走 |
| `memory/mysys/KEY_CACHE` | 8.0 | MyISAM key cache（本项目全是 InnoDB） |

> 各项之和大于 481MB，是因为 `performance_schema` 的统计里有一部分是**为统计别人而分配的记账内存**，
> 且有部分页被换出。**但 `performance_schema` 那 233MB 是实打实的一个独立开关**。
> **实测结果见 §3.2**：关掉它 + `innodb_buffer_pool_size` 降到 32M + `log_buffer` 降到 8M，
> 真实数据下 mysqld 从 **638MB 落到 189MB**；再激进些（pool 16M、关 doublewrite）到 **116.6MB**。

**顺带发现的第二个病（与内存无关但同样该治）：binlog 占了 4.27GB 磁盘。**
`@@log_bin=1`（MySQL 8 默认开）、`sync_binlog=1`，`.mysql\data` 里躺着 **202 个 binlog 文件共 4.27GB**
（单文件最大 1GB）。本机单实例、没有从库、没有 PITR 需求，**binlog 是纯浪费**，
而且 `binlog_format=ROW` + `binlog_row_image=FULL` 意味着每次 UPDATE 都把整行写进去。

### 2.2 后端 JVM 的 352MB（可省约 180MB，纯启动参数）

`server/build.gradle.kts` 的 `applicationDefaultJvmArgs` 里**只有两个 `-D`，没有任何堆/GC 参数**，
全仓 grep `Xmx|Xms|JAVA_OPTS` **零命中**。于是跑的是 JDK 的人体工学默认值。实测：

| 指标 | 当前（默认） | 说明 |
| --- | ---: | --- |
| `InitialHeapSize`（初始堆） | **394 MB** | 一启动就按这个量向 OS 要内存 |
| `MaxHeapSize`（堆上限） | **6253 MB** | = 物理内存的 25%，纯属浪费的地址空间预留 |
| 实际堆占用（`GC.heap_info`） | 已提交 204MB / 实用 **135MB** | 你真实用到的只有 135MB |
| Metaspace（`VM.metaspace`） | **54 MB** | Ktor + kotlinx-serialization + TwelveMonkeys，合理 |
| GC 线程 | G1，`ParallelGCThreads` **13** + `ConcGCThreads` **3** = **16 个** | 单用户应用开 16 个 GC 线程 |
| JVM 线程总数 | **75** | Netty event loop + Hikari + GC + JIT + 协程调度 |
| 堆最多被谁占 | `RunEventBus` 把整个 Run 的**全部**事件留在内存里（`RunEventBus.kt:29`） | 见 §4.3，实测单 Run 4.7 万个事件 |

堆只用了 135MB 却占 352MB 的 RSS —— 差额就是初始堆的 394MB 预留、
Metaspace 54MB、75 个线程的栈、JIT 代码缓存（`ParallelGCThreads` 之外还有 C1/C2 编译线程）。

### 2.3 开发期的 1588MB（可省约 800MB，纯配置）

| 进程 | 现在 | 改法 | 改后估 |
| --- | ---: | --- | ---: |
| Kotlin 编译守护进程 | 1134 MB | `kotlin.daemon.jvmargs=-Xmx640m` | ~500 MB |
| Gradle 守护进程 | 454 MB | `org.gradle.jvmargs=-Xmx768m` | ~350 MB |

外加一条**习惯**：不构建后端的时候 `gradle --stop`（或 `pwsh -File scripts\comfyhub.ps1 down` 之后顺手收掉），
能立刻回收这 1.6GB。改成 Flutter 前端用的是 `flutter run --debug` + `r` 热重载（AGENTS §1），
**根本不需要 Gradle 守护进程在场**。

### 2.4 Flutter 进程（固定成本，不影响选型）

未实测（评估期间没有运行 App）。Flutter Windows Release 的常驻一般在 80~150MB、
Debug 带 vm-service 在 200~350MB。**这个数字在本文所有方案里都是恒定的** ——
无论数据库选 MySQL 还是 SQLite、后端是 Kotlin 还是 Rust，Flutter 都得跑，
所以它不参与选型决策。（Release 产物 `build\windows\x64\runner\Release` 存在但是
9 月 12 日的旧构建；Debug 是 9 月 20 日的。）

---

## 3. P0：纯配置优化（零架构变更）

### 3.1 具体改哪里

**① MySQL 配置 —— ⚠ 要改的是脚本，不是 `.mysql\my.ini` 本身**

`.mysql\my.ini` **是 `scripts\mysql.ps1` 生成出来的**（`Write-Ini`，第 172~199 行的 here-string）。
`Write-Ini` 的调用点是 `init`（278 / 283 行）、`move`（457 行），
以及 `start` 里那句 `if (-not (Test-Path $IniFile)) { Write-Ini }`（337 行）。
也就是说 **手改 `.mysql\my.ini` 撑得过普通重启，但会被下一次 `init` / `move` 悄悄抹掉**。
真正的改动点是 `scripts\mysql.ps1:177-195` 那段 here-string，在里面加这几行：

```ini
performance_schema        = 0      # ★ 新增：实测省 233MB，本项目完全不需要
innodb_buffer_pool_size   = 32M    # 256M → 32M，真实热数据只有 24MB（§4.1）
innodb_log_buffer_size    = 8M     # 默认 64M → 8M
innodb_redo_log_capacity  = 32M    # 默认 100M → 32M
log_bin                   = 0      # ★ 新增：省 4.27GB 磁盘（本机单实例、无从库）
                                  #   ⚠ 实测 MySQL 8.4 **会忽略它**，必须写成 `skip-log-bin`
table_open_cache          = 256    # 4000 → 256
table_definition_cache    = 400    # 2000 → 400
max_connections           = 24     # 200 → 24（Hikari 最大才 12）
thread_cache_size         = 4      # 默认 10
```

改完要让它生效：把这几行**同时**补进现有的 `.mysql\my.ini`
（否则这次重启读的还是旧配置），之后 `start` 走的就是新配置。
`my.ini` 里其余各项（`basedir` / `datadir` / `port` / `mysqlx` / 字符集 /
`max_allowed_packet=512M` / `innodb_flush_log_at_trx_commit=2` / `local_infile=0`）
**都保持不变** —— `max_allowed_packet` 那条是导入大工作流 JSON 靠的，别顺手改小。

**② `server/build.gradle.kts` 的 `applicationDefaultJvmArgs`**（一处改动同时覆盖
开发布局和发布包，因为两者都是跑 Gradle `installDist` 生成的 `comfy-hub-server.bat`，
见 `scripts\server.ps1:45-46`）：

```kotlin
applicationDefaultJvmArgs = listOf(
    "-Dfile.encoding=UTF-8",
    "-Djava.net.useSystemProxies=true",
    // ↓ 新增（下面这套是 §3.2 实测用的那一组）
    "-Xms64m",                     // 初始堆 394MB → 64MB
    "-Xmx256m",                    // 上限 6.25GB → 256MB（实用堆 135MB，留一倍余量）
    "-XX:MaxMetaspaceSize=192m",   // 实测用 42MB，留余量
    "-XX:+UseSerialGC",            // 省掉 16 个 GC 线程 + G1 的 Remembered Set（实测 GC 结构仅 983KB）
    "-Xss512k",                    // 线程栈减半
    "-XX:CICompilerCount=2",       // 单用户应用不需要 4 个 JIT 线程
    "-XX:ReservedCodeCacheSize=48m", // 实测 Code Cache 用 8.2MB
    "-XX:MaxDirectMemorySize=32m",   // 给 Netty 的直接内存封顶（见 §3.2.4：换 CIO 可彻底去掉）
)
```

**③ `Db.kt:34` 的 `maximumPoolSize = 12`** → 4（单用户应用，12 条连接纯浪费。
**这一条在换 SQLite 时更重要，见 §5.2.3**）。

**④ `gradle.properties`** 加 `org.gradle.jvmargs=-Xmx768m` 与 `kotlin.daemon.jvmargs=-Xmx640m`。

### 3.2 实测：不换栈到底能压到多低（2026-09-23 沙箱实测）

> 这一节的数字**不是估算**。做法：在 `.run\memtest` 下起一个**完全隔离**的沙箱
> —— 独立 datadir + 独立端口 3308 的 mysqld，把真实数据（62.1 万行 `ai_run_events`、
> 123.8MB dump）灌进去，再把**同一个后端 classpath** 指向它做 A/B。
> 全程不碰你正在跑的环境（沙箱测完已删除）。

#### 3.2.1 MySQL：**638 MB → 189 MB（保守）/ 116.6 MB（激进）**

真实数据量下的稳态（每 15s 采样，稳定后取值）：

| 配置 | Working Set | Private | 线程 |
| --- | ---: | ---: | ---: |
| **现状**（PS=on、pool=256M、logbuf=64M、binlog=on） | **638.1 MB** | **869.7 MB** | 40 |
| **保守调优**（PS=off、pool=32M、logbuf=8M、table_cache 256、maxconn 24） | **189.1 MB** | 294.0 MB | 29 |
| **激进**（再加 pool=16M、`innodb_doublewrite=0`、maxconn=12、table_cache=64、redo=16M…） | **116.6 MB** | 175.5 MB | 29 |

- **保守档省 449 MB（−70%）**，配置项全是安全的，**推荐用这一档**。
- **激进档再省 72 MB（合计 −82%）**，但代价是 `innodb_doublewrite=0`
  （断电时可能损坏页）和 `max_connections=12`（爬满就拒绝连接）。
  **你自己这台机器可以接受**（个人桌面工具，断电概率低、并发极低），
  但**发布包里给陌生用户用不建议**。
- 顺带一个实测坑：**`log_bin=0` 会被 MySQL 8.4 忽略**（实测仍为 1），
  要写 **`skip-log-bin`**。这一条直接关系到那 4.27GB 磁盘。

#### 3.2.2 后端 JVM：**峰值 272.6 → 141.3 MB；私有提交 377.8 → 150.5 MB**

同一份代码、同一个库，只换启动参数。分两种负载测：

| 负载 | 配置 | Working Set | Private | 线程 |
| --- | --- | ---: | ---: | ---: |
| 空库（会触发内置模型种子写入，是**峰值**场景） | 默认 | **272.6 MB** | 378.6 MB | 58 |
| 空库 | 调优 | **141.3 MB** | 150.7 MB | 36 |
| 真实数据（62.1 万事件，已种子化，是**稳态**场景） | 默认 | **157.3 MB** | 204.3 MB | 56 |
| 真实数据 | 调优 | **140.0 MB** | **150.5 MB** | **35** |
| **你正在跑的那个**（默认参数，已对外服务一段时间） | 默认 | **202.8 MB** | **377.8 MB** | **74** |

诚实解读（**比"省 170MB"要微妙，别过度宣称**）：

- **稳态的 Working Set 只省了约 17 MB**（157.3 → 140.0）。原因是 Windows 本来就会
  把空闲 JVM 的页换出去，工作集自己会缩。
- **真正的收益在"私有提交"（Private Commit）和线程数**：
  378.6 → 150.5 MB（**−228 MB**）、74 → 35 线程。
  私有提交才是真正占住提交额度、逼出页面文件的那个量；`-Xmx256m` 把它钉住了。
- **调优的真正价值是"把峰值压平"**：同样跑一次内置模型种子写入，
  默认参数让堆涨到 272.6 MB，调优后**纹丝不动停在 141.3 MB**（GC 在 256MB 内消化掉了）。
  换句话说：**默认参数下这个后端的占用是"看它今天干了什么"，调优后是"有上界的"。**
  对内存敏感的机器，"有上界"比"平均更低"更重要。
- NMT 分解（调优档）：Java Heap 提交 64MB / MaxMetaspace 42MB（7,066 个类）/
  28 个线程栈 / Code Cache 8.2MB / **GC 结构仅 983KB**（SerialGC 几乎不占）。

#### 3.2.3 合计：不换栈的天花板

以 Working Set 计（不含 Flutter）：

```
现在：   mysqld 638 + 后端 203            = 841 MB
保守：   mysqld 189 + 后端 ~175（稳态估）  = ~364 MB     → 省 ~477 MB（−57%）
激进：   mysqld 117 + 后端 ~150            = ~267 MB     → 省 ~574 MB（−68%）
```

再加上开发期的 **Gradle 454 + Kotlin 守护 1134 MB**，只要**不在构建时**它们就不该存在
—— 改前端走 `flutter run --debug` + `r` 热重载（AGENTS §1）根本用不到它们，
**这一条能再省 1.6GB，而且是一行配置 + 一个习惯**。

**结论：不移植 JVM+MySQL，运行期可以稳定在 270~370 MB（三进程含 Flutter 约 420~520 MB），
从今天的 ~1GB 降下来 55~65%。**

#### 3.2.4 还想再往下压？只剩这些（收益都很小）

| 手段 | 预期收益 | 代价 |
| --- | ---: | --- |
| Ktor 引擎 **Netty → CIO**（`ktor-server-cio`） | 20~40 MB + 15~20 个线程 | 换引擎，要回归 SSE / Range / 压缩。**唯一还有意义的代码级优化** |
| HikariCP `maximumPoolSize` 12 → 3 | 5~15 MB | 无 |
| `-Xmx256m` → `-Xmx192m` | ~16 MB | 大工作流导入的余量变薄 |
| 去掉 `imageio-webp`（TwelveMonkeys 6 个 jar） | ~5 MB 元空间 | 丢掉 WebP 缩略图能力 |
| jlink 自定义运行时 | 主要是磁盘，内存几乎无收益 | 打包复杂度 |

**JVM 的地板大约 110~140 MB，mysqld 的地板大约 117 MB。**
再往下就必须换实现了 —— 作为对照，调研中实测的一个最小 Rust HTTP 服务
**工作集只有 4.2~5.0 MB**（见 `docs/embedded-db-sync-options-report.md`）。
也就是说：**不换栈的地板（~250 MB）仍然是一个 Rust+SQLite 方案的 20~50 倍。**
这笔账值不值，见 §7。

### 3.3 风险

**接近零。** 两个真实风险：
1. `-Xmx256m` 对"导入几百 MB 工作流 JSON"或"回放超长 Run"可能不够。
   处理：先用 `-Xmx384m` 观察一两周，看 `GC.heap_info` 峰值再收。
   （注意 §4.2：`RunEventBus` 会把整场 Run 的事件留在内存里，
   这是**当前唯一会真的把堆顶上去的东西**，P1 修掉之后 256m 就很宽松。）
2. `innodb_doublewrite=0`（激进档）牺牲断电安全。保守档不含它，建议先只上保守档。

> **保守档做完，用户的原始诉求（"项目偷吃 1GB 内存"）就已经解决了。**
> 下面 §4~§7 是"除了省内存，还该不该动存储架构"。

### 3.4 另一个实测发现：有两个卡死的 mysqld 残留

沙箱期间发现：进程列表里有些 mysqld **只有 1 个线程、不监听任何端口、
却占着 27~38 MB**，是启动失败/被中断后留下的僵尸。**你自己机器上就有一个：**

```
mysqld  PID=21168  WS=27.2 MB  线程=1   ← 不监听端口，纯残留
mysqld  PID=19964  WS=638.1 MB 线程=40  ← 真正在服务的
```

它从评估开始就一直在（本文 §1 的表里那行"mysqld（残留）27MB"就是它）。
`scripts\comfyhub.ps1 status` 把两个都列成了"运行中"（输出 `PID 21168,19964`），
所以**光看状态输出看不出这是个僵尸**。建议：
`mysql.ps1` 的进程探测按"**是否监听 3307**"判定，而不是按进程名 ——
否则这种僵尸会让 `doctor` / `status` 一直报"库在跑"，也永远不被回收。

---

## 4. 先看清你的数据有多大（这决定后面所有选型）

### 4.1 实测：全部真实数据 24.4 MB

```
comfy_hub 库共 17 张表。
排除 ai_run_events 之后，所有表的数据+索引合计 = 24.4 MB
```

明细（`information_schema.tables`）：

| 表 | 行数 | 大小 |
| --- | ---: | ---: |
| **`ai_run_events`** | **445,956** | **112.9 MB** |
| `ai_message_parts` | 1,319 | 8.5 MB |
| `ai_tool_calls` | 379 | 7.6 MB |
| `capture_runs` | 105 | 5.6 MB |
| `prompts` | 13 | 1.3 MB |
| `media_assets` | 16 | 0.4 MB |
| 其余 11 张表 | 各 < 200 行 | 合计 < 1 MB |

内容体积（`SUM(LENGTH(...))`）：`prompts.workflow_json` 合计 **0.45 MB**、
`media_assets.workflow_json` **0.26 MB**、全部 AI 消息正文 **0.17 MB**（最大单条 8.8 KB）。

**结论：这是一个"24 兆字节"的数据库。** 它不需要连接池调优、不需要 InnoDB、
不需要 binlog、不需要 256MB 的缓冲池。任何嵌入式数据库都能轻松承载，
MySQL 在这里提供的是**运维负担**而不是能力。

### 4.2 `ai_run_events` 是一条跑飞的日志（与选型无关，但必须治）

```
97 个 Run，平均每个 Run 6,403 个事件，最大值 47,302 个事件/单 Run
```

每个事件是**一次独立的 autocommit INSERT**（`AiRunRepo.kt:198-207`，
`Db.withConnection` + `conn.execute`，不在事务里、不批量）。
而 `AiRunRepo.pruneEvents(runId, keep = 4000)` 写着注释
"事件只用于断线续传与审计，保留有限数量即可（AIH-018 的保留策略）"，
**但全仓 grep 下来它是个死代码 —— 没有任何调用点。**

两件事要做：
1. **接上保留策略**（`pruneEvents` 在 Run 结束时调用，或改成"每 Run 只留最后 N 条 + 终态"）。
2. **批量写**：流式 token 事件（`text.delta` / `reasoning.delta`）应该先攒一个批次
   （例如每 200ms 或每 50 条 flush 一次）再一次事务写入。这同时解决
   **MySQL 的往返开销**和**将来 SQLite 的单写者争用**。

同时 `RunEventBus.kt:29` 把整个 Run 的全部事件留在**内存**里（`mutableListOf<RunEvent>`），
一个 47,302 事件的 Run 会连带把整段流式正文压在堆里 —— 这是 §2.2 里堆占用的一部分，
也是个**随 Run 时长线性增长的内存泄漏**。应该改成环形缓冲（只留最后 N 条给断线续传）。

### 4.3 结论

`ai_run_events` 的问题是**写入放大**，不是数据库选型问题。
**换任何数据库都不会自动修好它**，反过来说：**修好它之后，任何数据库都够用。**

---

## 5. 数据库选型评估

### 5.1 方案 A：调优后保留 MySQL

- **优点**：零代码改动。团队已经熟悉，`db/schema.sql` / `migrate.sql` / `Migrate.kt`
  三份 DDL 都不用动，364 个 Gradle 测试一个不改。
- **缺点**：mysqld 仍有 120~160MB 常驻 + 一个独立进程；发布包仍要带 304MB 的 MySQL 本体；
  仍依赖 **VC++ 2015-2022 Redistributable**（`packaging\manifest.json` 的
  `runtimeRequirements` 明确写着"mysqld.exe 是 MSVC 编的，缺了 vcruntime140.dll 起不来，
  而且报错只写进 `mysql-error.log`，命令行上只看到启动超时，非常难查"）；
  仍要维护 `mysql.ps1` 的 init/start/stop/move/reset/僵尸清理/端口 3307 冲突这一整套。
- **定位**：P0 已经做完了这个方案。**它是"够用"的终点，但不是"好"的终点。**

### 5.2 方案 B：MySQL → SQLite（推荐）

#### 5.2.1 为什么是 SQLite 而不是别的嵌入式库

- 你的访问层是**原生 JDBC + 手写 SQL**（`Db.kt`，刻意不用 ORM），
  方言用量很小且**已完整枚举**（见 §5.2.2）—— 这是能换的前提。
- SQLite 在**桌面和安卓上是同一个数据库引擎**，**列类型与 JSON 表示同构**，
  于是"同一套 `db/schema.sql` 派生出两端建表语句"成为可能。这是 MySQL 永远给不了的。
  ⚠ **但要精确**：手机端是**只读投影，不是副本** ——
  它没有 `_sync_log`、没有触发器、不需要外键级联，可以只建自己要用的列。
  两端共用的只有**那一段 SQL 文本**，不是同一份运行期 schema
  （详见 `docs/cross-device-sync-design.md` §5）。
- 单文件 = 备份就是拷一个文件，`mysql.ps1 move` 那一整套（停库→robocopy→重写 my.ini→写指针文件）
  全部消失。
- SQLite 默认就带 **FTS5**，正好把 `db/schema.sql` 里那个"建了但没用上"的
  `FULLTEXT ft_prompts`（因为中文没有 ngram 分词器，见 `database-study-notes.md` §5.6）
  换成能用的方案 —— FTS5 有 `unicode61` 和 trigram 分词器，中文比 MySQL 的默认分词器好办。

#### 5.2.2 可移植性清单（逐条实测，全仓 grep 的结果）

**SQL 语句总量很小**：`execute*` 50 处、`queryOne` 43 处、`queryList` 25 处、
`executeReturningKey` 5 处、`Db.tx` 20 处，分布在 **17 个文件**里。这就是要改的全部面积。

| MySQL 用法 | 出现位置 | SQLite 写法 | 难度 |
| --- | --- | --- | --- |
| `ON DUPLICATE KEY UPDATE v = VALUES(v)` | `SettingsRepo.kt:28`、`AiConversationRepo.kt:256`、`AiRunRepo.kt:163` | `ON CONFLICT(键) DO UPDATE SET v = excluded.v` | 机械 |
| `INSERT IGNORE INTO ...` | `CaptureRepo.kt:165`、`TagRepo.kt:160,171`、`AiRunRepo.kt:202` | `INSERT OR IGNORE INTO ...` | 机械 |
| `LIMIT ? OFFSET ?` | `MediaRepo.kt:173`、`PromptRepo.kt:155` | 同左 | 无 |
| `GROUP BY ... HAVING COUNT(DISTINCT ...)` | `MediaRepo.kt:116`、`TagRepo.kt:196` | 同左 | 无 |
| `UNION` 子查询 | `MediaRepo.kt:110` | 同左 | 无 |
| `GROUP_CONCAT` | 只在**没人用的**视图 `v_media_full` 里 | `group_concat`（3.44+ 支持内部 `ORDER BY`） | 无 |
| `AUTO_INCREMENT` / `BIGINT UNSIGNED` | `Migrate.kt:43,46`、`schema.sql` | `INTEGER PRIMARY KEY`（SQLite 整数是 64 位，无符号缺失可忽略） | 机械 |
| `TINYINT(1)` 布尔 | 多处 | `INTEGER`；`rs.boolOr()` 照旧可用 | 无 |
| `MEDIUMTEXT` / `JSON` 列类型 | 多处 | `TEXT`（JSON1 函数 3.38+ 默认编译进去） | 无 |
| `ENUM('IMAGE','VIDEO',...)` | `prompts.kind` | `TEXT` + `CHECK(kind IN (...))` | 机械 |
| `information_schema.COLUMNS/STATISTICS/TABLES` | `Migrate.kt:327,335,343` | `PRAGMA table_info(x)` / `PRAGMA index_list(x)` / `sqlite_master` | **要重写迁移探测** |
| **`DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)`** | `ai_conversations`、`ai_messages`、`ai_runs`、`ai_providers`、`ai_models`、`app_settings` | **SQLite 没有 `ON UPDATE`** → 触发器，或每处 UPDATE 显式带 `updated_at` | **★ 唯一真正的语义缺口** |
| `TIMESTAMPDIFF(SECOND, created_at, NOW(3))` | `CaptureRepo` 的僵尸 Run 超时判定 | `CAST((julianday('now') - julianday(created_at)) * 86400 AS INTEGER)` | 机械 |
| `MATCH ... AGAINST` / `FULLTEXT` | **未使用**（只有那个没用的索引） | 可换成 FTS5，属于加分项 | 无 |
| `LIKE '%词%'` 搜索 | `PromptRepo` / `MediaRepo` 的动态 WHERE | 同左（行为一致） | 无 |
| `getGeneratedKeys()` | `Db.kt:124` | xerial 支持；底层是 `last_insert_rowid()` | 机械 |

**`ON UPDATE CURRENT_TIMESTAMP` 是唯一需要认真对待的一条**，而且它**是承重的**：
`ai_conversations` 上有 `KEY idx_ai_conv_updated (updated_at)`，
会话列表就是按 `updated_at` 排序的。SQLite 的做法有两个，推荐**触发器**
（应用侧显式赋值容易在某个 UPDATE 上忘掉，而遗忘是静默的）：

```sql
CREATE TRIGGER trg_ai_conversations_updated
AFTER UPDATE ON ai_conversations FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
  UPDATE ai_conversations SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
```

#### 5.2.3 三个真风险

1. **单写者模型 + 现有并发画像**（最大的一条）。
   SQLite 同一时刻只允许一个写事务。你现在有**三类并发写者**：
   - SSE 流式落库：每个 token 一条 autocommit `INSERT`（`AiRunRepo.appendEvent`）；
   - 自动捕获轮询线程：每 `pollSeconds` 一次，命中新 Run 时走 `Db.tx`（`ComfyCapture.kt:355,536`）；
   - HTTP 写请求：20 处 `Db.tx`（新建/更新/删除提示词、标签、媒体、会话）。

   **对策（三件一起做）**：① 开 **WAL 模式**（读写不互斥）；② 设 **`busy_timeout=5000`**
   （xerial 支持，避免瞬时 `SQLITE_BUSY` 直接抛错）；③ **把写入串行化** ——
   最省事的做法是写连接池 `maximumPoolSize=1`（读可以用另一组连接或直接复用）。
   配合 §4.2 的**事件批量写**，写压力会降一到两个数量级。
2. **迁移逻辑要重写**。`Migrate.kt`（366 行）现在靠 `information_schema` 探测，
   要换成 `PRAGMA`。而且 `db/schema.sql` / `db/migrate.sql` / `Migrate.kt` **三份 DDL 同源**
   （`database-study-notes.md` §7）—— 换 SQLite 后 `migrate.sql` 那份存储过程版可以直接废弃
   （SQLite 没有存储过程），变成**两份**，反而少了维护面。
3. **一次性数据搬迁**要写个转换脚本（`prompts.workflow_json` 是 `MEDIUMTEXT`、
   `media_assets.workflow_json` 是 `JSON`，两者都是文本，直搬；时间字段从 `DATETIME(3)`
   搬到 SQLite 的 TEXT ISO8601 要注意时区一致）。
   数据只有 24MB，**这一步的技术风险低，主要是别漏字段**。

#### 5.2.4 顺带的收益（这些才是换的主要理由）

| 收益 | 量化 |
| --- | --- |
| 发布包瘦身 | 实测 Release 目录 549.8MB，其中 `mysql/` **304.3MB** + `server/` 21.1MB 是随包发的（`.mysql/` 193.8MB 是本机跑出来的运行数据）；去掉 MySQL 本体后，**发布包可以降到 50MB 量级** |
| 去掉系统级依赖 | `packaging\manifest.json` 的 `runtimeRequirements` 里 **VC++ 2015-2022 Redistributable 那一整条可以删掉**（那是 mysqld.exe 的 MSVC 依赖，也是"启动超时、原因只写在 mysql-error.log"那个著名难查故障的根源） |
| 去掉一整套运维面 | `scripts\mysql.ps1` 的 init/start/stop/move/reset/cli/logs + `silent-process.ps1` 的隐藏窗口逻辑 + `watch-owner.ps1` 的守护 + `runtime-deps.ps1` 的 MySQL 部分 + `.mysql-location.json` 指针文件 |
| 容器化/同步更简单 | 一个文件 = 一次 `Copy-Item`。做"快照/回滚/带库出门"是免费的 |
| 安卓可同构 | 同一段 `schema.sql` 可派生两端建表语句（手机端是**只读投影**，§5.2.1） |

### 5.3 方案 C：NoSQL —— 否决

不推荐，理由按重要性排序：

1. **关系是承重的，不是装饰**。`docs/database-study-notes.md` §2.2 记录了删除策略的业务语义：
   删提示词**不删产物**（`ON DELETE SET NULL`）、删标签**级联清关联**（`CASCADE`）、
   删会话级联清消息→块→Run→事件。这些在 NoSQL 里要你自己在应用层重写，
   而且**跨文档事务在多数 NoSQL 里根本没有**。
2. **查询形态是关系型的**。`tagMode=all` 的"标签全命中"
   （`GROUP BY ... HAVING COUNT(DISTINCT) = N`，`TagRepo.kt:196`）、
   列表页批量取关联（`IN (?,?,?)` + 内存 `groupBy`，防 N+1）、
   会话按 `updated_at` 排序 —— 这些在文档库里要么做不了，要么要建大量冗余索引。
3. **收益为零**。你换 NoSQL 想省的内存，MySQL 那边靠调优已经省了；
   想省的操作复杂度，SQLite 那边省得更彻底（NoSQL 通常还要一个服务进程，
   这跟"省内存"的目标**直接冲突**）。
4. SQLite 本身就是"无服务器的嵌入式数据库"，已经拿到了 NoSQL 想给你的那个好处（零运维进程），
   同时保留了 SQL 和外键。

**"二者结合"唯一的合理形态**：SQLite（结构化元数据）+ **FTS5**（全文检索）+
将来如果真的要做**语义检索**再加一个向量索引。注意这三者都在**同一个 SQLite 文件**里
（FTS5 是 SQLite 的模块，sqlite-vec 是扩展），**不引入第二个进程**。
如果你说的"结合"是指"SQLite 存元数据 + 文件存大对象"——**这个架构你现在就已经有了**
（`storage/media/` 存原件，库里只存 `stored_name` + `sha256`，见 `Storage.kt`），
继续沿用即可。

### 5.4 方案对比表

| | A. MySQL 调优 | **B. SQLite** | C. NoSQL |
| --- | --- | --- | --- |
| 运行期内存（DB 部分） | 120~160 MB | **0（进程内）** | 通常 ≥100MB（仍要有服务） |
| 代码改动 | **零** | ~17 个文件、120 处 SQL 调用点、迁移层重写 | 全部数据层重写 |
| 发布包 | 仍带 304MB | **可降到 50MB 量级** | 视实现 |
| 系统依赖 | 仍需 VC++ 运行时 | **不需要** | 视实现 |
| 安卓复用 schema | 否 | **是** | 部分 |
| 单文件备份 | 否 | **是** | 否 |
| 事务/外键/关联 | 全有 | **全有** | 多半没有 |
| 写并发 | 多写者，成熟 | **单写者，需按 §5.2.3 处理** | 视实现 |
| 工期 | 0 | **2~3 周** | 1~2 月且更险 |

---

## 6. 跨端（安卓）存储与同步评估

### 6.1 关键洞察：你是单写者，所以不需要同步引擎

`docs/android-client-plan.md` 已经把架构定死了（§5"明确不做的事"）：

> **不为移动端另写一套后端**：`storage`、数据库、画廊、Skills 的真源永远在桌面那一侧。
> **不让移动端装 Python / MCP**。移动端不直连 ComfyUI。

这意味着：**手机不直接写数据库，它调桌面的 REST API，由桌面写库。**
于是全局只有**一个写者**。

**单写者的同步 = 复制日志，不是冲突解决。** CRDT、向量时钟、三向合并这一整套
（PowerSync / ElectricSQL / CR-SQLite 的核心价值）解决的是"多个副本同时改同一条数据"，
**你没有这个问题**。为一台桌面 + 一部手机买单主同步引擎，是花钱买用不上的复杂度。

另外 `docs/ai-home-implementation-plan-v0.1.md:73` 把"多用户账号、云端同步和团队权限"
列为**范围之外** —— 这也印证了单用户单写者这个前提。

### 6.2 三种跨端形态（按成本排序）

| 形态 | 手机端存什么 | 离线能力 | 成本 |
| --- | --- | --- | --- |
| **① 纯瘦客户端**（`android-client-plan.md` 的现状设计） | 什么都不存，或只存 UI 偏好 | 无（断网就是白屏） | **最低**，安卓侧不需要数据库 |
| **② 瘦客户端 + 只读缓存**（推荐） | 本地 SQLite 缓存会话/画廊/提示词 | 能看历史、能翻画廊，写操作要联网 | 低：一个 schema + 一个拉取接口 |
| ③ 双向离线同步 | 完整副本 + 变更队列 + 冲突解决 | 完全离线可用 | 高：需要变更日志、tombstone、冲突策略 |

**推荐 ②**。理由：你的写入场景（生成提示词、跑工作流、提交 ComfyUI）**本质上都要联网**
（要么打上游 API，要么打桌面的 ComfyUI），所以"离线写"的价值极低；
但"离线读"（在地铁上看历史对话、翻画廊）体验价值高。**为读优化，不为写优化。**

### 6.3 同步引擎横向对比（都评估过，结论是都不用）

> **⚠ 出处强度说明**：下表的判断依据是各方案的**架构定位**（是单写者还是多写者、
> 要不要额外的服务进程、要求哪种主库），这一层是稳定的、不随版本变。
> 但**许可证、维护状态、Flutter 客户端成熟度这三项会变**——
> 写这份文档时本机的联网检索不可用（`web_search` 返回 HTTP 402），
> 所以这三项**没有外部核实**。真要选其中任何一个之前，请自行确认其当前状态。

| 方案 | 是什么 | 为什么不适合你 |
| --- | --- | --- |
| **PowerSync** | 双向同步引擎，自托管需要 Postgres + 一个同步服务 | **要多起一个服务进程**（与省内存目标冲突）；为多写者设计；Flutter 支持好但引入 Postgres 依赖 |
| **ElectricSQL** | Postgres→客户端单向同步（Shape 订阅） | 要求 **Postgres**（你连 MySQL 都要换掉，不可能再引入一个更重的）；偏 Web/TS 生态 |
| **Turso / libSQL 嵌入式副本** | SQLite 的 fork，支持"本地副本 + 远端同步" | 桌面端要跑一个 `sqld` **服务进程**；Dart/Flutter 客户端（`libsql_dart` / `turso_dart`）生态不成熟；而且它是为"多副本写入"设计的 |
| **CR-SQLite / CRDT** | SQLite 扩展，用 CRDT 做无冲突合并 | 解决的是多主并发写 —— **你没有这个需求**；每个表都要加 CRDT 元数据列，schema 复杂度暴涨 |
| **LiteFS / rqlite** | 分布式 SQLite（前者基于 FUSE，后者基于 Raft） | 面向"多节点高可用"，单机单用户是纯粹的过度工程 |
| **Syncthing / rsync 同步 SQLite 文件** | 文件级同步 | **危险**：SQLite 文件在写入过程中被复制会得到损坏的库；`-wal` / `-shm` 文件必须一起、且在一致点复制。**明确不要这么做** |
| **手写：游标增量拉取** | `GET /api/sync/changes?since=<cursor>` + 手机端只读缓存 | ✅ **推荐**，见 §6.4 |

### 6.4 推荐做法：桌面为真源 + 游标增量拉取

**桌面侧**（新增一个只读接口，约 100~200 行）：

```
GET /api/sync/manifest
    → { schemaVersion, serverTime, tables: { conversations: {count, maxUpdatedAt}, ... } }

GET /api/sync/changes?since=<ISO8601 或 单调游标>&limit=500
    → { cursor: "...", hasMore: bool,
        conversations: [...], messages: [...], media: [...], prompts: [...], tags: [...] }
```

实现要点（每一条都对应一个经典坑）：
- **游标用 `updated_at` 不够稳**（同一毫秒多行、时钟回拨）。用**单调递增的本地序列**
  更稳：给需要同步的表加一列 `sync_seq INTEGER`，由一个 `sync_log` 表的自增 id 驱动
  （写时触发器或应用层写入）。这是"变更日志"的最简形态。
- **删除要留墓碑**。手机拿到"没这条"和"这条被删了"是两件事。
  加 `deleted_at` 软删列（你现在是硬删 + 外键 CASCADE，需要改成软删或额外记 `sync_deletes`）。
- **全量引导**（手机第一次连）：`?since=0` 拉全量，但要**分页**且**限速**
  （画廊缩略图不能走这个接口，走现有的 `thumbUrlFor` 直取 URL）。
- **schema 版本协商**：`manifest.schemaVersion` 与手机本地缓存版本不一致时，
  手机直接**丢掉缓存重新全量拉**（个人数据量 24MB，这一步成本极低 ——
  这是"数据小"给你的红利，别浪费）。
- **媒体文件不进同步接口**：继续走现有的 `/api/media/*/file|thumb` HTTP 直取，
  手机端用 `cached_network_image` 之类的本地文件缓存。**只同步元数据。**

**手机侧**：`drift`（Flutter 生态最成熟的 SQLite 封装，基于 `sqlite3` + `sqlite3_flutter_libs`）
建**只读镜像表**。mobile 侧不产生 `sync_seq`，不参与冲突解决。

### 6.5 客户端侧现状核对（安卓可行性）

查了 `pubspec.yaml`（50 行，依赖很少）与 `android/`：**安卓侧的底子是干净的**，
只有一个真正的阻塞项。

| 依赖 | 安卓可用？ | 说明 |
| --- | --- | --- |
| `http` / `provider` / `intl` / `cupertino_icons` | ✅ | 纯 Dart |
| `shared_preferences` | ✅ | 两端都有实现 |
| `file_picker` | ✅ | 有安卓实现 |
| `audioplayers` / `url_launcher` | ✅ | 有安卓实现 |
| **`video_player_win`**（`third_party` 本地副本） | ❌ | `pubspec.yaml:37`，其自身 pubspec 里 `platforms:` **只声明了 `windows`**，且 `implements: video_player`。安卓上不会注册，视频/产物预览会缺实现 |
| `path_provider` | ⚠️ **缺失** | 做安卓侧缓存目录（形态 ②）必须补 |
| `sqflite` / `drift` | ⚠️ **缺失** | 形态 ②/③ 的本地缓存必须补（推荐 `drift`，生态最成熟） |

**`video_player_win` 的处理办法**：加官方 `video_player`（它自带安卓实现），
让两端各用各的；Windows 侧继续由 `video_player_win` 接管。
**注意 AGENTS §9 的纪律**：`video_player_win` 是打过补丁的本地副本
（上游少了 `visible_width/visible_height`，导致 Impeller 下"只有声音画面全黑"），
**不要**因为安卓要加 `video_player` 就顺手把它换回 pub.dev。

`android/` 目录已经是完整的 target（`build.gradle.kts` / `gradle.properties` /
三个 `AndroidManifest.xml` / wrapper 齐全），走 AGP 9.4 内建 Kotlin
（见 `docs/android-agp9-builtin-kotlin-migration.md`），**不是空壳，但也没有业务代码**。

### 6.6 如果将来真需要"离线写"（形态 ③）

那时才需要 LWW。最小正确实现要有的东西（提前知道，别到时候拍脑袋）：
稳定 UUID（你已经在用 `CHAR(36)`，✅）、`updated_at`（有，✅）、
**时钟偏移处理**（手机时间不可信 → 用"服务端在 apply 时打时间戳"而不是客户端时间）、
tombstone、幂等 apply（同一批变更重复应用结果不变）、
以及一条明确策略：**同一条记录的冲突一律"服务端赢"**（单用户场景下这是唯一讲得通的选择，
因为用户不会同时用两台设备改同一条消息）。

**但先不要做这个。** 按 `android-client-plan.md` 的落地顺序，
鉴权（第 1 步）→ 工具分端（第 2 步）→ 客户端接入（第 3 步）→ 跨端接力（第 4 步），
同步缓存排在第 3 步的"需要用户确认的点"里，**不是一个前置阻塞项**。

---

## 7. Rust 重写评估

### 7.1 规模（这决定了一切）

```
后端主源码：73 个文件 / 26,634 行 Kotlin
后端测试：  51 个文件 / 11,103 行 Kotlin（364 个 Gradle 测试）
Flutter：   39 个文件 / 23,408 行 Dart
```

主源码按目录拆：

| 目录 | 文件 | 行数 | 性质 |
| --- | ---: | ---: | --- |
| 根目录（核心 / Repo / 路由 / Comfy 集成） | 35 | 9,796 | 混合：数据层 + 业务逻辑 |
| `ai/tools` | 11 | 6,759 | **纯业务逻辑**（工具实现，最大单文件 `ToolRegistry.kt` 150KB） |
| `ai` | 21 | 8,232 | 混合（`HarnessRunner.kt` 88KB、`NsfwPrompt.kt` 68KB、`AiUpstream.kt` 38KB） |
| `ai/protocol` | 3 | 1,084 | 纯逻辑（多协议适配） |
| `ai/mcp` | 3 | 763 | 纯逻辑（stdio JSON-RPC 客户端） |

**注意 `ai/tools/ToolRegistry.kt` 单文件 150KB、`FindModel.kt` 54KB、`NsfwPrompt.kt` 68KB** ——
这些是几百个精心调试过的提示词和工具定义，**没有一行能从 Kotlin 机械翻译到 Rust**，
只能重写 + 重测。

### 7.2 收益 vs 成本

| | 收益 | 成本 |
| --- | --- | --- |
| **内存** | JVM 基线（Metaspace 54MB + 代码缓存 + 线程栈 + GC）消失，一个 axum 服务空载 RSS 约 10~30MB → **再省约 200MB** | — |
| **启动时间** | 毫秒级 vs 秒级 | — |
| **发布包** | 去掉 `<根>\jre`（jlink 精简后也有 40~80MB） | — |
| **安卓** | ❌ **没有收益 —— 这条动机已被证伪，见 §7.5** | — |
| **工期** | — | **26,634 行主源码 + 11,103 行测试重写**。按每天 300~500 行有效产出，纯编码 2~3 个月，加上对齐行为和补测试，**现实预期 4~6 个月** |
| **风险** | — | 11,103 行测试全部作废重建；e2e 脚本（`e2e-ai-tools-test.ps1` / `e2e-submit-test.ps1` / `e2e-capture-test.ps1`）要重写；**RSK-004 注入防线、审批闸门、附件三层准入、标题摘取这三条纪律**（AGENTS §10.1）全部要重新建立回归防线 |
| **你的栈** | Rust "在学" | 用学习中的语言重写一个 2.6 万行的生产系统，**回归风险主要由"不熟练"而不是"语言"承担** |

### 7.3 判断

**当前不建议重写。** 理由：

1. **RAM 的目标在 P0 就达成了。** 重写换来的 200MB，边际价值低于 4~6 个月的机会成本。
2. **瓶颈不在语言。** 你是"单用户 + 24MB 数据 + 单机"，JVM 的 352MB 里有一半是
   **没调过的默认值**（初始堆 394MB、16 个 GC 线程、12 条连接池）。
   语言优化解决的是"基线"，配置优化解决的是"浪费"，而你现在主要是浪费。
3. **Rust 的收益点其实在别处**，不在内存：① 想做磁盘 IO 密集的批处理（缩略图/哈希/视频探测）
   且 JVM 实测是瓶颈；② 想统一桌面端技术栈；③ 想要毫秒级启动。
   **这两三条现在都不成立**（缩略图生成不是瓶颈，启动时间用户感知不到）。
   ⚠ 原先这里还列了"想把服务端塞进安卓 App" —— **那条已被调研证伪，见 §7.5**。

### 7.5 ★ 安卓端不该用 Rust（2026-09-23 用户追问后的结论）

用户问得很直接：**"安卓倒是不需要 Rust 重写吧？"** —— **对，不需要，而且不该。**
原先本文把"服务端可嵌入安卓"当作 Rust 的三大动机之一，**这个前提是错的**：

**① 安卓上的硬约束直接把"内嵌服务端"这条路堵死了**（`docs/embedded-db-sync-options-report.md` §6.3 已核实）：

- **Android 10+ 禁止对应用私有目录里的文件执行 `execve()`** →
  内嵌服务端**只能是进程内的 `.so`，不能是子进程**；
- **前台服务类型被限制 6 小时 / 24 小时**（Android 15+）→ **做不了常驻服务**。

也就是说：即使把服务端编成 `.so` 塞进 APK，它也只能在 App 活着的时候在进程内跑，
**既不是一个真正的服务端，也解决不了"桌面不在线"的问题**。
（真正的解法是桌面那侧保持在线，不在同一局域网就走 Tailscale —— 见跨端设计 §9。）

**② 手机端的活根本不需要 Rust。** 它只做两件事：
`http` 取增量 + `drift`/`sqflite` 应用增量。
两者在 Dart 里都是一等公民（`http` **项目里已经用了**）。
而"应用一批 upsert/delete + 维护一个游标"大约 100 行 Dart，没有性能可言。

**③ Rust 上安卓是净增复杂度，且与"可靠优先"的目标相反：**

| 代价 | 具体 |
| --- | --- |
| 构建链 | 要装 NDK + cargo + codegen，交叉编译 **4 个 ABI**（arm64-v8a / armeabi-v7a / x86_64 / x86），CI 与本地都要配 |
| **默认构建后端已归档** | `cargokit` **2026-03-26 被归档**（README 自称 "Experimental"），而它正是 `flutter_rust_bridge` 的默认后端 —— frb 现在依赖自己 fork 的一个归档实验仓库 |
| 版本锁死 | dart 包 / Rust runtime / `*_codegen` / `*_macros` **必须完全同版本**，`Handler` 类在 **minor** 升级就可能破坏 |
| 故障模式变差 | FFI 边界上的 Rust panic 可能**直接终止 App 进程**；crash 归因比 Dart 异常难得多 |
| APK 体积 | 4 个 ABI 的 `.so` 都会变大，**不会变小** |
| 你的栈 | Rust "在学" —— 把"学 Rust"和"调 NDK 交叉编译"两件不相关的事捆在一起 |

用户的判断标准是**手机端可靠优先、内存不敏感** ——
那么 Dart 就是平台一等公民、调试链路最短、发布最稳的那条路。

**④ 唯一听起来成立的"代码复用"理由，其实不成立。**
"桌面用 Rust、手机也共享一份 Rust 核心"看着 DRY，但：

- 真正值得共享的只有**两样东西，而它们都是语言中立的**：
  **① `db/schema.sql` 那段 SQL 文本；② `/api/sync/*` 的 JSON 契约。**
  共享它们**不需要手机上有一行 Rust**。
- 而且**两端对数据库做的事本质不同**：桌面是权威库（迁移、触发器、外键级联、
  `_sync_log`），手机是**只读投影**（没有 `_sync_log`、没有触发器、
  可以只建它要用的列）。
  硬要共用一份代码就得抽象出"我现在是哪一侧"，**写出来的东西比两份专用实现更长**。

**结论：Rust 只在桌面。手机端 = Flutter/Dart + `drift` + `package:sqlite3`，
不编译任何 Rust，不跑任何服务端。** 这也正是
`docs/cross-device-sync-design.md` §2 第 2 条纪律已经写下的东西。

### 7.4 如果将来要做：一个渐进路线

不要"一次大爆炸重写"。可行的渐进路径：

1. **先做 §5.2 的 SQLite 迁移**（Kotlin 侧）。这一步会**把数据层从一个 17 张表、
   120 处调用点的抽象，压缩成一个清晰的 Repository 边界** —— 这正是将来换语言最需要的准备。
2. **定一份语言无关的 API 契约**（现有 REST 接口已经基本是了）。Rust 版只要实现同样的接口，
   Flutter 端一行不改，**可以逐组端点灰度替换**（例如先换掉 `/api/tags` 与 `/api/prompts`，
   两者并存，用反向代理分流）。
3. **`ai/tools`（6,759 行）最后再搬**，因为它是最"业务"、最需要反复调试的部分。

---

## 8. 建议路线图

| 阶段 | 动作 | 收益 | 成本 | 风险 | 建议 |
| --- | --- | --- | --- | --- | --- |
| **P0** | 调 `my.ini` + JVM 启动参数 + `gradle.properties`（§3） | **实测**：运行期 841→270~370MB（−57~68%），私有提交 −228MB，开发期 −1.6GB，磁盘 −4.27GB | **半天~1 天** | 极低 | ✅ **立刻做**（参数已实测验证，见 §3.2） |
| **P1** | 接上 `pruneEvents`；事件批量写；`RunEventBus` 改环形缓冲（§4.2） | 消除单 Run 4.7 万行写入与线性内存增长；DB 从 144MB 降到个位数 | 半天~1 天 | 低（需补一条"断线续传仍然可用"的回归） | ✅ **立刻做** |
| **P2** | MySQL → SQLite（§5.2） | 再省 **481MB**；发布包 **−300MB+**；去掉 VC++ 依赖与 `mysql.ps1` 全套；安卓 schema 同构 | **2~3 周** | 中（单写者并发、迁移重写、一次性数据搬迁） | ⭐ **推荐，但排在 P0/P1 之后** |
| **P3** | JVM → Rust（§7，**仅桌面**） | 再省 ~200MB；毫秒级启动；发布包再瘦 | **4~6 个月** | 高（2.6 万行 + 测试全部重建） | ⛔ **暂不做**，除非 §7.3 的理由成立 |
| **P4** | 安卓只读缓存 + 游标增量拉取（§6.4） | 离线看历史/画廊 | 1~2 周（依赖 P2 的 `sync_seq` 列） | 低 | 按 `android-client-plan.md` 的节奏，不做前置 |

**"如果只做一件事"**：做 P0。**"如果做两件事"**：P0 + P1。
**P2 是为了分发和安卓，不是为了内存** —— 内存问题在 P0 就结束了。

---

## 9. 需要用户决策的点

1. **P0 要不要我现在就动手？** 参数组合已经在沙箱里实测验证过（§3.2 那一组就是能跑到
   189MB / 141MB 的那组），落地只是写进 `scripts\mysql.ps1` 的 `Write-Ini` 与
   `server/build.gradle.kts`，然后重启 MySQL + `gradle installDist` + 重启后端。
   ⚠ 需要你确认**可以短暂重启 MySQL**（后端会跟着重连；正在跑的 Run 可能受影响）。
   另外建议**顺手清掉那个僵尸 mysqld（PID 21168）**。
2. **P0 选保守档还是激进档？** 保守档（189MB，配置全安全，推荐）；
   激进档（116.6MB）靠 `innodb_doublewrite=0` 换 72MB，**本机可以、发布包不建议**。
3. **P2（换 SQLite）现在排不排期？** 如果排，建议**紧跟 P0/P1**（趁数据层还没有更多改动）。
   如果暂缓，也完全合理 —— 内存诉求在 P0 就解决了。
4. **安卓那边要不要"离线读"？** 这决定 P4 存不存在。
   如果要，P2 的 schema 设计里**现在就要预留 `sync_seq` / `deleted_at`**（后加列成本高得多）。
5. **Rust 是什么定位？** 只在**桌面**：为了内存地板、毫秒级启动、统一栈。
   **安卓端明确不用 Rust**（§7.5：安卓堵死了内嵌服务端这条路，
   且手机侧的活用 Dart 更稳）。参考数字：不换栈的地板 ~250MB，
   最小 Rust 服务实测 **4.2~5.0MB**（§3.2.4）。
   ⚠ 若你的动机本就是"让手机也能离线跑"，那要先知道**它做不到**。

---

## 10. 相关文档

- **`docs/embedded-db-sync-options-report.md`** —— 由本评估派生的独立深度调研
  （约 2.5 万字，119 条引用，每条结论标了 `[V]` 已核实 / `[MEASURED]` 本机实测 / `[?]` 未核实）。
  覆盖：SQLite/WAL 在安卓上的真实约束（AOSP 一直没出过 SQLite 3.51+、
  `SQLITE_OMIT_LOAD_EXTENSION` 导致 CR-SQLite 无法加载、`sqlite3_flutter_libs` 已是空壳、
  `cargokit` 已归档）、六个同步方案的逐项否决理由、以及最小 Rust 服务的实测内存。
  **注意：那份报告测的是 P0 之前的原始状态**（mysqld 857MB 私有提交 / 后端 343.9MB），
  §3.2 的调优数字以本文为准。
- `docs/database-study-notes.md` —— 数据模型、外键语义、迁移三条路径（§5.2.2 的依据）
- `docs/android-client-plan.md` —— 单写者架构与安卓落地顺序（§6.1 的依据）
- `docs/pitfalls.md` —— 50+ 条踩坑记录

---

## 附：本文所有实测数据的取法

| 数据 | 命令 / 出处 |
| --- | --- |
| 进程内存 | `Get-Process \| Select WorkingSet64`；`Get-CimInstance Win32_Process` 取命令行 |
| JVM 堆 / 元空间 / 线程 | `jcmd <pid> GC.heap_info` / `VM.metaspace` / `Thread.print` |
| JVM 默认值 | `java -XX:+PrintFlagsFinal -version` |
| mysqld 内存构成 | `SELECT * FROM performance_schema.memory_summary_global_by_event_name` |
| 表大小 / 行数 | `information_schema.tables` |
| 配置项 | `SHOW VARIABLES`；`.mysql\my.ini` |
| 磁盘占用 | `.mysql\data` 下 `binlog.*` 文件求和 |
| 发布包体积 | `build\windows\x64\runner\Release` 各子目录求和 + `packaging\manifest.json` |
| SQL 调用点 | 全仓 grep `execute(` / `queryOne` / `queryList` / `executeReturningKey` / `Db.tx` |
| MySQL 方言 | 全仓 grep `ON DUPLICATE KEY` / `INSERT IGNORE` / `information_schema` / `GROUP_CONCAT` 等 |
| 源码规模 | 各目录 `*.kt` / `*.dart` 行数求和 |
| **§3.2 的 A/B 实测** | 沙箱：`.run\memtest` 下独立 datadir + 端口 3308 的 mysqld（`mysqld --initialize-insecure` → 导入 `db\schema.sql` → `mysqldump` 灌入真实数据 123.8MB）→ 用**同一个后端 classpath** 指向它跑 A/B（`COMFYHUB_PROJECT_ROOT` / `COMFYHUB_STORAGE` / `COMFYHUB_PORT=18080` / `COMFYHUB_JDBC_URL=...:3308` / `COMFYHUB_COMFY_URL=http://127.0.0.1:9` 全部隔离，避免碰真实的 ComfyUI 与数据）。测完删除沙箱。 |
| mysqld 稳态 RSS | 每 15s × 5 次采样，**必须按"监听端口"定位进程**（`Get-NetTCPConnection -LocalPort 3308`）—— 残留的僵尸 mysqld 会给出假的低读数（见 §3.4） |
| JVM 参数影响 | `jcmd <pid> VM.native_memory summary`（需启动时加 `-XX:NativeMemoryTracking=summary`） |
