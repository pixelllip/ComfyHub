# ComfyHub 数据库学习笔记

> 目的：把本项目里"和数据库有关的一切"抽出来，按 **选型 → 建模 → 访问层 → SQL 技巧 → 事务 → 迁移 → 运维 → 与代码的边界**
> 串成一条可学习的线。每条结论都能在仓库里找到出处（文末有文件索引）。
>
> 读完你应该能回答：为什么不用 ORM？为什么库在 3307？`run_key` 和 `sha256` 各防什么？
> 新增一个字段要改几个地方？文件和库谁先落、谁先删？

---

## 0. 一图看懂：数据库在这个项目里的位置

```
┌──────────────────────── Flutter App (lib/) ────────────────────────┐
│  设置页只存"偏好"（shared_preferences: 例如 mysql 数据目录）        │
│  ★ 完全不碰 SQL，一行 JDBC 都没有，所有数据都走 HTTP               │
└───────────────────────────────┬────────────────────────────────────┘
                                │ REST / JSON  (127.0.0.1:8080)
┌───────────────────────────────▼────────────────────────────────────┐
│  Kotlin 后端 (server/, Ktor + Netty)                               │
│  Routes  →  Repo（PromptRepo / MediaRepo / TagRepo / CaptureRepo…）│
│                         │ 原生 JDBC + PreparedStatement            │
│                         ▼                                          │
│              Db.kt —— HikariCP 连接池 + queryList/queryOne/tx      │
└───────────────────────────────┬────────────────────────────────────┘
                                │ JDBC (mysql-connector-j 9.4.0)
┌───────────────────────────────▼────────────────────────────────────┐
│  MySQL 8.4（免安装 zip 版，127.0.0.1:3307，库名 comfy_hub）         │
│  prompts / tags / prompt_tags / media_assets / media_tags          │
│  capture_runs / app_settings  +  视图 v_media_full                 │
└────────────────────────────────────────────────────────────────────┘
                                ▲
                    scripts\mysql.ps1 负责这台库的"生老病死"
                    （init / start / stop / move / schema / seed / cli …）
```

**三条最重要的分层原则**（学习时先记住这三条，细节都从这里长出来）：

1. **库只存元数据，二进制文件在磁盘**（`storage/media/`）。库里有 `stored_name`（磁盘文件名）和
   `sha256`（内容指纹），没有 BLOB。理由：备份、迁移、流式播放（Range）都比塞 BLOB 简单。
2. **前端零 SQL**。Dart 侧只有 `settings_store.dart` 用 `shared_preferences` 存本地偏好，
   真正的业务数据一律通过 REST 走后端。好处：后端换实现（甚至换数据库）前端不用动。
3. **SQL 全部集中在 `*Repo.kt`**。路由层只做参数解析和 HTTP 语义，SQL 只在 Repo 里写。

---

## 1. 技术选型与理由

| 选择 | 具体 | 为什么（代码里的原话/推断） |
| --- | --- | --- |
| 数据库 | MySQL **8.4**（zip 免安装版） | 免安装、免管理员；开发者机器上解压即用，发布包能整包带走 |
| 端口 | **3307** | 避开本机常见的 3306，不会跟系统里已有的 MySQL 打架 |
| 库名 | `comfy_hub` | |
| 字符集 | `utf8mb4` / `utf8mb4_unicode_ci` | 提示词里有 emoji 和生僻字；`utf8mb4` 才是"真 UTF-8"（`utf8` 是 3 字节残缺版） |
| 访问方式 | **原生 JDBC，不上 ORM** | `Db.kt` 注释写明：查询里有 `GROUP_CONCAT` / JSON / 动态条件等 MySQL 方言，直接写 SQL 更可控，也避免 ORM 版本兼容问题 |
| 连接池 | **HikariCP 6.3.3** | 事实标准，配置少、性能好 |
| 驱动 | `com.mysql:mysql-connector-j:9.4.0` | MySQL 官方驱动新坐标（老的 `mysql:mysql-connector-java` 已废弃） |
| 时间类型 | `DATETIME(3)`（毫秒精度） | `CURRENT_TIMESTAMP(3)` 默认值 + `ON UPDATE` 自动维护 `updated_at` |
| 迁移 | **自己写的 `Migrate.kt` + `information_schema` 探测** | 不引 Flyway/Liquibase：需求只是"加列加表"，手写 100 行够用且零依赖 |

`server/build.gradle.kts` 里就两行是数据库相关：

```kotlin
implementation("com.zaxxer:HikariCP:6.3.3")
implementation("com.mysql:mysql-connector-j:9.4.0")
```

---

## 2. 数据模型（ER 关系）

```
                        ┌─────────┐
                        │  tags   │  全局标签词表（normalized 唯一）
                        └────┬────┘
              ┌──────────────┴──────────────┐
              │                             │
      ┌───────▼────────┐            ┌───────▼────────┐
      │  prompt_tags   │            │   media_tags   │   两张纯关联表
      │ PK(prompt_id,  │            │ PK(media_id,   │   复合主键天然去重
      │    tag_id)     │            │    tag_id)     │
      └───────┬────────┘            └───────┬────────┘
              │                             │
        ┌─────▼──────┐   1    0..n   ┌──────▼────────┐
        │  prompts   ├──────────────►│ media_assets  │
        │  提示词主体 │               │ 图片/视频/音频 │
        └─────┬──────┘               └───────────────┘
              │  ON DELETE SET NULL（删提示词不删产物，只解除关联）
              │
        ┌─────▼────────┐        ┌───────────────┐
        │ capture_runs │        │ app_settings  │
        │ 一次生成的记录│        │ 运行期 k/v 设置│
        └──────────────┘        └───────────────┘
```

### 2.1 七张表 + 一个视图

| 表 | 主键 | 关键唯一约束 | 作用 |
| --- | --- | --- | --- |
| `prompts` | `id` BIGINT UNSIGNED AUTO_INCREMENT | — | 提示词 + 全部生成参数（checkpoint / loras / sampler / steps / cfg / seed / 尺寸 / batch） |
| `tags` | `id` | `uk_tags_normalized (normalized)` | 全局标签词表，`use_count` 冗余热度计数 |
| `prompt_tags` | `(prompt_id, tag_id)` | 复合主键 | 提示词 ↔ 标签 多对多 |
| `media_assets` | `id` | — | 产物元数据（`prompt_id` 可空，`sha256` 用于内容去重） |
| `media_tags` | `(media_id, tag_id)` | 复合主键 | 产物 ↔ 标签 多对多 |
| `capture_runs` | `id` | **`uk_capture_runs_key (run_key)`** | 自动捕获的幂等账本 |
| `app_settings` | `k` | 主键即唯一 | 运行期设置 k/v（自动捕获配置存成一段 JSON） |
| `v_media_full` | — | 视图 | 产物 + 关联提示词 + `GROUP_CONCAT` 聚合标签名 |

