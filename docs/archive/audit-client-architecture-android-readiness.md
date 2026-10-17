# ComfyHub client audit — architecture, persistence, Android readiness, desktop coupling, data volume, sync

Scope audited (read, not guessed): all 39 files under `lib/` (8,660 raw lines), `pubspec.yaml`, `pubspec.lock`, `.flutter-plugins-dependencies`, the whole `android/` tree, `docs/android-agp9-builtin-kotlin-migration.md`, `docs/android-client-plan.md`, `docs/inbound-auth-design.md`, `db/schema.sql`, `db/migrate.sql`, the Ktor route files under `server/src/main/kotlin/com/comfyhub/`, `scripts/mysql.ps1`, `server/build.gradle.kts`, `README.md`. **No files were modified.**

---

## 1. Client architecture

### 1.1 Transport layer — two hand-written REST clients, both thin wrappers over `package:http`

| Layer | File | Lines | Scope |
|---|---|---|---|
| Library REST client | `lib/core/api_client.dart` | 324 | prompts / tags / media / capture |
| AI REST+SSE client | `lib/core/ai_api_client.dart` | 421 | `/api/ai/*` + `/api/capture/jobs` + `/api/prompts/{id}/workflow` |

Both are plain classes holding `final String baseUrl` and `http.Client _client` (`api_client.dart:22-26`, `ai_api_client.dart:11-15`). There is no generated client, no OpenAPI, no gRPC, no protobuf — every call is a hand-written method with a hand-written DTO in `lib/models/models.dart` (708 lines) or `lib/models/ai_models.dart` (1,835 lines). Errors are normalised into `ApiException` (`api_client.dart:9-19`, thrown at `:62-66`) and `AiApiException` (`ai_api_client.dart:499-507`).

`ApiException` message for an unreachable backend is literally `'无法连接后端 $baseUrl'` (`api_client.dart:73`, `:85`, `:302`, `:378`).

### 1.2 Base URL / port discovery

Three mechanisms, in priority order, all in `lib/core/settings_store.dart`:

1. **User-entered URL** (wins everything): `_kBaseUrl = 'comfyhub.baseUrl'` in `SharedPreferences` (`settings_store.dart:9`), read at `:149-151`; `_baseUrlFromUser` flag at `:25`/`:35`. Set via `setBaseUrl` (`:177-196`).
2. **Port file discovery** (desktop auto): `.run/api.json` at the project root, read by `_baseUrlFromPortFile()` (`settings_store.dart:87-106`) — takes `port` (validated `1..65535`) and `host`, remapping `0.0.0.0`/`::` back to `127.0.0.1` (`:98-99`). The candidate roots come from `_projectRootCandidates()` (`:113-145`): explicit root → `COMFYHUB_PROJECT_ROOT` env → walk up ≤8 levels from `Platform.resolvedExecutable` and `Directory.current` looking for `scripts/comfyhub.ps1`.
3. **Hardcoded defaults** (`defaultBaseUrl()`, `settings_store.dart:60-75`):
   - Web: `http://127.0.0.1:8080` (`:61`)
   - **Android: `http://10.0.2.2:8080`** (`:67`) — with a comment explicitly saying it deliberately does **not** read the port file, because the phone and the backend are different machines
   - Everything else: port file, else `http://127.0.0.1:8080` (`:74`)

The port is **not fixed** any more: the backend picks a port at startup and records it in `.run/api.json`; the client re-reads it on every cold start (`settings_store.dart:86` "用完**不缓存**") and after every backend restart — `refreshBaseUrlFromPortFile()` (`:168-175`), called from `app.dart:139`, `backend_launcher.dart:268`, `:375`, `:424`, `settings_page.dart:94`.

**Verified against the live running backend:** the default is `COMFYHUB_PORT=0` = OS-chosen ephemeral port, not 8080 (`Config.kt:66-74`); 8080 is only a legacy candidate. `.run/api.json` on this machine currently reads `{"port":1986,"pid":20756,"host":"127.0.0.1","url":"http://127.0.0.1:1986","requestedPort":0,"fellBack":false}`, and `Get-NetTCPConnection` confirms PID 20756 listening on `127.0.0.1:1986`. **The `127.0.0.1:8080` literals in `app.dart:240`, `settings_store.dart:61`, `:74`, `settings_page.dart:1083` are therefore fallbacks that are only correct when the port file is unreadable** — the app normally discovers the real port. The launch-screen text at `app.dart:240` ("MySQL（127.0.0.1:3307）→ Kotlin 后端（127.0.0.1:8080）") is stale in the same way.

A failed bind also falls back to a further random port (`Application.kt:160-188`), which is exactly the failure mode the port-file re-read at `backend_launcher.dart:370-377` exists to absorb.

`LibraryStore` rebuilds its `ApiClient` when `baseUrl` changes (`library_store.dart:76-84`); `AiWorkspaceStore` (`ai_workspace_store.dart:140-150`) and `NsfwStore` (`nsfw_store.dart:27-37`) do the same via a `_cachedFor` string compare.

### 1.3 Full endpoint inventory the app calls

Verified by reading both clients and cross-checking each path against the Ktor route registrations (`Application.kt:588`, `PromptRoutes.kt:28`, `MediaRoutes.kt:41`, `TagRoutes.kt:23`, `CaptureRoutes.kt:32`, `AiRoutes.kt:82`).

#### A. Health / stats
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/health` | `api_client.dart:93-94`; also raw `http.get` in `backend_launcher.dart:114,120` |
| GET | `/api/stats` | `api_client.dart:96-97` |

#### B. Prompts
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/prompts` | `api_client.dart:103-129` |
| GET | `/api/prompts/{id}` | `:131-132` |
| POST | `/api/prompts` | `:134-135` |
| PUT | `/api/prompts/{id}` | `:137-138` |
| DELETE | `/api/prompts/{id}` | `:140` |
| POST | `/api/prompts/{id}/duplicate` | `:142-143` |
| POST | `/api/prompts/{id}/favorite` | `:145-146` |
| POST | `/api/prompts/{id}/tags` | `:148-150` |
| DELETE | `/api/prompts/{id}/tags/{tagId}` | `:152-154` |
| GET | `/api/prompts/{id}/media` | `:156-159` |
| GET | `/api/prompts/{id}/workflow` | `api_client.dart:363`; **also** `ai_api_client.dart:343-350` |

#### C. Tags
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/tags` | `api_client.dart:165-173` |
| GET | `/api/tags/categories` | `:175-176` |
| POST | `/api/tags` | `:178-184` |
| PUT | `/api/tags/{id}` | `:186-192` |
| DELETE | `/api/tags/{id}` | `:194` |

#### D. Media / gallery
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/media` | `api_client.dart:200-228` |
| GET | `/api/media/{id}` | `:230-231` |
| PATCH | `/api/media/{id}` | `:250-270` (title / notes / favorite / source / nsfw / promptId / clearPrompt) |
| DELETE | `/api/media/{id}` | `:272` |
| POST | `/api/media/upload` (multipart, field `files`) | `:275-304` |
| GET | `/api/media/{id}/prompt` | `:239-248` (204 = no link) |
| GET | `/api/media/{id}/workflow` | `:366` |
| GET | `/api/media/{id}/poster` | `:237` — URL string only, consumed by `Image.network` |
| GET | `/api/media/{id}/file`, `/thumb` | server-supplied relative paths in `MediaAsset.fileUrl`/`thumbUrl` (`models.dart:323-324`), absolutised by `api_client.dart:42-43` |

#### E. ComfyUI capture / config
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/capture/config` | `api_client.dart:310-311` |
| PUT | `/api/capture/config` | `:313-315` |
| GET | `/api/capture/status` | `:317-318`; **also** `ai_home_page.dart:4086` |
| GET | `/api/capture/locate` | `:321-322` |
| POST | `/api/capture/locate/apply` | `:325-327` |
| POST | `/api/capture/poll` | `:330-331` |
| POST | `/api/capture/import` | `:334-348` |
| POST | `/api/capture/import-workflows` | `:355-360` |
| GET | `/api/capture/jobs` | `ai_api_client.dart:455-456` |

#### F. AI — providers / credentials / models
| Method | Path | Client call site |
|---|---|---|
| GET/POST | `/api/ai/providers` | `ai_api_client.dart:58-64` |
| PUT/DELETE | `/api/ai/providers/{id}` | `:66-69` |
| POST | `/api/ai/providers/{id}/test` | `:72-73` |
| POST | `/api/ai/providers/{id}/discover-models` | `:76-77` |
| GET/PUT/DELETE | `/api/ai/providers/{id}/credentials` | `:79-90` |
| GET/PUT/DELETE | `/api/ai/web-search/credential` | `:97-108` |
| GET/PUT | `/api/ai/providers/{id}/models` | `:112-120` |

#### G. AI — conversations / runs / SSE
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/ai/conversations` (`?includeArchived=1`) | `ai_api_client.dart:379-385` |
| POST | `/api/ai/conversations` | `:387-392` |
| PATCH/DELETE | `/api/ai/conversations/{id}` | `:394-397` |
| GET | `/api/ai/conversations/{id}/messages` | `:399-402` |
| POST | `/api/ai/conversations/{id}/messages` | `:404-418` |
| POST | `/api/ai/conversations/{id}/runs` | `:430-447` |
| POST | `/api/ai/runs/{id}/cancel` | `:449` |
| **GET (SSE)** | `/api/ai/runs/{id}/events?after=N` | `:459-496` |
| GET | `/api/ai/runs/{id}/question` | `:217-223` |
| POST | `/api/ai/questions/{id}/answer` \| `/skip` | `:229-245` |

#### H. AI — tools / policy / skills / memory / NSFW / catalog
| Method | Path | Client call site |
|---|---|---|
| GET | `/api/ai/tools` | `ai_api_client.dart:186-192` |
| GET/PUT | `/api/ai/tools/policy` | `:194-199` |
| POST | `/api/ai/tool-calls/{callId}/approve` \| `/deny` | `:203-207` |
| GET | `/api/ai/skills`, `/skills/{name}`, `/skills/roots` | `:125-156` |
| POST/DELETE | `/api/ai/skills`, `/skills/{name}`, `/skills/rescan` | `:137-161` |
| GET/PUT/POST/DELETE | `/api/ai/memory`, `/memory/entries`, `/memory/delete` | `:165-182` |
| GET/PUT | `/api/ai/nsfw` | `:253-259` |
| GET/POST | `/api/ai/builtin/status`, `/builtin/sync` | `:264-273` |
| GET/POST/PUT | `/api/ai/capabilities/status`, `/refresh`, `/probe-toggle`, `/probe` | `:278-298` |

