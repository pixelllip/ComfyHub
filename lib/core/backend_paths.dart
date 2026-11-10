/// **「根目录在哪、后端 exe 在哪」—— 全 App 唯一的一份解析。**
///
/// 以前这件事住在 `scripts\backend.ps1`（`Resolve-RustExe`）与
/// `backend_launcher.dart` 两处，靠"两边都写一遍、别写歪"维持。2026-09-30 起
/// **App 自己直接起后端**（不再经过 pwsh），所以 Dart 侧成了唯一真源 ——
/// `scripts\backend.ps1` 只剩 e2e / dev 那条旁路，它的候选顺序是**本文件的镜像**
/// （AGENTS §7 的纪律：改一边必须改另一边）。
///
/// # 两种布局（AGENTS §7）
///
/// | | 源码树（开发） | Release 产物 / 发布包 |
/// | --- | --- | --- |
/// | 根目录 | 从 exe 往上找 `server-rs\Cargo.toml` 或 `db\schema.sqlite.sql` | `viewer.exe` 所在目录本身 |
/// | 后端 exe | `<根>\server-rs\target\{release,debug}\comfyhub-server-rs.exe` | `<viewer.exe 同级>\backend\comfyhub-server-rs.exe` |
/// | 数据库 | `<根>\.db\comfyhub.db` | 同一份约定（可写数据都在根目录内，便携） |
///
/// Release 布局把后端当成 **App 自己的资产**（`windows\CMakeLists.txt` 在 release
/// 装配时 `cargo build --release` 并把它装到 `backend\`），所以"往上找根目录"这条
/// 在发布包里几乎用不到 —— 但那一步仍然要有：用户可能把 `viewer.exe` 挪出去单跑。
///
/// # 三条纪律
///
///  1. **`COMFYHUB_SERVER_EXE` 始终最高优先**：它是 `AGENTS §12` 里"把后端映像挪到
///     工作区外"的唯一开关（写限制按**完整性标签 + 父子继承**判，与谁启动、什么档位无关）；
///  2. **找不到就如实回 null**，由调用方给人话（"跑一次 `cargo build --release`"
///     或"设 `COMFYHUB_SERVER_EXE`"）—— 绝不猜一个路径去起；
///  3. **根目录判据要认两种布局**，别只按源码树的路径写：发布包里根目录 = 包目录。
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import 'library_identity.dart';

/// 后端可执行文件名（两边布局一致；`windows\CMakeLists.txt` 与 `packaging\manifest.json` 也按它找）
const String kServerExeName = 'comfyhub-server-rs.exe';

/// 环境变量覆盖（`AGENTS §12` 的那个开关）
const String kServerExeEnv = 'COMFYHUB_SERVER_EXE';

/// 一次布局解析的结果
@immutable
class BackendLayout {
  const BackendLayout({required this.root, this.exe, this.exeSource});

  /// 运行时根目录（`.db` / `storage` / `.run` 都在它下面）
  final String root;

  /// 后端 exe 的完整路径；null = 没找到（调用方给修法）
  final String? exe;

  /// 这个 exe 是从哪个候选找到的（给人看，排查"起的是哪一份"）
  final String? exeSource;

  bool get hasExe => exe != null;

  @override
  String toString() => 'BackendLayout(root=$root, exe=$exe, from=$exeSource)';
}

/// 这个目录**像不像**运行根目录。
///
/// 判据刻意用"只读、且两种布局都有"的东西，任何一个命中就算：
///   · `server-rs\Cargo.toml`  —— 源码树（开发）
///   · `db\schema.sqlite.sql`  —— 两种布局都有（DDL 是给人看的副本）
///   · `backend\comfyhub-server-rs.exe` —— Release 产物（后端是 App 自己的资产）
bool looksLikeRoot(String dir) {
  if (dir.isEmpty) return false;
  return _isSourceRoot(dir) || _isBundleRoot(dir);
}