### 2.2 建模上值得学的 8 个细节

1. **`normalized` 列做唯一键，`name` 列做展示名。**
   标签"8K"和"8k"应当是同一个标签 → `normalized = 小写去空格`（`TagRepo.normalize`）建唯一索引，
   而 `name` 保留用户第一次输入的大小写。这是"展示与规范分离"的经典手法。

2. **外键的删除策略是按业务语义分别设计的**，不是一刀切：
   - `prompt_tags` / `media_tags` → `ON DELETE CASCADE`：删提示词，它的关联行自然没意义；
   - `media_assets.prompt_id` → `ON DELETE SET NULL`：删提示词**不删产物**（用户辛苦生成的图不能跟着消失），
     只是"变成未归类"；
   - `capture_runs.prompt_id` → `ON DELETE SET NULL`：删提示词后仍然保留"这次生成发生过"的日志。

3. **JSON 列的使用是克制的**：`loras`（`[{name, weight}]`）、`extra_params`（任意 k/v）这种
   "结构不固定、又不需要按内部字段检索"的才用 JSON；能检索的字段（`kind` / `favorite` /
   `checkpoint`）一律抽成正式列并建索引。

4. **枚举用 `ENUM('IMAGE','VIDEO','AUDIO','MIXED')`**，写入前在 Kotlin 侧也做一次白名单校验
   （`PromptRepo.KINDS`）—— 应用层和数据库层双重兜底，脏值进不来。

5. **布尔用 `TINYINT(1)` + `DEFAULT 0`**，Kotlin 侧用 `rs.boolOr("favorite")` 读，写入用 `if (x) 1 else 0`。

6. **冗余计数 `tags.use_count`**：为了"按热度排序"不用每次 `COUNT(*)` 三表 JOIN。
   代价是必须维护，项目选择**改动后整体重算**（见 §5.4），数据量在个人量级时这是最省心的正确解。

7. **`DATETIME(3)` 而不是 `TIMESTAMP`**：`TIMESTAMP` 有 2038 问题和时区自动转换的坑；
   `DATETIME` 存"字面时间"，配合服务端统一用 `Instant`（`Db.kt` 里 `Instant ↔ Timestamp` 互转）。

8. **同一个概念在不同表里用了不同类型 —— 这是个可改进点（练手素材）**：
   `prompts.workflow_json` 是 `MEDIUMTEXT`，而 `media_assets.workflow_json` 是 `JSON`。
   两者存的是同一种东西（ComfyUI 工作流快照），类型却不一致。选 `JSON` 的好处是能校验合法性、
   能按路径查询；选 `MEDIUMTEXT` 是因为工作流可能有几百 KB、写入前已经由应用层保证是合法 JSON。
   **统一成 `JSON` 更一致**，值得作为第一个改动练手（注意 `JSON` 列最大 1GB 但受 `max_allowed_packet` 约束）。

### 2.3 索引清单与"它为什么在那里"

| 索引 | 支撑的查询 |
| --- | --- |
| `idx_prompts_kind` | 列表页按 IMAGE/VIDEO/AUDIO 过滤 |
| `idx_prompts_created` | 默认排序 `created_at DESC` |
| `idx_prompts_favorite` | 只看收藏（低基数列，个人库够用） |
| `idx_prompts_source_ref` / `idx_media_source_ref` | 按 ComfyUI `prompt_id` 反查"这次生成入库了没" |
| `idx_media_prompt` | 提示词详情页列出它的产物、`GROUP BY prompt_id` 统计数量 |
| `idx_media_sha256` | **导入前去重**的关键索引（`WHERE sha256 = ?`） |
| `idx_tags_normalized`（UNIQUE） | `ensure()` 的"存在即复用" |
| `idx_tags_use_count` / `idx_tags_category` | 标签页按热度/分类排 |
| `uk_capture_runs_key`（UNIQUE） | **幂等防重**的核心 |
| `ft_prompts`（FULLTEXT，**目前没被用**） | 见 §5.6 的说明，是个"预埋但未启用"的索引 |

---

## 3. 访问层：`Db.kt` 逐段精读（最值得抄的部分）

### 3.1 连接池配置

```kotlin
jdbcUrl = cfg.jdbcUrl                 // jdbc:mysql://127.0.0.1:3307/comfy_hub?...
driverClassName = "com.mysql.cj.jdbc.Driver"
maximumPoolSize = 12
minimumIdle = 2
connectionTimeout = 10_000            // 从池里拿连接的等待上限
idleTimeout = 60_000
maxLifetime = 1_800_000               // 30 分钟，短于 MySQL wait_timeout 才不会拿到死连接
isAutoCommit = true                   // 默认自动提交；需要事务时显式关（见 tx）
initializationFailTimeout = -1        // ★ 不做 fail-fast
addDataSourceProperty("cachePrepStmts", "true")
addDataSourceProperty("prepStmtCacheSize", "250")
addDataSourceProperty("prepStmtCacheSqlLimit", "2048")
```

**`initializationFailTimeout = -1` 是本项目最实用的一课。** 默认值下连接池初始化时连不上库会直接抛异常，
进程当场起不来。而这里的启动方式常常是"MySQL 和后端被脚本并行拉起"，后端往往先于数据库就绪。
设成 `-1` 表示"池先建好，连接失败不报错"，然后由 `Application.waitForDatabase()` 自己带重试地等：

```kotlin
private fun waitForDatabase(cfg: AppConfig, attempts: Int = 20, delayMs: Long = 1500): Boolean {
    repeat(attempts) { i ->
        try {
            if (!Db.isInitialized) Db.init(cfg)
            Db.withConnection { conn -> conn.queryOne("SELECT COUNT(*) FROM prompts") { it.getLong(1) } }
            return true                       // ← 真打一次查询，确认库和表都在
        } catch (e: Exception) {
            Db.close(); Thread.sleep(delayMs)  // 失败要关掉池重建，否则拿到的是坏池
        }
    }
    return false
}
```

要点：① **重试**而不是崩溃；② 探活要**真执行一条能验证表存在的 SQL**，不能只测 TCP 端口；
③ 每次失败 `Db.close()` 再重建，避免复用半死的连接池。

JDBC URL 里的参数也值得看：