#### I. AI — attachments / preflight
| Method | Path | Client call site |
|---|---|---|
| POST | `/api/ai/attachments` (multipart, field `files`) | `ai_api_client.dart:307-318` |
| GET | `/api/ai/attachments/{id}` \| `/file` \| `/thumb` | `:322-327`, `:1086-1110` server-side |
| DELETE | `/api/ai/attachments/{id}` | `:353-354` |
| POST | `/api/ai/preflight` | `:362-375` |

**Total: ~95 distinct method+path combinations the client uses across 9 feature groups.** The backend registers **101** route handlers in total (`Application.kt:577-614` mounts `promptRoutes()`, `tagRoutes()`, `mediaRoutes()`, `captureRoutes()`, `aiRoutes()`, plus `/api/health` and `/api/stats`) — so the client exercises ~94 % of the surface. The remainder (`POST /api/ingest/comfyui`, `GET /api/prompts/{id}/api-graph`, `GET /api/capture/workflows`) is consumed by the optional ComfyUI node and scripts, not the Flutter app.

### 1.4 State management

`provider` with five `ChangeNotifier`s mounted at the root (`app.dart:27-48`): `SettingsStore`, `BackendLauncher`, `LibraryStore`, `AiWorkspaceStore`, `NsfwStore`. Notably `NsfwStore` is deliberately **global** (`app.dart:41-47`, `nsfw_store.dart:16-17`) because blur must apply on gallery, prompts, tags and AI bubbles at once.

---

## 2. Local persistence in Flutter — what actually exists

### 2.1 The complete answer: `shared_preferences` only. Nothing else.

`grep` over all of `lib/` for `shared_preferences|SharedPreferences|sqflite|hive|drift|path_provider|getApplicationDocumentsDirectory|getTemporaryDirectory`:

- **`shared_preferences`: 2 files only** — `lib/core/settings_store.dart:5`, `lib/state/ai_workspace_store.dart:6`.
- **`sqflite`: 0 matches. `hive`: 0. `drift`: 0. `isar`: 0. `objectbox`: 0.**
- **`path_provider`: 0 matches in `lib/`** (it is present only as a transitive dependency — see `.flutter-plugins-dependencies`, it is pulled in by `file_picker`/`audioplayers`, not used by app code).
- The only other file I/O in `lib/` is **read-only** and only for port discovery / project-root detection: `File(...).existsSync()`, `file.readAsStringSync()` in `settings_store.dart:90-93`, `:125`, `:135` and `backend_launcher.dart:156`, `:167`, `:185`, `:203`, `:215`, `:221`, `:251`, `:586`. **No Dart code anywhere in `lib/` ever writes a file.**

### 2.2 Every persisted key (complete list — 11 keys, all in `SharedPreferences`)

From `settings_store.dart:9-14`:

| Key | Type | Written | Read | Meaning |
|---|---|---|---|---|
| `comfyhub.baseUrl` | String | `:195` | `:149` | user-entered backend URL; **absent = auto-discover** |
| `comfyhub.pageSize` | int | `:202` | `:152` | page size, clamped `8..120` (`:199`) although the slider offers `8..96` (`settings_page.dart:670-672`) |
| `comfyhub.autoStartBackend` | bool | `:209` | `:153` | default `true` |
| `comfyhub.stopServicesOnExit` | bool | `:216` | `:154` | default `true` |
| `comfyhub.projectRoot` | String | `:227` | `:155` | path containing `scripts/comfyhub.ps1` |
| `comfyhub.mysqlDataDir` | String | `:238` | `:156` | MySQL instance dir override |

From `ai_workspace_store.dart:29-33`:

| Key | Type | Written | Read | Meaning |
|---|---|---|---|---|
| `comfyhub.ai.lastProviderId` | String | `:45` | `:52` | remembered provider |
| `comfyhub.ai.lastModelId` | String | `:46` | `:53` | remembered model |
| `comfyhub.ai.lastReasoningEffort` | String | `:47` | `:54` | remembered effort (`wire`) |
| `comfyhub.ai.draft.<conversationId>` | String | `:91` | `:102`, `:982` | per-conversation composer draft text |
| `comfyhub.ai.draftAttachments.<conversationId>` | String (JSON) | `:92-95` | `:103` | per-conversation attachment **metadata only** (`AiAttachment.toJson`, `ai_models.dart:957-963` — id/name/modality/mime/size, **no bytes, no path**) |

### 2.3 Explicit statement: all library and chat state is in memory + server

- `LibraryStore` holds `Paged<Prompt> prompts`, `Paged<MediaAsset> media`, `List<Tag> tags`, `LibraryStats stats` as plain fields (`library_store.dart:28`, `:48`, `:65`, `:70`). Every mutation ends with a `refresh*()` that re-fetches from the server (`library_store.dart:102-109`, `:379-440`). **Nothing is written to disk.**
- `AiWorkspaceStore` holds `List<AiConversation> conversations`, `AiConversation? conversation`, `List<AiMessage> messages` (`ai_workspace_store.dart:159-161`), re-fetched via `_api.listConversations()` (`:919`, `:1169`, `:1962`) and `_api.listMessages(id)` (`:1093`, `:1943`).
- `NsfwStore._revealed` is explicitly **memory-only by design** (`nsfw_store.dart:44-48`): "关掉 App 再打开时重新打码".
- `BackendLauncher.logLines` is memory-only, capped at 200 lines (`backend_launcher.dart:92`, `:676-678`).
- All SSE streaming state (`_liveReasoning`, `_liveSegments`, `_liveToolCalls`, `_runIds`) is memory-only (`ai_workspace_store.dart:1721`, `:1728`, `:1731`, `:1741`) and lost on restart — including `_runIds`, so `retryOfRunId` degrades to `null` after a restart (`ai_api_client.dart:446`).

**Consequence for Android: the app has no offline mode at all.** Turn off the backend or lose Wi-Fi and every page is empty except the composer draft. `StartupGate` even offers a "跳过，先进入界面" button on the launch screen (`app.dart:286-289`) whose comment claims "离线只看本地缓存" (`app.dart:74`) — **there is no local cache to read**; that comment is inaccurate.

The one partial exception is `zoomable_image_view.dart:100` using `NetworkImage(url)`, which benefits from Flutter's in-memory `ImageCache` (default 100 MB / 1000 entries, referenced at `media_thumb.dart:19`) — RAM only, gone on restart.

---

## 3. What runs where

### 3.1 Pure client rendering of server data (portable as-is)

| Feature | Files | Why portable |
|---|---|---|
| Prompt library: list / search / tag filter / sort / paginate | `prompts_page.dart` (789), `prompt_detail_page.dart` (377), `prompt_edit_page.dart` (324) | Only `ApiClient` calls; zero `dart:io`; adaptive column layout (`adaptive_layout.dart`, columns = width/550, max 4) |
| Tag management + tag results | `tags_page.dart` (327), `tag_results_page.dart` (167) | Same |
| Gallery grid, filters, multi-select, bulk ops | `gallery_page.dart` (564), `media_thumb.dart` (291) | Grid is responsive (`SliverGridDelegateWithMaxCrossAxisExtent`, `gallery_page.dart:308-313`) |
| Media detail (metadata, prompt panel, params, tags, workflow viewer) | `media_detail_page.dart` (717), `workflow_viewer.dart` (286), `prompt_params.dart` (148) | Layout already has a narrow branch (`media_detail_page.dart:236-247`, `:638`) |
| AI conversation UI: message list, markdown, tool cards, permission toggle, model picker, ComfyUI progress panel | `ai_home_page.dart` (5,007), `markdown.dart` (464), `markdown_view.dart` (273) | Fully responsive: 900 px and 1200 px breakpoints (`ai_home_page.dart:256-267`), narrow layout uses `Drawer` + `showModalBottomSheet` (`:324-340`) |
| Settings panes for AI providers / models / credentials / tool policy | `ai_provider_settings_page.dart` (2,061), `ai_tools_settings_page.dart` (650) | Pure REST + widgets; credential values are write-only by design (`ai_provider_settings_page.dart:12-15`) |
| NSFW blur + reveal | `nsfw_blur.dart` (99), `nsfw_store.dart` (104) | Backend is the source of truth for the flags (`ai_api_client.dart:253-259`) |

### 3.2 Features that inherently require the desktop machine

| Capability | Where | Why it cannot move |
|---|---|---|
| **Booting MySQL + the JVM backend** | `backend_launcher.dart:261-393` (spawns `pwsh -File scripts\comfyhub.ps1 up`), guarded by `!Platform.isWindows` at `:286-288` | Shells out to PowerShell 7 to start `mysqld.exe` + `java`. Message when blocked: `'当前平台不支持自动启动本地服务（仅 Windows 桌面端）。'` |
| Stop / restart / relocate services | `backend_launcher.dart:396-436` (`runAction`), `:575-613` (`moveMysqlData` → `scripts\mysql.ps1 move`), `:451-516` (`releaseOnExit`, `armOwnerWatch`, `disarmOwnerWatch`) | Same |
| **ComfyUI submission** | Server-side (`ComfySubmitter.kt`), driven by the AI's `comfy_submit` tool; the client only triggers runs and renders `mediaIds` from `tool.completed` (`ai_workspace_store.dart:1528`, `ai_home_page.dart:2414`) | ComfyUI + the GPU + the workflow files live on the desktop |
| **ComfyUI auto-capture / polling** | `api_client.dart:310-360`, `settings_page.dart:138-377`, `:1016-1199` | Reads the local `output/` directory and PNG-embedded `workflow`/`prompt` chunks; `GET /api/capture/jobs` asks `127.0.0.1:8188` directly (`CaptureRoutes.kt:54-67`) |
| **Local file access** | `settings_page.dart:320-345` (`FilePicker.getDirectoryPath` → `POST /api/capture/import`), `:373-377`, `upload_sheet.dart:72-85`, `ai_home_page.dart:3176-3195` | Desktop picker returns **absolute paths**; both upload paths send paths, not bytes (`api_client.dart:292-294`, `ai_api_client.dart:310-313`) |
| **MCP / comfy-mcp tooling** | Server-side only (`ai/mcp/McpToolBridge.kt`); client never touches it (`docs/android-client-plan.md:136-143`) | Owns a Python runtime + child process on the desktop |
| **"Open in Explorer" for storage / skills / project root** | `settings_page.dart:459-467` (`Uri.directory`), `ai_home_page.dart:4996-5006` (`Uri.file(path, windows: true)`) | Filesystem paths that don't exist on the phone |
| **Opening media with the system player** | `media_detail_page.dart:148-155`, `ai_home_page.dart:3829-3832` | `file:`/backend-URL hand-off to the OS shell |
| **MySQL data migration** | `settings_page.dart:392-424` → `Lever.moveMysqlData` | Stops `mysqld`, robocopies the instance dir |
| **Theming for Windows fonts** | `theme.dart:25-26`, `:40` (`Microsoft YaHei UI`) | Falls back gracefully but is tuned for Windows |

