# Embedded Database + Cross-Device Sync: Options Report

**Subject:** replacing MySQL with an embedded database for a single-user desktop-hub + Android-lean-client Flutter app, with LAN sync (internet later).
**Date of research:** 2026-09-23.
**Method:** primary sources fetched directly (SQLite official docs, vendor docs, pub.dev, crates.io, GitHub REST API, AOSP gitiles via `pwsh`), plus **measurements taken on the actual development machine**, plus four delegated research streams whose findings are incorporated with their own evidence markers intact.

**How to read the evidence markers:**

| Marker | Meaning |
| --- | --- |
| **[V]** | Verified — a fetched primary source states it, or I ran the measurement myself. |
| **[V, MEASURED]** | Measured on this machine during this session. |
| **[I]** | Inference — my reasoning from verified facts, not stated by a source. |
| **[?]** | Unverified — could not confirm. Do not rely on it. |

> **Tooling constraints that shaped this report, and the workaround that mattered:** `web_search` was dead for the entire session (`HTTP 402: Insufficient Balance` on the search endpoint — the error directs to Settings → Plugins → Plugin configuration → Web search, or `DEEPSEEK_SEARCH_BASE_URL` / `web-search-deepseek.baseURL`), so **nothing here came from search discovery**; it all came from directly-named URLs. Critically, **`pwsh` + `Invoke-WebRequest` reaches hosts that the `web_fetch` tool cannot** — including `developer.android.com`, `android.googlesource.com`, `raw.githubusercontent.com`, and `docs.oracle.com` (a mirror of the MySQL docs, since `dev.mysql.com` returns 403). **Several sections of this report — the whole Android version table, the Android platform constraints, and the MySQL/Postgres memory defaults — exist only because of that workaround.** GitHub HTML is likewise unreachable via `web_fetch`; `api.github.com` works but rate-limits quickly.

> **A note on the delegated research:** four parallel research streams fed this report (sync engines; CRDT/file-sync/CloudKit; Android+Flutter SQLite; Rust/memory; plus precedent research). Their findings are folded in throughout, and where an agent measured something itself I have labelled it as measured rather than implying I ran it. **Corrections they surfaced are called out inline** where they contradict a common belief — most notably: `cargokit` is **archived**; `sqlite3_flutter_libs` is a **no-op EOL stub**; `sqlite3_native_assets` is **discontinued**; LiteFS is **not** archived despite being dormant; and the widely-cited "SQLite is 10–20× faster" page is **disclaimed by SQLite itself**.

---

## 0. Executive summary

1. **The memory argument for leaving MySQL is strong and I measured it on this machine.** The MySQL server process holds **486 MB working set / 857 MB private commit**, and it is configured with a **256 MB `innodb_buffer_pool_size`** — roughly **twice the entire logical dataset** (137 MB) and **~10× the actual user content** (~13 MB once you exclude one append-only event-log table). SQLite's own documented memory floor is on the order of **~50 KB per connection plus a lazily-allocated ~2 MB page cache**. The server's footprint, not SQLite's, dominates either way.

2. **The real prize is deleting the JVM process, and that is a separate decision from the database.** The Kotlin/Ktor backend on this machine is **344 MB working set / 396 MB private commit**, with 77 threads. Swapping MySQL→SQLite while keeping Ktor saves you the MySQL process (~480 MB); swapping the whole server to a small native process saves ~830 MB. **[V, MEASURED]** For scale, I measured a **minimal Rust HTTP server at 4.23–5.00 MB working set** on this machine, and published cross-checks put `axum` under 15 MB idle and Ktor at 300–400 MB. So Rust *would* win by ~2 orders of magnitude — but **you can remove most of the 344 MB without leaving Kotlin** (§6.4), so the language migration is a *sequencing* question, not a blocker.

3. **Android's built-in SQLite is not a viable target for this app, and this is now verified rather than assumed.** AOSP's own sources give the per-release versions — **Android 13 still ships SQLite 3.32a**, and the **newest system SQLite anywhere is 3.50.6 (Android 17)**. AOSP compiles it with **no FTS5, no math functions, and `SQLITE_OMIT_LOAD_EXTENSION` set** (extension loading is compiled out, which alone rules out CR-SQLite on system SQLite), and with `DQS=1` where drift uses `DQS=0`. Also: **every shipping Android system SQLite falls inside the WAL-reset bug's vulnerable range** (3.7.0–3.51.2). **Bundle your own SQLite** — `package:sqlite3` v3 does it for you and ships **3.53.4**, past the fix. Sources: §3.1.

4. **"Single writer" is a non-issue at this workload.** SQLite allows one writer at a time, but writes are milliseconds and queue. The official guidance is that a site under 100K hits/day is fine on SQLite, and the "high concurrency" caveat is about many simultaneous writers. With one user and two devices, this is not the constraint that matters. **But two configuration details do matter and are easy to miss: use `BEGIN IMMEDIATE` for writing transactions, and note that drift does not enable WAL for you** (§2.6, §3.3).

5. **None of the off-the-shelf sync engines is a clean fit.** PowerSync is the only one with a GA Flutter SDK and a real write-back queue, but self-hosted it needs a bucket-storage database (MongoDB or Postgres) plus an auth service plus your own upload endpoint — the vendor's own minimum dev sizing is **512 MB for the service plus 2 GB for MongoDB**, which defeats the purpose on a RAM-scarce box. ElectricSQL is **read-path only** (no write sync at all), has **no Dart client**, requires Postgres, and **shut down its cloud on 2026-08-11**. Turso's current `Turso Sync` has the right semantics and the lightest runtime, but its **only Dart binding is a 0.1.0 community package with 238 downloads**, and its self-hosted sync server is **documented for development and testing only**. The one product with an official Flutter SDK, **Ditto, is proprietary and commercial**. No CRDT-on-SQLite option offers Dart bindings at all.

6. **Recommendation:** move the storage layer to **SQLite (WAL) via `package:sqlite3` v3 + drift**, keep the hub as the single authority, and implement the client sync as **an application-level delta API over the existing HTTP surface** — i.e. a disciplined version of the hand-rolled approach, not a generic sync engine. The app already has `ApiClient(baseUrl, ...)` with an injectable base URL, so "point the Android client at the LAN hub" is already architecturally supported. Treat Rust as a *later, optional* step rather than a precondition. Details and reasoning in §8.

7. **Do not sync the SQLite file itself with Syncthing/rsync/Dropbox.** This is not a style preference; SQLite documents specific corruption mechanisms for exactly this pattern — including a section literally titled *"Why Can't I Just Use Ordinary rsync?"* — and WAL mode makes it strictly worse because a committed transaction may live only in the `-wal` sidecar. Use `sqlite3_rsync`, `VACUUM INTO`, or the backup API instead (§4.7).

8. **The honest gap in the precedent evidence:** there is **no published measured "we replaced MySQL/Postgres with SQLite and saved N MB of RSS" study**, and **no verified production JVM→Rust RSS report** — the commonly cited Figma and Discord examples are *not* those migrations. The memory case in this report therefore rests on **your own measurements** plus SQLite's documented and now-locally-confirmed memory model, not on someone else's case study. §7 says so explicitly.

---

## 1. Measured baseline: what this app actually is today

I inspected the working tree at `D:\myProject\FlutterProject\viewer` and measured the running processes. **These are measurements, not estimates [V].**

### 1.1 Stack

| Component | What it is |
| --- | --- |
| Backend | Kotlin 2.2.20, **Ktor 3.3.3** (Netty engine), kotlinx-serialization, Java 21 target |
| DB access | **HikariCP 6.3.3** + `com.mysql:mysql-connector-j:9.4.0` (raw JDBC, MySQL dialect features used directly) |
| Other | logback 1.5.18, TwelveMonkeys `imageio-webp` 3.12.0 |
| DB server | MySQL 8.4.3 (portable zip install), instance dir `.mysql`, port 3307 |
| Client | Flutter; `lib/core/api_client.dart` with `ApiClient(baseUrl, ...)`, `baseUrl` injectable |
| Layout | `server/` (Kotlin), `lib/` (Dart), `android/`, `db/*.sql`, `comfyui/`, `storage/` |

Source: `server/build.gradle.kts` [V], `lib/core/api_client.dart` [V].

### 1.2 Memory, as measured

| Process | Working set | Private commit | Threads | Notes |
| --- | --- | --- | --- | --- |
| `mysqld` (the serving instance) | **486.1 MB** | **856.7 MB** | 39 | PID 19964 |
| **ComfyHub Kotlin/Ktor backend** | **343.9 MB** | **395.7 MB** | 77 | PID 20756, identified by its `-Djava.net.useSystemProxies=true -Dfile.encoding=UTF-8` args matching `build.gradle.kts` |
| Gradle/Kotlin daemons (2 more JVMs) | 453.9 / 1133.8 MB | — | — | Android Studio tooling, **not** runtime footprint |

**Runtime DB + backend cost ≈ 830 MB working set / ~1.25 GB private commit.** Machine total RAM: **23.3 GB**.

### 1.3 MySQL's own configuration explains the footprint

Read from the live server via `SELECT @@...` [V]:

| Setting | Value | Comment |
| --- | --- | --- |
| `innodb_buffer_pool_size` | **256 MB** | **~1.9× the entire logical dataset.** This is allocated independent of how little data you have. |
| `innodb_redo_log_capacity` | 100 MB | |
| `log_bin` | 1 (on) | Binary logging enabled by default in MySQL 8.4 |
| `binlog_format` | ROW | Row-based ⇒ verbose binlogs |
| `binlog_expire_logs_seconds` | 2,592,000 | **30 days** retention |

### 1.4 Data on disk vs data that matters

| Thing | Size |
| --- | --- |
| `.mysql` total | **4,799.8 MB** |
| ↳ **binlogs** (largest files) | **~4,308 MB** — i.e. **~90 % of the on-disk footprint is replication/CDC logs for a single-user app** |
| ↳ `#innodb_redo` | 100 MB |
| ↳ `comfy_hub` schema | **218.2 MB** |
| InnoDB logical size (17 relations) | **137.3 MB** |
| ↳ `ai_run_events` alone | **112.9 MB / 445,956 rows** — an append-only run-event log |
| **Real user content** (`prompts`, `media_assets`, `tags`, `ai_conversations`, `ai_messages`) | **~2.6 MB across ~190 rows** |

Schema: 16 tables + 1 view (`v_media_full`). Largest tables: `ai_run_events` (446K rows), `ai_message_parts` (1,319), `ai_tool_calls` (379), `capture_runs` (105). Actual library tables are tiny: `prompts` 13 rows, `media_assets` 16, `tags` 16, `ai_conversations` 21.

**This is the single most important number in the report [I]:** the dataset that sync actually needs to move between desktop and phone is on the order of **a few megabytes**, and the bulk of the 137 MB is one append-only telemetry table that the phone does not need in full. Any sync design that treats "the database" as one blob to replicate is solving the wrong problem.

### 1.5 Migrations today

`server/src/main/kotlin/com/comfyhub/Migrate.kt` is a **340-line hand-rolled, convergent (idempotent) migration runner** [V]:

- uses `CREATE TABLE IF NOT EXISTS` plus DDL
- probes `information_schema.COLUMNS` / `information_schema.STATISTICS` / `TABLES` for existence before `ALTER TABLE ... ADD COLUMN` / `ADD INDEX`
- **has no version table** (`SHOW TABLES FROM comfy_hub` returns no `schema_migrations` / `schema_version` row) — it converges toward the desired shape rather than replaying an ordered chain

Companion SQL files: `db/schema.sql` (19.6 KB), `db/migrate.sql` (14.3 KB), `db/seed.sql` (3.7 KB).

**[I]** This is good news for a SQLite port: the convergent style maps almost directly onto SQLite (`PRAGMA table_info` instead of `information_schema`), and there is no versionbook-keeping to migrate.

---

## 2. Embedded SQLite in a server process

### 2.1 Memory footprint: SQLite vs MySQL/Postgres for a small single-user dataset

**SQLite's documented memory behaviour [V]** (all from <https://sqlite.org/malloc.html> and <https://www.sqlite.org/pragma.html#pragma_cache_size>):

| Component | Documented size | Source |
| --- | --- | --- |
| Lookaside allocator (per connection) | **48 KB default** (40 slots × 1200 B), split into 93 × 128 B + 30 × 1200 B slots. Was 120 KB before SQLite 3.31.0 (2020-01-22). | malloc.html §3.3.1 |
| Page cache (per open database file) | `PRAGMA cache_size` default is **-2000 ⇒ 2,048,000 bytes (2 MB)**. **It is an upper bound, allocated in smaller chunks on demand, not a reservation.** | pragma.html#cache_size |
| General-purpose memory | "A typical application might only use about **40 KB** of general-purpose memory. This gives a value of **N** of around **100 KB**." | malloc.html §4.1 |
| Which dominates | "It is not unusual to see the database page cache consume over **10 times more memory** than the rest of SQLite combined." | malloc.html §3.2 |
| Hard cap | SQLite "will refuse to allocate more than about **2 GB** of memory at one go"; "SQLite seldom ever allocates more than about 8 KB at a time" | malloc.html §2.1 |

**[I] Therefore a SQLite-backed process's RAM is dominated by the host language runtime, not by SQLite.** The SQLite-attributable budget is comfortably under ~3 MB for this workload (`cache_size` is a ceiling reached only if the working set is that large). This is the crux: *SQLite cannot be the reason a server uses hundreds of megabytes.*

#### And that budget is now measured, not just documented [V, MEASURED]

A local probe (Windows, CPython 3.13.1, bundled SQLite 3.45.3, DB = 35.5 MiB / 400,000 rows) on this machine:

| Stage | Working set | Private commit | Δ |
| --- | --- | --- | --- |
| Bare interpreter | 16,552 KiB | 9,436 KiB | — |
| After `import sqlite3` | 16,928 KiB | 9,496 KiB | +376 KiB |
| **Connection open + first query** | 20,056 KiB | 12,056 KiB | **+3,128 KiB WS / +2,560 KiB commit** |
| 12 full table scans, default cache (`-2000`) | 20,064 KiB | 12,056 KiB | +8 KiB |
| 12 scans with `cache_size=60000` pages (**~234 MiB cap**) | 22,408 KiB | 14,476 KiB | **+2,344 KiB** — *not* +234 MiB |
| Connection closed | 18,276 KiB | 10,824 KiB | −4,132 KiB |
| `mmap_size=1 GiB`, 12 scans | 22,280 KiB | 11,196 KiB | +4,004 KiB WS but only **+372 KiB commit** |

**[I] Three things this proves that the docs only assert:**
1. **One SQLite connection in a live process costs ~3 MB**, not hundreds.
2. **`cache_size` is a cap, not a commitment** — a 234 MiB cap produced +2.3 MB. That is the documented "allocated in smaller chunks on an as-needed basis" behaviour, confirmed.
3. **`mmap_size` shifts cost from private commit into file-backed working set** (+4 MB WS, +0.4 MB commit) — exactly the "shares pages with the OS page cache / may need less RAM" property, and the reason **`mmap_size` genuinely helps on a RAM-constrained box** for read-heavy work (SQLite is explicit that it *"does not significantly change the performance of database changes… mostly a benefit for queries"*, and warns that on **Windows a memory-mapped file cannot be truncated**, so `VACUUM`/`auto_vacuum` size reductions *"silently fail"*).
- **[?] Caveats:** Windows Working Set ≠ Linux RSS; single-threaded, single run, small DB; **not a published figure**. And note **`sqlite3_soft_heap_limit64()` is not a usable RSS cap** — it *"does not know about or interact with"* the page-cache or lookaside allocators **[V]**.

#### Contrast with MySQL and PostgreSQL [V — sources obtained via mirrors]

`dev.mysql.com` returns **HTTP 403** to this environment, but the same documentation is served from Oracle's mirrors and cross-checked between two CDNs:

| Setting | Default | Source |
| --- | --- | --- |
| MySQL `innodb_buffer_pool_size` | **128 MiB**, and **"InnoDB allocates memory for the entire buffer pool at server startup, using `malloc()` operations"** | [Oracle mirror, buffer-pool-resize](https://docs.oracle.com/cd/E17952_01/mysql-8.4-en/innodb-buffer-pool-resize.html), [memory-use](https://docs.oracle.com/cd/E17952_01/mysql-8.4-en/memory-use.html) |
| The only official MySQL hardware floor I could find | *"The default configuration is designed to permit a MySQL server to start on a virtual machine that has approximately **512 MB of RAM**."* **[?]** There is **no** "MySQL requires at least X MB" statement in the install pages | memory-use.html |
| PostgreSQL `shared_buffers` | **128 MB** (min 128 kB) | [runtime-config-resource](https://www.postgresql.org/docs/current/runtime-config-resource.html) |
| PostgreSQL `work_mem` | **4 MB**, multiplied by `hash_mem_multiplier` (2.0) | same |
| PostgreSQL `maintenance_work_mem` | 64 MB × up to `autovacuum_max_workers` | same |
| PostgreSQL docs' own advice | *"If you have a dedicated database server with **1 GB or more of RAM**, a reasonable starting value for shared_buffers is **25% of the memory**"* — which on a 16 GB box means **4 GB** | same |

**Two traps worth internalising [V]:**
- **`work_mem` is per sort/hash *operation*, not per connection.** The docs are explicit: *"a complex query might perform several sort and hash operations at the same time… **several running sessions could be doing such operations concurrently**. Therefore, the total memory used could be **many times** the value of work_mem."* Do not multiply `work_mem` by connection count and call it a footprint; and do not reason about Postgres memory from that number at all.
- **Never sum per-process RSS for PostgreSQL.** Tom Lane, on the record twice: *"The RSS numbers probably count all or nearly all of that for each process, but of course there's really only **one copy** of the shared memory segment… the actual additional memory used per process is probably just a few meg"* ([2012](https://www.postgresql.org/message-id/6892.1352863143%40sss.pgh.pa.us)); *"Many Unix tools such as 'top' will **count the shared memory segment as memory consumed by each Postgres process**"* ([2021](https://www.postgresql.org/message-id/792460.1611184756%40sss.pgh.pa.us)). A PGConf.DE 2025 session agrees: *"A newly created connection consumes only up to **10 MB** of physical memory, independent of the work_mem setting."*
- **[?]** There is **no documented minimum RAM for PostgreSQL**; `install-requirements.html` lists only build dependencies, and the two wiki pages that are usually cited for hardware requirements **do not exist**.

**This deployment, measured [V]:** `innodb_buffer_pool_size` is set to **256 MB** (2× the upstream default), allocated at startup regardless of dataset size, against a **137 MB** logical dataset whose real user content is **~2.6 MB**. That plus 100 MB redo capacity, per-connection buffers, and thread stacks accounts for the observed process. Note that **working set fluctuates** between observations (**486.1 MB** and **607.9 MB** in two runs) because it includes reclaimable file-backed pages, while **private commit was stable at ~857 MB** (856.7 / 859.0). **[I] Quote the private-commit figure if you want a stable number; working set is the one that moves.**

**[I] One claim to stop making:** the famous "SQLite is 10–20× faster than MySQL/Postgres" figure comes from <https://sqlite.org/speed.html>, which SQLite **itself banners as meaningless**: *"This document is very very old… The numbers here have become meaningless. This page has been retained only as an historical artifact."* The comparison was SQLite 2.7.6 vs PostgreSQL 7.1.3 vs MySQL 3.23.41 on 2002-era hardware. **Do not cite it.** SQLite's actual defensible argument is qualitative: *"In a client/server database, each SQL statement requires a message round-trip… **Queries do not involve message round-trips, only a function call**"* ([np1queryprob.html](https://sqlite.org/np1queryprob.html)). **[?] I found no modern apples-to-apples SQLite-vs-MySQL/Postgres latency microbenchmark for a small local dataset** — that number would have to be your own measurement.

### 2.2 WAL mode

From <https://sqlite.org/wal.html> [V]:

**Advantages**
1. "WAL is significantly faster in most scenarios."
2. "WAL provides more concurrency as readers do not block writers and a writer does not block readers. Reading and writing can proceed concurrently."
3. Disk I/O "tends to be more sequential".
4. "WAL uses many fewer fsync() operations".

**Disadvantages (verbatim, and all of them matter here)**
1. **"All processes using a database must be on the same host computer; WAL does not work over a network filesystem."** Because the wal-index needs shared memory.
2. Multi-database `ATTACH` transactions are atomic per database, **not** across the set.
3. **Page size cannot be changed once in WAL mode.**
4. (Relaxed in 3.22.0/2018: read-only WAL databases can be opened if `-shm`/`-wal` exist or are creatable.)
5. "WAL might be very slightly slower (**perhaps 1% or 2%**)" for read-mostly workloads.
6. "There is an additional quasi-persistent `-wal` file and `-shm` shared memory file associated with each database, which can make SQLite less appealing for use as an **application file-format**." ← **This is precisely why file-level sync is dangerous.**
7. "There is the extra operation of checkpointing."
8. (Large-transaction penalty removed in 3.11.0/2016.)

**Checkpointing mechanics [V]:** default is automatic at **1000 pages (~4 MB)**; a checkpoint must stop at the end mark of any active reader, so **a long-running read transaction can prevent a checkpointer from making progress**, and with "many concurrent overlapping readers and there is always at least one active reader, then no checkpoints will be able to complete and hence the WAL file will grow without bound". Read performance also degrades as the WAL grows, so keeping it small matters. `synchronous=NORMAL` means the checkpoint is **the only** operation issuing an fsync.

**`SQLITE_BUSY` in WAL mode [V]** — the doc is explicit that writers-blocking-readers is only "*mostly* true". Documented cases where a query can still return `SQLITE_BUSY`: another connection holding exclusive locking mode; the last connection closing and cleaning up; and a crashed connection's recovery holding an exclusive lock. **So WAL does not remove the need for a busy handler.**

**The WAL-reset bug [V]** — genuinely important and recent, from the same page:

> On **2026-03-03**, one of the SQLite developers found and fixed a bug that could, in rare cases, lead to database corruption… likely present in **all versions from 3.7.0 (2010-07-21) through 3.51.2 (2026-01-09)**. It is fixed in **3.51.3 (2026-03-13)**, with backports in 3.44.6 and 3.50.7.

Conditions: "two or more database connections open on the same file, in separate threads or processes, and when those two connections attempt to write or checkpoint at the same instant." The developers "were unable to reproduce the bug organically"; as of a 2026-08-24 update, a reproducer without the test hack exists. The doc's own risk framing: the occurrence rate "appears to be less than or equal to the expected occurrence rate of SSD malfunctions and/or cosmic-ray hits."

**[I] Action item:** whatever version you ship, **ensure the bundled SQLite is ≥ 3.51.3**. This is directly checkable in the Dart stack: `package:sqlite3` v3 downloads/compiles SQLite via a build hook and lets you pin the amalgamation URL (see §3.2), and `rusqlite`'s `bundled` feature compiles a vendored copy.

### 2.3 Concurrency model (single writer)

From <https://sqlite.org/whentouse.html> [V]:

- "SQLite supports an unlimited number of simultaneous readers, but it will only allow **one writer at any instant in time**. For many situations, this is not a problem. **Writers queue up.** Each application does its database work quickly and moves on, and **no lock lasts for more than a few dozen milliseconds**."
- "SQLite will handle more write concurrency than many people suspect."
- Rule-of-thumb guidance: "any site that gets fewer than **100K hits/day** should work fine with SQLite… a conservative estimate"; sqlite.org itself runs ~400–500K HTTP requests/day with 15–20 % dynamic pages on one VM.
- The explicit anti-pattern: "avoid using SQLite in situations where the same database will be accessed directly (without an intervening application server) and **simultaneously from many computers over a network**."
- **Crucially, the doc blesses the exact architecture this app wants:** "Systems designers report success using SQLite as a data store on **server applications** running in the datacenter… With this pattern, the overall system is still client/server: clients send requests to the server and get back replies over the network. But instead of sending generic SQL… the client requests and server responses are **high-level and application-specific**. The server translates requests into multiple SQL queries… **Developers report that SQLite is often faster than a client/server SQL database engine in this scenario.** Database requests are serialized by the server, so **concurrency is not an issue**."

**[I]** This is the canonical "SQLite as a server-side database" pattern, and it is exactly what "desktop hub with a local backend + HTTP clients" already is. The Android phone should talk to the hub's API, never to the database file.

### 2.4 What a server must do about backups

From <https://sqlite.org/backup.html> and <https://sqlite.org/lang_vacuum.html> [V]:

The naive approach — "lock, copy the file, unlock" — has three documented shortcomings: writers must wait; it cannot copy in-memory DBs; and **"if a power failure or operating system failure occurs while copying the database file the backup database may be corrupted following system recovery."**

Three safe methods, all explicitly endorsed for **live** databases:

1. **`VACUUM INTO 'file'`** — "will make a vacuumed copy of a live SQLite database into a separate file." Transactional: the output is a consistent snapshot, and with `synchronous` at NORMAL or FULL, SQLite fsyncs the output, so a power failure after completion should not corrupt it. Advantage: minimal size and **"all deleted content is purged from the backup, leaving behind no forensic traces."** Disadvantage: "uses fewer CPU cycles" is *not* true of VACUUM INTO — the backup API "uses fewer CPU cycles and can be executed incrementally". Needs up to **2× the database size in free disk space** (VACUUM generally). The target file **must not already exist** (or must be empty) or the command fails.
2. **The Online Backup API** (`sqlite3_backup_init/step/finish`) — the original method; can run **incrementally**, holding the source lock only during actual reads, so other users continue. "The effect of completing the backup call sequence is to make the destination a bit-wise identical copy of the source database as it was when the copying commenced."
3. **`sqlite3_rsync`** (SQLite ≥ 3.47.0, 2024-10-21) — "makes a copy of a live SQLite database to or from a remote system using an SSH connection."

**Two practical warnings from the docs [V]:**
- If the backup is restarted frequently enough (because writers keep touching the source), **"it may never run to completion"**. Schedule backups for quiet periods.
- "It is also safe to make a copy of an SQLite database file as long as there are **no transactions in progress**… If the previous write transaction failed, then it is important that any rollback journal (`-journal`) or write-ahead log (`-wal`) be copied together with the database file itself."

**[I] For this app:** a daily `VACUUM INTO` to a timestamped file, run from the hub server on a timer, is the right choice — it is one statement, needs no extra library, and yields a compact single-file artifact that is *also* a convenient thing to hand to a phone for a bootstrap (see §6.7). Note the 2× disk requirement and the "file must not exist" rule.

### 2.5 What a server must do about migrations

SQLite's own tool for this is **`PRAGMA user_version`** — a 32-bit signed integer stored in the database header (offset 60), which "applications that use SQLite as their application file-format" use to track their schema version; `PRAGMA application_id` (offset 68) stores a magic number so tools like `file(1)` can identify the file type. Both are listed in the [PRAGMA reference](https://www.sqlite.org/pragma.html#pragma_user_version) [V].

**[I] The recommended pattern** (standard practice, consistent with the above):

1. On open, read `PRAGMA user_version`.
2. If it is less than the code's target, run the migration steps for each version in order inside **one transaction**, then set `user_version` to the new value and commit.
3. Bump `user_version` in the same transaction as the DDL, so a crash cannot leave the version ahead of the schema.
4. Never edit a migration that has shipped.
5. `PRAGMA foreign_keys` is **per-connection and off by default** — set it on every connection if you want FK enforcement. So does `PRAGMA journal_mode` need to be *at least once* persisted (it is persistent, unlike other journal modes — wal.html §3.3), but `PRAGMA synchronous`, `PRAGMA cache_size`, `PRAGMA busy_timeout`, and `PRAGMA foreign_keys` are per-connection and must be re-applied each time.
6. Run `PRAGMA optimize` — the docs' recommendation: "Applications with short-lived database connections should run `PRAGMA optimize` once, just prior to closing each database connection. Applications that use long-lived database connections should run `PRAGMA optimize=0x10002` when the connection is first opened, and then also run `PRAGMA optimize` periodically, perhaps **once per day**. All applications should run `PRAGMA optimize` after a schema change" [V].

**[I] Note the alternative and why it is better here:** the existing `Migrate.kt` is *convergent* (probe-then-alter). Convergent DDL is robust and needs no version bookkeeping, and it ports to SQLite easily by swapping `information_schema` probes for `PRAGMA table_info(t)` / `PRAGMA index_list(t)`. But it becomes slow and ambiguous once you need **data** migrations or **destructive** changes (dropping/renaming columns, backfilling), because "make it look like this" cannot express "transform old values into new ones once". **[I] Recommendation: keep convergence for additive DDL, add `user_version`-gated ordered steps for anything that transforms data.**

### 2.6 A short list of SQLite server gotchas worth knowing up front

All from SQLite's own docs [V]:

- **`PRAGMA busy_timeout` must be set.** `SQLITE_BUSY` is still possible in WAL mode (wal.html §9). Set a timeout rather than retrying manually.
- **Windows cannot truncate a memory-mapped file** (mmap.html): "on Windows, if an operation such as VACUUM or auto_vacuum tries to reduce the size of a memory-mapped database file, the size reduction attempt will **silently fail**, leaving unused space at the end of the database file." **This is a concrete Windows-desktop reason to think carefully before enabling `mmap_size`.** Memory-mapped I/O is **disabled by default** and its stated advantages are "faster" plus "**may need less RAM** since it shares pages with the operating-system page cache".
- `PRAGMA cache_spill` (on by default) "has the side-effect of acquiring an EXCLUSIVE lock on the database file" — relevant if you disable it in long transactions.
- **`journal_mode=OFF` and `MEMORY` risk corruption** (§7 of howtocorrupt.html). Do not use them.
- **Do not use `mmap` if the application can have stray pointers** — "a stray pointer that overwrites any part of that mapped space will immediately corrupt the database file" (howtocorrupt.html §5).
- **Keep the connection count low and deliberate.** Every connection costs ~48 KB lookaside plus page-cache headroom, and more connections mean more chances for checkpoint starvation. A single-writer thread plus a small reader pool is the shape to aim for.

> **⚠ `BEGIN IMMEDIATE` is the single most important line of code in a SQLite server.** Independent benchmarking calls it *"one of the biggest footguns of SQLite"*: a plain `BEGIN` (i.e. `BEGIN DEFERRED`) starts as a read transaction and only tries to upgrade to a write lock at the first write statement — and **the lock upgrade can fail with `SQLITE_BUSY` *without* honouring `busy_timeout`**, because SQLite cannot safely retry an upgrade that would deadlock. The fix is to take the write lock up front: **use `BEGIN IMMEDIATE` for every transaction that will write.** Recommended companion PRAGMAs from the same source, all verifiable against SQLite's docs: `journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL`, `temp_store=memory`, plus **one dedicated writer connection**. [Source: Sylvain Kerkour, "Optimizing SQLite for servers", 15 Feb 2024 — [V]] **[I] This interacts directly with §5: if your sync outbox is written in the same transaction as the mutation (as it must be), that transaction writes — so it must be `BEGIN IMMEDIATE`, or you will see intermittent `SQLITE_BUSY` under concurrency and be tempted to blame the sync logic.**

---

## 3. SQLite on Android

### 3.1 What Android ships, and why you should not rely on it

**This gap got closed.** `developer.android.com` and `android.googlesource.com` are unreachable through the `web_fetch` tool in this environment but **reachable via `pwsh` + `Invoke-WebRequest`**, so the table below is from **AOSP's own `platform/external/sqlite` per-branch sources** (base64-decoded gitiles `?format=TEXT` responses, plus `sqlite3.h` in the `dist/` trees). **[V]**

| Android | API | **system SQLite** |
| --- | --- | --- |
| 10 | 29 | **3.22.0** |
| 11 | 30 | **3.28.0** |
| 12 | 31 / 32 | **3.32a** |
| 13 | 33 | **3.32a** |
| 14 | 34 | **3.39.2** |
| 15 | 35 | **3.44.3** |
| 16 | 36 | **3.44.4** |
| 16 QPR2 | 36.1 | **3.50.4** |
| 17 | 37 | **3.50.6** |

Source pattern: `https://android.googlesource.com/platform/external/sqlite/+/refs/heads/android14-release/README.version?format=TEXT` (and the equivalent `dist/…/sqlite3.h` per branch). **[V]**

**Three conclusions that change the design [V]:**
1. **You can rely on SQLite ≥ 3.39 only on API 34+.** AOSP still shipped **3.32a as recently as Android 13**.
2. **You can never rely on 3.51+.** The newest system SQLite anywhere in that table is **3.50.6 (Android 17)**. **SQLite 3.51+ has never shipped on Android.**
3. **Every shipping Android system SQLite falls inside the WAL-reset bug's vulnerable range** — that bug spans **3.7.0 through 3.51.2, fixed in 3.51.3 (2026-03-13)** (§2.2), and the backports exist only for 3.44.6 and 3.50.7, which are not what ships. **[V]** for the range; **[I]** for "the shipping builds lack the backport".

**[?] Android 8/9 exact versions were not verifiable** — those AOSP refs don't expose `README.version` or `dist/`. `sqflite`'s own doc lists API 27 → 3.19 and API 26 → 3.18 (<https://raw.githubusercontent.com/tekartik/sqflite/master/sqflite/doc/version.md>) — **[V]** as a *sqflite claim*, not as AOSP truth.

#### Android's system SQLite is compiled with important things turned OFF

From AOSP's `dist/Android.bp` `cflags` (identical on `main` and `android17-release`) **[V]**: 

**Enabled:** `SQLITE_ENABLE_FTS3`, `SQLITE_ENABLE_FTS3_BACKWARDS`, `SQLITE_ENABLE_FTS4`, `SQLITE_ENABLE_DBSTAT_VTAB`, `SQLITE_ENABLE_MEMORY_MANAGEMENT`, `SQLITE_ENABLE_BATCH_ATOMIC_WRITE`, `SQLITE_ENABLE_BYTECODE_VTAB`, `SQLITE_DEFAULT_AUTOVACUUM=1`, `SQLITE_SECURE_DELETE`, `SQLITE_TEMP_STORE=3`, `SQLITE_THREADSAFE=2`, `SQLITE_DEFAULT_JOURNAL_SIZE_LIMIT=1048576`, `SQLITE_DEFAULT_FILE_FORMAT=4`.

**NOT present — and these are the ones that matter:**
- **`SQLITE_ENABLE_FTS5` → absent. The system SQLite has no FTS5 at any Android version.** FTS5 requires this compile-time flag. **[V]**
- **`SQLITE_OMIT_LOAD_EXTENSION` → present. Extension loading is compiled out system-wide.** **[V]** ← **This is decisive for the CRDT option**: you **cannot** load CR-SQLite into Android's system SQLite, at any API level. It also independently confirms the §4.5 conclusion.
- `SQLITE_ENABLE_JSON1` → absent. But JSON functions became **core built-ins in SQLite 3.38.0**, so JSON is available **only on API 34+ (3.39.2+)** and **not on API ≤ 33 (3.32a)**. **[V]** for the mechanism ([json1.html](https://sqlite.org/json1.html)); **[I]** for the per-API conclusion.
- No `SQLITE_ENABLE_MATH_FUNCTIONS`, no `SQLITE_ENABLE_SESSION`/`PREUPDATE_HOOK`, no `SQLITE_ENABLE_RTREE`.
- **`SQLITE_DQS` is not set** → default DQS=1, i.e. double-quoted string literals are *accepted*. drift compiles with **`SQLITE_DQS=0`**. **[I]** That is a real behavioral divergence between "system SQLite" and "bundled SQLite" that can change query semantics.
- WAL is **not** a compile-time option — it is a runtime journal mode, so system SQLite does support WAL. **[V]**

**[I] Net: "just use Android's SQLite" is a trap for this app.** You would lose FTS5, lose JSON on most devices, lose extension loading, and inherit a WAL-corruption race on multi-connection access — to save the cost of bundling a library that the modern Dart stack bundles for you by default.

#### The official Android API, and its WAL defaults

From `developer.android.com` (fetched via `pwsh`) **[V]**:
- **`SQLiteDatabase`: "Write-ahead logging is not enabled by default."** Passing `ENABLE_WRITE_AHEAD_LOGGING` to `openDatabase(...)` is documented as **more efficient** than calling `enableWriteAheadLogging()` afterwards.
- `enableWriteAheadLogging()` **returns `false`** if attached databases exist, if the DB is read-only, or if it is in-memory; it throws `IllegalStateException` if transactions are in progress.
- `SQLiteOpenHelper.setWriteAheadLoggingEnabled(boolean)`: *"Write-ahead logging cannot be used with read-only databases so the value of this flag is ignored if the database is opened read-only."*
- **`SQLiteDatabase` doc: "When the database has any attached databases, then execution of queries in parallel is NOT possible."**
- Writers should use `beginTransactionNonExclusive()`.
- **[I] Gotcha:** `isDbLockedByOtherThreads()` is deprecated since API 16 and **always returns `false`** — do **not** use it to detect contention.

### 3.2 Bundling your own SQLite in Flutter — and a significant 2026 change

**This part changed recently and the older advice is now wrong. [V]**

| Package | Status as of 2026-09-23 | Evidence |
| --- | --- | --- |
| **`sqlite3`** | **3.6.0**, published **9 days ago**. 464 likes, 2.45 M downloads, **MIT**. "Because this library uses **hooks**, it bundles SQLite with your application and **doesn't require any external dependencies or build configuration**." Prebuilt for Android `armv7a/aarch64/x86/x64`, iOS, macOS, Linux, **Windows `aarch64/x64/x86`**, Web. Deps: `code_assets`, `hooks`, `native_toolchain_c`. | [pub.dev/packages/sqlite3](https://pub.dev/packages/sqlite3) |
| **`sqlite3_flutter_libs`** | **EOL.** Version literally `0.6.0+eol`; description "Not used anymore, update to version 3.x of package:sqlite3 instead." "Starting from version `0.6.0`, this package **no longer does anything**." | [pub.dev/packages/sqlite3_flutter_libs](https://pub.dev/packages/sqlite3_flutter_libs) |
| **`sqlite3_native_assets`** | **discontinued** — "replaced by: **sqlite3**". Its native-assets functionality was folded into `package:sqlite3` v3. (It said "this package will eventually replace `sqlite3_flutter_libs`"; in the end the replacement landed inside `sqlite3` itself.) | [pub.dev/packages/sqlite3_native_assets](https://pub.dev/packages/sqlite3_native_assets) |

**So the modern answer is: depend on `package:sqlite3` v3, and SQLite is bundled via Dart build hooks.** No CMake, no NDK wiring of your own.

**[V] And the version it bundles is already the one you want: SQLite 3.53.4** (from the package changelog: *"Upgrade SQLite to 3.53.4 and SQLite3 Multiple Ciphers to 2.5.0"*). That is **newer than any Android system SQLite (3.50.6 ceiling)** and **past the WAL-reset fix (3.51.3)** — so the default configuration already gives you the §2.2 fix, with no pinning required.
- **[V] The bundled build is also strictly more capable than Android's system SQLite:** it enables `SQLITE_ENABLE_FTS5`, `SQLITE_ENABLE_RTREE`, `SQLITE_ENABLE_MATH_FUNCTIONS`, `SQLITE_ENABLE_DBSTAT_VTAB`, `SQLITE_ENABLE_SESSION`, `SQLITE_ENABLE_PREUPDATE_HOOK`, and `SQLITE_DQS=0`. Recall from §3.1 that AOSP has **none** of FTS5, math functions, or extension loading, and uses `DQS=1` — a real semantic divergence, not just a feature gap.
- **[V] Mechanism:** pre-compiled shared libraries per release on GitHub, downloaded at build time with **sha256 verification** against values pinned in the pub.dev package.

**And you can still pin/customise the build [V]** via `hooks: user_defines: sqlite3:` in `pubspec.yaml` (the config namespace survives the migration from `sqlite3_native_assets`):
- `source.amalgamation: <URL>` — e.g. pin `https://sqlite.org/2025/sqlite-amalgamation-3490100.zip`. Use this if you ever need a *specific* SQLite version.
- `source.local: third_party/sqlite3/sqlite3.c`
- `source.system` / `process` / `executable` — load the OS SQLite instead (**[I] do not do this on Android**, per §3.1)
- `source: false` — build nothing; supply your own custom library via a derived `CodeAsset`
- `defines.defines: {SQLITE_MAX_SCHEMA_RETRY: 2, ...}` and `defines.default_options: false`
- The same docs cover using **SQLite3MultipleCiphers or SQLCipher** instead of SQLite (encryption), on native platforms and on web.

> **⚠ Build risk specific to *this* repo [I, grounded in `AGENTS.md`]:** the hook **downloads binaries from GitHub releases at build time**. This repository's documented network reality is that `github.com` is reachable **only via the `127.0.0.1:7890` proxy**, while `dl.google.com`/Maven must **bypass** it. A build hook that fetches from GitHub is therefore a **new, proxy-dependent failure surface** — and the failure will look like a broken Flutter build, not a network problem. The documented escape hatch is `url_pattern` pointed at an internal mirror, or `source.local` with a vendored `sqlite3.c`. **Expect to have to deal with this on first setup, and note it in the pitfalls doc.**

**[I] Note the practical cost:** this makes SQLite compilation part of the build, so a working C toolchain and Dart build-hook support are prerequisites for every platform including Windows desktop. That is a real (one-time) build-plumbing change.

### 3.3 `drift` — the recommended Dart layer

[V] from [pub.dev/packages/drift](https://pub.dev/packages/drift):

- **drift 2.35.0**, published **13 days ago** → actively maintained.
- **2.47 K likes, 1.31 M downloads**, **MIT**, **Flutter Favorite**.
- Depends on `sqlite3 ^3.4.0` — i.e. it is already on the modern, hook-bundled SQLite.
- Platforms: Android, iOS, macOS, **Windows**, Linux, web.
- Feature claims relevant here: built-in **transactions**, **schema migrations**, "builtin **threading support**, allowing you to run database code across isolates with **zero additional effort**", reactive auto-updating query streams, compile-time-checked SQL.
- Sponsored by PowerSync (among others) — a signal that the PowerSync/drift integration is a first-class path.

**[I]** drift is the right choice over raw `sqlite3` here because it supplies exactly the two things you must otherwise hand-roll for a server: **ordered schema migrations** and **off-isolate query execution** (so the Flutter UI isolate never blocks on disk I/O). `flutter test` in this repo already runs 196 cases in ~23 s, so adding drift-generated code will be visible in build times; budget for it.

### 3.4 `sqflite` — the caveat that matters

[V] from [pub.dev/packages/sqflite](https://pub.dev/packages/sqflite):

- **sqflite 2.4.4**, published **12 days ago**, **5.57 K likes**, 3.15 M downloads, **BSD-2-Clause**. **It is maintained** — I want to be explicit about that, because "sqflite is abandoned" is a common but incorrect claim.
- Platforms declared: **Android, iOS, macOS only.** Linux/Windows/DartVM require the separate `sqflite_common_ffi`; web is experimental via `sqflite_common_ffi_web`. **[I]** So for a Windows-desktop hub, sqflite is not the natural fit; `sqlite3`/drift work natively on Windows.
- **Its own "Current issues" section is the decisive quote:**
  > "Due to the way transaction works in SQLite (threads), **concurrent read and write transaction are not supported**. All calls are currently synchronized and **transactions block are exclusive**. I thought that a basic way to support concurrent access is to open a database multiple times but **it only works on iOS as Android reuses the same database object**. I also thought a native thread could be a potential future solution however **on android accessing the database in another thread is blocked while in a transaction**…"
- It also documents that it uses **the platform's SQLite version**, with features varying by device.

**[I] Verdict: do not build the server on sqflite.** Its documented Android behaviour (exclusive transactions, no concurrent read/write, cross-thread access blocked during a transaction) throws away WAL's central benefit and is exactly wrong for a hub that must serve reads while writing. Use `package:sqlite3` + drift.

### 3.5 Android-specific pitfalls to plan around

**Read the marker on each row.** Rows marked **[V]** are documented in a source I fetched (the archived `sqlite3_flutter_libs` readme or SQLite's own docs). Rows without a marker are **[I]** — inferences from verified mechanics, and the specific numbers are things to test rather than documented guarantees. I could not reach `developer.android.com`, so anything that would normally be cited there is flagged.

| Pitfall | Why it bites | Mitigation |
| --- | --- | --- |
| **WAL sidecar files** | A committed transaction may live only in `db-wal` until a checkpoint. Any copy/backup that grabs `db` alone silently loses data — SQLite: "If a database file is separated from its WAL file, then transactions that were previously committed to the database might be lost, or the database file might become corrupted" [V]. | Never copy the file. Export via `VACUUM INTO` or the backup API, or (for sync) ship rows, not files. |
| **Android auto-backup** | Android's `allowBackup` can archive app data. A WAL-mode DB backed up mid-transaction yields an inconsistent pair. | **[?]** I could not fetch the Android docs. **Do not rely on platform auto-backup for the SQLite file**; exclude it and implement explicit export. |
| **`SQLITE_BUSY` under a single-process multiprocess/thread mix** | WAL does not eliminate `SQLITE_BUSY` (wal.html §9). | Set `PRAGMA busy_timeout`, and route all writes through one coordinator. |
| **Old system SQLite / the WAL-reset bug** | System SQLite may predate 3.51.3 (2026-03-13). | Bundle SQLite via `package:sqlite3` and pin the amalgamation. |
| **Native library loading on Android 6** — **[V]** | From the archived `sqlite3_flutter_libs` 0.5.32 readme: *"There appears to be a problem when loading native libraries on **Android 6**… If you're seeing those crashes, you could try setting `android.bundle.enableUncompressedNativeLibs=false` in your `gradle.properties`… Alternatively, you can use the `applyWorkaroundToOpenSqlite3OnOldAndroidVersions` method… It will try to open `sqlite3` in Java, which seems to work more reliably."* | Use the workaround if you support Android 6 — **and note the readme's own warning**: *"As `applyWorkaroundToOpenSqlite3OnOldAndroidVersions` uses **platform channels**, there may be issues when using it on a **background isolate**. We recommend awaiting it in the **main isolate**, before spawning a background isolate that might use the database."* **[V]** This **directly conflicts with "run your DB in a background isolate" (§3.6) on old Android** — plan for it. |
| **`SQLITE_IOERR_GETTEMPPATH 6410`** — **[V]** | Same readme: *"If you have complex queries failing with a `SQLITE_IOERR_GETTEMPPATH 6410` error, you could try to **explicitly set the temporary path** used by sqlite3."* | Configure the temp path explicitly rather than relying on Android's default temp dir. |
| **Extension loading for CRDT extensions** | Needs a custom SQLite build with extension loading; SQLite: *"For security reasons, **extension loading is turned off by default**"*, and a build can omit it entirely via `SQLITE_OMIT_LOAD_EXTENSION`. **[V]** | Only attempt if you commit to shipping your own library; otherwise skip CRDT-on-SQLite (§4.5). |
| **Low-RAM devices** | Page cache is an upper bound allocated on demand, so this is usually fine — but the default 2 MB/connection × several connections adds up. | Set a smaller `cache_size` on the phone; the phone's dataset is small anyway. |
| **Windows file locking + antivirus** | Windows has no unlink-while-open (which is protective), but AV/indexers scanning the DB directory can hold handles and cause `SQLITE_BUSY` (howtocorrupt.html §2). | Keep the DB in a dedicated directory excluded from AV real-time scanning; keep `-wal`/`-shm` next to it. |

**[V] Also from the same archived readme:** the bundled SQLite *"uses the recommended compile-time options. Additionally, it ships the `fts5` module by default (the `json1` module is part of the default build in recent sqlite3 versions)."* The Android ABIs bundled are `arm64-v8a`, `armeabi-v7a`, `x86`, `x86_64` — if you do not ship 32-bit `x86`, add an `abiFilters` entry to shrink the APK.

**[I] One caution about a widely-repeated claim:** that readme documents *that* bundling is the norm and *how* to work around old-Android loading bugs — it does **not** claim the system SQLite is outdated. **I could not verify a "Android's system SQLite is too old" claim from any source I fetched**, so treat that specific assertion as **[?]**. The argument for bundling still stands on its own, because of the WAL-reset bug (§2.2) and because bundling is what gives you a *pinned, known* version at all.

### 3.6 Practical concurrency on Flutter

**[I]** SQLite's C API is synchronous; `package:sqlite3` "provides direct and synchronous access to SQLite. For most apps, **running SQLite in a background isolate or web worker is a much better option**" [V, same pub.dev page]. drift has this built in. **Use drift's isolate support rather than calling `sqlite3` from `build()`.**

---

## 4. Sync engines compared

### 4.1 Summary table

| Option | Maintained? | License | Self-hostable? | Flutter/Dart? | Verdict for single-user LAN |
| --- | --- | --- | --- | --- | --- |
| **PowerSync (self-hosted)** | **Yes, active.** Service `pushed_at` 2026-09-22; v1.26.1 released 2026-09-11; Dart SDK `pushed_at` 2026-09-22. **[V]** | Client SDKs **Apache-2.0**; **service+CLI FSL-1.1-ALv2 → Apache-2.0 after 2 years**. **[V]** | **Yes — "PowerSync Open Edition", GA, free.** Docker `journeyapps/powersync-service`. No Dashboard when self-hosted. Needs bucket storage (MongoDB **or** Postgres) + JWT auth + your own upload endpoint. **[V]** | **Yes — GA.** `powersync` 2.4.0, Apache-2.0, verified publisher `powersync.com`, 154 likes, 29.6 K downloads. **[V]** | **Right feature set, wrong weight.** Only option with a real GA Flutter write-back queue; but vendor's own minimum is **512 MB service + 2 GB MongoDB**. Fails the RAM constraint. |
| **ElectricSQL** (now "Electric Sync") | Sync service **actively developed** (`pushed_at` 2026-09-09, `@core/sync-service@1.8.1` 2026-09-07, 10,368 ★, Apache-2.0). But **Electric Cloud shut down 2026-08-11** (joined Databricks). Old architecture explicitly unsupported. **[V]** | **Apache-2.0** (whole repo). No FSL/AGPL. **[V]** | **Yes, fully.** Needs **Postgres ≥14 + logical replication**, `DATABASE_URL`, `ELECTRIC_SECRET`, and a **persistent local filesystem** (shape metadata SQLite **corrupts on NFS/EFS**). **[V]** | **No.** Official clients are **TypeScript and Elixir only**. Dart package `electricsql` is **discontinued**; the vendor states the old system is unsupported. **[V]** | **Wrong shape entirely.** **Read-path only** — "Writes go through your existing backend." No Dart client. Requires Postgres. Rules itself out. |
| **Turso / libSQL** — legacy **Embedded Replicas** | libSQL maintained (`pushed_at` 2026-09-16, 17,227 ★, MIT) but vendor now says "**you should be using Turso, not libSQL, if you are using sync**". **[V]** | **MIT**. **[V]** | **Points at Turso Cloud.** **[?]** whether a self-hostable `sqld` remains for production is unverified. | **No official SDK.** Community: `libsql_dart` 0.9.0 (5 months old, 275 downloads, MIT). **[V]** | Writes go to the **remote primary by default** (`offline:true` to change that) — i.e. *not* local-first. Physical-page replication: one frame = **4 kB**, so a 1-byte row writes 4 kB. Docs warn: "Do not open the local database while the embedded replica is syncing. This can lead to data corruption." **[V]** |
| **Turso Sync** (current) | Built on the Rust **Turso Database** rewrite: 24,355 ★, `pushed_at` **2026-09-23**, MIT, docs now list it "Production-ready". **[V]** | **MIT**. **[V]** | **Local sync server ships in the CLI** (`tursodb ./server.db --sync-server 0.0.0.0:8080`, same protocol as Cloud, no auth token needed) — but docs scope it to "**development and testing**". Production = Turso Cloud; BYOC "we are **building**". **[V]** | **No official SDK** — Flutter/Dart is classed under "Community SDKs… don't come with official Turso support". Community: `turso_dart` **0.1.0** (3 months old, **2 likes, 238 downloads**). **[V]** | **Best operational fit, worst SDK maturity.** Right semantics (local-first `push()`/`pull()`, Last-Push-Wins, CDC), one binary, no external DB. Blocked by a 0.1.0 one-maintainer Dart binding. |
| **CR-SQLite / CRDT** | **Not archived**, **3,800 ★**, MIT, last commit **2026-08-10** — but **last *tagged release* was 2024-01-17**, so it is **feature-frozen**; Android is its least-downloaded target (167 vs 20,228 for linux-x86_64). **[V]** | **MIT**. **[V]** | N/A (an SQLite extension). | **No Dart bindings at all.** Automerge/Loro Dart packages are **v0.0.0 stubs whose repos 404**. **[V]** | **[I] Hard on Android** (needs a custom SQLite with extension loading), restricted schema (no checked FKs, no non-PK UNIQUE), 2.5× slower inserts — and CRDTs solve concurrent multi-writer editing, which one user on two devices mostly does not have. Overkill. |
| **Ditto** | Active, commercial. | **Proprietary / closed-source** (old OSS repo 404s). **[V]** | No — vendor cloud/mesh. | **Yes — official Flutter SDK.** **[V]** | **[I] The only off-the-shelf product with a real Flutter SDK — and the only genuine build-vs-buy alternative here.** Rejected for a single-user self-hosted app: closed-source commercial platform to move ~2.6 MB between two devices. |
| **LiteFS** | **Not archived**, **4,879 ★**, Apache-2.0, but `pushed_at` **2026-05-11** (≈4 months quiet), 62 open issues. **[V]** | **Apache-2.0**. **[V]** | It's infra you run. | No. | **[I] Architecturally wrong.** It is "a **FUSE-based file system** for replicating SQLite databases across a cluster of machines" — a server-cluster tool. FUSE on Android is not a thing. |
| **rqlite** | **Very active** — 17,756 ★, MIT, last push **2026-09-22**, 809 forks. **[V]** | **MIT**. **[V]** | Yes, it *is* the server. | Client is HTTP+JSON; no Dart SDK exists but one is trivial. **[I]** | **[I] Overkill and wrong shape.** Raft consensus across always-on servers, not offline-first peers — it reduces to "hub is a server, phone is an HTTP client", i.e. the design already recommended, minus the offline story. See §4.6. |
| **Hand-rolled LWW + tombstones** | You own it. | Yours. | Yes, in-process. | **Yes — pure Dart/Kotlin.** | **[I] The pragmatic choice here.** §5 details the minimum viable correctness bar. |
| **Litestream** (backup, not sync) | **Yes, active.** **14,389 ★**, Apache-2.0, **not archived**, last push **2026-09-21**, 153 open issues. Site: "v0.5.x — Latest — **Actively maintained**". **[V]** | **Apache-2.0** **[V]** | Yes, runs beside the DB. | **No.** | **It is a backup/DR tool, not a sync engine.** "Each database replicates to a **single** replica destination"; one-way; no merge, no conflict resolution, no return channel. Useful for §2.4, useless for the phone. See §4.6a. |
| **`sqlite3_rsync`** (SQLite's own tool) | Ships with SQLite; core feature. **[V]** | Public domain (SQLite). | Yes, over SSH. | No — desktop/CLI tool. | **The *supported* way to move a whole DB file**, and SQLite explains why plain rsync is unsafe. Still **one-way** and needs SSH. See §4.7. |
| **Syncthing/rsync/Dropbox on the SQLite file** | n/a | n/a | n/a | n/a | **Actively dangerous — see §4.7, where SQLite itself explains why.** |
| **CloudKit / Drive appdata as transport** | n/a (platform features) | Apple: proprietary platform. Drive: Google ToS. | No — vendor-hosted. | CloudKit: **no** (Apple platforms only). Drive: via REST. | **CloudKit is disqualified: its declared platforms are iOS/iPadOS/macOS/Catalyst/tvOS/visionOS/watchOS — no Android, no Windows [V].** Drive appdata is an opaque blob store with LWW-on-the-whole-blob and **unverified** specifics. See §4.8. |

### 4.2 PowerSync — detail

**What it is [V]:** the service owns the **read path** (replicate from source DB → buckets → stream to clients); your backend owns the **write path**. Clients hold a **real SQLite file**. Your declared schema becomes **SQLite views over schemaless JSON** in `ps_data__<table>`; the SDK also maintains `ps_data_local__*`, `ps_untyped`, `ps_oplog`, `ps_crud` (the upload queue), `ps_buckets`, `ps_migrations`.

**Consequences of the JSON-view design that matter [I]:**
- Each row normally exists in **at least two tables** (`ps_data__*` and `ps_oplog`) — "Most rows are present in at least two tables." **So the client database is substantially larger than the logical data.** The docs explicitly describe a `ps_oplog` copy that "may be newer" than the `ps_data__*` copy until a full checkpoint lands.
- Your columns are extracted from JSON via `CAST(data ->> '$.col' as TEXT)`. **Indexing real columns is not the default**; there is a "Raw Tables" escape hatch for when the JSON-view system's limits bite.
- **[I]** For a table like this app's `ai_run_events` (446 K rows), this design would be a poor fit; you would either sync it as a Raw Table or, better, not sync it at all.

**Source databases [V]:** **Postgres, MongoDB, MySQL, SQL Server, Convex** (Postgres/Mongo GA; SQL Server/MySQL **Beta**; Convex Experimental). **Note a discrepancy I found:** the FAQ says "Currently, PowerSync can only read from **Postgres** databases directly", while the pub.dev README for `powersync` says "Syncs between SQLite on the client-side and **Postgres, MongoDB or MySQL** on the server-side" and the architecture page lists all five. **[I]** The FAQ sentence appears to be a stale answer to a question about replicating *via an API*; the architecture/Feature-Status pages are the better guide. **But MySQL support is Beta and the docs contradict themselves, so verify before betting on it.**

**Server RAM [V]** (from the deployment-architecture page — the only engine here that publishes numbers):

| Profile | Memory |
| --- | --- |
| Minimal/dev | service **512 MB**, 1 vCPU + **MongoDB 2 GB**, 1 vCPU (replica-set mode) + LB |
| Production baseline | replication container **1 GB** + **2× API containers 1 GB each** + **3-node MongoDB 2+ GB each** + LB + daily compact job (up to 1 GB) |
| Heavier | raise both container types to **2 GB / 2 vCPU** |

It is a **Node.js** service (`NODE_OPTIONS=--max-old-space-size-percentage=80`). Scaling: +1 API container per 100 connections, hard cap 200 connections/container. The API container must be on a **dedicated subdomain**. **The Dashboard is unavailable when self-hosted.**

**Licensing nuance [V]:** client SDKs are Apache-2.0, but the **service is FSL-1.1-ALv2** — source-available, *not* OSI open source. Its "Permitted Purpose" excludes a "Competing Use" (making it available to others in a commercial product that substitutes for it), but **explicitly includes internal use and access**, and there is an **"Grant of Future License": an irrevocable Apache-2.0 license effective on the second anniversary of each version's release.**

**[I] For this app:** personal/internal single-user use of Open Edition is squarely permitted and free. The licensing is not the blocker. **The blocker is RAM:** ~2.5–3.5 GB of extra processes to sync a few megabytes between two devices, on a box that is already hosting ComfyUI.

**[?]** Whether the Dart client accepts a plaintext `http://<LAN-IP>` endpoint is **unverified** (docs only ever show HTTPS behind a load balancer; the SDK source fetch failed). This matters for a LAN-only setup and is the first thing to test if PowerSync is ever reconsidered.

### 4.3 ElectricSQL — detail

**The pivot, in the vendor's own words [V]:** the 2024 "Electric Next" post says the rebuild "has **superseded the previous Electric**", the old code is preserved as `electric-old`/`legacy.electric-sql.com`, and "**However caveat emptor — we are not supporting the old system.**" The new system was deliberately sequenced: "1. start with **read-path only**, 2. then add support for optimistic write patterns with tentativity, 3. then add support for through-the-DB writes."

**Then in 2026 [V]:** "**Electric Cloud is winding down.** Cloud users will need to self-host or move to another provider… Moving forward, Electric will be building with Neon inside Databricks" (2026-08-11). Corroborated by PowerSync's blog noting Electric Cloud "has shut down".

**Net position [I]:** the sync service is alive and Apache-2.0, so self-hosting is viable *if* it fits. It does not fit here, for three independent reasons: (a) **read-path only**, so it does none of the "edit on the phone" work; (b) **no Dart client**, and the community one is discontinued; (c) it **requires Postgres ≥14 with logical replication** — i.e. it does not let you drop MySQL in favour of SQLite; it makes you run a *different*, heavier database.

**[V]** Also worth knowing: the deployment guide warns that an inactive logical-replication slot grows WAL until the disk fills (recommends `max_slot_wal_keep_size`), and that "Electric exposes public access to the contents of your database" by default. The shape-metadata SQLite **corrupts on NFS/EFS** unless `ELECTRIC_SHAPE_DB_EXCLUSIVE_MODE=true`.

### 4.4 Turso / libSQL — detail, and be careful which one you mean

Three products share the name [V]:

1. **libSQL** — a production-ready **fork of SQLite** in C (17,227 ★, MIT, `pushed_at` 2026-09-16). Same file format and API; adds vector search.
2. **Turso Database** (formerly *limbo*) — a **full rewrite of SQLite in Rust** (24,355 ★, MIT, `pushed_at` 2026-09-23, **983 open issues**); adds concurrent writes, async I/O, CDC, FTS, WASM. Docs now list maturity as "Production-ready".
3. **Turso Cloud** — the hosted service; free tier 100 databases / 5 GB / 500 M rows read / 10 M rows written / 3 GB syncs per month.

**The vendor's own pivot advice [V]:** "regardless of the situation and use case, **you should be using Turso, not libSQL, if you are using sync**." Reasons given for Embedded Replicas' shortcomings: physical-page replication is wasteful; "no visibility on what changed" meant "Embedded Replicas by default **still sent writes to the remote database**, keeping just the reads local"; keeping pages ordered "was challenging… the local database often diverged, and had to be **re-bootstrapped from the cloud**."

**Embedded Replicas [V]:** reads always local ("microsecond-level"); **writes go to the remote primary and are NOT written to the local file first** unless `offline: true`. Status: "fully supported in production. For new projects that need sync, we recommend Turso Sync." Gotchas: **"Do not open the local database while the embedded replica is syncing. This can lead to data corruption."** One replication frame = **4 kB**.

**Turso Sync [V]:** explicit `push()`/`pull()`; **local changes pushed as logical mutations (CDC)**, remote changes **pulled as physical pages**; "the remote database acts as the **source of truth**… **fully distributed, peer-to-peer synchronization between devices is not supported**." Default conflict resolution is **Last-Push-Wins at row level**, with a `transform` hook for things like counters (`value = value + ?`). Official SDKs: TypeScript, Python, Go, Rust. Backed by real benchmark numbers vs Embedded Replicas: 3,000 sequential inserts **152 s → 17 s** and **34.6 MB → 2.1 MB**; 200 read-your-writes cycles **48.6 s → 156 ms**.

**Dart reality [V]:** there is **no official Dart SDK** — Flutter sits under "Community SDKs", defined as *"community-driven and don't come with official Turso support."* The community bindings are:
- `libsql_dart` **0.9.0** — ~5 months old, **20 likes / 275 downloads**, MIT, built on `flutter_rust_bridge` + native toolchain hooks. Covers Embedded Replicas (the legacy mechanism).
- `turso_dart` **0.1.0** — ~3 months old, **2 likes / 238 downloads**, MIT. Covers **Turso Sync** (`db.pull()` / `db.push()`).

**[I]** One hobby-scale publisher maintaining both, ~250 downloads/month each. That is the weakest link in the whole evaluation, and it is a *dependency* risk, not just a maturity one.

**Self-hosting [V]:** the CLI ships a local sync server (`tursodb ./server.db --sync-server 0.0.0.0:8080`) that "implements the same sync protocol as Turso Cloud", requires **no auth token**, and is pointed at by clients via plain `http://host:port`. **But the docs scope it to "development and testing"**, with no production self-hosting story, no auth, no HA or backup guidance. BYOC ("the entire Turso Cloud, running inside your own cloud account") is described as something being **built**, not shipped.

**[I] Verdict:** conceptually the closest fit to this app's shape (local-first writes, LWW, a file, no external DB), but not adoptable today on the strength of a 0.1.0 Dart package and an explicitly dev-only self-hosted server. **Worth revisiting in 6–12 months.**

### 4.5 CR-SQLite and CRDT approaches

**[V]** `vlcn-io/cr-sqlite`: "Convergent, Replicated SQLite. Multi-writer and CRDT support for SQLite." **3,800 ★**, 125 forks, 56 open issues, **MIT**, **not archived**, language Rust, last commit **2026-08-10**. So it is *not* abandoned — a common claim I was able to falsify.

**But "not archived" is doing a lot of work there. The sharper picture [V]:**

- **Last tagged release: v0.16.3, 2024-01-17** — i.e. **~2.6 years** before the last commit. The recent commits (2026-08-02 → 08-10) are *"build all android abis"*, *"align android loadable to 16 kb page size"*, *"build windows arm64 loadable"*, *"fix ios simulator build"*, *"pad macho headers…"* — **ABI/packaging hygiene, not CRDT features.** **[I] Verdict: alive but feature-frozen.**
- **Android is by far its least-exercised target.** v0.16.3 release-asset download counts: `linux-x86_64` **20,228**; `linux-aarch64` **16,897**; `darwin-aarch64` 2,342; `win-x86_64` 1,546; **`aarch64-linux-android` only 167**; `ios-dylib.xcframework` 366. **[V]** That is a **~2-orders-of-magnitude** gap — you would be an early adopter on the platform you care most about.
- **No Dart/Flutter support.** vlcn.io's Languages appendix lists JS/C/C++/Python/Rust/Go/Java/.NET/Swift — **Dart is absent**; the only *complete* networking implementations are **JavaScript**, and the docs say *"If you'd like to get started rolling your own, see the example implementation of whole CRR sync."* **[V]** So on Kotlin + Dart you write the entire transport yourself on top of the `crsql_changes` virtual table.
- **Schema restrictions you must design around.** CRR tables **may not have**: checked FK constraints; UNIQUE constraints other than the PK; CHECK constraints depending on other columns. **[V]** For a schema with 16 tables and existing constraints, that is a real redesign.
- **Cost:** the README states **inserts into CRRs are ~2.5× slower** than plain SQLite tables (reads are unaffected); counter and rich-text CRDTs are **still unimplemented** (issue #65). You must call `SELECT crsql_finalize();` before closing each connection. **[V]**
- **Extension loading is required**, which means shipping your own SQLite. SQLite: *"For security reasons, extension loading is turned off by default"*, and a build can omit it via `SQLITE_OMIT_LOAD_EXTENSION` **[V]** (<https://www.sqlite.org/loadext.html>).

**[I] Why it is nonetheless a poor fit here:**
1. **It solves concurrent multi-writer editing**, which a single user on two devices mostly does not have. CRDT machinery buys merge semantics you will rarely exercise, at the cost of per-column metadata, larger rows, a 2.5× insert penalty, and a restricted schema.
2. **You still have to build and own the transport** — in two languages — because there is no Dart story.
3. **[I] The Dart/Flutter path is workable but non-trivial:** `package:sqlite3` v3 bundles SQLite for Android `armv7a/aarch64/x86/x64` and lets you supply a custom amalgamation, so loading an extension is *possible* — but it means owning a C build for Android and Windows, plus a bleeding-edge target.

#### Other CRDT / local-first options (GitHub API, 2026-09-23) [V]

| Project | Stars | License | Archived | Last push | Dart/Flutter | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| **sqlsync** (`orbitinghail/sqlsync`) | 2,915 | Apache-2.0 | no | **2025-11-19** | none | Rust CRDT-over-SQLite. Site says *"currently usable for prototypes"*; ~10 months since last push. **[I] Not production-ready.** |
| **Automerge** | 6,615 | MIT | no | **2026-09-21** | **no usable binding** | Excellent, healthy JS/Rust CRDT. Dart package is a **v0.0.0 "WIP" stub whose declared repo 404s**. |
| **Loro** | 6,161 | MIT | no | **2026-09-21** | **no usable binding** | Healthy Rust CRDT with real Swift/Python/C-FFI bindings — **but no Dart**. `flutter_loro_ffi` is **discontinued** (29 downloads). |
| **Ditto** | closed source | **proprietary** | n/a | active | **Yes — official Flutter SDK** | **[I] The only option here with a real, official Flutter SDK — and it is a commercial product.** The main build-vs-buy datapoint. |
| **ObjectBox** | `objectbox-dart` 1,235 | Apache-2.0 (core) | no | 2026-09-22 | **Yes (official Flutter)** | Sync is a **separate commercial add-on** (`sync.objectbox.io`). **[?]** pricing page 404'd, so "commercial licence required" is likely but unconfirmed. |
| **Realm / MongoDB Atlas Device Sync** | `realm-kotlin` 1,103 | Apache-2.0 | no | **2025-10-31** | Android via Kotlin MP | **Device Sync is deprecated/EOL.** All `mongodb.com/docs/atlas/device-sync/*` URLs now **404**; Ditto ships a dedicated *"Migration Guide for Atlas Device Sync"*. **[?] Exact EOL date not verified** — the commonly cited "announced 2024-09-30, EOL 2025-09-30" is not confirmed by a source I fetched. |
| **PouchDB / CouchDB** | 17,612 | Apache-2.0 | no | 2026-09-13 | none (JS) | HTTP-replication document DB, not SQLite; no Dart. |
| **RxDB** | 23,391 | Apache-2.0 | no | 2026-09-21 | none (JS/TS) | JS-runtime; not applicable to Kotlin/Rust/Dart. |
| **WatermelonDB** | 11,784 | MIT | no | 2026-09-16 | none (React Native) | RN/web only; no Dart. |

**[V] Both CRDT leaders' Dart packages are stubs, and this was checked carefully:**
- pub.dev `automerge` = **v0.0.0**, published 2026-08-18, **unverified uploader**, README says **"WIP"**, 47 downloads, declared repo `github.com/graddotdev/automerge` → **404**.
- pub.dev `loro` = **v0.0.0**, published 2026-08-18, declared repo `github.com/graddotdev/loro` → **404**. `flutter_loro_ffi` (`alexcdever/loro-dart`) is **`discontinued`**, ~9 months old, 0 likes, **29 downloads**.
- Loro's org does publish real bindings (`loro-swift`, `loro-py`, `loro-ffi`) — **no Dart** among them.

**[I] Recommendation: skip CRDTs at this stage.** No CRDT-on-SQLite option gives you both Kotlin/Rust *and* Dart today, and the problem (one user, two devices, rarely simultaneous) does not call for CRDT merge semantics. Revisit only if genuinely concurrent editing of the same record becomes a requirement.

**[I] The one genuine build-vs-buy finding in this whole report:** **Ditto is the only off-the-shelf product with an official Flutter SDK** (`docs.ditto.live` lists Flutter alongside Kotlin/Swift/React Native/JS/Java/.NET/C++/Rust/Python/Go), with mesh/P2P networking and a DQL query language — and it is **proprietary and commercially licensed** (the old `getditto/ditto` GitHub repo 404s; there is a "Start Free" portal and pricing page). For a single-user self-hosted app, adopting a closed-source commercial sync platform to move ~2.6 MB between two devices is a poor trade — but it is worth knowing that this is the actual alternative to hand-rolling.

### 4.6 LiteFS, rqlite, dqlite

**LiteFS** — `superfly/litefs`, "a **FUSE-based file system** for replicating SQLite databases across a cluster of machines." **4,879 ★**, 128 forks, 62 open issues, Apache-2.0, Go, created 2022-04-21, last push **2026-05-11**. **[V]**

**Important correction to a common belief: LiteFS is NOT archived** — `archived: false`, `disabled: false` **[V]**. But "not archived" is misleading here, and the maintainer's own words settle it. Asked directly in issue #436 *"Is LiteFS abandoned?"*, Ben Johnson replied on **2024-09-09** **[V]**:

> *"LiteFS is currently in a **stable, working state** but we've **moved development resources elsewhere at the moment**. The project is **not abandoned** but we **don't have a timeline for when development may pick up again** in the future."*

Corroborating details **[V]**: the last *meaningful* release was **v0.5.11 (2024-01-24)**; v0.5.14 (2025-04-22) was CI-only ("Add GitHub token permissions for releases"). Release artifacts are **Linux only** — `amd64`, `arm6`, `arm7`, `arm64`; **no Android, Windows, macOS, or iOS artifacts.**

**[I] Why it is unsuitable for a phone client:**
1. It is a **FUSE filesystem** — a Linux-kernel facility. There is no FUSE for unprivileged Android apps, no iOS equivalent, no Windows implementation.
2. Releases are Linux-only and it is designed to run inside a Fly/Docker VM.
3. It is **primary + read-replica coordinated by leases**, not multi-master. It has no concept of an intermittently-offline mobile client that produces its own writes.
4. It replicates at the **file/page level via FUSE interception** — the exact mechanism SQLite warns against across machine boundaries (§4.7). It works only because LiteFS controls *both* ends on Linux.

**[I] Verdict: dormant, Linux-only, wrong layer.** Quote the maintainer rather than saying "deprecated".

**rqlite** — "The lightweight, fault-tolerant database built on SQLite." **17,756 ★**, 809 forks, 74 open issues, **MIT**, created 2014-08-23, last push **2026-09-22** → **very actively maintained.** **[V]**

**[I] What it is, and why it is the wrong shape:** a *cluster of server processes* that agree on a **Raft log** and apply it to SQLite. One elected leader takes writes; followers provide HA and read scaling. Clients speak HTTP(S)+JSON. That gives you **strong consistency across always-on servers** — not **offline-first multi-master between a hub and a phone**. At best the phone becomes an HTTP client of the hub, which is the design already recommended here *minus* the offline story. You would not run a Raft node on a phone.

**[?]** I did not verify rqlite's measured memory footprint, and did not check pub.dev for a Dart rqlite client. **[I]** A Dart client would be easy to write (plain HTTP+JSON), but the daemon's resource cost is strictly above an embedded SQLite.

**dqlite** (Canonical) — "Embeddable, replicated and fault-tolerant SQL engine"; C; **4,386 ★**, 255 forks, 48 open issues, last push **2026-09-18** → active. **[V]**
- **[?]** The GitHub API reports its license as **`Other` / `NOASSERTION`**. dqlite is generally LGPL-3.0 by project convention, but **I did not verify the LICENSE file**, so treat the licence as *"LGPL-family, unverified here"* — and note that an LGPL C library is a real consideration for a closed-source desktop app.
- **[I]** It is an embedded Raft-SQLite used by LXD/Incus: a **C library you link into a server**, not a mobile sync layer. Same "cluster of peers, not offline-first clients" objection as rqlite.

**Bedrock** (Expensify) — **[?] NOT VERIFIED in this session.** It is Expensify's SQLite-based distributed/queued database used to run their own backend. **I make no claims about its status or licence** — do not rely on anything about Bedrock from this report. (It also would not answer the phone-client question: it is a server-side cluster database.)

**[I] Summary: none of these gives a phone a local writable replica with sane conflict handling. They are server-HA tools. Skip this whole category.**

### 4.6a Litestream — a backup tool, and why it is not a sync engine

The brief asked about Litestream alongside the sync engines, so it is worth being precise: **Litestream is excellent at what it does, and what it does is not sync.** [V] from <https://litestream.io/how-it-works/>:

- "Litestream is a **streaming replication tool** for SQLite databases. It runs as a separate background process and continuously copies write-ahead log pages from disk to a replica. This **asynchronous replication provides disaster recovery** similar to what is available with database servers like Postgres or MySQL."
- "**Each database replicates to a single replica destination.**"
- Mechanism: "Litestream works by **effectively taking over the checkpointing process.** It starts a long-running read transaction to prevent any other process from checkpointing and restarting the WAL file. Instead, it continually reads new WAL pages and manually calls out to SQLite to perform checkpoints as necessary."
- WAL pages are packaged into **LTX files** with monotonically incrementing **TXIDs** and checksums; a background compactor tiers them **L0** (per-sync) → **L1/L2/L3** (merged every 30 s / 5 min / 1 h) plus a **snapshot every 24 h**.
- Restore "fetches the most recent snapshot that does not overshoot the requested restore point and then applies each subsequent LTX file in TXID order"; "any gap in the sequence would otherwise result in a **corrupted database file**". Default retention is **24 hours**.
- There is an optional `litestream-vfs` extension for **read-only** queries served from replica storage.
- Version note on the site: "**v0.5.x** — Latest — **Actively maintained** with new features and bug fixes", with v0.3.14 as the previous line. **[V]**
- **Repo status [V]:** `benbjohnson/litestream` — **14,389 ★**, Apache-2.0, **not archived**, last push **2026-09-21**, 153 open issues, created 2020-10-06. The homepage frames it as "**No-worry backups** … Continuously stream SQLite changes to your preferred cloud storage or local files. Quickly **recover** to your most recent replicated transaction if your server goes down." **Litestream is in good shape — notably healthier than LiteFS, despite sharing an author.**
- **[?] Two quality signals I could not open** (search-result titles only, from the agent's research — **verify before citing**): issue `#754` "CRITICAL: ltx v0.5.0 breaks replication with 'no flags allowed' error" and issue `#1083` "**Silent** Replication Failure in v0.5.6+ When SQLite Reuses WAL Space". If the second is accurate it is a serious silent-failure class bug *for a backup tool* — worth reading before adopting.

**[I] Why it does not solve this problem:**
1. **One-way, one destination.** It copies *from* a live database *to* a replica. There is no merge step and therefore **no conflict resolution at all** — the concepts do not exist in its model.
2. **No Android/Flutter client.** It is a Go binary that runs next to the database. A phone cannot run it against the hub's file, and should not (§4.7).
3. **[I] It competes with your process for the checkpointing role.** "Taking over the checkpointing process" via a long-running read transaction is exactly the behaviour SQLite's WAL doc warns can cause **checkpoint starvation** and unbounded WAL growth ([wal.html §6](https://sqlite.org/wal.html) [V]). That is presumably handled internally, but it means Litestream wants to *own* the database's lifecycle — which is in tension with a server process that is itself opening write connections.
4. **[I] It adds a process to a RAM-constrained box**, for a job that a single `VACUUM INTO` on a timer can do for zero extra processes at this scale (§2.4).

**[I] Verdict: relevant as a possible replacement for the ad-hoc backup story if you outgrow `VACUUM INTO` (e.g. you want point-in-time recovery), but it is not an answer to device sync.** It belongs in §2.4's category, not §4's.

### 4.7 Syncthing / rsync / Dropbox on the SQLite file — and why it is dangerous

**This is the option that looks easiest and is the most likely to destroy data.** SQLite's own documentation gives the mechanisms; these are not folklore. **[V]**

From <https://sqlite.org/useovernet.html> (a dedicated page on exactly this):
- "Because of the disordering and outright data loss which occur in network filesystem writes… **it is critical that sets of file writes can be accurately known to be done before a subsequent set of file writes begins.**"
- "**A similar hazard arises with file locking in network filesystems. SQLite relies on exclusive locks for write operations, and those have been known to operate incorrectly for some network filesystems. This has led to database corruption.**"
- "The bottom line is that network filesystem sync and locking reliability vary among implementations and installations… **Rely upon it at your (and your customers') peril.**"
- "if the network link is inserted into the **File I/O** channel, transactions may fail (as for the API Call insertion) but with the additional effect that **the remote database is corrupted**."
- The documented safe alternatives are: use a client/server DB; or "**Host an SQLite database in WAL mode, but do all reads and writes from processes on the same machine that stores the database file. Implement a proxy that runs on the database machine that relays read/write requests from remote machines.**" ← **This is precisely the recommended architecture for this app.**

From <https://sqlite.org/howtocorrupt.html>:
- §1.2 "**Backup or restore while a transaction is active.** Systems that run automatic backups in the background might try to make a backup copy of an SQLite database file while it is in the middle of a transaction. **The backup copy then might contain some old and some new content, and thus be corrupt.**"
- §1.3 "**Deleting a hot journal.** … If the hot journal files are moved, deleted, or renamed after a crash or power failure, then automatic recovery will not work and **the database may go corrupt**."
- §1.4 "**Mispairing database files and hot journals**" — the explicitly corruption-causing list:
  - "Swapping journal files between two different databases."
  - "Overwriting a journal file with a different journal file."
  - "Moving a journal file from one database to another."
  - "**Copying a database file without also copying its journal.**"
  - "**Overwriting a database file with another without also deleting any hot journal associated with the original database.**"
- §2.1 "some filesystems contain bugs in their locking logic such that the locks do not always behave as advertised. This is especially true of **network filesystems and NFS in particular**. If SQLite is used on a filesystem where the locking primitives contain bugs, and if two or more threads or processes try to access the same database at the same time, then **database corruption might result**."

**Two more places SQLite says it outright [V]:**
- <https://www.sqlite.org/lockingv3.html> §6 ("How To Corrupt Your Database Files"): **"Your best defense is to not use SQLite for files on a network filesystem."**
- <https://www.sqlite.org/atomiccommit.html> §9.1 ("Broken Locking Implementations"): **"You are advised to avoid using SQLite on a network filesystem in the first place, since performance will be slow. But if you must use a network filesystem to store SQLite database files, consider using a secondary locking mechanism to prevent simultaneous writes to the same database even if the native filesystem locking mechanism malfunctions."** The same page: *"We have received reports of implementations of both Windows network filesystems and NFS in which locking was subtly broken."*
- <https://www.sqlite.org/faq.html> Q5: *"this locking mechanism might not work correctly if the database file is kept on an NFS filesystem. This is because `fcntl()` file locking is broken on many NFS implementations… People who have a lot of experience with Windows tell me that **file locking of network files is very buggy and is not dependable**. If what they say is true, sharing an SQLite database between two or more Windows machines might cause unexpected problems."*

And the WAL-specific twist [V, wal.html §4]: "The WAL file is **part of the persistent state** of the database and **should be kept with the database if the database is copied or moved**. If a database file is separated from its WAL file, then transactions that were previously committed to the database **might be lost, or the database file might become corrupted**."

**Why the `-shm` file makes it worse [V, wal.html §7]:** the wal-index is *"implemented using an ordinary file that is **mmapped** for robustness"*, because *"the only way we have found to guarantee that all processes accessing the same database file use the same shared memory is to create the shared memory by mmapping a file in the same directory as the database itself."* **[I]** A sync tool that copies or rewrites an mmapped coordination file underneath a live process is asking for trouble.

**[I] The failure mode in one sentence:** a file-sync tool copies whole files on its own schedule with no knowledge of SQLite's locking, ordering, or sidecars. It can therefore (a) copy `db` without the `-wal` that holds the newest committed transaction, (b) copy `db` mid-transaction so it contains a torn mix of old and new pages, (c) restore one device's `db` over another's while a hot journal exists, or (d) do all of this and report success. Syncthing, rsync, Dropbox and iCloud all fall in this class. **WAL mode makes it worse than journal mode, not better**, because more committed state lives outside the main file.

**On Syncthing specifically [V, <https://docs.syncthing.net/users/faq.html>]:** its own FAQ states what it syncs is **"File contents; File modification times"** (plus optional permissions/symlinks/xattrs) — no locking, no transaction awareness, no application awareness. It resolves conflicts by keeping **conflict copies**, not by merging. And it says of itself: *"**Is Syncthing my ideal backup application? No.** Syncthing is not a great backup application because all changes to your files (modifications, deletions, etc.) will be propagated to all your devices."* **[I]** A tool that propagates every change to every device, with no application awareness, is the worst possible partner for a live SQLite file.

**[?] On Dropbox/iCloud/Drive corruption *anecdotes*:** I could **not** verify any specific user report — search was unavailable and community pages were unreachable. **[I] I deliberately do not lean on forum anecdotes here, because the primary sources above are strictly stronger**: they describe the *mechanism* by which any file-level copier corrupts a live database, which covers Dropbox, iCloud, Drive, and Syncthing without needing a bug report.

**[I] Also note the inverse trap:** even *read-only* access from a file-sync daemon can matter — howtocorrupt §2.2 describes a thread doing a plain `open()`/`read()`/`close()` on a database file dropping POSIX advisory locks held by other threads in the same process, leading to two writers and corruption. SQLite added defenses in 3.51.0 (2025-11-04), but the doc concludes "they are not a cure-all."

#### SQLite answers this question directly

SQLite's `sqlite3_rsync` documentation contains a section literally titled **"Why Can't I Just Use Ordinary rsync?"** — which is the exact question the brief asked. Quoting it in full because it is the best primary source in this report **[V, <https://sqlite.org/rsync.html> §6]**:

> "**Ordinary rsync does not understand SQLite transactions.** Rsync will make a copy of ORIGIN into REPLICA, however **the copy might not be consistent**. Parts of the copy might be from one transaction, while other parts might be from a different transaction. **The database copy might be corrupt.**
>
> If no other processes are connected to the database for the entire time that rsync is running, and if the database does not have a hot journal, then rsync will make a consistent copy of the database. But **if you cannot guarantee that both of those conditions are met, then rsync might generate a corrupt copy.** The sqlite3_rsync utility, on the other hand, always generates a consistent copy."

**[I] Read the escape clause carefully, because it is the trap:** rsync is safe *only* if (a) nothing is connected for the entire run **and** (b) there is no hot journal. On a hub where a server process is always connected, condition (a) is never true. On a phone where the app may be killed mid-write, (b) is exactly the scenario you cannot rule out. **So the documented safe case does not describe this app.**

#### `sqlite3_rsync` — the supported way to move a whole database

If you ever *do* need to copy the database file over a network, SQLite ships a tool that understands transactions **[V, same page]**:

- `sqlite3_rsync ORIGIN REPLICA` makes REPLICA a copy of ORIGIN, over SSH. One side may be `USER@HOST:PATH`; **at least one side must be local** ("They cannot both be databases on other machines").
- "**Both databases may be 'live' while this utility is running.** Other programs can have active connections… REPLICA becomes a copy of a snapshot of ORIGIN as it existed when the sqlite3_rsync command started." REPLICA is read-only while it runs.
- **Bandwidth is the selling point:** the protocol exchanges page hashes. "if the origin and replica start out being very similar (the usual case) then the total bandwidth is often **less than 0.01% of the database size**. In tests, a **500 MB database will typically synchronize with about 20 KB** of network traffic." (Before 3.50.0 the floor was ~0.5 %.)
- Available since SQLite 3.47.0 (2024-10-21); the "both must be in WAL mode and same page size" limitation was **removed in 3.50.0 (2025-05-29)**.
- **Windows caveats [V]:** "a single-letter HOST without a USER@ prefix will be interpreted as a **Windows drive-letter**, not as a hostname" (so `C:...` is ambiguous), and the doc's author notes he "has never had any success in getting **SSHD to run on Windows**".
- A known protocol-negotiation bug (missing `fflush()`) can **hang** when 3.50.0+ talks to 3.49.1 or earlier; workaround `--protocol 1`.

**[I] Still one-way.** It makes REPLICA a copy of ORIGIN. It is a replication/backup primitive, not multi-master sync, and it needs an SSH server — awkward on Windows, impractical on Android. **Useful for hub→hub or hub→NAS backup; not for the phone.**

**Verdict: do not do this.** Not with the file, and not with the file plus sidecars.

### 4.8 CloudKit and Google Drive `appDataFolder` as transport

#### CloudKit — verified, and disqualified on platform grounds

**The object model [V]** (<https://developer.apple.com/documentation/cloudkit/ckdatabase>): each user has **three** databases — a **public** database for all users of the app; a **private** database *"accessible only to the user of the current device"*; and a **shared** database for records other iCloud users share with them. *"All access to the private and shared databases requires an iCloud account."* Also worth noting: *"Because it provides minimal offline caching support, CloudKit relies on the presence of the network and, optionally, a valid iCloud account."*

**Change notification is first-class [V]:** `CKDatabase` has full subscription CRUD (`subscriptions(for:)`, `modifySubscriptions(saving:deleting:)`, `deleteSubscription(withID:)`), plus `subscriptions/modify`, `tokens/create` and `tokens/register` for APNs pushes.

**Conflict handling is genuinely mature — and is the one thing here that would be useful [V]:**
- `CKModifyRecordsOperation.RecordSavePolicy` has exactly **three** cases: **`ifServerRecordUnchanged`** — *"only proceed if the record's change tag matches that of the server's copy"*; **`changedKeys`** — *"save only the fields of a record that contain changes"*; **`allKeys`**.
- `CKError.Code.serverRecordChanged`: *"the server's version of the record is newer than the local version the client's trying to save. **Your app needs to handle this error, resolve any conflicts in the record, and attempt another save.**"* The error's `userInfo` carries **three** records — client, server, and **ancestor** (the last-synced original). *"When a conflict occurs, your app needs to **merge all changes into the record for the [server record] key**… **Merging into either of the other two copies of the record results in another conflict error** because those records have the old record change tag."*
- **[I] So CloudKit gives optimistic concurrency (change tags) plus a true 3-way-merge *primitive* (client/server/ancestor) — but it does not auto-merge; you write the merge.** And for a whole SQLite file stored as a blob there is no meaningful field-level merge: it degenerates to LWW on the entire blob unless you diff at the application level.

**Size limits [V]** (Apple's "Data Size Limits", doc updated 2016-06-13 — an **archived** page, so treat as historical-but-best-available):

| Property | Value |
| --- | --- |
| Max operations in a request | **200** |
| Max records in a response | **200** |
| Max tokens in a request | **200** |
| **Max record size (excluding Asset fields)** | **1 MB** |
| **Max file size of an Asset field** | **50 MB** |
| Max delete-self source references to one target | 750 |

**[I] Practical consequence:** a SQLite file must go in a `CKAsset` (≤ 50 MB), not in record fields (≤ 1 MB) — and assets upload **separately** from the record, so "record + asset" is **not atomic**.

**Platform availability — this alone disqualifies it [V]:** the CloudKit framework's declared platforms are exactly **iOS 8.0, iPadOS 8.0, Mac Catalyst 13.0, macOS 10.10, tvOS 9.0, visionOS 1.0, watchOS 3.0**. **There is no Android and no Windows.** (From the CloudKit framework declaration, © 2026 Apple.)
- There *is* a server-side escape hatch: **CloudKit Web Services** REST API at `api.apple-cloudkit.com/database/1/[container]/[environment]/…`, authenticated by **API token** or **server-to-server key** (ECDSA P-256; signed requests valid 10 minutes; web auth tokens expire in 30 minutes, or 2 weeks with "Keep me signed in"). **[V]** But it still terminates in Apple's ecosystem, and private-database access still requires an **Apple ID sign-in**. **[V]**
- **[I] Verdict: not a viable Android + Windows transport.** For a Flutter app whose two targets are Android and Windows, CloudKit cannot be the transport at all — independent of any other consideration. This is now verified rather than assumed.

#### Google Drive `appDataFolder` / `drive.appdata` — partially verified only

**What I verified [V]:** the page **exists and is titled "Store application-specific data"**, and sits under Drive API → **"Work with app and file data"**, alongside "Manage file metadata" and "Add custom file properties", next to the sibling guide **"Choose scopes"** (canonical URL <https://developers.google.com/drive/api/guides/appdata>, read via the `developers.google.cn` mirror with `?hl=en`). So `appDataFolder` is a **documented, supported Drive concept for app-private data**, scope-gated via the Drive scopes guide (`drive.appdata`).

**What I could NOT verify [?]** — flagged because it would be easy to fill in from memory and I am not doing that: the article body never came through (the `.cn` mirror emits ~30 KB of navigation before the article and truncation hit first; `r.jina.ai`, `googleapis.com/discovery`, and `archive.org/wayback/available` were all unreachable). **These specifics need re-verification before anyone relies on them:**
- the hidden-app-data-folder semantics (invisible in the user's Drive UI and to `drive.file`),
- the `drive.appdata` scope's stated limits,
- **whether appdata counts against the user's Drive storage quota**,
- current documented limits on file count and size.

**[I] Verdict as a transport, given the above:** it is a **single-blob transport** — you `files.create`/`files.update` one opaque SQLite file (or a small set of blobs) into `appDataFolder`. What it does **not** give you: conflict resolution beyond last-write-wins **on the whole file**; transactional atomicity between the `db` file and its `-wal`; any consistent-snapshot guarantee; any change-feed you can merge; or any offline story beyond your own queue. It requires OAuth (straightforward on Android; on **Windows desktop** you need a loopback/device-code flow or a service account), and you would re-derive revision handling from `files.get` metadata (`headRevisionId`/`version`) plus `changes.list`.

**[I] That is the §4.7 hazard with an HTTP API in the middle** — *unless* you always quiesce/checkpoint and upload a `VACUUM INTO` / backup-API snapshot rather than the live file. Given the size constraints and latency, it is workable for **occasional full-database pushes** and poor for **frequent small deltas**. Since you need an OAuth-protected HTTP endpoint on the hub anyway, and the hub is on the same LAN, there is no reason to route through Drive.

**Verdict: not recommended.** Use it only as an off-site *backup* destination for a `VACUUM INTO` artifact, if at all.

---

## 5. The "hand-rolled LWW" option in detail

Given §4, this is the option most likely to be chosen, so it deserves the most care. **[I]** for the whole section except where cited.

### 5.1 Why LWW is the right *class* of solution here

Last-write-wins is widely criticised, correctly, because **it silently discards concurrent writes** — that is its defining property, not a bug. **[?]** I was unable to fetch a specific canonical citation (the Kleppmann TPDS paper on a CRDT JSON datatype and the HLC paper at `cse.buffalo.edu` both failed to fetch; search was down), so I am stating this as received wisdom rather than citing it.

**[I] But the criticism assumes concurrent writers on the same records.** In this app:
- exactly **one user**, who is physically at one device at a time;
- the hub is authoritative and always reachable on the LAN;
- the phone does "browsing and light editing".

Under those conditions true concurrency is rare, and when it happens the correct behaviour is usually "the later human intent wins" — which is what LWW implements. **The failure you must actually prevent is not "LWW lost a merge"; it is "LWW silently resurrected a deleted row" or "a row came back from the dead because a tombstone was pruned", and "clock skew made an older edit win".** Those are addressable.

### 5.2 Minimum correct implementation

**1. Stable, globally-unique IDs — never autoincrement.**

- Every syncable row gets a **client-generated UUIDv4 (or v7)** primary key. **[I]**
- Rationale: with two devices, `AUTOINCREMENT` guarantees collisions, and reconciling "my row 7" vs "your row 7" after the fact is not recoverable. The existing schema uses `PRIMARY KEY (id)` with server-assigned integers, so **this is a schema change, not a detail.** PowerSync's FAQ independently says auto-incrementing IDs are "generally not recommended" for sync **[V]**.
- Keep a **`device_id`** (stable per install) alongside for debugging, but do not let it into the key.

**2. `updated_at` + explicit clock-skew handling.**

- Store a **monotonic-ish, globally comparable** timestamp. Two options:
  - **Hybrid Logical Clock (HLC)** — the principled answer. Cited reference: Kulkarni, Demirbas, Madappa, Avva, Leone, *Logical Physical Clocks and Consistent Snapshots in Globally Distributed Databases* (<https://cse.buffalo.edu/~demirbas/publications/hlc.pdf>) — **[?] I could not fetch the PDF to verify its contents**, so I am citing it as the standard reference rather than quoting it.
  - **Simpler for a hub-and-spoke topology: a server-assigned logical revision.** Since the hub is authoritative and always reachable on LAN, it can hand out a monotonically increasing `rev` on every accepted write. Devices do not need a comparable wall clock at all. **This is materially simpler and strictly more robust than HLC for this topology**, and it is my recommendation. **[I]**
- **Why clock skew is a real hazard, with a number:** Cockroach Labs' writeup on living without atomic clocks documents that "using **NTP** for clock synchronization is likely to give somewhere between **100 ms and 250 ms**" of offset [V, <https://www.cockroachlabs.com/blog/living-without-atomic-clocks/>]. The same article gives the canonical illustration of the resulting anomaly — a transaction on a lagging node committing "in the past" and inverting causal order, which they call "causal reverse". **A phone whose clock is 4 minutes fast will win every conflict forever** if you use raw `datetime('now')`. That is not a theoretical concern; it is the single most likely way this scheme breaks.
- **If you do use wall-clock LWW anyway:** clamp device clocks against the hub's time on each sync (reject or adjust timestamps outside a tolerance window), and use a per-device `(lamport, device_id)` tiebreaker so equal timestamps resolve deterministically. **[I]**

**3. Soft-delete tombstones — and never prune them carelessly.**

- Deleting a row sets `deleted_at` / a `deleted` flag rather than `DELETE`ing it, so the deletion can propagate. **[I]**
- Schema detail worth getting right: put tombstones **in the same table** (with a partial index excluding them from normal queries, e.g. `CREATE INDEX ... WHERE deleted_at IS NULL`) rather than in a separate table — it keeps FK relationships and the change-log trigger simple. **[I]**
- **The classic failure:** a tombstone is garbage-collected after N days, but a device that has been offline longer than N then syncs, does not see the delete, and **re-uploads the deleted row as a new insert — resurrecting it**. **[I]**
- **The fix is a horizon, not a timeout:** never prune a tombstone until **every known device has acknowledged a revision newer than the tombstone's**. Track per-device `last_seen_rev` on the hub and prune only below `min(last_seen_rev)`. If a device is retired, remove it explicitly. **[I]**
- Corollary: **the hub must know its device set.** Add a `devices` table; an unknown device should be made to do a full bootstrap rather than an incremental sync.

**4. Per-table change log / `changes` outbox.**

- Every mutation writes a row to a **`changes` outbox** in the *same transaction* as the mutation, with: `rev` (server-assigned), `table_name`, `row_id`, `op` (`insert`/`update`/`delete`), and either the full new row as JSON or a column diff. **[I]**
- **Same transaction is the whole point.** If you write the row and then the log entry separately, a crash between them silently desynchronises devices forever. This is the "dual write" problem.
- A device pulls with `WHERE rev > $last_rev ORDER BY rev LIMIT n`. Store `last_rev` per device on the hub.
- **Use `rev` (monotonic, hub-assigned) rather than a timestamp as the cursor.** Timestamps have ties and skew; a monotonic counter does not. **[I]** The vendor equivalent of this pattern exists in the tools reviewed: PowerSync's `ps_oplog` + buckets **[V]**, Turso Sync's logical CDC **[V]** — so this is the same shape, hand-built.
- **[I] SQLite-specific implementation choice:** you can populate the outbox with **triggers** (`AFTER INSERT/UPDATE/DELETE` on each synced table) so it cannot be forgotten by application code. Trade-off: triggers are easy to get subtly wrong and hard to migrate; a repository-layer helper is more explicit but can be bypassed. **Given `db/migrate.sql` already centralises DDL, triggers are defensible here.** Whichever you pick, add a test that asserts the outbox and the table agree after a mixed workload.

**5. Idempotent apply.**

- Applying the same change twice must be a no-op. With UUID keys: `INSERT ... ON CONFLICT(id) DO UPDATE SET ... WHERE excluded.updated_at > existing.updated_at` (or `excluded.rev > existing.rev`). **[I]**
- Idempotency is what makes **retry** safe, and retry is mandatory because LAN connections drop, the phone sleeps, and Android kills background work. Without idempotency, every retry risks a duplicate or a lost update.
- The hub should also be idempotent on **uploads**: the client sends `(device_id, client_seq)` or a content hash so a re-sent batch after a network failure is recognised and not applied twice.

**6. Conflict policy — write it down explicitly.**

- **Field-level LWW is usually better than row-level** for this app: if the phone edits `tags` while the desktop edits `notes`, row-level LWW discards one edit wholesale; field-level keeps both. **[I]** Field-level costs a per-field timestamp map (or a diff in the change log) and is meaningfully more code — **decide deliberately rather than stumbling into row-level.**
- **Special-case the collections.** Tags/tag-links are set-valued, and per §5.1 the natural merge is **union / add-wins**, not LWW. **[I]** PowerSync's own FAQ draws the same line: real-time collaboration works "as long as users do not edit the same data… **Concurrently working on text documents is not supported out of the box. This is solved better by CRDTs**" **[V]**. So it is legitimate to say "LWW for scalars, set-union for tags, and *no* concurrent editing of long text — last save wins and that is documented behaviour."
- **Expose conflicts rather than hiding them.** Because the user is one person, the highest-value behaviour is often "keep both and tell me", not "silently pick one". A simple version: when a field-level conflict is detected, write the losing value into a `conflicts` table surfaced in the UI.

**7. Initial bootstrap of a full dataset.**

- A device with no data needs everything, and this is where naive designs fall over. **[I]** Two viable approaches:
  - **(a) Paginated full pull** — the same `changes` cursor starting at 0. Simple, one code path, resumable, but slow for large tables and puts load on the hub.
  - **(b) Snapshot + tail** — export a snapshot at a known `rev` (for SQLite: `VACUUM INTO`, which SQLite documents as producing a consistent live snapshot and needing the target file not to pre-exist **[V]**), ship it, then pull `changes WHERE rev > snapshot_rev`. Much faster, and the compaction that `VACUUM INTO` does is a bonus. Cost: a second code path, and you must be careful that the snapshot rev is captured atomically with the snapshot.
- **[I] Recommendation: (a) first, because correctness is easier to reason about, then add (b) if bootstrap time actually hurts.** Given the real user content here is ~2.6 MB, (a) will be instantaneous — **do not build (b) speculatively.**
- **Explicitly exclude the big append-only table.** `ai_run_events` is 112.9 MB of 137 MB and 446 K of 448 K rows, and it is a *run event log* **[V, measured]**. Syncing it to a phone is almost certainly wrong. Decide per-table what syncs, and make that a first-class part of the schema/design rather than an accident.
- **Media/files are a separate problem.** `storage/` holds images and videos that are far larger than the database. **[I]** Do not put blobs in the sync path; serve them from the hub by URL (the app already has `mediaPosterUrl(id)` / `fileUrlFor(...)` helpers and the repo notes that media URLs are backend-computed) and cache on the phone.

**8. Schema version negotiation.**

- The phone must not run against a schema it does not understand. **[I]**
- Send the schema version in every request/response; on mismatch, the client either refuses and prompts to update, or performs a full re-bootstrap. **Do not attempt to apply a delta to an unknown schema.**
- Bump the version in the same transaction as the DDL (see §2.5) so the version cannot be ahead of the schema.
- Keep a **minimum supported client version** on the hub so an old phone gets a clear error instead of corrupt data.
- **[I]** Because this app already hand-rolls convergent DDL, add `user_version` alongside it rather than replacing it, and treat `user_version` as the number you negotiate.

### 5.3 Classic failure modes — the checklist

| # | Failure | How it happens | Prevention |
| --- | --- | --- | --- |
| 1 | **Resurrection** | Tombstone pruned before an offline device synced; device re-uploads the row | Prune only below `min(device.last_seen_rev)`; per-device acks |
| 2 | **Silent data loss** | Row-level LWW discards the other device's independent field edit | Field-level LWW, or accept and document it; set-union for tags |
| 3 | **Clock skew wins forever** | Phone clock fast by minutes ⇒ its writes always "newer" | Hub-assigned monotonic `rev`; or clamp + `(lamport, device_id)` tiebreak |
| 4 | **Dual-write desync** | Row written but outbox entry not (crash, error between) | Outbox written **in the same transaction**; triggers or transactional repo |
| 5 | **Non-idempotent retry** | Network failure ⇒ client resends ⇒ duplicate insert / double-apply | UUID keys + `ON CONFLICT DO UPDATE`; upload batch ids |
| 6 | **Cursor by timestamp** | Ties and skew make `> last_synced_at` skip or repeat rows | Cursor on monotonic `rev`, never a timestamp |
| 7 | **Bootstrap that cannot resume** | Phone killed mid-initial-sync (Android will kill it) | Resumable cursor; checkpoint progress; expect interruption |
| 8 | **Schema drift** | Client applies deltas built for an older schema | Version negotiation + full re-bootstrap on mismatch |
| 9 | **UUID collision by laziness** | "UUID" implemented as `timestamp+random` or a local counter | Real UUIDv4/v7 from a vetted generator |
| 10 | **Unbounded outbox growth** | `changes` never compacted; hub disk fills | Compact below `min(last_seen_rev)`; the hub already demonstrated this failure mode with 4.3 GB of binlogs |
| 11 | **Deletes invisible to FKs** | Tombstoned parent, live child, FK enforced | Decide FK policy explicitly; `PRAGMA foreign_keys` is per-connection and off by default **[V]** |
| 12 | **Two writers on one file** | Desktop app and a second process/backup tool both open the DB; or a file-sync daemon | Single coordinator process; keep AV/file-sync out of the DB directory (§2.6, §4.7) |
| 13 | **Assuming WAL means no locking errors** | `SQLITE_BUSY` still happens (wal.html §9 **[V]**) | `PRAGMA busy_timeout`; single writer |
| 14 | **Syncing more than intended** | 446 K-row event log replicates to the phone | Explicit per-table sync allowlist; measure before shipping |

**[I] The meta-point:** items 1–4 are the ones that lose data *silently*, and all four are cheap to prevent (one prune rule, one transaction boundary, one integer cursor, one clock decision). Item 12 is the one that can corrupt the file itself and is prevented by architecture, not code.

---

## 6. Rust embedded server on Android

### 6.1 Is it viable? Yes — and the numbers now exist

**[I] Technically yes, and the RAM win is real and large. But it is the wrong *first* move for this team, and it is not required to get the RAM win.**

**Measured on this machine [V, MEASURED]** — a minimal Rust HTTP server, release profile (`opt-level=3`, `lto`, `codegen-units=1`, `strip`, `panic=abort`), Windows x64:

| Probe | exe size | Working set (idle) | Private commit | Threads |
| --- | --- | --- | --- | --- |
| `std::net::TcpListener` + hand-rolled HTTP/1.1 (**zero crates**) | **0.13 MB** | **4.23–4.26 MB** | 0.71 MB | 4 |
| **`tiny_http` 0.12** hello-world | **0.31 MB** | **4.41–5.00 MB** | 1.02 MB | 9 |

Working set was **unchanged after 1,200 requests**. Artifacts: `.research\rss-probe\`.

**So a minimal Rust HTTP server is a ~4–5 MB process.** Compare the two numbers you actually care about **[V, MEASURED]**: this app's **Ktor 3.3.3 + Netty + HikariCP + MySQL-connector** service at **343.9 MB WS / 395.7 MB private commit / 77 threads**, and `mysqld` at **486–608 MB WS / ~857 MB private commit**.

**Published cross-checks [V]:**

| Server, same box | Published RSS |
| --- | --- |
| Rust `axum`, idle | **"under 15 MB"** — <https://blog.ggoggam.dev/til/jvm-memory-cost> |
| Go `net/http`, idle | 20–30 MB (same source) |
| Ktor, idle | **300–400 MB**, ~220 MB after tuning (same source) |
| Rust (Axum), peak @500 concurrent, full API | **33 MB** (idle 19 MB) — <https://dev.to/netfishx/same-api-in-java-go-rust-and-kotlin-a-4-language-benchmark-2fn6> |
| Kotlin/Ktor 3.4 + Netty, peak @500 | **650 MB** (same source) |
| Java/Spring, peak @500 | 372 MB (same source) |

**[?] Caveats I am obliged to attach to those two sources:** both are single-author benchmarks, not peer-reviewed; the dev.to run used **different databases per language** (MySQL 8.4 for Java, PostgreSQL 18 for the others), different API paths, and the author states the Java implementation "wasn't fully feature-equivalent" and Kotlin's JIT "may not have been fully warmed up". Use them for **order of magnitude**, not precision. **TechEmpower could not be verified** — its results table is JS-rendered and unreadable here, so **do not cite TechEmpower for RSS on my authority.**

**[I] The interesting question is therefore not "is Rust smaller than the JVM?"** — it plainly is, by ~2 orders of magnitude for a minimal server — **but "is the rewrite worth it?",** because the measured 344 MB includes Ktor, Netty, HikariCP, JDBC, kotlinx-serialization, logback, and the TwelveMonkeys WebP reader, and **a large fraction of that is removable without leaving Kotlin** (§6.4).

**Library facts [V]:**
- **`flutter_rust_bridge` 2.13.0** (2026-08-23; prerelease 2.14.0-beta.2), **MIT**, **5,417 ★**, last push **2026-09-13**, 31 open issues, 663 pub likes, **Flutter Favorite**, 7.35 M crates.io downloads, platforms Android/iOS/Windows/Linux/macOS/Web. Supports `async fn`, Rust→Dart callbacks, and **Rust `StreamSink<T>` → Dart `Stream<T>`** (documented as usable even after the Rust function returns). Its own framing: "Just write down *normal* Rust code… **The bridge will generate all glues in between.**"
- **`rusqlite` 0.40.2** (2026-08-08), **MIT**, 4,405 ★, last push 2026-09-21. **`bundled` compiles SQLite 3.53.2** from source via the `cc` crate and **ships pregenerated bindings, so no `bindgen`/libclang is needed** at build time. **[V]** — and note **3.53.2 is past the WAL-reset fix (3.51.3)**, which is a genuine advantage over Android's 3.50.6 ceiling (§3.1). Relevant features for a sync engine: `unlock_notify` (clean `SQLITE_BUSY` handling), `preupdate_hook` (capture changes without triggers), `backup`, `blob`, `session`, `load_extension`.
- **[?] Verified negative: rusqlite's own docs never mention Android.** The Android path (`bundled` → `cc` → `cargo-ndk` env vars) is **inferred and untested**. `cargo-ndk` **4.1.2** exists and *"derives which environment variables to read the same way as the `cc` crate"*, which is what `libsqlite3-sys`'s build script needs — so the path is plausible, not proven.
- **[V] `ndk-context` 0.1.1** (MIT OR Apache-2.0) is the standard way to get a `JavaVM` + `Context` from Rust: `ndk_context::android_context()` → `jni::JavaVM::from_raw(ctx.vm())` → `attach_current_thread()` → call back into Kotlin for `Context`/`filesDir`. **[I]** For a server-only FFI library you want `ndk-context` + `jni` only, **not** `android-activity`'s event loop.

### 6.2 The standard Flutter+Rust patterns — and a correction about `cargokit`

**Correction [V]: `cargokit` is ARCHIVED.** `irondash/cargokit` — **`"archived": true`**, banner *"This repository was archived by the owner on **Mar 26, 2026**. It is now read-only."* 97 ★, last push 2025-06-07, license reported as `NOASSERTION` **[?]**. Its README's own first line calls it **"Experimental"**.

**[V] The important structural point: `cargokit` is not an alternative to `flutter_rust_bridge` — it is frb's default build backend.** frb's docs: *"flutter_rust_bridge uses **Cargokit** for seamless integration of cargo build with flutter applications and packages."* And, on the archive: *"That repository has been archived; flutter_rust_bridge now uses **the fork maintained by the flutter_rust_bridge team**."* So the real choice is:

| Approach | What it is | Trade-off |
| --- | --- | --- |
| **frb codegen + Cargokit** (default) | Annotate normal Rust; `flutter_rust_bridge_codegen generate` emits Dart bindings + Rust glue. Handles arbitrary types, async, streams. | Highest convenience. Adds a codegen step, a codegen dependency in CI, and **you depend on frb's fork of an archived experimental repo**. |
| **Cargokit alone + hand-written `dart:ffi`** | Cargo build wired into Gradle/CMake, correct `jniLibs` layout, **no codegen**. | You write every `lookupFunction` typedef, struct layout, `Pointer` lifetime and `free`. **[I] Defensible for a small, stable API surface; painful once types multiply.** |
| **frb + `native-assets` backend** (newer) | `--integration-backend native-assets`; uses Dart build hooks + `flutter_rust_bridge_hooks`. Requires **frb ≥ 2.13.0-beta.2** and a Flutter/Dart SDK with build-hooks support. | **[I]** The frb team treating Cargokit as a legacy path is a signal about direction of travel. Still beta. |

**[V] frb gotchas worth knowing before committing** (mostly from the project's own troubleshooting page):
- **Generated code size is an acknowledged complaint** — there is a FAQ entry literally titled *"Generated code is so long"*. **No MB figure is published.** **[?]** No official frb binary-size delta or build-time measurement exists; the project's performance page explicitly declines to publish figures ("quite coarse… rough reference").
- **Android `libc++_shared` pain:** resolving `__cxa_pure_virtual` requires a `build.rs` line **and copying `libc++_shared.so` into each `jniLibs/<abi>` dir**.
- **NDK 23 breaks `cargo ndk`** (`unable to find library -lgcc`) — documented workaround is downgrading to NDK 22.
- **Cargokit is coupled to your Flutter version**: Flutter < 3.32.0 needs a pre-`9b878b73` Cargokit copy; ≥ 3.32.0 needs that commit or later. Mismatch symptom: `dlopen failed: library "librust_lib_my_app.so" not found`.
- **Version locking:** the Dart package, Rust runtime, `flutter_rust_bridge_codegen`, and `flutter_rust_bridge_macros` must all be **exactly** the same version; `Handler` classes may break on a *minor* bump.

**[V] And note the real compile-time cost** (a common hidden expense): an independent 4-language benchmark measured a clean **Rust server build at 98.3 s vs Java 5.6 s / Go 9.4 s / Kotlin 14.1 s**. That is a whole server rather than an frb library, so treat it as directional — but expect Rust to be the slowest to compile in your loop, which matters for a team *learning* Rust.

### 6.3 FFI vs localhost HTTP — now measured, and it settles the architecture

**[V, MEASURED on this machine]** — a Rust `cdylib` (101.5 KB stripped) called from Dart 3.13.3 on Windows x64, JIT and AOT; versus two Rust HTTP servers driven by a std-only Rust keep-alive client (so no PowerShell/.NET overhead), n = 20,000, `TCP_NODELAY`, 2,000-request warmup:

| Operation | Cost |
| --- | --- |
| **Dart → Rust FFI, `int -> int`** | **9.9 ns (JIT) / 10.1 ns (AOT)** per call |
| 1 KiB slice → `u64` checksum (includes the 1,024-byte Rust loop) | 635 ns |
| Rust `alloc(1 KiB)` + Dart `free` (cross-boundary allocation round trip) | 49 ns |
| **localhost HTTP round trip — `std` hand-rolled server** | **12.7 µs p50** (p90 19.1, p99 33.4) |
| **localhost HTTP round trip — `tiny_http`** | **24.1 µs p50** (p90 37.8, p99 69.0) |

**That is a ~1,300×–2,400× transport-cost difference**, before HTTP parsing, headers, JSON serialization on both sides, and a server-side thread. Artifacts: `.research\rss-ffi\`, `.research\ffi_bench.dart`.

**[I] Caveats, stated so the numbers aren't over-read:** the 10 ns figure is a **best-case leaf call** and is much faster than the widely-repeated "~100 ns" community figure — modern Dart may be inlining it, so treat it as a floor, not a general bound. The Dart SDK's own open tracking issue ([dart-lang/sdk#52692](https://github.com/dart-lang/sdk/issues/52692), `type-performance`) measured on an ARM Mac that an FFI dylib call costs **~2× a closure call and ~3× a plain function call**. **[?] No official Dart doc states an FFI latency figure** — `dart.dev/interop/c-interop` makes no overhead claim. Also note a methodological warning from my own run: the *same* localhost measurement through PowerShell/.NET `HttpClient` gave p50 ≈ **1.29–1.76 ms**, i.e. the managed client dominated by ~2 orders of magnitude — **most "localhost is slow" anecdotes are measuring the client, not the transport.**

**[I] Architecture consequences:**
- **FFI does avoid serialization — but not copying.** My 1 KiB case still cost 635 ns and an alloc/free round trip 49 ns. "No serialization" means you skip JSON/HTTP framing and can hand over a `Pointer` to a buffer; it does not mean zero cost.
- **The decisive argument for FFI over localhost HTTP is not latency, it is *lifetime*.** A localhost server must **stay alive** to serve, and on Android that is exactly what the platform refuses to guarantee (below). An FFI library has **no lifetime of its own** — it lives exactly as long as the Flutter process.
- **On the phone, this is moot anyway:** the Android client is a *separate device*. It cannot use FFI to reach the hub; it needs the network regardless. **FFI-vs-HTTP only concerns the desktop**, where the Flutter UI and the backend share a machine.
- **On the desktop it is a genuine simplification:** in-process means no loopback HTTP hop, no port, no localhost TLS question, and no "is the server running?" state machine. **[I]** The repo already carries `lib/core/backend_launcher.dart` plus `scripts/*.ps1` purely to start/stop/diagnose that backend process — which is evidence of what that state machine costs. But it is a *desktop* architecture change, and it does not help the phone.
- **frb already gives you the shape you'd want:** sync/async functions **and** Rust→Dart `Stream`s, which is what a "push updates" HTTP endpoint would otherwise provide. **[V]**

### 6.3 Running a Rust HTTP server inside an Android app — now with primary sources, and the answer is a firm no

**This section got its citations.** `developer.android.com` is reachable via `pwsh` + `Invoke-WebRequest` (it is only the `web_fetch` tool that fails), so the constraints below are **[V]** from Android's own docs rather than inferred.

**Rust cross-compilation is fine [V]:** Android is a **Tier 2** Rust target — `aarch64-linux-android`, `arm-linux-androideabi`, `armv7-linux-androideabi`, `i686-linux-android`, `thumbv7neon-linux-androideabi`, `x86_64-linux-android` (`riscv64-linux-android` is Tier 3). Rust supports "the most recent LTS Android NDK" and "by default all API levels supported by the NDK". <https://doc.rust-lang.org/rustc/platform-support/android.html>

**But you cannot spawn a Rust binary at all [V] — this is the decisive constraint:**
> Android 10 (API 29) behaviour change: *"**Removed execute permission for app home directory.** Execution of files from the writable app home directory is a W^X violation. Apps should load only the binary code that's embedded within an app's APK file. **Untrusted apps that target Android 10 cannot invoke `execve()` directly on files within the app's home directory.**"* — <https://developer.android.com/about/versions/10/behavior-changes-10>

**[I]** So an embedded server on Android can only ever be an **in-process `.so` loaded via `dlopen`/JNI** — never a separate Rust executable you unzip and spawn. **[?]** Shipping the ELF inside `jniLibs/` and exec'ing *that* path is a community-discussed workaround; it is **not documented or sanctioned by Google and I did not verify it.**

**And even as an in-process listener, Android will not keep it alive [V]:**
- **Loopback binding is not documented as restricted.** `INTERNET` is a **normal permission**, granted at install time ([Connect to the network](https://developer.android.com/training/basics/network-ops/connecting)). **[?]** But note the honest reading: **no Android doc says loopback is exempt from anything** — the correct statement is *"no restriction is documented"*, not *"a doc permits it."*
- **Background services are stopped.** Android 8.0: an app is "foreground" only if it has a visible activity, a foreground service, or another foreground app bound to it; otherwise it gets *"a window of several minutes"* and then *"the system stops the app's background services, just as if the app had called `Service.stopSelf()`"* — [Background Execution Limits](https://developer.android.com/about/versions/oreo/background).
- **A foreground service now needs a type, and the plausible types are time-capped.** Android 14+: you must declare `android:foregroundServiceType` plus the matching permission. For an arbitrary localhost server the only honest fit is **`specialUse`**, which requires a `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` justification **reviewed in Google Play Console** ([Service types](https://developer.android.com/develop/background-work/services/fgs/service-types)). The alternative, `dataSync`, is semantically "data transfer" and good enough — **but Android 15+ caps `dataSync` and `mediaProcessing` at "a total of 6 hours in a 24-hour period"**, tracked per type, after which the system calls `Service.onTimeout()` and restarting throws `ForegroundServiceStartNotAllowedException`. **6 h per 24 h is not a long-running server.** ([Foreground service timeouts](https://developer.android.com/develop/background-work/services/fgs/timeout))
- **Doze and App Standby defer background network** — Doze *"reduces battery consumption by deferring background CPU and network activity"*; App Standby Buckets restrict device resources, and in the **restricted** bucket the restrictions apply **even while charging** ([Doze and App Standby](https://developer.android.com/training/monitoring-device-state/doze-standby), [App Standby Buckets](https://developer.android.com/topic/performance/appstandby)).
- **Worst case, the network is cut entirely:** if Android vitals flags the app and the user accepts a restriction, *"on AOSP builds, restricted apps **cannot run jobs, trigger alarms, or use the network, except when the app is in the foreground**"* ([Background work restrictions](https://developer.android.com/develop/background-work/background-tasks/bg-work-restrictions)). A localhost server in such an app just stops serving.
- **[?] Unverified either way:** whether an *already-open* loopback listener keeps accepting during Doze. No Android doc addresses loopback specifically. **[I] The only safe reading is "not guaranteed".**

**[I] And it buys nothing architectural:**
1. **The phone is a client.** Its database is local; there is no server for it to run. Putting an HTTP server inside the Android app creates a *localhost* dependency between a Flutter UI and a Rust library **on the same device** — strictly worse than calling the database directly via FFI, which is ~10 ns instead of ~13–24 µs (§6.3) and has no lifetime problem.
2. **The only reason would be code reuse** — the same Rust server on desktop and Android. But the phone does not need the hub's server behaviour (it does not host ComfyUI, and it is not authoritative). **Reuse here is a false economy.**

**Verdict: run Rust on the desktop if you run it at all, and then as an in-process library — do not run an HTTP server inside the Android app, and do not plan to spawn a Rust binary on Android at all.**

### 6.4 A cheaper path to the same RAM win

**[I] — this is the important practical point.** Before rewriting in Rust, note how much of the 344 MB is *optional*:

1. **Drop MySQL.** Removes the separate `mysqld` process entirely: **−486 MB working set**. Even keeping Kotlin, this is the largest single win and requires no Rust.
2. **Drop HikariCP + the MySQL JDBC driver** from the Kotlin server (both become unnecessary with SQLite; HikariCP's pool is also actively counterproductive against a single-writer SQLite). Removes classes, threads, and connection buffers.
3. **Reduce the JVM's own footprint** — the app currently passes only `-Dfile.encoding` and `-Djava.net.useSystemProxies`, so it is using default heap sizing on a 23 GB machine. **[I]** Setting `-Xmx`/`-XX:MaxRAMPercentage` and using a smaller GC would cut RSS materially. `[?]` I could not fetch the JVM/G1 documentation to cite a specific mechanism, so treat the direction as sound and the magnitude as unmeasured.
4. **Tune SQLite deliberately** (`busy_timeout`, modest `cache_size`, `synchronous=NORMAL` if you accept the durability trade-off documented in wal.html §2.3, `PRAGMA optimize` daily).
5. **Only then** evaluate whether the remaining JVM overhead justifies a Rust rewrite. **Re-measure after step 1** — that is the number that actually matters.

**[I]** If the goal is "ComfyUI needs the RAM", step 1 plus step 2 likely recovers ~500–600 MB for a fraction of the effort and risk of a language migration. Rust remains attractive long-term, but **making it a prerequisite for the RAM win would be sequencing the work backwards.**

---

## 7. Real-world precedent

**Evidence labels below are per-item. I want to be unusually explicit about two negatives, because they are the honest headline of this section:**

> **[?] There is no published, measured "we replaced MySQL/Postgres with SQLite and saved N MB of RSS" study that I could find.** The nearest quantified migrations measure *throughput* and *operational simplification*, not resident memory. If a report like that is the thing that would settle your decision, **it does not exist in the public record I could reach** — you will have to measure it yourself (which, per §1, is a day's work and would be the single most decision-relevant number you produce).
>
> **[?] There is likewise no verified production "we rewrote a JVM service in Rust and RSS went X → Y" report.** The commonly-cited examples are **not** that migration — see the Figma and Discord rows below, which are frequently miscited for it.

### 7.1 SQLite as a server-side database: official backing

| Finding | Evidence |
| --- | --- |
| **Officially endorsed, not a hack.** "Systems designers report success using SQLite as a data store on **server applications**… Developers report that **SQLite is often faster** than a client/server SQL database engine in this scenario… Database requests are serialized by the server, so **concurrency is not an issue**." | <https://sqlite.org/whentouse.html> §1 "Server-side database" **[V]** |
| **The framing that matters for this app:** "**SQLite does not compete with client/server databases. SQLite competes with `fopen()`.**" | <https://sqlite.org/whentouse.html> **[V]** |
| **SQLite runs sqlite.org itself** at ~400–500 K HTTP requests/day, 15–20 % dynamic, ~200 SQL statements per dynamic page, on a single VM sharing a physical host with 23 others, load average < 0.1. | <https://sqlite.org/whentouse.html> **[V]** |
| **Scale of deployment:** "most widely deployed database engine in existence"; "**over one trillion SQLite databases in active use**"; "approximately **two billion active deployments** with no serious problems"; database size limit **281 TB**. | Gaffney et al., *SQLite: Past, Present, and Future*, PVLDB 15(12):3535–3547, 2022, <https://www.vldb.org/pvldb/vol15/p3535-gaffney.pdf> **[V]**; <https://sqlite.org/howtocorrupt.html> **[V]** |
| **The one-writer cost in wall-clock terms:** "**Writes are serialized, but as writes normally only take milliseconds, applications simply take turns writing.**" Also: BLOBs "under about **100 KB**" are faster stored in SQLite than as files. | <https://sqlite.org/appfileformat.html> **[V]** |
| **Test rigour** (relevant to trusting it with your only copy): "**590 times as much test code** as library code" — 92,053 KSLOC of test vs 155.8 KSLOC of library; **51,445** TCL test cases; TH3 runs **50,362** cases in ~**2.4 M** instances; **6,754** `assert()`s; dbsqlfuzz applies on the order of **10⁹ test mutations per day**. The VLDB paper independently states "over **600 lines of test code for every line of code**" and "**100% of machine code branches**". | <https://sqlite.org/testing.html> **[V]**; VLDB 2022 **[V]** — *note the two sources give 590× vs "over 600×"; quote whichever you cite, don't blend them* |
| **Postgres is the sanctioned escape hatch** when the architecture is genuinely wrong. | <https://sqlite.org/useovernet.html> **[V]** |

### 7.2 The real precedent: Expensify — MySQL → SQLite, at scale

**The canonical case, and the one to read. [V]**

- Source: **"Scaling SQLite to 4M QPS on a single server (EC2 vs Bare Metal)"**, David Barrett, **8 Jan 2018** — <https://use.expensify.com/blog/scaling-sqlite-to-4m-qps-on-a-single-server>. (There is **no** "How Expensify got to 1 million users" SQLite post; that title circulates but does not exist. **[?]**)
- **"we don't use MySQL or Postgres — instead using none other than SQLite — and it works great."**
- **Throughput [V]:** bare metal **~4 M queries/sec**, vs the largest EC2 instance then available (x1e.32xlarge, 128 vCPU, 4 TB) capping at **~1.5 M QPS**. A trivial `SELECT 1;` reached ~**160 M QPS**.
- **Hardware [V]:** 1 TB DDR4, 3 TB NVMe, **192 physical cores** (384 threads). Benchmark DB: **10 B rows / 447 GB, fully RAM-cached, read-only**, NUMA-aware, hundreds of samples. A 30 B-row / **1.3 TB** table — larger than physical RAM — showed negligible impact.
- **What they had to change [V]:** SQLite patches to disable POSIX advisory locks, make `RANDOM()` deterministic, remove the `malloc()` global lock, and extend `max_mmap_size`; WAL for read concurrency; a page-locking branch for concurrent writes.
- **Bedrock**, their replication layer (private-blockchain journal, Paxos leader election, active/active, multi-DC, MySQL wire protocol): <https://bedrockdb.com/>, <https://bedrockdb.com/vs_mysql.html>, <https://bedrockdb.com/synchronization.html> **[V]**. Bedrock has been "under continuous development and operation for the past **8 years**" as of Oct 2016 ⇒ ~2008. **[V]**
- **The durability caveat, verbatim — read this one carefully [V]:** "the leader can 'race ahead' of the cluster: if it crashes with commits that haven't reached anyone else, **those commits are lost**… **There is no per-transaction way to ask for a stronger guarantee.**"
- **Framing [V]:** Barrett argues MySQL is "an old database designed for a time when disks were super slow and small… optimized for a series of constraints that no longer exist." (Echoes sqlite.org's own "SQLite competes with `fopen()`".)

**[I] How to use this honestly:** Expensify is the strongest evidence that **SQLite can be a production server database at scale**, and it is the reason the "SQLite isn't a real server DB" objection is dead. But note three limits: the 2018 benchmark is **read-only** against a fully-RAM-cached dataset; it predates the Spectre/Meltdown mitigations (they reported an early ~2.5 % hit); and **it reports no memory or ops-cost comparison**. It supports "SQLite is viable", not "SQLite saved us N MB".

### 7.3 Other verified precedents

| Case | What actually happened | Evidence |
| --- | --- | --- |
| **Bluesky / AT Protocol** — *the closest philosophical match to your app* | "We took a somewhat novel approach of giving **every user their own SQLite database**. By removing the Postgres dependency… **We didn't have to worry about things like replicas or failover.**" And explicitly: "SQLite worked really well because the PDS – in its ideal form – is a **single-tenant system**." Their PDS depends on `better-sqlite3` with **no Postgres dependency**. | Daniel Holmgren, via <https://simonwillison.net/2024/Apr/23/daniel-holmgren/> (23 Apr 2024) **[V]**; `packages/pds/package.json` **[V]** |
| **k3s** (Kubernetes distribution) | "**SQLite is the default datastore**." Constraint: "**SQLite cannot be used on clusters with multiple servers**" — HA requires embedded etcd or an external etcd/MySQL/Postgres. Server minimum: **2 cores / 2 GB RAM**. | <https://docs.k3s.io/datastore>, <https://docs.k3s.io/architecture> **[V]** — note the docs do **not** claim etcd is a RAM hog; that framing is unsourced. |
| **Home Assistant** | "The **default, and recommended, database engine is SQLite** which does not require any configuration." Supports MariaDB ≥ 10.3 / MySQL ≥ 8.0 / PostgreSQL ≥ 12 / SQLite ≥ 3.40.1. Also warns: "Changing database used by the recorder may result in **losing your existing history**. **Migrating data is not supported.**" | <https://www.home-assistant.io/integrations/recorder/> **[V]** |
| **Gitea** | SQLite is a builtin option ("If you plan to use SQLite, you can ignore this chapter") **but**: "**Be aware that SQLite does not scale**; if you expect your instance to grow at a later time, you should choose another database type." | <https://docs.gitea.com/installation/database-prep> **[V]** |
| **Cloudflare D1** | Confirms the "SQLite at the edge" pattern **and** that it is **not pure SQLite**: "**D1 is itself built on SQLite-in-DO**" (SQLite inside Durable Objects), with persistence replaced by a from-scratch **Storage Relay Service**. Honest limits: **max DB size 10 GB and "cannot be further increased"**; "**Each individual D1 database is inherently single-threaded, and processes queries one at a time**" — they scale *out* over many small per-tenant DBs, not up. | <https://developers.cloudflare.com/d1/>, <https://blog.cloudflare.com/building-d1-a-global-database/>, <https://blog.cloudflare.com/sqlite-in-durable-objects/> **[V]** |
| **Notion** | **Kept Postgres** (480 logical shards across 32 physical DBs; rejected Citus/Vitess). Their SQLite use is **client-side**: WASM SQLite + OPFS in a Web Worker, SharedWorker elects one "active tab" as writer. Measured benefit: page navigation **20 % faster** in all modern browsers — **28 % AU, 31 % CN, 33 % IN**. | <https://www.notion.com/blog/sharding-postgres-at-notion>; <https://www.notion.com/blog/how-we-sped-up-notion-in-the-browser-with-wasm-sqlite> **[V]** |
| **Discord** — *commonly miscited as a "moved to SQLite" story* | **Did not move a datastore to SQLite.** Cassandra (**JVM**) → **ScyllaDB (C++)**; 177 nodes → **72**; historical fetch p99 **40–125 ms → 15 ms**; insert p99 **5–70 ms → 5 ms**; migration estimated 3 months, took **9 days**; peak **3.2 M messages/s**. They did build a new **Rust** data-service tier (request coalescing + consistent-hash routing). **SQLite appears only as the local checkpoint store inside their Rust migrator.** | <https://discord.com/blog/how-discord-stores-trillions-of-messages> (6 Mar 2023) **[V]** |
| **Figma** — *also commonly miscited* | **Never adopted server-side SQLite**; they sharded **Postgres** (rejecting CockroachDB/TiDB/Spanner/Vitess). Their Rust story is **TypeScript/V8 → Rust**, not JVM → Rust: qualitative memory win ("uses so little memory compared to the old system that we can afford to fully parallelize all documents"), serialization "**over 10× faster**" — and a *separate* post reporting a Rust `BTreeMap` at "**more than 60 %** of a file's memory usage" replaced by a flat sorted `Vec`, saving "**20 % in memory costs across our entire multiplayer fleet**" with p99 deserialization 20 % faster. Their own framing of the metric: "the memory metric we really care about is **resident set size (RSS)**". | <https://www.figma.com/blog/how-figmas-databases-team-lived-to-tell-the-scale/>; <https://www.figma.com/blog/rust-in-production-at-figma/> (2018); <https://www.figma.com/blog/supporting-faster-file-load-times-with-memory-optimizations-in-rust/> **[V]** |

### 7.4 Independent measured benchmarks (useful for capacity intuition)

| Source | Numbers |
| --- | --- |
| **VLDB 2022 paper** (peer-reviewed, reproducible) — **[V]**, and note **its baseline is DuckDB, not Postgres/MySQL** | TATP OLTP: "**SQLite-WAL reaches a throughput of 10 thousand TPS**, which is **10× faster than DuckDB** for the small database and **500× faster** for the large database" (Raspberry Pi: 2× / 60×). **Footprint (Table 3): SQLite `-Os` = 900 KB binary / 15 s build / 340 MB build memory; DuckDB `-Os` = 32 MB / 5 min / 7.7 GB.** Their own optimizations then made SQLite "**4.2× faster on SSB**" on the Pi and **2.7×** overall on the cloud server. |
| **Wesley Aptekar-Cassels, "Consider SQLite"** (30 Dec 2021) — **[V]** | On an Equinix `m3.large.x86`, WAL + `synchronous=normal` + `temp_store=memory` + mmap: **13.78 µs/write ≈ 72,568 writes/s** (512 B, separate transactions); ~**496,770 reads/s (2.013 µs)**. On the cheapest **$5/month** box: **28,395 writes/s, 299,401 reads/s** — "~2.6× the time for writes and ~1.7× … reads … ~**3 % the price**". Reports Postgres as "**9.5× slower** same machine / **17.8×** same AZ / **50×** different AZ". Explicitly **not** for you if you "need to write dozens of MB/s… or have very complex or long-lived write transactions"; migrations are "worse"; a blocking transaction "will be blocking all writes". |
| **Sylvain Kerkour, "Optimizing SQLite for servers"** (15 Feb 2024) — **[V]** | **~8,300 writes/s and ~168,000 reads/s** on a ~€40/month 4-vCPU VM; "**~250,000+ writes/s** … when batching inserts". Recommended PRAGMAs: WAL, `busy_timeout=5000`, `synchronous=NORMAL`, `temp_store=memory` + `BEGIN IMMEDIATE` + a single writer connection. **Calls `BEGIN IMMEDIATE` "one of the biggest footguns of SQLite"** — see the box in §2.6. |
| **fly.io, "I'm All-In on Server-Side SQLite"** (9 May 2022) — **[V]** | Per-query latency "**10–20 microseconds**… **A 50–100× improvement** over an intra-region Postgres query" (Postgres: "up to a millisecond"). |
| **Litestream** — **[V] and a negative finding worth stating**: I found **no benchmarks** for it anywhere (blog, README, "How it works", "Alternatives", the fly.io essay, the 2025 revamp) — **no lag/throughput table, and no "$X vs managed Postgres" comparison**. Verified numbers that do exist: "**Typical costs are only about $1 per month**" (2021), batching into **one-second windows**, `max-sync-wal-bytes` default **64 MiB**, L1/L2/L3 compaction every **30 s / 5 min / 1 h**, snapshot every **24 h**, `l0-retention` **5 min**. Do not expect a Litestream performance claim to be citable — there isn't one. |

### 7.5 Measured on this machine — the most relevant precedent

**[V]** MySQL at **486.1 MB working set / 856.7 MB private commit** against a **137 MB logical dataset** whose real user content is **~2.6 MB**, with **4.3 GB of binlogs** on disk and a **256 MB `innodb_buffer_pool_size`** set independent of data size. Plus the Kotlin/Ktor server at **343.9 MB / 395.7 MB**.

**[I] The honest summary of §7:** I can substantiate *"SQLite is designed for exactly this server-side role, is trusted at enormous scale, and here is precisely what this app pays MySQL today."* I can also substantiate *"single-tenant-per-device SQLite is a deliberate, successful architecture choice at Bluesky, and the default recommendation of Home Assistant and k3s."* What I **cannot** substantiate, from any source, is a **quantified before/after RSS figure** for this class of migration — so the memory argument in this report rests on *your* measurements (§1) plus SQLite's *documented* memory model (§2.1), not on someone else's blog post. That is a weaker footing than a case study would give, and I would rather say so than imply otherwise.

---

## 8. What I'd pick given those constraints

### 8.1 The recommendation

**Storage: SQLite in WAL mode, bundled via `package:sqlite3` v3 (which already ships SQLite 3.53.4 — past the WAL-reset fix — so no pinning is needed), accessed through `drift`.**
**Sync: an application-level delta API over the hub's existing HTTP surface — a disciplined hand-rolled change-log, not a generic sync engine.**
**Server language: stay on Kotlin for now; delete MySQL (and HikariCP/JDBC) first, re-measure, and treat Rust as a deliberate later option.**
**Do not solve the phone's problem with FFI/Rust-on-Android — and do not plan to run *any* server process on Android.**

> **Two configuration details that are easy to get wrong and would bite immediately [V]:**
> 1. **Use `BEGIN IMMEDIATE` for every writing transaction** — a plain `BEGIN` can fail with `SQLITE_BUSY` *without* honouring `busy_timeout` on lock upgrade (§2.6). Your sync outbox transaction always writes, so this applies to it.
> 2. **drift does *not* enable WAL for you, and `readPool` defaults to 0.** You must pass a `setup:` callback that runs `pragma journal_mode = WAL`, and `readPool` is only effective with WAL. Without it, multi-isolate access produces "database is locked" errors (§3.3).

### 8.2 Why, against each constraint you gave

| Your constraint | How the recommendation answers it |
| --- | --- |
| **Single-user, not multi-tenant** | Every heavyweight option in §4 is priced for multi-tenancy (buckets, tenancy isolation, JWT/minter, Cloud). You need none of it. A hub-authoritative cursor protocol is the smallest thing that works. |
| **Desktop is the hub; phone is a lean client** | SQLite's own doc prescribes the proxy pattern for exactly this shape: keep the DB on the machine that owns it and "**implement a proxy that runs on the database machine that relays read/write requests from remote machines**" **[V]**. The app already has this proxy — it is the Ktor backend — and the Flutter client already has an injectable `baseUrl` **[V]**. The architecture is already right; only the storage engine changes. |
| **Sync over LAN now, internet later** | LAN-only is the *easy* case and it is where hub-authority is cheapest. Your protocol does not change when you later expose the hub over the internet (add TLS + auth); whereas options that assume a cloud rendezvous (Turso Cloud) or a Postgres replica (Electric) force a rethink. |
| **RAM is scarce because of ComfyUI** | The measured win is **deleting the 486 MB `mysqld` process** and then the HikariCP/JDBC machinery, replaced by a documented **~50 KB/connection + ≤2 MB page cache** SQLite budget **[V]**. No sync engine can beat "there is no extra process". PowerSync's floor (512 MB + 2 GB) and Electric's (Postgres + Elixir) are both *worse than today*. |
| **Team is learning Rust; TS beginner; has Flutter/Dart + Kotlin** | The recommended path uses **only skills the team already has** — Dart/Flutter for the client, Kotlin for the hub, SQL for both. Rust is deferred to a step where it is a *choice* rather than a blocker. This matters: the plan is executable next sprint. |
| **Android lean client** | `sqlite3` v3 bundles SQLite for Android `armv7a/aarch64/x86/x64` with no NDK wiring **[V]**; drift gives isolate offloading and migrations **[V]**. Avoid `sqflite` — its own docs say transactions are exclusive and cross-thread access is blocked during a transaction on Android **[V]**. |
| **Small dataset** | Measured: **~2.6 MB of real user content**; 137 MB only because of one 446 K-row event log **[V]**. So: **do not build a general replication engine for 2.6 MB.** And explicitly **exclude `ai_run_events` from sync**. |

### 8.3 Why not each of the alternatives

- **PowerSync** — the only one whose *feature set* genuinely fits (GA Flutter SDK, real local SQLite, real FIFO upload queue with retry, free Open Edition self-hosting **[V]**). Rejected on **RAM**: the vendor's own minimum is 512 MB service + 2 GB MongoDB, plus your backend and a JWT minter **[V]**. Spending ~2.5–3.5 GB to sync ~2.6 MB on a box that is RAM-starved by ComfyUI inverts the goal. Also: the client's JSON-view storage means rows live in ≥2 tables **[V]**, which is wasteful for exactly the table you most want to exclude anyway.
- **ElectricSQL** — rejected on three independent grounds: **read-path only** (does none of the write-back work), **no Dart client** (the community package is discontinued, and the vendor says the old system is unsupported), and it **requires Postgres ≥14 with logical replication** — i.e. it does not let you escape to SQLite, it makes you adopt a heavier DB. Cloud shut down 2026-08-11 **[V]**.
- **Turso** — the *conceptually* closest fit (local-first `push()`/`pull()`, row-level Last-Push-Wins, a real file, one optional binary) and by far the lightest runtime **[V]**. Rejected on **maturity**: no official Dart SDK ("community-driven and don't come with official Turso support"), the Turso Sync binding is **0.1.0 with 238 downloads from one publisher**, and the self-hosted sync server is **documented for "development and testing"** with no auth **[V]**. **This is the one to re-evaluate in 6–12 months** — if `turso_dart` reaches 1.x with production self-hosting, it becomes the obvious answer.
- **CR-SQLite / CRDTs** — solves concurrent multi-writer merging, which one user on two devices barely has; needs a custom SQLite build with extension loading, which is real work on Android and Windows; and it is **feature-frozen** (last tagged release 2024-01-17) with **no Dart bindings** (Automerge's and Loro's Dart packages are v0.0.0 stubs whose declared repos 404) **[V]**. Revisit only if you later want simultaneous editing of the same record.
- **Ditto / ObjectBox Sync** — the only options with an **official Flutter SDK**. Both are **commercial closed-source products** (ObjectBox's core is Apache-2.0 but Sync is a paid add-on) **[V]**. **[I] Rejected on principle and cost**: adopting a proprietary sync platform to move ~2.6 MB between one user's two devices is a poor trade, and it puts your data model behind a vendor boundary. **Worth naming as the alternative, though** — this is the real "buy instead of build" option, and if the hand-rolled protocol turns out to be a swamp, revisiting Ditto is the escape hatch.
- **Litestream** — **not a sync engine** (§4.6a): one-way, single-destination streaming backup with no return channel. Excellent for the hub's durability story; irrelevant to the phone.
- **LiteFS** — a FUSE-based cluster replication filesystem, **Linux-only artifacts**, and dormant: the maintainer said on 2024-09-09 that "we've moved development resources elsewhere" with no timeline **[V]**. Wrong layer, and FUSE does not exist on Android.
- **rqlite / dqlite** — very actively maintained (rqlite: 17,756 ★, pushed today) **[V]**, but Raft consensus across one desktop and one phone is enormous machinery for no benefit; these are server-HA tools, not device sync. dqlite's licence also reports as `NOASSERTION` (generally LGPL-family, unverified) **[?]**. **Bedrock** was not verified at all and is not claimed either way.
- **Syncthing/rsync/Dropbox on the file** — **documented corruption paths** (§4.7), stated by SQLite in its own words ("Your best defense is to not use SQLite for files on a network filesystem") **[V]**. Actively dangerous, and WAL makes it worse because committed state can live only in `-wal`.
- **CloudKit / Drive appdata** — CloudKit's declared platforms are **Apple-only (no Android, no Windows)**, which disqualifies it outright **[V]**; Drive appdata is an opaque blob store with no conflict model and several **unverified** specifics **[?]**, and since you need an authenticated HTTP endpoint on the hub anyway, routing through Google buys nothing (§4.8).

### 8.4 The concrete plan, in order

**Phase 0 — measure and decide (a day).**
Re-measure `mysqld` and the Ktor process; record the two numbers you are trying to beat (**486.1 MB** and **343.9 MB** working set today **[V]**). Confirm whether `ai_run_events` really needs to exist at all, and whether its 446 K rows should be pruned/rotated — **this is 112.9 MB of the 137 MB and has no sync value**.

**Phase 1 — delete MySQL from under Kotlin (highest RAM win per unit of risk).**
Keep Ktor; swap HikariCP + MySQL JDBC for `sqlite-jdbc`. Port `Migrate.kt`'s convergent DDL by replacing `information_schema` probes with `PRAGMA table_info` / `PRAGMA index_list`. Set `journal_mode=WAL`, `busy_timeout`, `foreign_keys`, and a modest `cache_size` on every connection. Add a daily `VACUUM INTO` backup from the server on a timer (mind the 2× disk and "target must not exist" rules **[V]**). **Turn binlogging off — you are not replicating, and it is currently 4.3 GB of your disk [V].** Re-measure. **[I] Expect to recover roughly 400–500 MB with no Rust and no client changes.**

**Phase 2 — client-side SQLite on Flutter (phone and desktop).**
Add `package:sqlite3` v3 + `drift`. **Pin the SQLite amalgamation to ≥ 3.51.3** via the hook `source.amalgamation` option to get the WAL-reset-bug fix **[V]**. Run DB work off the UI isolate (drift does this). Adopt `user_version`-gated migrations for anything that transforms data, alongside the existing convergent DDL for additive changes. Decide the per-table sync allowlist **now** (exclude `ai_run_events`; media via URL, not blobs).

**Phase 3 — the delta protocol.**
Implement §5.2 items 1–8 on the hub, in this order: (1) UUID keys, (2) hub-assigned monotonic `rev`, (3) `changes` outbox written in the same transaction as the mutation, (4) idempotent apply via `ON CONFLICT`, (5) tombstones with a **device-ack-based prune horizon** and a `devices` table, (6) a resumable bootstrap cursor, (7) explicit version negotiation, (8) written-down conflict policy (field-level LWW for scalars, set-union for tags, documented last-save-wins for long text). Write tests for the §5.3 failure list — especially **resurrection** and **idempotent retry**.

**Phase 4 — only if measurement still demands it.**
Re-evaluate the JVM's remaining footprint (`-Xmx`/GC tuning first — note default max heap is *one quarter of physical memory* per Oracle's GC tuning guide **[V]**, so on this 23 GB box the ceiling is defaulted high and is worth setting explicitly; and 77 threads × ~1 MB default stack is ~77 MB of reservation before heap **[V]**). Then, if a Rust rewrite is still justified, do it **on the desktop only**, as an **in-process library** — the measured gap is decisive: Dart→Rust FFI is ~**10 ns** per call vs ~**13–24 µs** for a localhost HTTP round trip (§6.3) **[V, MEASURED]**, and a minimal Rust HTTP server is a **~4–5 MB** process either way **[V, MEASURED]**. Use **`flutter_rust_bridge` 2.13.0** (MIT, Flutter Favorite, 5,417 ★, supports async + Rust→Dart `Stream`s) — **but note `cargokit`, its default build backend, was archived on 2026-03-26 and frb now depends on its own fork of it** (§6.2) **[V]** — with **`rusqlite` + `bundled`** (MIT; bundles SQLite 3.53.2, so past the WAL-reset fix; `unlock_notify` and `preupdate_hook` fit a sync engine) **[V]**. **Do not put an HTTP server inside the Android app, and do not plan to spawn a Rust binary on Android at all** — Android 10+ forbids `execve()` on files in the app home directory (§6.3) **[V]**.

**Phase 5 — revisit Turso.** Re-check `turso_dart` maturity and whether a *production* self-hosted Turso Sync server shipped. If both landed, it likely becomes the better answer than the hand-rolled protocol.

### 8.5 What would change my mind

- If **ComfyUI could move to another machine**, the RAM constraint evaporates and **PowerSync Open Edition becomes the best choice** — you get a battle-tested sync engine, a GA Flutter SDK, and a real upload queue for free, and 2.5 GB stops mattering.
- If **`ai_run_events` turns out to be essential on the phone**, the sync problem gets much bigger and a purpose-built engine becomes more attractive.
- If **true concurrent editing of the same records on both devices** becomes a requirement, LWW is the wrong foundation and CRDTs (or PowerSync + a CRDT layer) should be reconsidered.
- If the team decides the **Rust rewrite is a goal in itself** (learning objective), that is a legitimate reason to reorder — but do Phase 1 first anyway, because it is independent and it tells you how much is actually left to win.

---

## 9. What I could not verify (read this before relying on the report)

I want the gaps to be as visible as the findings.

| # | Gap | Consequence |
| --- | --- | --- |
| 1 | **`web_search` was dead for this entire session** (`HTTP 402: Insufficient Balance` on the search endpoint — the error directs to Settings → Plugins → Plugin configuration → Web search, or `DEEPSEEK_SEARCH_BASE_URL` / `web-search-deepseek.baseURL`). | All research came from directly-fetched URLs, not discovery. **Workaround that worked, and is worth reusing: `pwsh` + `Invoke-WebRequest` reaches hosts the `web_fetch` tool cannot** — including `developer.android.com`, `android.googlesource.com`, `raw.githubusercontent.com`, and DuckDuckGo's HTML endpoint (for discovery). Several sections of this report exist only because of that workaround. |
| 2 | **GitHub HTML unreachable via `web_fetch`**; `api.github.com` works but rate-limits fast. | Repo facts come from the REST API, or from `pwsh`-fetched HTML. |
| 3 | ~~`developer.android.com` unreachable~~ → **CLOSED via `pwsh`.** §3.1 now carries a verified per-API SQLite version table from AOSP, AOSP's compile-time flags, and the official `SQLiteDatabase`/`SQLiteOpenHelper` WAL behaviour. | **Closed.** The Android platform constraints in §3.5 and §6.3 are now **[V]**, not inferred. |
| 4 | ~~Apple/Google docs unreachable~~ → **Apple CLOSED.** CloudKit platform availability, size limits, and 3-way conflict semantics are documented in §4.8. | **Still open: Google Drive `appDataFolder` specifics** — the article body never loaded (`developers.google.com` unreachable even via `pwsh`; the `.cn` mirror truncates). The four unverified items are listed explicitly in §4.8. |
| 5 | ~~No Rust HTTP server RSS numbers~~ → **CLOSED.** Measured on this machine: **4.23–5.00 MB** working set for a minimal Rust server, plus published cross-checks (axum < 15 MB idle; Ktor 300–400 MB). §6.1. | **Closed.** |
| 6 | ~~`cargokit` status unverified~~ → **CLOSED, and the answer is a correction: `cargokit` is ARCHIVED** (2026-03-26), and frb now uses its own fork. §6.2. Also verified: Android is a **Tier 2** Rust target. | **Closed.** Still open: **`cargokit`'s SPDX license id** (GitHub reports `NOASSERTION`). |
| 7 | **The WAL-reset bug's impact on Android is now stronger than inferred, but still not a documented claim.** AOSP's **highest** system SQLite is **3.50.6**, inside the vulnerable 3.7.0–3.51.2 range, and the backports exist for 3.44.6/3.50.7 — which are not what ships. | **[V]** that every shipping Android system SQLite is *in the range*; **[I]** that shipping builds lack the backport. The advice "bundle your own SQLite, pinned ≥ 3.51.3" is robust either way — and note `package:sqlite3` bundles **3.53.4**, `rusqlite`'s `bundled` builds **3.53.2**. |
| 8 | ~~LiteFS deprecation unverified~~ → **CLOSED**: not archived, but the maintainer stated on 2024-09-09 that development resources moved elsewhere with no timeline. Quoted in §4.6. | **Closed.** |
| 9 | ~~CRDT landscape unverified~~ → **CLOSED**: CR-SQLite is feature-frozen (last *tagged release* 2024-01-17); Automerge/Loro Dart packages are **v0.0.0 stubs whose repos 404**; `flutter_loro_ffi` is discontinued; **Ditto is the only product with an official Flutter SDK and it is proprietary**. §4.5. | **Closed.** Residual: **Bedrock (Expensify) remains unverified** and is not claimed either way. |
| 10 | ~~§7 precedent largely unfilled~~ → **CLOSED.** §7 now has Expensify (with numbers and a durability caveat), Bluesky, k3s, Home Assistant, Gitea, Cloudflare D1, Notion, plus corrected Figma/Discord rows and independent benchmarks. | **Closed.** Two *negative* findings remain the honest headline and are stated in §7: **no published RSS before/after for MySQL/Postgres→SQLite, and no production JVM→Rust RSS report.** Both are gaps in the public record, not gaps in the research. |
| 11 | **No numeric memory figures published for Electric's sync service or Turso's sync server.** | Confirmed absent from the pages fetched (vendor publishes only qualitative claims for both). PowerSync is the only engine with published sizing. |
| 12 | **Whether PowerSync's Dart client accepts plaintext `http://<LAN-IP>`** (vs requiring HTTPS) is unverified. | Material if PowerSync is ever reconsidered for LAN-only use. |
| 13 | **MongoDB Atlas Device Sync EOL date unverified.** Deprecation corroborated (all `device-sync/*` docs URLs 404; Ditto ships a migration guide) but the commonly cited dates are not confirmed by a fetched source. | Stated as "deprecated, exact date unverified" in §4.5. |
| 14 | **ObjectBox Sync licensing terms unverified** (`objectbox.io/pricing/` 404s). `dqlite`'s license also reports as `NOASSERTION`. | Stated as "commercial add-on, terms unconfirmed" in §4.5/§4.6. |
| 15 | **Android 8/9 exact system SQLite version unverified** — those AOSP refs expose no `README.version` or `dist/`. **Microsoft: no official Google statement on Auto Backup vs `-wal`/`-shm`** — Android's autobackup page contains **zero** occurrences of "wal". | The Auto Backup + WAL interaction is an **open risk**, not a documented fact. Do not cite Google for it in either direction. |
| 16 | **Google Drive appdata: whether it counts against the user's Drive quota, and its file-count/size limits, unverified.** | Listed explicitly in §4.8. |
| 17 | **No verified TechEmpower RSS data** (results table is JS-rendered), and **no official frb binary-size/build-time measurement** (frb's own performance page declines to publish figures). | I cite blog/repo measurements for RSS, and label them as such. |
| 18 | **`sqlx`'s SQLite pooling behaviour** — the default `max_connections: 10` and the absence of a reader/writer split are confirmed from source and maintainer comments; **no benchmark of the resulting contention** was found. | Relevant only if Rust is chosen **and** `sqlx` is chosen over `rusqlite`. **[I] Recommendation in that case: `rusqlite` + one writer.** |
| 19 | **Not verified at all (do not treat as researched):** Signal Desktop's SQLite/SQLCipher internals; Tailscale state-store WAL corruption; Mozilla/Firefox SQLite guidance; Forgejo and Piwigo defaults; Expensify's Bedrock internals beyond its published pages. | No claims made about any of these. |

### Sources

**SQLite official** — [wal.html](https://sqlite.org/wal.html) · [howtocorrupt.html](https://sqlite.org/howtocorrupt.html) · [useovernet.html](https://sqlite.org/useovernet.html) · [whentouse.html](https://sqlite.org/whentouse.html) · [backup.html](https://sqlite.org/backup.html) · [lang_vacuum.html](https://sqlite.org/lang_vacuum.html) · [malloc.html](https://sqlite.org/malloc.html) · [mmap.html](https://sqlite.org/mmap.html) · [pragma.html](https://www.sqlite.org/pragma.html#pragma_cache_size)

**Dart/Flutter** — [sqlite3](https://pub.dev/packages/sqlite3) · [sqlite3_flutter_libs (EOL)](https://pub.dev/packages/sqlite3_flutter_libs) · [sqlite3_native_assets (discontinued)](https://pub.dev/packages/sqlite3_native_assets) · [drift](https://pub.dev/packages/drift) · [sqflite](https://pub.dev/packages/sqflite) · [powersync](https://pub.dev/packages/powersync) · [powersync_core (discontinued)](https://pub.dev/packages/powersync_core) · [flutter_rust_bridge](https://pub.dev/packages/flutter_rust_bridge)

**Sync engines** — [PowerSync service architecture](https://docs.powersync.com/architecture/powersync-service.md) · [client architecture](https://docs.powersync.com/architecture/client-architecture.md) · [self-hosting](https://docs.powersync.com/intro/self-hosting.md) · [deployment architecture](https://docs.powersync.com/maintenance-ops/self-hosting/deployment-architecture.md) · [FAQ](https://docs.powersync.com/resources/faq.md) · [FSL](https://powersync.com/legal/fsl) · [Electric sync](https://electric.ax/sync.md) · [Postgres Sync](https://electric.ax/sync/postgres-sync.md) · [Electric Next](https://electric.ax/blog/2024/07/17/electric-next.md) · [Electric joining Databricks](https://electric.ax/blog/2026/08/11/electric-joining-databricks.md) · [Turso Embedded Replicas](https://docs.turso.tech/features/embedded-replicas/introduction.md) · [Turso Sync benchmark](https://turso.tech/blog/sync-benchmark) · [Turso local sync server](https://docs.turso.tech/sync/local-sync-server.md) · [Turso SDKs](https://docs.turso.tech/sdk/introduction.md)

**Repos (GitHub REST API, 2026-09-23)** — [cr-sqlite](https://api.github.com/repos/vlcn-io/cr-sqlite) · [litefs](https://api.github.com/repos/superfly/litefs) · [rqlite](https://api.github.com/repos/rqlite/rqlite) · [powersync-service](https://api.github.com/repos/powersync-ja/powersync-service) · [electric](https://api.github.com/repos/electric-sql/electric) · [libsql](https://api.github.com/repos/tursodatabase/libsql) · [turso](https://api.github.com/repos/tursodatabase/turso) · [rusqlite (crates.io)](https://crates.io/api/v1/crates/rusqlite)

**Other** — [Cockroach Labs: living without atomic clocks](https://www.cockroachlabs.com/blog/living-without-atomic-clocks/) · [HLC paper (URL cited, PDF not fetched)](https://cse.buffalo.edu/~demirbas/publications/hlc.pdf)