```
useUnicode=true&characterEncoding=UTF-8   # 中文不乱码
allowPublicKeyRetrieval=true              # MySQL 8 默认 caching_sha2_password，不开这个首次连接会失败
useSSL=false                              # 本机回环，省掉 TLS 握手
rewriteBatchedStatements=true             # 预埋：批量 INSERT 时把多条语句重写成一条（当前代码还没用批量）
```

> 小知识：`cachePrepStmts` 系列是"客户端语句缓存"，想真正用上**服务端**预编译还得加
> `useServerPrepStmts=true`。当前只做客户端缓存，对本项目的负载足够。

### 3.2 一个小而全的 JDBC 工具箱

`Db.kt` 的价值在于它把 JDBC 最烦的样板代码压到 5 个函数：

```kotlin
Db.withConnection { conn -> ... }                  // 自动归还连接（use）
Db.tx { conn -> ... }                              // 事务：异常自动回滚

conn.queryList(sql, vararg params) { rs -> T }      // 查多行
conn.queryOne (sql, vararg params) { rs -> T }      // 查一行（结果是 T?）
conn.execute(sql, vararg params): Int               // INSERT/UPDATE/DELETE，返回影响行数
conn.executeReturningKey(sql, vararg params): Long  // INSERT 并取回自增主键
```

三个设计点：

1. **统一用 `PreparedStatement` + `?` 占位符**。全仓库没有一处字符串拼 SQL 值，
   连 `IN (?,?,?)` 也是按数量生成占位符再绑定参数（`joinToString(",") { "?" }`）。
   动态条件只拼"SQL 片段"（列名/固定子句），**值永远走参数**——这就是防 SQL 注入的标准姿势。

2. **`bindAll` 显式分派类型**：`String/Int/Long/Boolean/Double/Float/BigDecimal/ByteArray/Instant → setXxx`，
   特别是 `Instant → Timestamp.from(...)`，让上层代码只跟 `java.time` 打交道，不碰 JDBC 的
   `java.sql.Timestamp`。

3. **`ResultSet` 读取全部 NULL 安全**：`strOr` / `intOrNull` / `longOrNull` / `doubleOrNull` / `boolOr` /
   `instantOrNull`，其中 `wasNull()` 是必须的——`rs.getInt()` 对 SQL NULL 返回 0，
   不检查 `wasNull()` 就会把"没填的 steps"读成 0。这类 helper 看似琐碎，实际是消灭 NPE 的主力。

### 3.3 取回自增主键

```kotlin
fun Connection.executeReturningKey(sql: String, vararg params: Any?): Long =
    prepareStatement(sql, Statement.RETURN_GENERATED_KEYS).use { ps ->
        ps.bindAll(params); ps.executeUpdate()
        ps.generatedKeys.use { keys -> if (keys.next()) keys.getLong(1) else error("INSERT 未返回自增主键") }
    }
```

`RETURN_GENERATED_KEYS` 比 `SELECT LAST_INSERT_ID()` 更可靠（后者在连接池下容易拿错连接）。
注意 `error(...)` 而不是返回 0 —— 主键拿不到属于**不该发生的编程错误**，要炸得响。

---

## 4. 事务：边界画在哪里

```kotlin
fun <T> tx(block: (Connection) -> T): T = dataSource.connection.use { conn ->
    val prev = conn.autoCommit
    conn.autoCommit = false
    try { val r = block(conn); conn.commit(); r }
    catch (e: Throwable) { runCatching { conn.rollback() }; throw e }
    finally { runCatching { conn.autoCommit = prev } }   // ★ 还原，否则连接还回池里状态被污染
}
```

`finally` 里还原 `autoCommit` 是关键：连接归还池后会被复用，不还原会让下一次
"自动提交"的写入悄悄变成"永不提交"。

**什么时候用 `tx`**（判断标准：一次业务操作要动多张表）

| 场景 | 事务内做什么 |
| --- | --- |
| 新建提示词 | `INSERT prompts` + `INSERT tags`(按需) + `INSERT prompt_tags` + 重算 `use_count` |
| 更新提示词 | `UPDATE prompts` + `DELETE prompt_tags` + 重新插入 + 重算计数 |
| 产物入库 | `INSERT media_assets` + `INSERT media_tags` + 重算计数 |
| 捕获落库 | `INSERT prompts`（`createCaptured`）+ 标签关联 |
| 标签删除 | `DELETE tags`（外键 CASCADE 自动清关联行） |

**什么时候不用 `tx`**：单个 `UPDATE`（`setFavorite`）、纯读查询、以及**DDL**
（`Migrate.kt` 的 `ALTER TABLE`）——MySQL 的 DDL 是隐式提交的，包在事务里也没有回滚语义。

**已知的取舍**：`Migrate.run()` 用 `Db.withConnection` 而不是 `tx`，所以一轮迁移中途失败会留下
"改了一半"的结构。好在每一项都是幂等的（下次启动会继续把缺的补上），这也是它敢不包事务的原因。

---

## 5. 本项目里的 8 个 SQL 技巧（逐个能学）

### 5.1 动态条件 + 参数列表（搜索的核心模式）

```kotlin
val where = mutableListOf<String>()
val params = mutableListOf<Any?>()

if (!args.q.isNullOrBlank()) {
    val terms = args.q.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.take(8)   // 限制最多 8 个词
    terms.forEach { term ->
        where += "(p.title LIKE ? OR p.positive_prompt LIKE ? OR ... OR p.checkpoint LIKE ?)"
        repeat(5) { params += "%$term%" }        // 一个词要绑 5 次（对应 5 个 LIKE）
    }
}
if (!args.kind.isNullOrBlank()) { where += "p.kind = ?"; params += args.kind.uppercase() }
if (args.favorite == true)               where += "p.favorite = 1"
if (args.hasMedia == true)               where += "EXISTS (SELECT 1 FROM media_assets m WHERE m.prompt_id = p.id)"
if (args.hasMedia == false)              where += "NOT EXISTS (SELECT 1 FROM media_assets m WHERE m.prompt_id = p.id)"

val clause = if (where.isEmpty()) "" else "WHERE " + where.joinToString(" AND ")
```

学习点：
- **"几个词就 AND 几组"**：把 `"cyberpunk rain"` 拆成两个词，要求两个词都命中（AND），
  比整串 `LIKE '%cyberpunk rain%'` 命中率高得多——这是**应用层实现的简易分词搜索**。