/// **源码树判据：`server-rs\Cargo.toml`，或者"有 `db\schema.sqlite.sql` 但没有 `backend\exe`"。**
///
/// ⚠ 为什么不能只看 `db\schema.sqlite.sql`（2026-10-11 踩到的坑）：那份 DDL 是
/// **构建时拷进产物目录**的，所以 `build\windows\x64\runner\Debug\` 里也有一份 ——
/// 单看它会把产物目录误认成源码树，于是"往上找源码树"那一步被它截住，
/// Debug 版 App 又回到"用自己的空库 + 旧后端 exe"（界面报 HTTP 400）。
///
/// 判据拆成两半，靠"后端 exe 在不在旁边"区分：
///   · `Cargo.toml` 在 → 源码树（只存在于真正的源码树，不会出现在任何产物里）；
///   · 只有 schema、**没有** `backend\exe` → 源码树（有人只拷了 DDL 出来）；
///   · schema **和** `backend\exe` 都在 → **产物目录**（后端是 App 自己的资产），
///     这一条交给 [`_isBundleRoot`]，调用方会继续往上找真正的根。
bool _isSourceRoot(String dir) =>
    _has([dir, 'server-rs', 'Cargo.toml']) ||
    (_has([dir, 'db', 'schema.sqlite.sql']) && !_isBundleRoot(dir));

/// Release 产物 / 发布包根：后端就在它旁边的 `backend\` 里。
bool _isBundleRoot(String dir) => _has([dir, 'backend', kServerExeName]);

bool _has(List<String> parts) {
  try {
    return File(parts.join(Platform.pathSeparator)).existsSync();
  } catch (_) {
    return false;
  }
}

/// 从 [from]（默认 exe 所在目录 / 当前目录）往上找运行根目录；找不到返回 null。
///
/// ⚠ **显式给了 [from] 就只听它的**（用户手填的"项目根目录"）：那时连它自己都找不到
/// 就**原样回它** —— 用户可能刚从别处拷过来、还没来得及 build，报"找不到根目录"
/// 只会比"按他说的试一次"更让人困惑。
String? detectRoot({String? from}) {
  if (kIsWeb) return null;

  Iterable<String> starts() sync* {
    if (from != null && from.isNotEmpty) {
      yield from;
    } else {
      try {
        yield File(Platform.resolvedExecutable).parent.path;
      } catch (_) {}
      try {
        yield Directory.current.path;
      } catch (_) {}
    }
  }

  for (final start in starts()) {
    final hit = _searchUp(start);
    if (hit != null) return hit;
    if (from != null && from.isNotEmpty) return from;
  }
  return null;
}

String? _searchUp(String start) {
  try {
    var dir = Directory(start);
    String? bundleHit;
    for (var depth = 0; depth < 8; depth++) {
      // ⚠ **源码树优先于产物根**（2026-10-11 踩到的坑）。
      //
      // 场景：`flutter build windows --debug` 会把后端装进
      // `build\windows\x64\runner\Debug\backend\`，于是那个产物目录**命中了产物判据**、
      // 自己成了"根" —— 结果是 Debug 版 App 去用它自己那份空库
      // （`build\...\Debug\.db\comfyhub.db`，0 条产物）和那份**旧的后端 exe**
      // （不认识新参数 → 界面报 HTTP 400），而用户的真库一直在项目根下。
      //
      // 所以这里**不立刻返回**：先把产物命中记下来，继续往上找源码树根
      // （`server-rs\Cargo.toml` / `db\schema.sqlite.sql`）；找到就用它。
      // 整条链上没有源码树时才回退到产物根 —— 那时它是真的发布包/产物目录。
      if (_isSourceRoot(dir.path)) return dir.path;
      if (bundleHit == null && _isBundleRoot(dir.path)) bundleHit = dir.path;
      final parent = dir.parent;
      if (parent.path == dir.path) break;
      dir = parent;
    }
    return bundleHit;
  } catch (_) {
    // 路径不存在 / 权限不够：当没找到，交给上层报人话
  }
  return null;
}