**Product-level judgement already recorded in the repo:** `docs/android-client-plan.md:27-31` states the intended split — "移动端不直连 ComfyUI，也不装 Python"; the Android client should talk to the **desktop's** backend over LAN/USB, and `docs/android-client-plan.md:38-44` lists the current readiness (API ✅, remote bind ⚠️ opt-in via `COMFYHUB_ALLOW_REMOTE`, CORS ✅ configurable, **auth ❌ none**, tool-per-endpoint concept ❌ missing).

---

## 4. Android readiness

### 4.1 What exists under `android/` today — a stock template plus a real build migration

Files (excluding build output): `build.gradle.kts`, `settings.gradle.kts`, `gradle.properties`, `gradle/wrapper/*`, `gradlew`/`gradlew.bat`, `local.properties`, `viewer_android.iml`, `app/build.gradle.kts`, `app/src/{main,debug,profile}/AndroidManifest.xml`, `app/src/main/kotlin/com/example/viewer/MainActivity.kt`, `res/…` (launch backgrounds, five mipmap icon densities, `values/styles.xml`, `values-night/styles.xml`), `app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java`.

**It is a real, buildable Flutter app target — but a completely generic one.** There is no Android-specific Dart code, no second entrypoint, no `main_android.dart`, no flavor, no product-flavored manifest.

**`android/app/build.gradle.kts` (54 lines) — 100 % Flutter template, three unmodified TODOs:**
- `namespace = "com.example.viewer"`, `applicationId = "com.example.viewer"` (`:10`, `:21`) — **the default template id, with the template's own TODO comment still at `:20`**
- `minSdk = flutter.minSdkVersion`, `targetSdk = flutter.targetSdkVersion`, `compileSdk = flutter.compileSdkVersion` (`:11`, `:24-25`) — all delegated, none pinned
- `release { signingConfig = signingConfigs.getByName("debug") }` (`:34-39`) — **release is signed with the debug keystore; the TODO at `:36` is still unaddressed**
- `compileOptions` Java 17 (`:14-17`); Kotlin via AGP built-in, `jvmTarget = JVM_17` (`:46-50`)
- App label is the raw project name: `android:label="viewer"` (`AndroidManifest.xml:3`) — not "ComfyHub"

**`AndroidManifest.xml` (45 lines):** single `MainActivity`, `exported=true`, `launchMode=singleTop`, `taskAffinity=""`, `configChanges` allowing orientation/keyboard/screenSize/locale changes without restart (`:12`), `windowSoftInputMode="adjustResize"` (`:14`), Flutter embedding v2 (`:30-32`), one `<queries>` block for `ACTION_PROCESS_TEXT` (`:39-44`).

**Critically: there are NO `<uses-permission>` declarations at all.** Consequences:
- `INTERNET` is not declared in the main manifest. `file_picker`/other plugins often inject it via manifest merging, and Flutter's debug/profile manifests do add it — but **for a release build, talking to a LAN backend needs `android.permission.INTERNET` declared explicitly and verified in the merged manifest.**
- No `ACCESS_NETWORK_STATE`, no `READ_MEDIA_IMAGES`/`READ_MEDIA_VIDEO`/`READ_MEDIA_AUDIO` (needed on API 33+ to pick media), no cleartext-traffic exemption.
- **`usesCleartextTraffic` is not set.** Android 9+ (API 28+) blocks cleartext HTTP by default. The client's entire default URL set is `http://` (`10.0.2.2:8080`, `127.0.0.1:8080`, LAN IPs) — so **without a `network_security_config.xml` or `android:usesCleartextTraffic="true"`, every request will fail on a real device.** This is a hard blocker not mentioned in any doc.

**`android/gradle.properties` (25 lines):** `-Xmx8G -XX:MaxMetaspaceSize=4G` (`:1`), `android.useAndroidX=true` (`:2`), `android.builtInKotlin=true` (`:11`), `kotlin.incremental=false` (`:18`, workaround for the C:/D: drive-split cache bug), `android.newDsl=false` (`:25`).

**`android/settings.gradle.kts` (39 lines):** AGP `9.4.0` (`:22`), KGP `2.4.0` declared `apply false` purely to lift the classpath above Flutter's `>= 2.2.20` check (`:23-36`).

### 4.2 The migration doc confirms it was built and verified

`docs/android-agp9-builtin-kotlin-migration.md:376-384` records the measured result (2026-09-12): **`app-debug.apk` built successfully** (`√ Built build\app\outputs\flutter-apk\app-debug.apk`), no KGP warnings, `gradlew :app:kgpVersion` → `2.4.0`, `:app:printBuildVariants` → debug/release/profile, plus `flutter analyze` clean and `flutter test` 32 passing.