- `take(8)` 限制条件数量，防止有人粘一整段文章进来生成 40 组 LIKE 把库拖死。
- `EXISTS` / `NOT EXISTS` 表达"有没有关联产物"，比 `LEFT JOIN ... IS NULL` 更直白、优化器也友好。
- **同一个 `params` 列表被复用到 COUNT 查询和分页查询**：先 `COUNT(*)` 得总数，再查当页数据。
  两边的 WHERE 和参数完全一致，所以抽成了 `buildWhere()` 返回 `(clause, params)`。

### 5.2 稳定分页

```kotlin
val size = args.size.coerceIn(1, 200)     // 防止 ?size=100000 打爆内存
val page = args.page.coerceAtLeast(1)
val offset = (page - 1) * size
// 排序：sort=title 时用 "p.title ASC, p.id DESC" —— 加 id 兜底，避免同名时页与页之间错乱/丢行
```

`LIMIT ? OFFSET ?` 深度分页会越来越慢（要扫过前面所有行），数据量大了才需要换
"游标分页"（`WHERE id < ? ORDER BY id DESC LIMIT n`）。当前个人库量级不需要，但要知道这个天花板。

### 5.3 用 `GROUP BY ... HAVING COUNT(DISTINCT ...)` 实现"标签全命中"

需求：`tagMode=all` 时，"同时带有 [赛博朋克, 写实] 两个标签"的提示词。

```sql
p.id IN (
  SELECT pt.prompt_id FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
   WHERE t.normalized IN (?, ?)                     -- 先选出"命中任意一个"的
   GROUP BY pt.prompt_id
  HAVING COUNT(DISTINCT t.normalized) = 2           -- 再筛出"命中数量 = 标签个数"的
)
```

**"先 IN 再 GROUP BY HAVING 计数 = 需求个数"** 是关系数据库里做集合包含的经典写法。
`COUNT(DISTINCT)` 里的 DISTINCT 不能省（虽然 `prompt_tags` 复合主键保证了不会重复，写上是防御性的）。

`tagMode=any` 时就去掉 `GROUP BY/HAVING`，只留 `IN`。

### 5.4 冗余计数的整体重算

```sql
UPDATE tags t SET t.use_count = (
    (SELECT COUNT(*) FROM prompt_tags pt WHERE pt.tag_id = t.id)
  + (SELECT COUNT(*) FROM media_tags  mt WHERE mt.tag_id = t.id)
)
```

一行 SQL 把所有标签的计数刷成真值。**取舍**：O(标签数 × 关联行数) 比增量 `+1/-1` 贵，
但永远不会有"计数漂移"的脏数据，也不用处理"删除提示词要减哪些标签"的各种边界。
个人使用量级（几百到几万行）完全无感。
**调用时机**：每一次会改变关联关系的写操作之后（都在同一个事务里）。

> 注意这是"全表 UPDATE"，会锁很多行。数据量上了十万级就该换增量维护或定时任务重算。

### 5.5 批量取关联数据，消灭 N+1

列表页有 20 条提示词，每条都要带标签。**反面写法**是循环里查 20 次（N+1）；
本项目一次查完再在内存里分组：

```kotlin
fun tagsForPrompts(conn: Connection, promptIds: List<Long>): Map<Long, List<TagDto>> {
    if (promptIds.isEmpty()) return emptyMap()
    val marks = promptIds.joinToString(",") { "?" }          // IN (?,?,?,...)
    val sql = "SELECT pt.prompt_id, t.id, ... FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
                WHERE pt.prompt_id IN ($marks) ORDER BY t.name"
    return conn.queryList(sql, *promptIds.toTypedArray()) { rs -> rs.getLong("prompt_id") to mapTag(rs) }
        .groupBy({ it.first }, { it.second })                 // ★ 一次查询 + 内存分组
}
```

同款还有"每条提示词有几个产物"：

```sql
SELECT prompt_id, COUNT(*) AS c FROM media_assets WHERE prompt_id IN (?,?,?) GROUP BY prompt_id
```

`attachRelations()` 把这两个结果拼回 DTO。**这条模式（一次 IN 查询 + `groupBy`）能解决 90% 的 N+1。**

### 5.6 `LIKE` 而不是 `FULLTEXT`（含一个"预埋未启用"的索引）

`prompts` 表上建了全文索引 `FULLTEXT KEY ft_prompts (title, positive_prompt, negative_prompt, notes)`，
但 `PromptRepo` 里**没有一处 `MATCH ... AGAINST`**，搜索走的是 `LIKE '%词%'`。代码注释解释了原因：

> 按空白拆词，每个词都要命中（AND），兼容中文场景（用 LIKE 而非 FULLTEXT）

技术背景：MySQL InnoDB 的全文索引默认分词器按**空格/标点**切词，中文一整句会被当成一个"词"，
不装 **ngram 分词器**（`WITH PARSER ngram`）就搜不出中文。
另外 `MATCH` 要求关键词长度 ≥ `innodb_ft_min_token_size`（默认 3），搜 "8K" 这类短词也不好使。

**代价**：`LIKE '%...%'` 前置通配符**用不上 B-Tree 索引**，必然全表扫描。
个人库几千行无所谓；真要做大，正确做法是：

```sql
-- 需要时（MySQL 8.0.24+ 才支持在已有表上直接加 ngram 全文索引）
ALTER TABLE prompts ADD FULLTEXT ft_prompts_ngram (title, positive_prompt, notes) WITH PARSER ngram;
-- 查询改成
SELECT ... WHERE MATCH(p.title, p.positive_prompt, p.notes) AGAINST (? IN BOOLEAN MODE);
```

**学习点：建了索引不等于用得上索引。** 值得养成习惯——每次写下 `LIKE '%x%'` 时问一句
"这列上有索引能被用上吗？"

### 5.7 幂等三件套：`INSERT IGNORE` / `ON DUPLICATE KEY UPDATE` / 唯一键

**A. 靠唯一键 + `INSERT IGNORE` 抢占一次运行**（`CaptureRepo.beginRun`）：

```sql
INSERT IGNORE INTO capture_runs (run_key, source, status, raw) VALUES (?,?, 'running', ?)
```

返回值 `> 0` 表示"这次是我抢到的，去干活"；`= 0` 表示"已经有人处理过这个 `run_key`"。
**用一条 INSERT 的成败做分布式锁**，不需要先 SELECT 再 INSERT（那有竞态）。
随后还要处理两种残留状态：

```sql
SELECT prompt_id, status, TIMESTAMPDIFF(SECOND, created_at, NOW(3)) AS age
  FROM capture_runs WHERE run_key = ?
-- status='running' 且 age <= 600  → 真的有人在处理，让开
-- status='running' 但 age >  600  → 后端上次被杀留下的僵尸，重新抢占并刷新 created_at
```