/// 按顺序列出"后端 exe 可能在哪"（**顺序就是优先级**，别随手调）。
///
///  1. `COMFYHUB_SERVER_EXE`          —— 显式覆盖（把映像挪出受限区 / 自己放的一份安装）
///  2. `<exe 同级>\backend\`          —— **Release 产物**：后端是 App 自己的资产
///  3. `<根>\backend\`                —— 同上（`viewer.exe` 被挪出去、根还是老根时）
///  4. `<根>\server\`                 —— 旧发布包布局（兼容，`AGENTS §7` 那张表）
///  5. `<根>\server-rs\target\release\` —— **源码树优先用 release**
///  6. `<根>\server-rs\target\debug\`   —— 没编过 release 时用 debug
///
/// `env` 是留给用例的缝隙（读 [kServerExeEnv]），生产走 [Platform.environment]。
List<MapEntry<String, String>> serverExeCandidates(
  String root, {
  Map<String, String>? env,
}) {
  final sep = Platform.pathSeparator;
  final e = env ?? Platform.environment;
  final out = <MapEntry<String, String>>[];

  final explicit = (e[kServerExeEnv] ?? '').trim();
  if (explicit.isNotEmpty) out.add(MapEntry(kServerExeEnv, explicit));

  try {
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    out.add(
      MapEntry(
        'App 自带 backend\\',
        [exeDir, 'backend', kServerExeName].join(sep),
      ),
    );
  } catch (_) {}

  out
    ..add(MapEntry('backend\\', [root, 'backend', kServerExeName].join(sep)))
    ..add(
      MapEntry('server\\（旧发布包布局）', [root, 'server', kServerExeName].join(sep)),
    )
    ..add(
      MapEntry(
        'server-rs\\target\\release',
        [root, 'server-rs', 'target', 'release', kServerExeName].join(sep),
      ),
    )
    ..add(
      MapEntry(
        'server-rs\\target\\debug',
        [root, 'server-rs', 'target', 'debug', kServerExeName].join(sep),
      ),
    );
  return out;
}

/// 解析一次布局：根目录 + 后端 exe。
///
/// 根目录解析不出来时返回 null（那是"这台机器上没装/没构建 ComfyHub"，
/// 不是"后端文件丢了"，两种毛病要给两句不同的人话）。
BackendLayout? resolveLayout({String? projectRoot, Map<String, String>? env}) {
  final root = detectRoot(from: projectRoot);
  if (root == null) return null;
  for (final candidate in serverExeCandidates(root, env: env)) {
    try {
      if (File(candidate.value).existsSync()) {
        return BackendLayout(
          root: root,
          exe: candidate.value,
          exeSource: candidate.key,
        );
      }
    } catch (_) {
      // 单个候选探不动不影响别的候选
    }
  }
  return BackendLayout(root: root);
}

/// 运行期数据目录（两种布局同一份约定，AGENTS §13：内容一律在自己的文件夹里）。
String dbFileOf(String root) =>
    [root, '.db', 'comfyhub.db'].join(Platform.pathSeparator);

/// 库文件所在的目录（`<根>\.db`）—— 起后端前要先建好它
/// （SQLite 不替我们建目录，缺了报的错完全指不到这里）。
String dbParentOf(String root) => [root, '.db'].join(Platform.pathSeparator);

String storageDirOf(String root) =>
    [root, 'storage'].join(Platform.pathSeparator);

// ---------------------------------------------------------------------------
//  数据目录（2026-11-06）：库 + 产物 / AI 数据可以搬出程序目录
// ---------------------------------------------------------------------------
//
// 用户要"把数据放到别的盘"。判据只有一条：**数据目录里装什么**——
//   · `<数据目录>\.db\comfyhub.db`（库）
//   · `<数据目录>\storage\`（产物 / 缩略图 / 附件 / Skills / 记忆 / MCP 运行时）
//
// 程序目录（运行根目录）里留下的东西**不动**：`.run\`（端口记录）、`backups\`
// （库备份）、`comfyui\`（工具写白名单）、`skills\builtin\`、`app-support\`（偏好）。
// 这样 App / 脚本 / ComfyUI 捕获扩展读端口文件的位置不变，而大文件可以搬到别的盘。
//
// 覆盖值存在 `<根>\data-dir.json`，**不放偏好里**：偏好文件在 `<根>\app-support`，
// 而数据目录可能在可移动盘上；把"数据在哪"写进数据自己里面，等于拔了盘就忘了路。

/// 数据目录覆盖文件的名字（`<根>\data-dir.json`）。
const String kDataDirFileName = 'data-dir.json';

String dataDirFileOf(String root) =>
    [root, kDataDirFileName].join(Platform.pathSeparator);