So: **the Android target builds a real APK — but it has never been treated as a shippable product.** No signing config, no app id, no permissions, no launcher label, no icon, no release verification (`flutter build apk --release` is not in the doc's verification list at `:356-374`; only `--debug`).

### 4.3 Plugin platform coverage — what actually breaks on Android

From `.flutter-plugins-dependencies`, per-platform registrations:

| Platform | Plugins registered |
|---|---|
| windows | `audioplayers_windows`, `jni`, `path_provider_windows`, `shared_preferences_windows`, `url_launcher_windows`, **`video_player_win`**, `windows_file_picker` |
| android | `android_file_picker`, `audioplayers_android`, `jni`, `jni_flutter`, `path_provider_android`, `shared_preferences_android`, `url_launcher_android` |
| ios | `audioplayers_darwin`, `file_picker_darwin`, `path_provider_foundation`, `shared_preferences_foundation`, `url_launcher_ios` |

`pubspec.yaml` dependency-by-dependency verdict for Android:

| Dependency | `pubspec.yaml` | Android verdict |
|---|---|---|
| `http ^1.6.0` | `:22` | ✅ pure Dart |
| `provider ^6.1.5` | `:23` | ✅ pure Dart |
| `shared_preferences ^2.5.5` | `:24` | ✅ `shared_preferences_android 2.4.28` |
| `file_picker ^12.3.0` | `:27` | ⚠️ **builds** (`android_file_picker 1.1.1`) but **`PlatformFile.path` is `null` for content-URI picks** — see below |
| `intl ^0.20.3` | `:30` | ✅ pure Dart |
| **`video_player_win` (path: `third_party/video_player_win`)** | `:37-38` | ❌ **DESKTOP-ONLY. No Android (or iOS/macOS/Linux/web) implementation exists.** Registered only under `windows:` in `.flutter-plugins-dependencies` |
| `audioplayers ^6.8.1` | `:39` | ✅ `audioplayers_android 5.3.0`; header comment at `audio_player_view.dart:4` explicitly says "Windows / Android / iOS 都支持" |
| `url_launcher ^6.3.2` | `:42` | ✅ `url_launcher_android 6.3.33` |
| `flutter_localizations` | `:16-17` | ✅ pure Dart |
| `cupertino_icons ^1.0.8` | `:19` | ✅ assets |

**The single hard plugin blocker is `video_player_win`.** It is imported unconditionally at `lib/widgets/win_video_view.dart:5`, which is imported by `media_detail_page.dart:14`. Because `win_video_view.dart` is compiled into every build, an Android build must still resolve the package — and a federated plugin with no Android implementation is fine at *compile* time (the platform interface just throws `MissingPluginException`/`UnimplementedError` at runtime), so **the Android build will succeed but video playback will not exist.** The code already anticipates this:

```dart
// lib/widgets/win_video_view.dart:25-31
static bool get supported {
  try { return Platform.isWindows; } catch (_) { return false; }
}
```
```dart
// lib/pages/media_detail_page.dart:429-434
child: WinVideoView.supported
    ? WinVideoView(url: url, posterUrl: posterUrl)
    : _PreviewFallback(message: '当前平台不支持内嵌视频播放', …)
```

So on Android: **all video playback degrades to a placeholder.** Audio still works (`audioplayers_android`), images still work. A real Android port needs a `video_player`/`media_kit` Android implementation, or an "open externally" affordance. Note also that `WinVideoView` is imported **unconditionally** — a truly clean port would need conditional imports or a platform-dispatching wrapper.

Other desktop-only facts that will silently degrade rather than fail to build:

| Concern | Evidence | Android behaviour |
|---|---|---|
| Video **poster frames** | `media_thumb.dart:49-55`, `api_client.dart:233-236` — server extracts frame 1 via the **Windows thumbnail pipeline** | Server-side; on a phone talking to a desktop backend, posters still work. If the backend ever ran on Android, `GET /api/media/{id}/poster` would 204 |
| Thumbnails | `media_thumb.dart:227` `Image.network` | ✅ fine |
| Directory picking for import | `settings_page.dart:321`, `:374` `FilePicker.getDirectoryPath` | Android SAF returns a content URI tree, not a filesystem path — **this whole card is meaningless on a phone** (it configures the desktop's ComfyUI paths) |
| Attachment upload | `ai_api_client.dart:307-318` `MultipartFile.fromPath` with the comment `桌面端 file_picker 给的就是路径` (`:304-306`) | **Broken**: `file_picker` on Android with the default (bytes-not-cached) config gives `path == null` for `content://` URIs. The existing guard (`ai_home_page.dart:3183-3193`) catches this and shows `'这些文件读不到本地路径，已跳过：…'` (`ai_workspace_store.dart:1276-1280`) — i.e. **attachments cannot be sent from Android today.** Fix requires `withData: true`/`withReadStream: true` and a bytes/stream upload path |
| Media upload from gallery | `upload_sheet.dart:96-99` `.map((f) => f.path).whereType<String>()` | Same failure; error text literally says `'无法读取文件路径（Web 端暂不支持上传）'` (`upload_sheet.dart:103`) |
| Opening folders | `ai_home_page.dart:4997` `Uri.file(path, windows: true)` | No file manager; falls back to clipboard (`:4354-4359`) |
| Fonts | `theme.dart:40` `Platform.isWindows ? 'Microsoft YaHei UI'` | Returns `null` on Android → Flutter's default (Roboto/Noto). Chinese renders via the system fallback; **not verified** |

---

## 5. Desktop-only coupling — full enumeration with `file:line`

### 5.1 Process launching / PowerShell (the biggest block: 644 lines, `lib/core/backend_launcher.dart`)

The client **shells out to PowerShell** to manage the whole local stack. This is not incidental — it is the app's startup path (`app.dart:153-163` calls `launcher.ensureRunning()` from a post-frame callback).

| file:line | Code / behaviour |
|---|---|
| `backend_launcher.dart:29-41` | `_startProcess` → `Process.start(executable, arguments, workingDirectory:…, runInShell: false, mode: mode)` |
| `backend_launcher.dart:196` | `if (kIsWeb \|\| !Platform.isWindows) return null;` — shell resolution is Windows-only |
| `backend_launcher.dart:198-201` | Hardcoded candidates `r'C:\Program Files\PowerShell\7\pwsh.exe'`, `r'C:\Program Files\PowerShell\7-preview\pwsh.exe'` |
| `backend_launcher.dart:212` | Searches `PATH` for `pwsh.exe` then `powershell.exe` |
| `backend_launcher.dart:220-221` | Fallback `r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'` |
| `backend_launcher.dart:192-193` | `_isPwsh` — `shellPath.toLowerCase().endsWith('pwsh.exe')` |
| `backend_launcher.dart:230` | `_addOwnerArgs` — `if (kIsWeb \|\| !Platform.isWindows) return false;` then `args..add('-OwnerPid')..add('$pid')` |
| `backend_launcher.dart:286-288` | `_fail('当前平台不支持自动启动本地服务（仅 Windows 桌面端）。')` |
| `backend_launcher.dart:300-304` | Warns about PowerShell 7, suggests `winget install --id Microsoft.PowerShell` |
| `backend_launcher.dart:308-315` | Builds `-NoProfile -ExecutionPolicy Bypass -File <scripts\comfyhub.ps1> up` |
| `backend_launcher.dart:316-320` | Appends `-SkipBuild` when `_hasBuiltServer()` |
| `backend_launcher.dart:336-340` | `processStarter(shell, args, workingDirectory: script.parent.parent.path)` |
| `backend_launcher.dart:348-355` | Parses child stdout/stderr with `Utf8Decoder(allowMalformed: true)` + `LineSplitter` |
| `backend_launcher.dart:366-388` | Polls `/api/health` for up to 90 s (`waitDeadlineOverride` for tests, `:99`) |
| `backend_launcher.dart:396-436` | `runAction(action)` — same `-NoProfile -ExecutionPolicy Bypass -File` shape for `restart`/`down`/`logs` |
| `backend_launcher.dart:451-482` | `releaseOnExit()` → `comfyhub.ps1 release`, 8 s timeout (`:95`) |
| `backend_launcher.dart:488-498` | `disarmOwnerWatch()` → `comfyhub.ps1 unwatch` |
| `backend_launcher.dart:505-516` | `armOwnerWatch()` → `comfyhub.ps1 watch -OwnerPid $pid` |
| `backend_launcher.dart:519-547` | `_buildServiceArgs` — `if (kIsWeb \|\| !Platform.isWindows) return null;` |
| `backend_launcher.dart:575-613` | `moveMysqlData(newDir)` → `scripts\mysql.ps1 move -DataDir <newDir>` |
| `backend_launcher.dart:616-640` | `_exec` — a **second** direct `Process.start` at `:619` |
| `backend_launcher.dart:551-554` | `_spawnDetached` uses the injectable `processStarter` |

### 5.2 Windows path assumptions / project-root discovery

| file:line | Code |
|---|---|
| `backend_launcher.dart:148-175` | `detectProjectRoot` — walks ≤8 parents of `File(Platform.resolvedExecutable).parent.path` and `Directory.current.path` looking for `scripts\comfyhub.ps1` (`:167`) |
| `backend_launcher.dart:163`, `:184`, `:214`, `:245` | `Platform.pathSeparator` used to build Windows-style paths |
| `backend_launcher.dart:185` | `File('$root${sep}scripts${sep}comfyhub.ps1')` |
| `backend_launcher.dart:242-254` | `_hasBuiltServer()` probes `server\bin\comfy-hub-server.bat` and `server\build\install\comfy-hub-server\bin\comfy-hub-server.bat` — two Windows layouts |
| `settings_store.dart:87-106` | `_baseUrlFromPortFile` reads `<root>\.run\api.json` |
| `settings_store.dart:113-145` | `_projectRootCandidates` — same `scripts/comfyhub.ps1` walk; `:118` `Platform.environment['COMFYHUB_PROJECT_ROOT']` |
| `settings_store.dart:125`, `:128`, `:131`, `:135` | `Platform.resolvedExecutable`, `Directory.current`, `Platform.pathSeparator`, `File(...)` |
| `backend_launcher.dart:207-211` | `Platform.environment['PATH'].split(';')` — **semicolon** is the Windows separator |
| `settings_page.dart:915` | Hint text `r'例如 D:\myProject\FlutterProject\viewer'` |
| `settings_page.dart:952` | Hint text `r'例如 D:\mysql-data\comfyhub'` |
| `settings_page.dart:1091` | Hint text `r'例如 D:\Comfy-Desktop\ComfyUI-Shared\output'` |
| `app.dart:107-111` | Comment citing `windows\runner\flutter_window.cpp:50-70` for the exit path |

### 5.3 Platform branching

| file:line | Code |
|---|---|
| `backend_launcher.dart:196` | `!Platform.isWindows` → no shell |
| `backend_launcher.dart:230` | `!Platform.isWindows` → no `-OwnerPid` |
| `backend_launcher.dart:286` | `!Platform.isWindows` → hard fail with a user-facing message |
| `backend_launcher.dart:520` | `!Platform.isWindows` → no release/watch args |
| `backend_launcher.dart:581` | `!Platform.isWindows` → no MySQL move |
| `backend_launcher.dart:132` | `_isLoopback` — `127.0.0.1`/`localhost`/`::1` (used to short-circuit the port probe, `:118`) |
| `settings_store.dart:67` | `Platform.isAndroid` → `http://10.0.2.2:8080` (**the only Android-aware line in `lib/`**) |
| `settings_store.dart:61` | `kIsWeb` → `127.0.0.1:8080` |
| `win_video_view.dart:25-31` | `WinVideoView.supported` → `Platform.isWindows` |
| `win_video_view.dart:49-52` | `'当前平台不支持内嵌播放（仅 Windows）'` |
| `theme.dart:1`, `:40` | `import 'dart:io' show Platform;` → `if (Platform.isWindows) return 'Microsoft YaHei UI';` |
| `backend_launcher.dart:149` | `if (kIsWeb) return null;` |

### 5.4 File / folder dialogs and OS hand-off

| file:line | Code | Android impact |
|---|---|---|
| `upload_sheet.dart:73-77` | `FilePicker.pickFiles(type: FileType.custom, allowedExtensions: […21 extensions…])` | works, but `f.path` is null |
| `upload_sheet.dart:96-99` | `.map((f) => f.path).whereType<String>()` | **silently drops files** |
| `upload_sheet.dart:101-105` | `'无法读取文件路径（Web 端暂不支持上传）'` | misleading message on Android |
| `settings_page.dart:321-323` | `FilePicker.getDirectoryPath(dialogTitle: '选择 ComfyUI 的 output 目录…')` | SAF tree URI ≠ path |
| `settings_page.dart:334` | `store.api.importCaptureFolder(dir: dir, limit: 500)` | sends a Windows path to the backend |
| `settings_page.dart:374` | `FilePicker.getDirectoryPath(dialogTitle: '选择 ComfyUI 的输出目录')` | same |
| `settings_page.dart:459-467` | `_openFolder` → `launchUrl(Uri.directory(path))` | no file manager |
| `settings_page.dart:600` | `_openFolder('${_health!['storageDir']}')` — "打开" the storage dir | desktop-only |
| `settings_page.dart:688` | `_openFolder(launcher.projectRoot!)` | desktop-only |
| `ai_home_page.dart:3178` | `FilePicker.pickFiles(dialogTitle: '选择要发给 AI 的附件')` | attachment picker |
| `ai_home_page.dart:3184-3189` | `final path = f.path; … picked.add((name: f.name, path: path))` | path-based contract |
| `ai_home_page.dart:4341-4351` | `_openSkillsFolder` → `_openFolder` → `'打不开文件管理器，请手动打开：$path'` | degrades to a snackbar |
| `ai_home_page.dart:4354-4359` | `Clipboard.setData(ClipboardData(text: path))` — fallback | works |
| `ai_home_page.dart:4996-5006` | `Uri.file(path, windows: true)` then `launchUrl(..., LaunchMode.externalApplication)` | **`windows: true` is hardcoded** |
| `ai_home_page.dart:3829-3832` | `launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication)` for attachment originals | hands an HTTP URL to the OS |
| `media_detail_page.dart:148-155` | `canLaunchUrl(uri)` then `launchUrl(uri, LaunchMode.externalApplication)` for `m.fileUrl` | "用系统播放器打开" |
| `markdown_view.dart:16`, `:286` | `launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication)` for links | ✅ portable |
| `api_client.dart:292-294` | `http.MultipartFile.fromPath('files', path)` | path-based gallery upload |
| `ai_api_client.dart:310-313` | `http.MultipartFile.fromPath('files', f.path, filename: f.name)` | path-based attachment upload |

### 5.5 Window management / tray / desktop UX

| file:line | Code | Note |
|---|---|---|
| `app.dart:114-124` | `AppLifecycleListener(onExitRequested: _onExitRequested)` + `Future<AppExitResponse>` | Desktop window-close hook. On Android this fires on back-out/app-kill and would try to stop the (nonexistent) local services |
| `app.dart:149` | `AppLifecycleListener(onExitRequested: _onExitRequested)` | registered in `initState` |
| `app.dart:330` | `MediaQuery.sizeOf(context).width >= 900` → `NavigationRail` vs `NavigationBar` | responsive ✅ |
| `app.dart:343-359` | `NavigationRail` with `labelType: NavigationRailLabelType.all` | desktop-shaped but degrades |
| `ai_home_page.dart:256-267` | `wide = width >= 900`, `showPanel = wide && width >= 1200` | responsive ✅ |
| `ai_home_page.dart:308-327` | Three-column (`SizedBox(width: 240)` / `Expanded` / `SizedBox(width: 260)`) vs `Drawer` | responsive ✅ |
| `ai_home_page.dart:330-340` | `showModalBottomSheet(height: … * 0.6)` | mobile-shaped branch ✅ |
| `media_detail_page.dart:214` | `constraints.maxWidth >= 1000` → two-column | responsive ✅ |
| `media_detail_page.dart:638` | `constraints.maxWidth >= 460` → two-column key/value | responsive ✅ |
| `media_thumb.dart:256-275` | `MouseRegion(cursor: SystemMouseCursors.grab/grabbing)` | mouse cursor; harmless |
| `zoomable_image_view.dart:44` | Comment: "**滚轮**：以鼠标位置为中心放大 / 缩小（`InteractiveViewer` 自带…)" | desktop-first interaction design; pinch-zoom still works via `InteractiveViewer` |
| `zoomable_image_view.dart:256-257` | `MouseRegion(cursor: …)` | mouse-only affordance |
| `gallery_page.dart:326-327` | `onSecondaryTapDown: (details) => _showThumbMenu(context, m, details.globalPosition)` | **right-click only** — Android has no equivalent; long-press at `:318-321` is already used for selection mode |
| `media_detail_page.dart:271-272` | `onSecondaryTapDown` → `_showPreviewMenu` | right-click only |
| `context_menu.dart` (whole file, 265 lines) | Custom context-menu host (`ContextMenuScope` / `showAppContextMenu`) mounted at `gallery_page.dart:46` and `media_detail_page.dart:100` | desktop-only interaction; the underlying actions are reachable elsewhere (AppBar menus) |
| `app_menu.dart` (192) | `AppMenuButton` dropdown | ✅ portable |
| `win_video_view.dart:264-266` | `SingleActivator(LogicalKeyboardKey.escape): DismissIntent()` | Esc-to-exit fullscreen |
| `ai_home_page.dart:1881-1882`, `:3000` | `LogicalKeyboardKey.enter` / `numpadEnter` in `Shortcuts`/`Actions` | hardware-keyboard send |
| `ai_home_page.dart:1884` | `HardwareKeyboard.instance.isShiftPressed` | Shift+Enter newline |
| `ai_home_page.dart:554-559` | `highlightColor` / `splashColor` / `hoverColor` | mouse hover tuning |
| Numerous | `Tooltip` widgets (e.g. `ai_home_page.dart:2265`, `3662`, `3791`; `media_detail_page.dart:721`) | no hover on touch |
| `ai_home_page.dart:2781-2783` | Empty-state text promises **drag-and-drop**: `'也可以把文件拖进来当附件。'` | **`DragTarget`/`Draggable` do not exist anywhere in `lib/` (0 matches)** — the promise is unfulfilled even on desktop |
| `ai_home_page.dart:392` | `'--no-enable-impeller'` mentioned in a comment | Windows rendering note |

---

## 6. Data volume the client deals with

### 6.1 Endpoints returning large payloads

| Endpoint | Client size control | Server cap | Notes |
|---|---|---|---|
| `GET /api/media` | `size: 24` default (`api_client.dart:210`), user-settable 8–120 via `pageSize` (`settings_page.dart:670-672`, `settings_store.dart:199`) | `MediaRepo.kt:164` `coerceIn(1, 200)` | Each `MediaAsset` (`models.dart:286-391`) embeds the linked prompt summary: `promptTitle`, `promptPositive`, `promptNegative`, `checkpoint`, `seed`, `promptTags[]` — and the server joins those straight from the prompt row (`MediaRepo.kt:14`, `db/schema.sql:190-191`), so a page of 120 media carries 120 copies of the prompt text |
| `GET /api/prompts` | `size: 20` default (`api_client.dart:113`) | `PromptRepo.kt:146` `coerceIn(1, 200)` | Each `Prompt` embeds full `positivePrompt`, `negativePrompt`, `loras[]`, `extraParams` map, `tags[]` (`models.dart:87-176`). Both prompt columns are **`MEDIUMTEXT` — up to 16 MB per row** (`db/schema.sql:21-22`, `PromptRepo.kt:13-18`) |
| `GET /api/prompts/{id}/media` | **none** — returns *all* media for that prompt (`api_client.dart:156-159`) | none (`MediaRepo.kt:193-198`) | Called from `prompt_detail_page.dart` |
| `GET /api/tags` | `limit: 500` hardcoded default in the client (`api_client.dart:165`) | `TagRoutes.kt:32` default `500`, `TagRepo.kt:61` `coerceIn(1, 5000)`; **no offset/cursor at all** | Full vocabulary fetch, no pagination, on every `refreshTags()` (`library_store.dart:218-237`) |
| `GET /api/stats` | n/a | n/a | Counts only |
| `GET /api/ai/providers` | **none** | `SELECT *` whole provider table, no limit (`AiRepo.kt:29-31`) | Includes credential *status*, never the value |
| `GET /api/ai/providers/{id}/models` | **none** | no limit (`AiRepo.kt:87-89`) | 69 built-in models in practice (`AGENTS.md` §10) |
| `GET /api/ai/conversations` | **none** | `AiConversationRepo.kt:98,107` `limit = 200`, **not exposed as a query parameter**, **no offset** | Fetched at `ai_workspace_store.dart:919`, `:1169`, `:1962`. Each row runs a `message_count` subquery |
| **`GET /api/ai/conversations/{id}/messages`** | **none — no pagination at all** (`ai_api_client.dart:399-402`) | `AiConversationRepo.kt:161-193` — `SELECT * FROM ai_messages WHERE conversation_id = ? ORDER BY seq, created_at`, then a second unbounded query for **all** `ai_message_parts` | Each `AiMessage` carries `text` **plus** `parts[]` (reasoning blocks, tool calls, tool results with `jsonPayload`) — `ai_models.dart:772-816`. `text` is **`MEDIUMTEXT` (16 MB/row)** and every part's `json_payload` is inlined (`db/schema.sql:277,299`). Called on every conversation open (`:1093`) and after every completed run (`:1943`). **This is the single highest-risk payload in the product** |
| `GET /api/capture/status` | n/a | `recent[]` is **hardcoded to the last 20** runs (`ComfyCapture.kt:615`, `CaptureRepo.kt:206-212`) — not client-controllable | Polled from settings page and `ai_home_page.dart:4086` |
| `GET /api/capture/jobs` | **none** | `submitter.submissions(10)` fixed (`CaptureRoutes.kt:64`) | Polled every **1.5 s** while active, **6 s** idle (`ai_workspace_store.dart:682-689`), plus one extra unawaited refresh per `tool.completed` (`:1534`) |
| `POST /api/capture/import` | client sends `limit: 500` (`settings_page.dart:334`) | default 200, `ComfyCapture.kt:451` `coerceIn(1, 5000)` | Bulk import |
| `POST /api/media/upload` | client sends all selected paths in one multipart request | **`formFieldLimit = maxUploadBytes`, 4096 MB default** (`MediaRoutes.kt:80`, `Config.kt:84`) | Multi-GB request body |
| `GET /api/prompts/{id}/workflow`, `/api/prompts/{id}/api-graph`, `/api/media/{id}/workflow` | n/a | raw `MEDIUMTEXT`/JSON text (`CaptureRoutes.kt:154-183`), 30 s client timeout (`api_client.dart:370`) | Loaded on demand in the workflow viewer |
| `GET /api/media/{id}/file` \| `/thumb` \| `/poster`, `GET /api/ai/attachments/{id}/file` | n/a | whole blob via `respondFile`; **`Range` supported** by the `PartialContent(maxRangeCount=20)` plugin (`Application.kt:513-515`) | `Image.network` / `NetworkImage` / `AudioPlayer` / the video player fetch these directly. `/thumb` returns **204 for non-image** kinds and `/poster` **204 for non-video** (`MediaRoutes.kt:239-279`) |

Server-side mitigations that already exist: Ktor `Compression` (`Application.kt:510`), tool results truncated to **32,000 chars** (`ToolRegistry.kt:229`, applied `:2004-2005`), memory capped at **8,000 chars** (`MemoryStore.kt:51`; prompt injection truncates to 4,000, `:65`), inline attachment budget **8 MB single / 20 MB total** (`AiAttachments.kt:165`, `AiDomain.kt:602,607`).

### 6.2 Pagination — exactly where it exists and where it does not

**Has pagination (page + size, server-enforced ≤ 200):**
- `/api/prompts` → `PromptRoutes.kt:40-41` defaults `page=1, size=20`; `PromptRepo.kt:146-147` clamps
- `/api/media` → `MediaRoutes.kt:56-57` defaults `page=1, size=24`; `MediaRepo.kt:164-165` clamps

UI: A/Pager: `prompts_page.dart:352-356` (`_Pager`), `gallery_page.dart:348-370` (prev/next + "第 N / M 页"). `LibraryStore` also self-heals out-of-range pages by refetching the last page (`library_store.dart:128-131`, `:189-192`).

**Has a `limit` but no offset (take-the-top-N only):**
- `/api/tags` (`limit` only)
- `/api/ai/conversations` (`limit = 200`, no offset — `AiConversationRepo.kt:98`)
- `/api/capture/workflows` (`ComfyWorkflowFiles.kt:179,246` `limit.coerceIn(1, MAX_FILES)`)

**Has NO pagination whatsoever:**
- **`/api/ai/conversations/{id}/messages`** — the single biggest payload risk. Full history + full `parts` for every message, every time you open a conversation or finish a run.
- `/api/prompts/{id}/media`
- `/api/ai/skills` (metadata only, small)
- `/api/ai/memory` (capped at 100 entries / 8,000 chars by `ai_models.dart:1241-1250`)

**Client-side full-table loops (deliberate "delete until empty" patterns):**
- `library_store.dart:413-425` `deleteUnlinkedPrompts` — loops `page: 1, size: 200` until empty, `max = 1000`
- `gallery_page.dart:520-532` `_clearUnlinked` — loops `page: 1, size: 200` until empty, unbounded
- `prompts_page.dart:416` / `prompts_page.dart:421` — `size: 200` / `size: 5`
- `tag_results_page.dart:51-52` — **`size: 100` for both prompts and media, no pagination, silently truncates at 100** for a tag with more items
- `upload_sheet.dart:395` — `size: 30` prompt picker

**Batch operations issue N sequential single-item requests** (documented as intentional, `library_store.dart:383-385`): `setPromptsFavorite` (`:387-392`), `addTagsToPrompts` (`:394-399`), `deletePrompts` (`:401-406`), `gallery_page.dart:468-471`, `:477-481`, `:493-496`. Selecting 200 items = 200 round trips.

### 6.3 Streaming

**Exactly one streaming endpoint, SSE:** `GET /api/ai/runs/{id}/events` (`ai_api_client.dart:459-496`; server `AiRoutes.kt:553-586`).

- Client framing is hand-rolled (`ai_api_client.dart:467-495`): handles blank lines, `:` comments, `event:`, `data:`; ignores `id:` and `retry:` entirely.
- **`after=<seq>` reconnect exists on both sides and is never used by the client** — the only call site passes no `after` (`ai_workspace_store.dart:1423`), so every subscription replays the run from event 0. Server supports it (`AiRoutes.kt:555`), including DB replay after a backend restart (`:561-567`), with a 15 s heartbeat (`:571-575`).
- **No WebSocket anywhere.** `grep` for `WebSocket` gives only the Kotlin/Java `java.net` `OutboundProxy` matches, not app traffic.
- All other "live" data is **polling**, not streaming:
  - ComfyUI jobs: `Timer.periodic` at 1.5 s / 6 s (`ai_workspace_store.dart:682-689`), plus an extra refresh per `tool.completed` (`:1534`)
  - Backend readiness: 2 s poll loop up to 90 s (`backend_launcher.dart:366-388`)

**Client-side SSE state is memory-only**, so a stream drop mid-run ends the turn: `catch (e) { error = '事件流中断：$e'; }` then re-sync from server (`ai_workspace_store.dart:1604-1613`).

---

## 7. Sync implications

### 7.1 Entities, and whether they can/should sync to Android

Database tables (from `db/schema.sql`): `prompts`, `tags`, `prompt_tags`, `media_assets`, `media_tags`, `capture_runs`, `app_settings`, `ai_providers`, `ai_models`, `ai_conversations`, `ai_messages`, `ai_message_parts`, `ai_attachments`, `ai_runs`, `ai_run_events`, `ai_tool_calls`.

| Entity | Table | Sync to Android? | Reasoning |
|---|---|---|---|
| **Prompts** | `prompts` + `prompt_tags` | ✅ **Yes — best candidate.** Text-only, small, high value offline | Full DTO in `models.dart:87-258`; `GET /api/prompts` already paginated |
| **Tags** | `tags`, `prompt_tags`, `media_tags` | ✅ Yes — tiny (id, name, category, color, `useCount`), needed to render prompt chips | `models.dart:10-42` |
| **Media metadata** | `media_assets` | ✅ Yes (metadata only) — needed to browse the gallery offline | `models.dart:286-391` |
| **Media binaries** | `storage/media/**`, `storage/thumbs/**` | ⚠️ Conditional — per-item opt-in (favourite / recent). Full sync is GBs | Bytes are served by `GET /api/media/{id}/file` with `Range` |
| **Conversations** | `ai_conversations` | ✅ Yes — `id`, `title`, `messageCount`, `archived`, `updated_at` | `ai_models.dart:516-547` |
| **Messages + parts** | `ai_messages`, `ai_message_parts` | ✅ Yes, but **requires pagination first** (see 6.2) | Ordered parts include reasoning/tool payloads — consider syncing only `text`+`attachment`+`tool_call` part types |
| **Attachments** | `ai_attachments` + `storage/ai-attachments` | ⚠️ Metadata yes; binaries only if offline viewing is required | `AiAttachment.toJson` (`ai_models.dart:957-963`) is already metadata-only |
| **Runs / run events** | `ai_runs`, `ai_run_events` | ❌ No — ephemeral. `ai_run_events` is a replay log keyed `(run_id, seq)` |  |
| **Tool calls** | `ai_tool_calls` | ❌ No — run-scoped audit trail |  |
| **Capture runs** | `capture_runs` | ❌ **Desktop-only** — `run_key` is a ComfyUI `prompt_id` or `file:<sha256>`, and `raw` holds the API node graph | `AGENTS.md` §10.1 |
| **ComfyUI config / paths** | `app_settings` (`capture.*`) | ❌ **Desktop-only** — contains absolute Windows paths (`outputDir`) | `models.dart:491-555` |
| **AI providers / models / credentials** | `ai_providers`, `ai_models` | ⚠️ Provider+model catalogue could sync read-only for picking a model; **credentials must NOT leave the desktop** | `ai_provider_settings_page.dart:12-15` — keys are DPAPI-encrypted, write-only, never echoed. `docs/inbound-auth-design.md:30` flags them as the top-value asset |
| **Skills** | `<storage>\ai\skills` + `<project>\skills\builtin` | ❌ Desktop-only (disk is the source of truth, deliberately uncached — `ai_api_client.dart:124`) |  |
| **Long-term memory** | `<storage>\ai\memory.md` | ⚠️ Optional — a flat newline-per-entry text file (≤100 entries, ≤8,000 chars). Cheap to sync, and it changes AI behaviour | `docs/android-client-plan.md:64` explicitly leaves this open |
| **NSFW settings** | `app_settings` (`ai.nsfw`) | ✅ Read-only mirror at minimum — drives system-prompt injection, so it must stay server-authoritative | `nsfw_store.dart:8-17` |

### 7.2 Stable identifiers / timestamps / revision columns already present

**Good news for sync — the schema is already timestamped nearly everywhere.**

`db/schema.sql` column-level evidence:

| Table | Primary key | `created_at` | `updated_at` | Other |
|---|---|---|---|---|
| `prompts` | `id` (`:50`) | `DATETIME(3)` `:46` + index `idx_prompts_created` `:52` | `DATETIME(3)` `:47` | `source`, `source_ref` |
| `tags` | `id` (`:71`) | `:69` | — | `UNIQUE KEY uk_tags_normalized (normalized)` (`:72`) → **name is a natural key** |
| `prompt_tags` | `(prompt_id, tag_id)` (`:83`) | — | — | composite PK is the natural key |
| `media_assets` | `id` (`:114`) | `:110` + `idx_media_created` `:117` | `:111` | **`sha256`** (content hash), `source`, `source_ref`, index `idx_media_source_ref` (`Migrate.kt:30`) |
| `media_tags` | `(media_id, tag_id)` (`:129`) | — | — |  |
| `capture_runs` | `id` (`:149`) | `:148` + `idx_capture_runs_created` `:151` | — | **`UNIQUE KEY uk_capture_runs_key (run_key)`** (`:150`) — idempotency key |
| `app_settings` | `k` (`:164`) | — | `DATETIME(3)` `:162` | key/value |
| `ai_providers` | `id` (`:220`) | `:217` | `:218` | **`revision BIGINT NOT NULL DEFAULT 1 COMMENT '乐观锁'`** (`:216`) — an explicit optimistic-lock column |
| `ai_models` | `(provider_id, model_id)` (`:247`) | `:244` | `:245` | composite natural key |
| `ai_conversations` | `id` (`:266`) | `:263` | `:264` + `idx_ai_conv_updated` (`:267`) + `idx_ai_conv_archived (archived, updated_at)` (`:268`) | **`updated_at` is already the sync cursor** — the list is `ORDER BY c.updated_at DESC` (`AiConversationRepo.kt:104`) |
| `ai_messages` | `id` (`:286`) | `:283` | `:284` | **`seq`** — monotonic per conversation, used as the ordering key (`AiConversationRepo.kt:163` `ORDER BY seq, created_at`) |
| `ai_message_parts` | `(message_id, ordinal)` (`:301`) | `:300` | — | ordered by `ordinal` (`AiConversationRepo.kt:173`) |
| `ai_attachments` | `id` (`:325`) | `:324` + `idx_ai_attachments_created` (`:326`) | — |  |
| `ai_runs` | `id` (`:355`) | `:352` | `:353` + `idx_ai_runs_conv (conversation_id, created_at)` (`:356`) |  |
| `ai_run_events` | **`(run_id, seq)`** (`:368`) | `:367` | — | **`seq` is the SSE resume cursor** (`AiRoutes.kt:555` `after`) |
| `ai_tool_calls` | `id` (`:393`) | `:392` | — |  |

**Summary of what sync can build on today:**
1. **Monotonic per-conversation cursor:** `ai_messages.seq` + `ai_message_parts.ordinal` — synchronising new messages is `WHERE seq > :lastSeq`.
2. **Global time cursor:** `created_at` indexed on `prompts`, `media_assets`, `ai_attachments`, `ai_runs`, `capture_runs`; `updated_at` indexed on `ai_conversations`.
3. **Content-addressed identity:** `media_assets.sha256` — the same value is already used for cross-source dedup (`README.md:35`), so a phone uploading the same file is naturally idempotent.
4. **Natural/composite keys** already exist for `tags` (normalised name), `prompt_tags`, `media_tags`, `ai_models` — these need no id remapping.
5. **An optimistic-lock column already exists on exactly one table** — `ai_providers.revision` (`schema.sql:216`). `prompts`, `media_assets`, `tags` have **no** `revision`/`version`/`etag` column, so last-writer-wins on `updated_at` is the only option without a schema change. Note that `prompts.updated_at` is declared `DEFAULT CURRENT_TIMESTAMP(3)` **without `ON UPDATE`** (`schema.sql:47-48`) — updates must set it explicitly (they do; `PromptRepo.kt:224` region).
6. **Idempotency keys already in place** for the two write-heavy ingest paths: `capture_runs.run_key` (unique) and `media_assets.sha256`.

### 7.3 Blocking gaps for sync / remote Android — all evidenced

| Gap | Evidence | Consequence |
|---|---|---|
| **No inbound authentication at all** | `docs/inbound-auth-design.md:36-44`: no `Authentication` plugin, `ktor-server-auth` not on the classpath, zero `authenticate(` matches repo-wide; the only `Authorization` matches are outbound calls | Opening the LAN (`COMFYHUB_ALLOW_REMOTE=1` → bind `0.0.0.0`, `Config.kt:55-57`) exposes API keys, file read/write, ComfyUI submission and private media to anyone on the Wi-Fi. This is a **hard prerequisite**, not a nice-to-have |
| **Default bind is loopback-only** | `docs/inbound-auth-design.md:42`; `Config.kt:57` | Android cannot reach the desktop without an explicit opt-in |
| **No conflict resolution metadata** | no `revision`/`etag` on `prompts`/`media_assets`/`tags` (`schema.sql:16-56`, `:92-124`) | Concurrent phone+desktop edits need WAL/CRDT or a schema addition |
| **No delta/tombstone endpoint** | no `?since=` anywhere on `/api/prompts`, `/api/media`, `/api/tags`, `/api/ai/conversations` | Full re-fetch per sync; deletions are invisible (a row that disappears from page 1 is indistinguishable from a row on page 3) |
| **Chat history unpaginated** | `ai_api_client.dart:399-402`; `AiConversationRepo.kt:161-193` | A long tool-using conversation (up to 16 tool steps / 48 calls per run, `ai_models.dart:1364-1365`) returns everything, every time |
| **No cleartext-HTTP allowance in the Android manifest** | `android/app/src/main/AndroidManifest.xml` has no `usesCleartextTraffic` and no `network_security_config`; all default URLs are `http://` | **Every request fails on Android 9+ until this is added** |
| **No `INTERNET` permission declared** | same file, zero `<uses-permission>` | Must be verified in the *merged* release manifest |
| **Attachment upload is path-based** | `ai_api_client.dart:304-313`, `ai_home_page.dart:3184-3189` | Android cannot send a picked file without switching to bytes/stream (or `withData: true`) |
| **`file_picker` directory picking is meaningless on Android** | `settings_page.dart:321`, `:374` | Those settings are about the desktop's ComfyUI install |
| **No offline store of any kind** | §2 — `shared_preferences` only | Any sync plan must first choose and add a local database (`drift`/`sqflite`/`isar`); `path_provider` is not even a direct dependency yet |
| **`video_player_win` has no Android implementation** | `.flutter-plugins-dependencies`; `pubspec.yaml:37-38`; `win_video_view.dart:25-31` | Video degrades to a placeholder |
| **App identity is still the template** | `applicationId = "com.example.viewer"` (`android/app/build.gradle.kts:21`) | Cannot ship to Play |
| **Release builds sign with the debug key** | `android/app/build.gradle.kts:38` | Not shippable |
| **No Android integration test / CI** | `docs/android-agp9-builtin-kotlin-migration.md:356-374` verifies only `flutter build apk --debug` + `flutter analyze` + `flutter test` | No release-build or on-device verification exists |

### 7.4 What the repo's own plan already decides (so the audit does not re-litigate it)

`docs/android-client-plan.md` is explicit and current:
- `:27-31` — **"移动端不直连 ComfyUI，也不装 Python"**; the desktop stays the capability host; the phone talks to the desktop backend over LAN/USB.
- `:38` — the API is already sufficient; `:39` remote bind is an existing opt-in; `:40` CORS is configurable; `:41` **auth is missing**; `:43` there is **no per-tool "which end can use this" concept** (`ToolCategory` only groups skills/files/comfy/memory/web/ask).
- `:64` — an open product question: **should the Android client cache conversations locally for offline history?** This audit's §2 answers the factual half of that: today it caches *nothing*, so this is a from-scratch decision, not a modification.
- `:110` — first mobile scope: conversation list / chat / prompt-type Skills / web tools / **read-only gallery**.
- `:131` — **"不为移动端另写一套后端"**: storage, DB, gallery and Skills stay authoritative on the desktop.

### 7.5 Additional schema facts that constrain any sync design

Measured from `db/schema.sql` / `db/migrate.sql` / `Migrate.kt` and cross-checked against the running database:

1. **No soft-delete / tombstone column exists on any table.** Every delete is a hard `DELETE` (`MediaRepo.kt:263-267`, `AiConversationRepo.kt:153-155`, `PromptRepo.kt` delete path). A client that syncs on `updated_at > :lastSync` **can never observe a deletion** — a deleted prompt simply stops appearing, indistinguishable from a prompt on a page it did not fetch. Delta sync needs either a soft-delete flag or a server-side change log; neither exists today.
2. **Eight tables have no `updated_at` at all**: `tags`, `prompt_tags`, `media_tags`, `capture_runs`, `ai_message_parts`, `ai_run_events`, `ai_tool_calls`, `ai_attachments` (see the column table above). Timestamp-cursor sync is impossible for them.
3. **Tag-only edits are invisible even to `prompts.updated_at`.** `POST /api/prompts/{id}/tags` writes only `prompt_tags` plus `tags.use_count` (`PromptRoutes.kt:94-97`) — it does not touch the `prompts` row, so `prompts.updated_at` (the natural prompt sync cursor) does not move when the user re-tags a prompt. Same for `media_tags`.
4. **`prompts.updated_at` is declared without `ON UPDATE` in `db/schema.sql:47-48`** while `media_assets.updated_at` has it (`:111-112`) — the two look symmetric in the DTOs (`models.dart:112-113`, `:311-312`) but are maintained differently. Any sync that trusts "`updated_at` always advances on change" is wrong for `prompts`.
5. **Schema drift:** `media_assets.nsfw` exists only in `Migrate.kt:33-36` — it is **missing from `db/schema.sql`**, even though the client reads and writes it (`models.dart:309`, `:374`; `api_client.dart:259`, `:269`). A fresh install gets the column from the auto-migration at startup (`Application.kt:74-76`), so it works — but the canonical schema file is out of date.
6. **`ai_attachments.sha256 CHAR(64)` has no unique index** (`db/schema.sql:321`) unlike `media_assets.sha256` (`:104,118`) — attachment dedup is not enforced at the DB level.
7. **Timestamps are millisecond-precision ISO-8601 strings**, produced by `Instant.toString()` over `DATETIME(3)` (`Db.kt:160-162`), e.g. `2026-09-23T02:50:14.707Z`. The Dart side parses them with `DateTime.tryParse(...)?.toLocal()` (`models.dart:781-784`), so **all client-side timestamps are in local time** — a sync design must keep the raw UTC string or lose the timezone offset.

### 7.6 Compression / transport facts relevant to syncing over Wi-Fi

- The backend installs Ktor `Compression` (`Application.kt:510`), so JSON responses can be gzipped — relevant because prompts and message parts are large text.
- `PartialContent(maxRangeCount = 20)` is installed (`Application.kt:513-515`), which is what makes video scrubbing work over HTTP Range (`MediaRoutes.kt:230-237`) — the Android video story will need this even after swapping the player plugin.
- Upload limits: `POST /api/media/upload` uses `formFieldLimit = maxUploadBytes` with a **4096 MB default** (`MediaRoutes.kt:80`, `Config.kt:84`), while `POST /api/ai/attachments` caps at **32 MB/file** (`AiAttachments.kt:157`).
- Server-side payload mitigations already in place: tool results truncated to 32,000 chars (`ToolRegistry.kt:229`, applied `:2004-2005`), memory capped at 8,000 chars (`MemoryStore.kt:51`), inline attachment budget 8 MB single / 20 MB total (`AiAttachments.kt:165`, `AiDomain.kt:602,607`).
- **CORS allow-list contains no Android origin** (`Application.kt:520-534` — only `localhost`/`127.0.0.1`/`[::1]` plus `COMFYHUB_CORS_ORIGINS`). Harmless for a native client, blocking for any browser-based client.

---

## 8. The RAM question: what the JVM + MySQL actually cost, and what removing them would save

This is the half of the brief's premise ("dropping the JVM+MySQL stack to save RAM") that the client audit alone cannot answer, so it was measured against the **live processes on this machine**. These are observations of the current run, not projections.

### 8.1 MySQL is a separate OS process, not embedded

- `mysqld.exe` is resolved from `<MySQLHome>\bin\mysqld.exe` (`scripts/mysql.ps1:167-170`) and started **detached from the PowerShell process tree** via WMI with `SW_HIDE` (`scripts/mysql.ps1:302-311`, `scripts/silent-process.ps1`).
- Connection string `jdbc:mysql://127.0.0.1:3307/comfy_hub` (`Config.kt:75-80`).
- Generated `my.ini` (`scripts/mysql.ps1:172-199`) sets `port=3307`, `bind-address=127.0.0.1`, `max_connections=200`, `max_allowed_packet=512M`, **`innodb_buffer_pool_size=256M`**, `innodb_flush_log_at_trx_commit=2`, `local_infile=0` (`:177-190`).
- **Measured:** PID 19964 owns `127.0.0.1:3307` with `WorkingSetSize = 510,320,640 B ≈ 487 MiB`. **A second `mysqld.exe` (PID 21168, ≈27 MiB, started ~9 s before the backend) is also alive while only 19964 holds the port** — recorded here as an observed anomaly, not interpreted.
- HikariCP pool: `maximumPoolSize = 12`, `minimumIdle = 2`, `connectionTimeout = 10 s` (`Db.kt:34-37`).

### 8.2 The backend JVM has no heap ceiling configured

- `server/build.gradle.kts:78-88`: `applicationDefaultJvmArgs = listOf("-Dfile.encoding=UTF-8", "-Djava.net.useSystemProxies=true")` — **two flags, no `-Xmx`.** These are baked into `DEFAULT_JVM_OPTS` of the generated `comfy-hub-server.bat`; the live process command line confirms exactly those two `-D` flags.
- Repo-wide `Xmx|Xms|JAVA_OPTS` grep finds only `server/gradle.properties:1` (`-Xmx1536m`, the **Gradle/Kotlin build daemon**, not the runtime) and `android/gradle.properties:1` (`-Xmx8G`, Android builds only). `scripts/server.ps1:280-290` launches with only `COMFYHUB_STORAGE`, `COMFYHUB_PORT`, `JAVA_HOME`, `PATH`.

**Measured live backend JVM (PID 20756, via `jcmd VM.flags` / `GC.heap_info`):**

| Quantity | Value |
|---|---|
| `-XX:MaxHeapSize` (JVM ergonomic default = 25 % of physical RAM) | **6,253,707,264 B ≈ 5.82 GiB** |
| `-XX:InitialHeapSize` | 394,264,576 B ≈ 376 MiB |
| G1 heap committed / used at sample time | ≈ **204 MiB** / ≈ **133 MiB** |
| Metaspace used / committed | ≈ 53 MiB / 53.5 MiB |
| `ReservedCodeCacheSize` | ≈ 240 MiB |
| **OS working set** | 360,181,760 B ≈ **343 MiB** |
| Host RAM | 23.29 GB total, 10.54 GB free |
| GC | G1 |

### 8.3 The honest RAM statement

**Combined observed steady-state footprint ≈ 0.83 GB** (343 MiB backend JVM working set + 487 MiB `mysqld` working set, plus a 27 MiB stray `mysqld`).

The important nuance for the "drop the JVM+MySQL stack" decision: **the 5.82 GiB number is a ceiling, not consumption.** Because no `-Xmx` is set, the JVM reserves a max heap of 25 % of physical RAM — ~5.8 GiB on this 24 GB host, but **only ~1 GiB on a 4 GB target device**. The *actual* live footprint is ~343 MiB and has been stable across the sampling. A cheap, non-architectural first move is therefore to set an explicit `-Xmx` (e.g. `-Xmx512m`) in `server/build.gradle.kts:80` and re-measure, before concluding that the JVM must be replaced to save RAM.

What the client audit contributes to this decision: **almost none of the client's 8,660 Dart lines care which backend serves the data.** `ApiClient` and `AiApiClient` are plain HTTP+JSON (`api_client.dart:22-26`, `ai_api_client.dart:11-15`) against ~95 method+path combos with hand-written DTOs. Replacing Kotlin+MySQL with an embedded store would not simplify the client at all — it would require **new client code** to replace the missing server features it currently depends on:

| Server feature the client relies on | Client call sites | Cost to reimplement client-side |
|---|---|---|
| Pagination + multi-field search (prompts, media) | `api_client.dart:103-129`, `:200-228` | Full query/filter/sort engine in Dart over a local table |
| SHA-256 dedup on upload/import | `upload_sheet.dart:119`, server `CaptureRepo.kt` | Hashing + dedup in Dart |
| Thumbnail + video-poster generation | `media_thumb.dart:46-55`, `api_client.dart:233-237` | **Image/video decode in Dart — the poster path today is the Windows Media Foundation thumbnail pipeline** (`media_thumb.dart:49`) |
| PNG-embedded prompt/workflow extraction | `settings_page.dart:320-345` | PNG chunk parsing in Dart |
| Auto-capture polling of ComfyUI `/history` | `api_client.dart:310-360` | The client would have to poll ComfyUI itself — i.e. move desktop-only coupling *into* the app |
| HTTP Range for video scrubbing | `Settings about` block `settings_page.dart:721`; `media_detail_page.dart:148` | Serve files locally with Range support |
| Tool loop / SSE event replay / approval gate | `ai_workspace_store.dart:1423`, `ai_api_client.dart:459` | The whole agent runtime |
| Credential storage (DPAPI, write-only) | `ai_api_client.dart:9-10`, `ai_provider_settings_page.dart:12-15` | Platform keystore |

**Conclusion for the decision:** the client is a thin REST consumer; dropping the JVM+MySQL stack is a *server-side* decision that leaves the client structurally unchanged but removes ~7 server capabilities the client currently leans on. The measurable RAM saving is ~0.8 GB steady-state, and part of the "problem" (the 5.8 GiB ceiling) is a one-line `-Xmx` fix rather than an architecture rewrite. The `docs/android-client-plan.md:27-31` position — keep the desktop backend as the capability host and have Android talk to it over LAN — is consistent with everything found here.

---

## 9. Quantified bottom line

| Question | Answer | Key evidence |
|---|---|---|
| How does the client talk to the backend? | Two hand-written `package:http` REST clients, ~95 method+path combos, one SSE endpoint | `api_client.dart`, `ai_api_client.dart` |
| How is the port found? | `SharedPreferences` override → `.run/api.json` → hardcoded fallback `127.0.0.1:8080` (`10.0.2.2:8080` on Android). **The real port is ephemeral: `COMFYHUB_PORT` defaults to `0`, and the live instance is on `:1986`** | `settings_store.dart:60-106`, `Config.kt:66-74`, `.run/api.json` |
| Does it persist state locally? | **Yes, but only 11 `SharedPreferences` keys** (settings + model memory + per-conversation drafts). **No database, no files, no cache** | `settings_store.dart:9-14`, `ai_workspace_store.dart:29-33` |
| Is there any offline mode? | **No.** Every list is re-fetched; the launch screen's "离线只看本地缓存" comment is wrong | `library_store.dart:102-109`, `ai_workspace_store.dart:919`, `:1093` |
| How much of the UI is portable? | **~7,000 of 8,660 Dart lines (≈81 %)** are pure REST + widgets. The desktop-tied ~1,650 lines are concentrated in `backend_launcher.dart` (644), the file/OS-handoff call sites, and the Win-only video widget | §3.1 / §3.2 |
| Does the Android target build a real app? | **It builds a real debug APK, but it is an unshipped template**: `com.example.viewer`, debug signing, no permissions, no cleartext allowance, label "viewer" | `android/app/build.gradle.kts:20-39`, `AndroidManifest.xml`, `docs/android-agp9-builtin-kotlin-migration.md:376-384` |
| Desktop-only plugins with no Android equivalent? | **Exactly one: `video_player_win`.** Everything else has an Android implementation; `file_picker` builds but its path-based usage breaks | `.flutter-plugins-dependencies`, `pubspec.yaml:37-38` |
| Biggest single data-volume risk? | **`GET /api/ai/conversations/{id}/messages` — no pagination, returns all messages + all ordered parts (`text` is MEDIUMTEXT), fetched on every conversation open and after every run** | `ai_api_client.dart:399-402`, `AiConversationRepo.kt:161-193`, `ai_workspace_store.dart:1093`, `:1943`, `db/schema.sql:277,299` |
| Best sync foundation? | `ai_messages.seq` (per-conversation monotonic), `ordinal` parts, indexed `created_at`/`updated_at`, `media_assets.sha256`, composite natural keys — and one existing `revision` optimistic-lock column (`ai_providers`) | `db/schema.sql:46-47`, `:110-111`, `:150`, `:216`, `:263-264`, `:283-284`, `:301`, `:368` |
| Biggest sync blocker in the data model? | **No tombstone/soft-delete anywhere, and no `updated_at` on 8 tables** — a `updated_at > lastSync` client can never see deletions, and tag-only edits don't even move `prompts.updated_at` | §7.5 items 1-3; `PromptRoutes.kt:94-97` |
| Hardest blocker overall? | **No inbound auth** (prerequisite for opening the LAN), plus **no `usesCleartextTraffic`/`INTERNET`** in the Android manifest, plus **no local database** for the sync target | `docs/inbound-auth-design.md:36-44`, `AndroidManifest.xml`, §2 |
| What does the JVM+MySQL actually cost? | **≈0.83 GB working set** (343 MiB JVM + 487 MiB `mysqld`). The JVM's max heap is **unconfigured**, so it reserves 25 % of RAM (~5.8 GiB here) — a ceiling, not usage | §8.1-8.3, `server/build.gradle.kts:78-88`, `scripts/mysql.ps1:177-190` |

| Question | Answer | Key evidence |
|---|---|---|
| How does the client talk to the backend? | Two hand-written `package:http` REST clients, ~95 method+path combos, one SSE endpoint | `api_client.dart`, `ai_api_client.dart` |
| How is the port found? | `SharedPreferences` override → `.run/api.json` → hardcoded `127.0.0.1:8080` (`10.0.2.2:8080` on Android) | `settings_store.dart:60-106` |
| Does it persist state locally? | **Yes, but only 11 `SharedPreferences` keys** (settings + model memory + per-conversation drafts). **No database, no files, no cache** | `settings_store.dart:9-14`, `ai_workspace_store.dart:29-33` |
| Is there any offline mode? | **No.** Every list is re-fetched; the launch screen's "离线只看本地缓存" comment is wrong | `library_store.dart:102-109`, `ai_workspace_store.dart:919`, `:1093` |
| How much of the UI is portable? | **~7,000 of 8,660 Dart lines (≈81 %)** are pure REST + widgets. The desktop-tied ~1,650 lines are concentrated in `backend_launcher.dart` (644), the file/OS-handoff call sites, and the Win-only video widget | §3.1 / §3.2 |
| Does the Android target build a real app? | **It builds a real debug APK, but it is an unshipped template**: `com.example.viewer`, debug signing, no permissions, no cleartext allowance, label "viewer" | `android/app/build.gradle.kts:20-39`, `AndroidManifest.xml`, `docs/android-agp9-builtin-kotlin-migration.md:376-384` |
| Desktop-only plugins with no Android equivalent? | **Exactly one: `video_player_win`.** Everything else has an Android implementation; `file_picker` builds but its path-based usage breaks | `.flutter-plugins-dependencies`, `pubspec.yaml:37-38` |
| Biggest single data-volume risk? | **`GET /api/ai/conversations/{id}/messages` — no pagination, returns all messages + all ordered parts, fetched on every conversation open and after every run** | `ai_api_client.dart:399-402`, `AiConversationRepo.kt:161-193`, `ai_workspace_store.dart:1093`, `:1943` |
| Best sync foundation? | `ai_messages.seq` (per-conversation monotonic), `ordinal` parts, indexed `created_at`/`updated_at`, `media_assets.sha256`, composite natural keys — and one existing `revision` optimistic-lock column (`ai_providers`) | `db/schema.sql:46-47`, `:110-111`, `:150`, `:216`, `:263-264`, `:283-284`, `:301`, `:368` |
| Hardest blocker? | **No inbound auth** (prerequisite for opening the LAN) plus **no `usesCleartextTraffic`/`INTERNET`**, and **no local database** for the sync target | `docs/inbound-auth-design.md:36-44`, `AndroidManifest.xml`, §2 |