`TIMESTAMPDIFF(SECOND, 起始, 结束)` 是 MySQL 求时间差的函数；`NOW(3)` 取毫秒精度的当前时间。
**"超时可抢占"是幂等设计里必须有的一环**，否则进程崩溃后这个 key 就永久卡死了。

**B. `ON DUPLICATE KEY UPDATE` 做 upsert**（`SettingsRepo.put`）：

```sql
INSERT INTO app_settings (k, v) VALUES (?, ?) ON DUPLICATE KEY UPDATE v = VALUES(v)
```

更新配置只需要一句，不需要区分"插入还是更新"。`VALUES(v)` 引用的是"本次本想插入的值"。
（MySQL 8.0.20+ 推荐新写法 `AS new ... UPDATE v = new.v`，老写法仍兼容。）

**C. 种子数据可重复执行**（`db/seed.sql`）：

```sql
INSERT INTO tags (name, normalized, category, color) VALUES (...), (...) 
ON DUPLICATE KEY UPDATE name = VALUES(name), category = VALUES(category), color = VALUES(color);
```

配合 `INSERT IGNORE INTO prompt_tags SELECT ...`，整个 `seed.sql` 跑 100 遍结果都一样——
**演示数据脚本必须幂等**，否则用户手滑跑两次就多一份垃圾数据。

### 5.8 视图 + `GROUP_CONCAT`（以及它当前被冷落的事实）

`db/schema.sql` 定义了一个把"产物 + 提示词 + 标签名"拉平成一行的大视图：

```sql
CREATE OR REPLACE VIEW v_media_full AS
SELECT m.*, p.title AS prompt_title, p.positive_prompt, ...,
  (SELECT COALESCE(GROUP_CONCAT(t.name ORDER BY t.name SEPARATOR ','), '')
     FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
    WHERE pt.prompt_id = m.prompt_id) AS prompt_tag_names
FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id;
```

- `GROUP_CONCAT` 把"一对多"的标签压成一个逗号串，`COALESCE(..., '')` 保证没标签时返回空串而不是 NULL。
- `LEFT JOIN` 让"未关联提示词的产物"也能出现在结果里（这是画廊页要显示的主要内容）。

**但要如实指出：服务器代码并没有用这个视图。** `MediaRepo` 自己拼了等价的 `COLS + FROM`，
再在 Kotlin 里合并标签（因为要区分"产物自身标签"和"提示词标签"，SQL 串起来后无法区分）。
所以视图目前是**给人工排查/写报表用的**：

```sql
SELECT id, kind, prompt_title, prompt_tag_names FROM v_media_full ORDER BY created_at DESC LIMIT 20;
```

**学习点**：视图适合"固定形态的读模型"，一旦上层需要按来源拆分字段就不好使了。
两条路（SQL 里聚合 vs 应用层聚合）各有适用面，能读懂就够了。

---

## 6. 幂等与去重：两道防线

这是本项目数据库设计里最值得抄的一段思路。

| 防线 | 键 | 位置 | 防的是什么 |
| --- | --- | --- | --- |
| **内容去重** | `media_assets.sha256` + `idx_media_sha256` | `CaptureRepo.importFile` / `MediaRoutes.upload` | 同一张图被导入两次（历史目录重扫、用户重复上传、ComfyUI 输出目录里本来就有副本） |
| **流程幂等** | `capture_runs.run_key`（UNIQUE） | `CaptureRepo.beginRun/finishRun` | 同一次 ComfyUI 生成被处理两次（轮询重复、后端重启补捞、手动触发 + 自动轮询撞车） |

`run_key` 的两种取值（见 `schema.sql` 注释）：
- `ComfyUI 的 prompt_id`——轮询 / 推送 / 历史补捞都从 ComfyUI 的 `history` 里取到同一个 id；
- `import:<sha256>`——目录导入没有 prompt_id，就用文件内容哈希自己造一个唯一键。

导入流程的顺序（**这是最关键的顺序设计**）：

```
1. Files.isRegularFile(source)          → 文件在不在
2. MediaFiles.sha256(source)            → 算内容指纹
3. SELECT ... WHERE m.sha256 = ? LIMIT 1 → 库里有了？→ 直接返回 Duplicate（临时文件顺手删掉）
4. Files.copy/move → storage/media/<uuid>.<ext>   ← 先落盘
5. Db.tx { INSERT media_assets + 标签 + 重算计数 } ← 再写库
6. 失败 → Files.deleteIfExists(dest)     ← 补偿：把刚落盘的文件删掉
7. 生成缩略图（图片才做）
```

**为什么先落盘再写库？** 因为文件系统和数据库**没有共同的事务**。
两种失败组合里，先落盘更安全：
- 文件写了、库没写 → 留下一个孤儿文件（浪费几 MB，不影响功能，可事后清理）；
- 库写了、文件没写 → 库里有一条指向不存在文件的记录 → 前端打开就报"文件已丢失"，**是用户可见的坏状态**。

**删除时顺序反过来**（`MediaRoutes` 的 `delete`）：先删库记录，再删文件和缩略图。
同理，宁可留孤儿文件，也不留坏记录。

**残留风险（值得知道的诚实结论）**：步骤 4 和 5 之间如果进程被 kill，会留下无人认领的孤儿文件，
项目目前**没有清理任务**。想练手可以写一个 "扫描 `storage/media` 与 `media_assets.stored_name` 做差集" 的巡检接口。

---

## 7. 迁移与结构演进（三条路径，一个终点）

```
db/schema.sql ──── 全新安装（CREATE TABLE IF NOT EXISTS，一次成型）
      │
      │  两者结果必须一致 ←── 这是维护时要盯住的约定
      ▼
db/migrate.sql ─── 手动升级老库（存储过程 + DROP PROCEDURE，可重复执行）
      │
      ▼
Migrate.kt ─────── 后端每次启动自动执行（同样的事，Java 侧实现）
```

**为什么需要三份？** 因为 `CREATE TABLE IF NOT EXISTS` 对**已存在**的库不会补新列，
而 MySQL 8.4 **不支持 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`**（MariaDB 支持，MySQL 不支持）。
所以要"先查再改"：

```kotlin
private fun columnExists(conn: Connection, table: String, column: String): Boolean =
    (conn.queryOne("""
        SELECT COUNT(*) FROM information_schema.COLUMNS
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = ?
     """, table, column) { it.getInt(1) } ?: 0) > 0