/// 读"数据目录被指到哪儿了"；没设 / 文件坏了都回 `null`（= 用运行根目录）。
///
/// ⚠ **读不动 ≠ 没设过**：这两个结论对用户的意思完全相反（一个"数据在别的盘"，
/// 一个"数据就在这儿"）。所以读失败一定要留一行日志；界面上会看到"库里空空的"，
/// 排查时第一件事就是看这行。
String? readDataDirOverride(String root) {
  try {
    final file = File(dataDirFileOf(root));
    if (!file.existsSync()) return null;
    final value = jsonDecode(file.readAsStringSync());
    if (value is Map) {
      final raw = value['dataDir'];
      if (raw is String && raw.trim().isNotEmpty) return raw.trim();
    }
    debugPrint('[paths] data-dir.json 里没有可用的 dataDir，按运行根目录处理');
  } catch (e) {
    // 半截 JSON / 正在被替换：当"这次没读到"，走默认目录 —— 但要说出来
    debugPrint('[paths] data-dir.json 读不动，这次按运行根目录处理：$e');
  }
  return null;
}

/// 这次真正用的数据目录：覆盖值优先，否则就是运行根目录。
///
/// ⚠ 指针写的那个目录**未必真的是数据目录**：旧版允许把 `…\storage` 选成数据目录，
/// 于是库落进了 `storage\.db`，指针却停在上一级 —— 照指针找就会读到另一份旧库
/// （2026-11-10 用户报的"复制出去的构建对话不见了 / 角色卡是旧版"）。
/// 校正规则只有一份，在 [resolveDataDir] 里。
String dataDirOf(String root) => resolveDataDir(root).dataDir;

/// 写 / 清数据目录覆盖。`dataDir` 为 null 或空 = 恢复默认（删掉覆盖文件）。
///
/// **先写 `.tmp` 再 rename**：中途崩掉只会留下一个半截临时文件，
/// `readDataDirOverride` 读不到它就是"没设过"，不会把 App 指到一个不存在的目录。
bool writeDataDirOverride(String root, String? dataDir) {
  final file = File(dataDirFileOf(root));
  try {
    if (dataDir == null || dataDir.trim().isEmpty) {
      if (file.existsSync()) file.deleteSync();
      return true;
    }
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(jsonEncode({'dataDir': dataDir.trim()}));
    tmp.renameSync(file.path);
    return true;
  } catch (_) {
    return false;
  }
}

/// 数据目录里的库文件（`<数据目录>\.db\comfyhub.db`）。
String dataDbFileOf(String dataDir) =>
    [dataDir, '.db', 'comfyhub.db'].join(Platform.pathSeparator);

/// 数据目录里的库目录（`<数据目录>\.db`）。
String dataDbParentOf(String dataDir) =>
    [dataDir, '.db'].join(Platform.pathSeparator);

/// 数据目录里的产物 / AI 数据目录（`<数据目录>\storage`）。
///
/// 这是**标准布局**的落点。磁盘上实际是哪一层由 [resolvedStorageDirOf] 判 ——
/// 旧版可能把 `storage` 文件夹本身当数据目录用过，也可能被手动挪成扁平布局。
String dataStorageDirOf(String dataDir) =>
    [dataDir, 'storage'].join(Platform.pathSeparator);

/// 产物目录里**算内容**的那几个子目录（识别布局 / 迁移共用）。
const List<String> kStorageContentDirs = <String>[
  'media',
  'thumbs',
  'ai',
  'ai-attachments',
  'ai-thumbs',
];

/// 这个目录像不像一个**产物目录**（下面有 `media` / `thumbs` / `ai` … 且不是空的）。
///
/// 判据刻意用"实际有内容"，不用"目录存在"：`storage\media` 这类空壳会被后端 /
/// 启动代码建出来，拿空壳当布局判据会把真正的产物目录挡在外面。
bool looksLikeStorageDir(String dir) {
  for (final name in kStorageContentDirs) {
    final sub = Directory([dir, name].join(Platform.pathSeparator));
    try {
      if (sub.existsSync() && sub.listSync(followLinks: false).isNotEmpty) {
        return true;
      }
    } catch (_) {
      // 读不动的目录当"没有"：布局判断不该被一次权限问题整个打断
    }
  }
  return false;
}