```

`information_schema` 是 MySQL 的"数据字典库"（元数据都在这），
`COLUMNS` / `STATISTICS` / `TABLES` 三张表分别对应用来探测**列 / 索引 / 表**是否存在。
`TABLE_SCHEMA = DATABASE()` 表示"当前连接的库"，不用把库名写死。

然后：

```kotlin
private fun addColumn(conn, table, column, ddl): Int {
    if (columnExists(conn, table, column)) return 0     // 幂等：存在就返回 0（表示无变更）
    return conn.execute("ALTER TABLE $table ADD COLUMN $column $ddl")
}
```

`Migrate.run()` 返回"改了几项"，启动日志会打 `数据库结构已补齐（N 项变更）` 或 `数据库结构已是最新`——
**看一眼日志就知道这台机器的库是什么状态**，是个很省事的可观测性设计。

`db/migrate.sql` 里用**存储过程**包住所有 `IF NOT EXISTS ... THEN ALTER ...`：

```sql
DROP PROCEDURE IF EXISTS comfyhub_migrate;
DELIMITER $$
CREATE PROCEDURE comfyhub_migrate()
BEGIN
  IF NOT EXISTS (SELECT 1 FROM information_schema.COLUMNS WHERE ...) THEN
    ALTER TABLE prompts ADD COLUMN source VARCHAR(32) NULL DEFAULT 'Manual';
  END IF;
  ...
END$$
DELIMITER ;
CALL comfyhub_migrate();
DROP PROCEDURE comfyhub_migrate();
```

`DELIMITER $$` 是因为存储过程内部有分号，必须换一个"语句结束符"告诉客户端哪里才是整条语句的结尾；
用完再 `DELIMITER ;` 换回来。**这是命令行客户端的行为，不是 SQL 标准**（用 JDBC 就不需要）。

### 7.1 新增一个字段要动哪些地方（改结构的 checklist）

以"给提示词加一个 `rating` 评分字段"为例，一共 5 处，**漏一处就会出现"新库有、老库没有"**：

1. `db/schema.sql` —— 新建库时的定义（`rating TINYINT NULL`）
2. `db/migrate.sql` —— 老库升级（一段 `IF NOT EXISTS ... ALTER TABLE`）
3. `server/.../Migrate.kt` —— 自动迁移（`changed += addColumn(conn, "prompts", "rating", "TINYINT NULL")`）
4. `Models.kt` 的 `PromptDto` / `PromptInput` —— API 出入参
5. `PromptRepo.kt` —— `COLUMNS` 列表、`mapPrompt()` 映射、`INSERT`/`UPDATE` 的列与占位符、`bindAll` 参数顺序

（如果前端要显示，再改 Dart 侧的模型和页面。）

### 7.2 一处文档与脚本不一致（可以顺手修掉）

`README.md` 的 §8.2 写：

```powershell
pwsh -File scripts\mysql.ps1 migrate   # 内容就是 db\migrate.sql
```

但 `scripts/mysql.ps1` 第 27 行的 `ValidateSet` 是
`init, start, stop, restart, status, cli, schema, seed, reset, logs, move`，**没有 `migrate`**，
第 472 行的 `switch` 里也没有对应分支。所以这条命令现在会直接被参数校验拦下、报
"参数不属于集合"。修法就两行：

```powershell
# 第 27 行 ValidateSet 里加 'migrate'
# switch 里加一个分支
'migrate' { Invoke-SqlFile (Join-Path $ProjectRoot 'db\migrate.sql') $null; Write-Host '迁移完成' -ForegroundColor Green }
```

> 通读文档时对照脚本验证，是发现这类"文档漂移"最有效的方法。

---

## 8. 数据库的运维：`scripts/mysql.ps1`

这个脚本是**唯一入口**（App 的启动/停止按钮最终也调到它），它管着一台便携 MySQL 的完整生命周期。

### 8.1 动作一览

| 动作 | 干什么 |
| --- | --- |
| `init` | 建 `data\` + `mysqld --initialize-insecure`（root 空密码）→ 启动 → 导入 `schema.sql` → 建应用账号 → 导入 `seed.sql` |
| `start` | 起 `mysqld`；**发现 `data\mysql` 不存在就先自动 `init -SkipSeed`** |
| `stop` / `restart` | `mysqladmin shutdown`，超时再强杀；会提醒"后端还活着，停库它就废了" |
| `status` | 进程 / 版本 / `prompts=N media=N tags=N` / 四个关键路径 |
| `schema` / `seed` | 单独导 SQL 文件 |
| `reset` | 停库 → `Remove-Item -Recurse .mysql\data` → `init`（核弹级） |
| `cli` | 打开 `mysql` 交互命令行 |
| `logs` | 看 `mysql-error.log` 最后 60 行 |
| `move -DataDir <新位置>` | 停库 → `robocopy` → 重写 `my.ini` → 写指针文件 → 从新位置启动 |

### 8.2 `my.ini` 逐条精读（Server 端配置）

```ini
[mysqld]
basedir=<MySQL 解压目录>          # 程序目录
datadir=<实例目录>/data           # 数据目录 ★ 备份/搬家就是搬它
port=3307                         # 避开 3306
bind-address=127.0.0.1            # ★ 只监听本机，不对局域网暴露（安全默认值）
mysqlx=0                          # 关掉 X Protocol（默认 33060），少开一个端口
character-set-server=utf8mb4      # 服务端字符集
collation-server=utf8mb4_unicode_ci
max_connections=200               # 连接池最大才 12，200 是留余量
max_allowed_packet=512M           # ★ 允许单条 SQL/单次传输最大 512MB（导入大 JSON 工作流要靠它）
innodb_buffer_pool_size=256M      # ★ InnoDB 缓存池：最重要的性能参数（个人机 256M 足够）
innodb_flush_log_at_trx_commit=2  # ★ 每次提交写 OS 缓存、每秒 fsync：性能↑，极端断电可能丢最后 1 秒
log-error=<实例目录>/mysql-error.log
local_infile=0                    # ★ 禁用 LOAD DATA LOCAL INFILE（防客户端被诱导读本地文件）