/// **产物目录实际在哪** —— 三种布局都认（2026-11-09 用户报的 bug）。
///
/// | 布局 | 长什么样 | 来历 |
/// | --- | --- | --- |
/// | 标准 | `<数据目录>\storage\media` | 默认，也是迁移写出来的样子 |
/// | 双层 | `<数据目录>\storage\storage\media` | 旧版把 `storage` 文件夹选成数据目录 |
/// | 扁平 | `<数据目录>\media` | 用户把里层 `storage` 的内容挪到数据目录本身 |
///
/// 顺序是"标准 → 双层 → 扁平"：标准布局有内容时永远优先，免得两套目录并存时
/// 读到旧的那一套。三种都不像就回标准路径，由调用方去建。
String resolvedStorageDirOf(String dataDir) {
  final canonical = dataStorageDirOf(dataDir);
  if (looksLikeStorageDir(canonical)) return canonical;

  final nested = [canonical, 'storage'].join(Platform.pathSeparator);
  if (looksLikeStorageDir(nested)) return nested;

  if (looksLikeStorageDir(dataDir)) return dataDir;
  return canonical;
}

/// 产物是不是直接放在数据目录里（扁平布局）。
bool isFlatStorageLayout(String dataDir) =>
    _samePath(resolvedStorageDirOf(dataDir), dataDir);

// ---------------------------------------------------------------------------
//  数据目录校正（2026-11-10）：指针写的那一级，未必真的是数据目录
// ---------------------------------------------------------------------------
//
// 现场（用户报"复制出去的构建对话不见了、画廊加载不出来、角色卡是旧版"）：
//
//   · `data-dir.json` 写的是 `D:\dist\ComfyHub`；
//   · 真正的库却在 `D:\dist\ComfyHub\storage\.db\comfyhub.db`（57 个会话）；
//   · 指针那一级也有一份 `D:\dist\ComfyHub\.db\comfyhub.db`，但那是**上一份构建
//     残留的旧库**（3 个会话、2 张角色卡）。
//
// 于是 App 照着指针打开旧库 —— 数据明明在，界面却"什么都没有"。这不是猜出来的：
// `storage\` 里出现 `.db\comfyhub.db` 只有一种来历，就是**那个 `storage` 本身当过
// 数据目录**（旧版允许把 `…\storage` 选成数据目录，见 pitfalls 第 188 条）。正常布局
// 里 `storage` 只装产物，绝不会装库。
//
// 判据（都在这一处，界面 / 脚本 / 迁移共用）：
//   · 指针指的目录不在了 → 回运行根目录，并说出来（别悄悄开一个空库）；
//   · `<选中目录>\storage\` 本身是数据目录 → 以它为准；两边都有库时**更大的那份**
//     才可能是正在用的那份（残留那份通常只有几百 KB 到几 MB）。

/// 这个目录里有没有一份库（`<目录>\.db\comfyhub.db`）。
bool hasDataDb(String dir) {
  if (dir.isEmpty) return false;
  try {
    return File(dataDbFileOf(dir)).existsSync();
  } catch (_) {
    return false;
  }
}

/// `…\storage` **本身**是不是一个数据目录；是就返回它的路径，否则 null。
String? nestedDataDirOf(String dir) {
  if (dir.isEmpty) return null;
  final nested = dataStorageDirOf(dir);
  return hasDataDb(nested) ? nested : null;
}

/// 库 / 文件大小的"人话"写法（界面与日志共用一份，别各处自己算）。
String sizeText(int bytes) {
  if (bytes <= 0) return '空的';
  const kb = 1024;
  if (bytes < kb * 10) return '$bytes 字节';
  if (bytes < kb * kb) return '${(bytes / kb).round()} KB';
  final mb = bytes / (kb * kb);
  if (mb < 1024) return '${mb.toStringAsFixed(mb < 10 ? 1 : 0)} MB';
  return '${(mb / 1024).toStringAsFixed(1)} GB';
}

int _fileBytes(String path) {
  try {
    final file = File(path);
    return file.existsSync() ? file.lengthSync() : 0;
  } catch (_) {
    return 0;
  }
}

bool _dirExists(String path) {
  if (path.isEmpty) return false;
  try {
    return Directory(path).existsSync();
  } catch (_) {
    return false;
  }
}

/// 这次的数据目录是**怎么定下来的**（界面上按它说人话，别各拼一份）。
enum DataDirPick {
  /// 指针（或运行根目录）指的那一份 —— 用户显式选的，或者有身份且就是它。
  chosen,

  /// 按"上次用的那份库"的记录切回来的（复制出去的构建 / 换盘 / 目录改名）。
  remembered,
}

/// 一个"看着像数据目录"的候选（真的有一份库才算）。
///
/// ⚠ 候选只用来**报告**，不用来自动挑：哪份是"你的库"由身份（[DataDirResolution.usedLibraryId]）
/// 说了算，判断不出来时交给用户点一下 —— **绝不按大小 / 时间 / 层级去猜**
/// （2026-11-10 复查：写死盘符、"谁更大就选谁"这类规则只对某一台机器成立）。
@immutable
class DataDirCandidate {
  const DataDirCandidate({required this.path, required this.bytes});

  final String path;

  /// 库文件有多少字节（给人看的量级；读不到就是 0）。
  final int bytes;

  @override
  String toString() => 'DataDirCandidate($path, $bytes)';
}

/// 一次数据目录解析的完整结论（界面上的说明、启动日志都按它说，别各拼一份）。
@immutable
class DataDirResolution {
  const DataDirResolution({
    required this.root,
    required this.chosen,
    required this.dataDir,
    required this.storageDir,
    required this.dbFile,
    required this.pick,
    this.pointer,
    this.pointerMissing = false,
    this.storageMissing = false,
    this.candidates = const <DataDirCandidate>[],
    this.usedLibraryId,
    this.chosenLibraryId,
    this.knownLibraryId,
    this.knownLibraryPath,
    this.pointerHasOtherLibrary = false,
    this.rememberedMissing = false,
  });

  /// 运行根目录（`.run` / `backups` / `comfyui` 仍按它算）。
  final String root;

  /// 按指针（或运行根目录）选的目录 —— **校正之前**的那个。
  final String chosen;

  /// 这次真正用的数据目录（库和产物都在这儿）。
  final String dataDir;

  /// 产物目录实际在哪（标准 / 双层 / 扁平，见 [resolvedStorageDirOf]）。
  final String storageDir;

  /// 库文件的完整路径。
  final String dbFile;

  /// 这次是怎么定的（见 [DataDirPick]）。
  final DataDirPick pick;

  /// `data-dir.json` 里写的那个（null = 没设过，用运行根目录）。
  final String? pointer;

  /// 指针指的目录不在了，这次回落到运行根目录。
  final bool pointerMissing;

  /// 库在，产物目录却不在 —— 典型是"只复制了程序，没复制 storage"。
  final bool storageMissing;

  /// 这次考虑过的所有"像数据目录"的候选（按优先级，**只留真有库的**）。
  ///
  /// 只用于**报告**（界面 / 日志：让用户知道机器上还有哪些库），不参与挑选。
  final List<DataDirCandidate> candidates;

  /// 除了这次用的那份之外，还有哪些库。
  List<DataDirCandidate> get otherCandidates => candidates
      .where((c) => !_samePath(c.path, dataDir))
      .toList(growable: false);

  /// 这次用的那份库的身份（null = 老库还没身份）。
  final String? usedLibraryId;

  /// 指针 / 运行根目录那一份库的身份（null = 那里没有库，或老库没身份）。
  final String? chosenLibraryId;

  /// 偏好里记着"上次用的是哪一份"（null = 还没有记录）。
  final String? knownLibraryId;

  /// 偏好里记着的那个位置。
  final String? knownLibraryPath;

  /// 指针那一级有**另一份有身份的库**（用户显式选过的），这次尊重指针但要说出来。
  final bool pointerHasOtherLibrary;

  /// 有记录，但那份库在这台机器上找不到了。
  final bool rememberedMissing;

  /// 指针写的和实际用的不是同一个目录。
  bool get pointerDiffers => pointer != null && !_samePath(pointer!, dataDir);

  /// 这次用的是不是"上次那份库"（没有记录时算 true —— 无从比较，不该吓人）。
  bool get matchesRemembered =>
      knownLibraryId == null || usedLibraryId == knownLibraryId;