[client]
port=3307
host=127.0.0.1
default-character-set=utf8mb4     # 客户端也统一，避免中文乱码
```

**三个 DBA 常识从这里可以学到：**

- `innodb_flush_log_at_trx_commit`：`1`=每次提交都 fsync（最安全最慢），`2`=写 OS 缓存+每秒 fsync，
  `0`=交给后台线程。这台机器是"个人桌面工具"，选 `2` 换启动/写入速度，可接受。
- `bind-address=127.0.0.1` + 应用账号只授权 `comfy_hub.*`：**纵深防御**。
  就算密码是弱密码（`comfyhub/comfyhub`），外网也连不上、连上了也只能碰这一个库。
- `local_infile=0`：老版本 MySQL 的 `LOAD DATA LOCAL INFILE` 有"客户端被服务端反向读取任意文件"的经典漏洞，
  用不到就关掉。

### 8.3 应用账号与权限

```sql
CREATE USER IF NOT EXISTS 'comfyhub'@'localhost' IDENTIFIED BY 'comfyhub';
CREATE USER IF NOT EXISTS 'comfyhub'@'127.0.0.1' IDENTIFIED BY 'comfyhub';
GRANT ALL PRIVILEGES ON `comfy_hub`.* TO 'comfyhub'@'localhost';
GRANT ALL PRIVILEGES ON `comfy_hub`.* TO 'comfyhub'@'127.0.0.1';
```

两个要点：
- **`'user'@'host'` 是一个整体**：`@'localhost'`（走 socket/本机）和 `@'127.0.0.1'`（走 TCP）
  在 MySQL 眼里是**两个不同的账号**，都得建。JDBC 用 `127.0.0.1` 连，所以缺了后者就会
  "密码明明对却连不上"。
- 权限**只授到 `comfy_hub.*`**，不是 `*.*`。后端永远用这个账号跑，root 只在 init 时用。

### 8.4 实例目录可以在任意位置（解析顺序）

```
1. 命令行 -DataDir <路径>              （最高优先）
2. 环境变量 COMFYHUB_MYSQL_DIR
3. 指针文件 <项目>\.mysql-location.json 里的 dataDir   ← move 会写它
4. 默认 <项目>\.mysql
```

相对路径按项目根展开，最终规范化成绝对路径。四个脚本（`mysql.ps1` / `server.ps1` / `comfyhub.ps1` /
`pack-release.ps1`）**刻意重复实现同一套顺序**（项目选择"不引共享模块"），
所以**改一处必须四处对齐**——AGENTS.md 专门强调了这点。

**`move` 的安全设计**：先停库（不停库复制 InnoDB 文件不可靠）→ 目标已有 `data\` 且没加 `-Force` 就拒绝 →
`robocopy /E /COPY:DAT /R:1 /W:1`（退出码 < 8 都算成功，这是 robocopy 的约定）→
**按新位置重写 `my.ini`**（复制过去的 `my.ini` 里还是旧路径，不改就起不来）→ 写指针文件 → 从新位置启动。
**源目录不删**，确认无误后用户自己删。

### 8.5 首次运行的自动化

`Do-Start` 里有这么一段（很关键的用户体验设计）：

```powershell
if (-not (Test-Path (Join-Path $DataDir 'mysql'))) {
    Write-Host '==> 数据目录还没初始化，先建库建表（首次启动会慢一点）…'
    Do-Init -SkipSeed      # ★ 故意不灌演示数据
    return
}
```

发布包解压出来**没有 `data\` 目录**，少了这一步，用户双击 `viewer.exe` 必然失败。
而自动初始化**故意不灌 `seed.sql`**——不能往用户的正式库里塞演示提示词，
只有人手敲 `mysql.ps1 init` 才灌。

**学习点：初始化数据（seed）和结构（schema）要分开，并且默认只做结构。**

---

## 9. 库与文件：为什么不是 BLOB

| 存哪 | 存什么 | 谁负责 |
| --- | --- | --- |
| MySQL | 元数据：`stored_name`（`<uuid>.<ext>`）、`original_name`、`sha256`、`size_bytes`、`width/height`、`duration_ms`、`mime_type` | 可查询、可关联、可事务 |
| `storage/media/` | 原始文件 | 磁盘 IO / 流式传输 |
| `storage/thumbs/` | `<mediaId>.jpg` 缩略图（最长边 512，由 Java `ImageIO` 生成，不依赖 ffmpeg） | 画廊页秒开 |
| `storage/tmp/` | 上传中转（`.part` 临时文件） | 大文件先落临时再入库 |

这么设计的好处：

- **视频拖进度条**要用 HTTP `Range`（Ktor 的 `PartialContent` 插件），
  直接 `call.respondFile(path)` 就能支持；BLOB 读出来再切片要重写一遍这套逻辑。
- **备份/迁移简单**：`mysqldump` 只导元数据（几 MB），图片自己 rsync。
- **`sha256` 去重**：内容相同的文件只存一份，库里记不重复的记录 → 直接省磁盘。

代价与纪律（呼应 §6）：
- 库和文件**必须手动保持一致**；
- 写：**先文件后库**，失败补偿删文件；
- 删：**先库后文件**；
- 路径安全：`Storage.resolveMedia` 里做了**目录穿越防护**（`normalize()` 后校验 `startsWith(mediaDir)`），
  因为 `stored_name` 理论上来自数据库、但也可能被构造过的输入污染。

```kotlin
fun resolveMedia(storedName: String): Path? {
    val p = mediaDir.resolve(storedName).normalize()
    if (!p.startsWith(mediaDir)) return null      // ★ ../.. 一律拒绝
    return p.takeIf { Files.isRegularFile(it) }
}
```

---

## 10. 与前端/后端的边界

- **Dart 侧没有数据库代码。** `lib/core/settings_store.dart` 用 `shared_preferences`
  存的是"本机偏好"（例如 MySQL 数据目录、后端地址），跟业务表无关。
- App 启动服务靠 `lib/core/backend_launcher.dart` 调 `scripts\comfyhub.ps1`——
  也就是**前端不直接连库，它连的是脚本**。
- 所有业务数据走 `GET/POST /api/...`（完整清单见 README §7）。

**这个边界带来的好处**：数据库结构可以自由演进（只要 Repo 和 DTO 映射跟着改），
App 不需要重新发版；反过来，App 也永远不可能因为"直接连库"而在别人的机器上连错实例。

**`app_settings` 是个有意思的例子**：自动捕获的配置（ComfyUI 地址、输出目录、轮询间隔）
既要在 App 设置页里改，又要被后端轮询线程读到 → **存数据库**，两边共用同一份，
避免"两边配置各存一份、互相打架"。整个对象序列化成一段 JSON 存在 `app_settings.v` 里：

```kotlin
private const val KEY_CAPTURE = "capture.config"
fun captureConfig(cfg: AppConfig): CaptureConfig =
    get(KEY_CAPTURE)?.let { runCatching { AppJson.decodeFromString(CaptureConfig.serializer(), it) }
        .getOrElse { /* 解析失败回退默认值并 warn */ } }
    ?: CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir)
```

**"k/v 表 + 一整段 JSON"** 适合"结构会变、不按字段查、整存整取"的配置。
注意它放弃了 schema 校验——所以 `saveCaptureConfig` 里做了**规范化 + 范围钳制**
（`pollSeconds.coerceIn(1, 600)`、URL 去尾斜杠），读取时还 `runCatching` 兜一层，
坏数据也不会让服务起不来。

---

## 11. 动手练习清单（建议按顺序做）

> 前置：`pwsh -File scripts\comfyhub.ps1 up` 把库和后端拉起来；进库用 `pwsh -File scripts\mysql.ps1 cli`。

**SQL 基本功**

1. 列出所有表并看清每个表的引擎/字符集：
   `SELECT TABLE_NAME, ENGINE, TABLE_COLLATION FROM information_schema.TABLES WHERE TABLE_SCHEMA='comfy_hub';`
2. `SHOW CREATE TABLE media_assets\G` —— 对照 `db/schema.sql` 看外键和索引是怎么落地的。
3. 用视图查最近 10 条产物及其提示词标签：
   `SELECT id, kind, prompt_title, prompt_tag_names FROM v_media_full ORDER BY created_at DESC LIMIT 10;`
4. 手写 `tagMode=all` 的等价查询，验证和接口结果一致：
   `SELECT p.id FROM prompts p WHERE p.id IN (SELECT pt.prompt_id FROM prompt_tags pt JOIN tags t ON t.id=pt.tag_id WHERE t.normalized IN ('赛博朋克','写实') GROUP BY pt.prompt_id HAVING COUNT(DISTINCT t.normalized)=2);`
5. 用 `EXPLAIN` 对比 `LIKE '%雨%'`（全表扫描，`type=ALL`）和 `WHERE kind='IMAGE'`（走索引）的区别。
6. 给 `prompts` 加 ngram 全文索引，用 `MATCH ... AGAINST` 搜中文，和 `LIKE` 对比结果与 `EXPLAIN`。

**工程化练习**

7. 按 §7.1 的 checklist 给提示词加一个 `rating` 字段（5 处都要改），然后用 `mysql.ps1 migrate` 验证老库升级。
8. 修掉 §7.2 那个 `mysql.ps1 migrate` 缺失的分支（两行代码）。
9. 把 `prompts.workflow_json` 的类型从 `MEDIUMTEXT` 统一成 `JSON`（注意：`ALTER TABLE` 要处理已有的非法值）。
10. 写一个"孤儿文件巡检"：比较 `storage/media/` 下的文件名集合与 `SELECT stored_name FROM media_assets`，
    列出没被任何记录引用的文件（对应 §6 的残留风险）。
11. 给 `capture_runs` 加一个"清理 30 天前记录"的动作，注意别删还在 `running` 且未超时的。
12. 压测一下 `LIMIT 200000, 20` 的耗时，再改写成游标分页 `WHERE id < ? ORDER BY id DESC LIMIT 20` 对比。

---

## 12. 关键文件索引

| 文件 | 看它学什么 |
| --- | --- |
| `db/schema.sql` | 全新安装的完整结构：表 / 索引 / 外键 / JSON 列 / 视图 |
| `db/migrate.sql` | 存储过程 + `information_schema` 探测实现的幂等迁移 |
| `db/seed.sql` | 幂等演示数据（`ON DUPLICATE KEY UPDATE` + `INSERT IGNORE ... SELECT`） |
| `server/.../Db.kt` | ★ HikariCP 配置、`tx` 事务模板、参数绑定、NULL 安全读取、取回自增主键 |
| `server/.../Config.kt` | JDBC URL 参数、环境变量覆盖、`describe()` 不打印密码 |
| `server/.../Migrate.kt` | 启动自动迁移；和 `migrate.sql` 一一对应 |
| `server/.../Application.kt` | `waitForDatabase` 重试、`/api/health` 探库、`SQLException` 统一映射成 500 |
| `server/.../PromptRepo.kt` | 动态 WHERE、分页、搜索、批量关联、`createCaptured` |
| `server/.../MediaRepo.kt` | 跨表 JOIN、两来源标签合并、`UNION` + `HAVING`、`stats` 聚合 |
| `server/.../TagRepo.kt` | 多对多维护、`normalized` 规范化、`use_count` 整体重算、防 N+1 |
| `server/.../CaptureRepo.kt` | ★ 两次幂等（`sha256` / `run_key`）、`INSERT IGNORE` 抢占、僵尸超时、文件与库的补偿顺序 |
| `server/.../SettingsRepo.kt` | k/v + JSON 配置的 upsert 与容错 |
| `server/.../MediaFiles.kt` | `sha256`、类型识别、缩略图（不依赖 ffmpeg） |
| `server/.../Storage.kt` | 目录布局、`<uuid>.<ext>` 命名、**目录穿越防护** |
| `scripts/mysql.ps1` | ★ 库的生命周期：init/start/stop/move/reset、`my.ini`、账号授权、实例目录解析 |
| `src/main/resources/logback.xml` | 日志级别与输出（排查 SQL 错误看这里） |

---

## 13. 一页速查（结论汇总）

- 库：MySQL 8.4 @ `127.0.0.1:3307`，库名 `comfy_hub`，账号 `comfyhub/comfyhub`（仅授权本库）。
- 7 表 1 视图；多对多用**复合主键的纯关联表**；删提示词用 `SET NULL` 保产物，删标签用 `CASCADE`。
- 访问层：**原生 JDBC + HikariCP**，五函数工具箱（`withConnection` / `tx` / `queryList` / `queryOne` / `execute`）。
- 只用 `?` 占位符，**永不拼值**；动态条件只拼固定片段。
- 事务用在"一次操作动多张表"；DDL 不包事务（隐式提交）；`tx` 的 `finally` 必须还原 `autoCommit`。
- 幂等：`media_assets.sha256` 防重复文件，`capture_runs.run_key` 防重复处理，`running` 超 600s 可抢占。
- 文件与库无共同事务：**写→先文件后库（失败删文件）；删→先库后文件**；孤儿文件是已知残留风险。
- 迁移三条路径（`schema.sql` / `migrate.sql` / `Migrate.kt`）结果必须一致；加字段要改 5 处。
- 搜索用 `LIKE '%词%'`（因为中文没有 ngram 分词器），索引建了 `ft_prompts` 但没用上——**建了索引 ≠ 用上索引**。
- 冗余计数 `tags.use_count` 靠"改动后整体重算"保正确，牺牲性能换零漂移。
- 库只存元数据，文件在 `storage/`；`stored_name` 取路径时做了目录穿越防护。
- 前端零 SQL；运行期配置进 `app_settings`（k/v + 一段 JSON），App 与后端共用一份。