  /// 界面上要说的一句话（没有异常时是 null，横幅一个字都不占）。
  ///
  /// 顺序就是"用户最需要先知道什么"：打开的是另一份库 > 上次那份找不到 > 按记录
  /// 切回来了 > 指针失效 > 产物不在。
  ///
  /// ⚠ 别把"同一棵树里还有别的库"写进这里：那不是故障，天天弹横幅只会变成噪音
  /// （2026-11-10 用户截图：每次启动都顶着一条黄条）。它只出现在设置页的
  /// 「还发现另一份库」一节，由用户自己决定要不要换。
  String? get notice {
    if (pointerHasOtherLibrary) {
      return '这次打开的不是上次使用的那一份。上次使用的是 '
          '${knownLibraryPath ?? '上一份'}。如需换回，请点「换回上次那份」。';
    }
    if (rememberedMissing) {
      return '上次使用的那份数据已找不到，这次打开的是 $dataDir。';
    }
    if (pick == DataDirPick.remembered) {
      return '这次打开的是上次使用的那份，位置是 $dataDir。';
    }
    if (pointerMissing) {
      return '上次使用的存放位置已不存在，这次使用的是 $dataDir。';
    }
    if (storageMissing) {
      return '这个位置中只有对话记录，没有图片和视频。请把原来的图片文件夹一并复制过来。';
    }
    return null;
  }

  @override
  String toString() =>
      'DataDirResolution(root=$root, chosen=$chosen, dataDir=$dataDir, '
      'storage=$storageDir, pick=$pick, pointer=$pointer, '
      'ids(used=$usedLibraryId, chosen=$chosenLibraryId, known=$knownLibraryId))';
}

/// 解析这次真正用的数据目录（**唯一真源**：App、设置页、`scripts\backend.ps1`）。
///
/// # 判定顺序（改之前先读完这段）
///
/// 1. **指针 / 运行根目录** 定"想用哪儿"（`chosen`）；
/// 2. 如果偏好里记着"上次用的是哪一份库"（`knownLibraryId`）：
///    · `chosen` 就是那一份 → 用它；
///    · `chosen` 里是**另一份有身份的库** → 尊重用户显式指定的位置，但把
///      `pointerHasOtherLibrary` 亮出来（界面提醒 + 一键换回去）；
///    · `chosen` 里没有库、或者那份库**没有身份**（老数据）→ 听记录的：
///      先看记录里的路径还在不在，再在候选里按身份找；找到就用它（`remembered`）。
///      ⚠ 这一条就是"复制出去的构建照样打开你的库"「换盘 / 改名也不丢」的实现。
/// 3. **没有记录时就用 `chosen`**，别的候选只在 `notice` / 设置页里**列出来**让用户自己选。
///    ⚠ 这里**不许**按大小 / 修改时间 / 层级去猜"哪份是你的库"：那是拿某台机器的
///    情况当通用规则（2026-11-10 复查）；判据只能是**身份**，判断不了就问用户。
/// 4. 候选只包含**同一棵树里**的目录（`chosen` / 根目录 / 它们的 `storage` 层），
///    不做全盘搜索 —— 搜盘既慢又容易把别人的库当成你的。
DataDirResolution resolveDataDir(
  String root, {
  String? knownLibraryId,
  String? knownLibraryPath,
}) {
  final pointer = readDataDirOverride(root);

  var chosen = root;
  var pointerMissing = false;
  if (pointer != null) {
    if (_dirExists(pointer)) {
      chosen = pointer;
    } else {
      // 指针指向的目录不在了（换机器 / 拔了移动盘 / 手动删了）：回运行根目录。
      // ⚠ **不能**照着不存在的路径建一个空库 —— 那正是"数据明明在，界面却空了"。
      pointerMissing = true;
      debugPrint('[paths] data-dir.json 指的 $pointer 不在了，这次用运行根目录 $root');
    }
  }

  final chosenLibraryId = readLibraryId(chosen);
  final candidates = _dataDirCandidates(root: root, chosen: chosen);
  final known = (knownLibraryId ?? '').isEmpty ? null : knownLibraryId;

  var dataDir = chosen;
  var pick = DataDirPick.chosen;
  var pointerHasOtherLibrary = false;
  var rememberedMissing = false;

  if (known != null) {
    if (chosenLibraryId == known) {
      // 指针这一级就是上次那份库 —— 最正常的一条路
    } else if (chosenLibraryId != null && hasDataDb(chosen)) {
      // 指针里是另一份**有身份**的库：那是用户显式选过的位置，尊重它，只提醒。
      pointerHasOtherLibrary = true;
    } else {
      final remembered = _rememberedDataDir(
        id: known,
        path: knownLibraryPath,
        candidates: candidates,
      );
      if (remembered != null) {
        if (!_samePath(remembered, chosen)) {
          dataDir = remembered;
          pick = DataDirPick.remembered;
          debugPrint('[paths] 按"上次用的库"($known) 打开：$remembered（指针是 $chosen）');
        }
      } else {
        rememberedMissing = true;
        debugPrint('[paths] 上次用的库($known)找不到了，这次用 $chosen');
      }
    }
  }

  final others = candidates
      .where((c) => !_samePath(c.path, dataDir))
      .map((c) => c.path)
      .toList(growable: false);
  if (others.isNotEmpty) {
    // 只是报告，不换：换哪份由用户点（设置 → 数据目录）
    debugPrint('[paths] 同一棵树里还发现别的库：${others.join('、')}');
  }

  final storage = resolvedStorageDirOf(dataDir);
  final dbFile = dataDbFileOf(dataDir);
  // 库在、产物目录却整个不在：典型是"只复制了程序，没复制 storage"。
  // 只按"目录不存在"判 —— 刚装好的空 storage 是正常的，不该吓人。
  final storageMissing = hasDataDb(dataDir) && !_dirExists(storage);

  return DataDirResolution(
    root: root,
    chosen: chosen,
    dataDir: dataDir,
    storageDir: storage,
    dbFile: dbFile,
    pick: pick,
    pointer: pointer,
    pointerMissing: pointerMissing,
    storageMissing: storageMissing,
    candidates: candidates,
    usedLibraryId: readLibraryId(dataDir),
    chosenLibraryId: chosenLibraryId,
    knownLibraryId: known,
    knownLibraryPath: knownLibraryPath,
    pointerHasOtherLibrary: pointerHasOtherLibrary,
    rememberedMissing: rememberedMissing,
  );
}

/// 同一棵树里"可能是我们的数据目录"的那些位置（**只留真有库的**，按优先级）。
///
/// 为什么要把 `storage` 层也算进来：旧版允许把 `…\storage` 选成数据目录，于是库落在
/// `…\storage\.db`。它是不是你要的那一份，靠**身份**判，不靠层级。
List<DataDirCandidate> _dataDirCandidates({
  required String root,
  required String chosen,
}) {
  final out = <DataDirCandidate>[];
  void add(String dir) {
    if (dir.isEmpty || !hasDataDb(dir)) return;
    for (final existing in out) {
      if (_samePath(existing.path, dir)) return;
    }
    out.add(DataDirCandidate(path: dir, bytes: _fileBytes(dataDbFileOf(dir))));
  }

  void addTree(String base) {
    add(base);
    final storage = dataStorageDirOf(base);
    add(storage);
    add([storage, 'storage'].join(Platform.pathSeparator));
  }

  addTree(chosen);
  if (!_samePath(root, chosen)) addTree(root);
  return out;
}

/// 按身份找"上次那份库"：先看记录里的路径，再在候选里找。
String? _rememberedDataDir({
  required String id,
  required String? path,
  required List<DataDirCandidate> candidates,
}) {
  if (path != null && path.isNotEmpty && hasDataDb(path)) {
    if (readLibraryId(path) == id) return path;
  }
  for (final candidate in candidates) {
    if (readLibraryId(candidate.path) == id) return candidate.path;
  }
  return null;
}

String _pathLower(String path) {
  var value = path.replaceAll('/', Platform.pathSeparator);
  while (value.length > 1 && value.endsWith(Platform.pathSeparator)) {
    value = value.substring(0, value.length - 1);
  }
  return value.toLowerCase();
}

bool _samePath(String a, String b) => _pathLower(a) == _pathLower(b);

String runDirOf(String root) => [root, '.run'].join(Platform.pathSeparator);

/// 后端**实际**监听的端口记录（`.run\api.json`），后端自己写的，我们只读。
///
/// 读不到 / 格式坏了都回 null，**绝不抛**（它在启动与退出的关键路径上）。
Map<String, dynamic>? readPortFile(String root) {
  try {
    final file = File(
      [runDirOf(root), 'api.json'].join(Platform.pathSeparator),
    );
    if (!file.existsSync()) return null;
    final value = jsonDecode(file.readAsStringSync());
    if (value is Map) return value.cast<String, dynamic>();
  } catch (_) {
    // 文件正在被后端原子替换（写 .tmp → rename）/ 半截 JSON：都当"这次没读到"
  }
  return null;
}
