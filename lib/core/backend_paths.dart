/// **「根目录在哪、后端 exe 在哪」—— 全 App 唯一的一份解析。**
///
/// 解析逻辑只有这一份：`scripts\backend.ps1` 是它在脚本侧的**镜像**（仅服务 e2e / dev），
/// 候选顺序必须与本文一致（AGENTS §7：改一边必须改另一边）。
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
/// 在发布包里几乎用不到，但仍需保留：`viewer.exe` 可能被单独移动后运行。
///
/// # 三条纪律
///
///  1. **`COMFYHUB_SERVER_EXE` 始终最高优先**：它是 `AGENTS §12` 里"把后端映像挪到
///     工作区外"的唯一开关（写限制按**完整性标签 + 父子继承**判，与谁启动、什么档位无关）；
///  2. **找不到就如实回 null**，由调用方给出明确提示（"执行一次 `cargo build --release`"
///     或"设置 `COMFYHUB_SERVER_EXE`"）—— 不猜测路径启动；
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
/// ⚠ **显式传入 [from] 时只使用该路径**（手动填写的"项目根目录"）：即使校验未通过，
/// 也**原样返回它**，由调用方按该路径尝试。
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
      // ⚠ **源码树优先于产物根**。
      //
      // `flutter build windows --debug` 会把后端装进
      // `build\windows\x64\runner\Debug\backend\`，该产物目录同时命中产物判据、
      // 成为"根" —— 结果是 Debug 版 App 使用该目录下的空库
      // （`build\...\Debug\.db\comfyhub.db`，0 条产物）和旧的后端 exe
      // （不认识新参数 → 界面报 HTTP 400），而真实库位于项目根目录。
      //
      // 因此这里**不立即返回**：先记录产物命中，继续向上查找源码树根
      // （`server-rs\Cargo.toml` / `db\schema.sqlite.sql`）；找到后使用它。
      // 整条链上没有源码树时才回退到产物根。
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
//  数据目录（2026-11-06）：库 + 产物 / AI 数据可移出程序目录
// ---------------------------------------------------------------------------
//
// 判据只有一条：**数据目录包含什么** ——
//   · `<数据目录>\.db\comfyhub.db`（库）
//   · `<数据目录>\storage\`（产物 / 缩略图 / 附件 / Skills / 记忆 / MCP 运行时）
//
// 运行根目录中的内容不变：`.run\`（端口记录）、`backups\`（库备份）、`comfyui\`
// （工具写白名单）、`skills\builtin\`、`app-support\`（偏好）。因此 App / 脚本 /
// ComfyUI 捕获扩展读取端口文件的位置不变，大文件可放到其他磁盘。
//
// 覆盖值存放在 `<根>\data-dir.json`，**不放在偏好里**：偏好文件位于
// `<根>\app-support`，而数据目录可能位于可移动磁盘；把"数据在哪"写入数据自身，
// 移除磁盘后配置仍可读。

/// 数据目录覆盖文件的名字（`<根>\data-dir.json`）。
const String kDataDirFileName = 'data-dir.json';

String dataDirFileOf(String root) =>
    [root, kDataDirFileName].join(Platform.pathSeparator);

/// 读"数据目录被指到哪儿了"；未设置 / 文件损坏均返回 `null`（= 使用运行根目录）。
///
/// ⚠ **读取失败不等于未设置**：两种结论含义相反（"数据在其他磁盘"与"数据在此处"）。
/// 读取失败必须记录一行日志；界面会显示"库为空"，排查时首先查看该日志。
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

/// 本次实际使用的数据目录：覆盖值优先，否则为运行根目录。
///
/// ⚠ 指针指向的目录**未必是数据目录**：旧版允许把 `…\storage` 选为数据目录，
/// 库因此落在 `storage\.db`，而指针停留在上一级 —— 按指针查找会读到另一份旧库。
/// 校正规则只有一份，在 [resolveDataDir] 中。
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

/// **产物目录实际位置** —— 识别三种布局。
///
/// | 布局 | 形态 | 来源 |
/// | --- | --- | --- |
/// | 标准 | `<数据目录>\storage\media` | 默认布局，迁移后的形态 |
/// | 双层 | `<数据目录>\storage\storage\media` | 旧版把 `storage` 文件夹选为数据目录 |
/// | 扁平 | `<数据目录>\media` | 里层 `storage` 的内容被移动到数据目录本身 |
///
/// 顺序为"标准 → 双层 → 扁平"：标准布局有内容时优先，避免两套目录并存时读取旧数据。
/// 三种均不匹配时返回标准路径，由调用方创建。
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
//  数据目录校正（2026-11-10）：指针指向的层级未必是数据目录
// ---------------------------------------------------------------------------
//
// 已知形态：
//   · `data-dir.json` 指向 `<根>\dist\ComfyHub`；
//   · 实际库位于 `<根>\dist\ComfyHub\storage\.db\comfyhub.db`；
//   · 指针层级也存在 `<根>\dist\ComfyHub\.db\comfyhub.db`，但那是上一份构建残留的旧库。
//
// 按指针打开旧库时，数据仍在磁盘上，界面却显示为空。判据：`storage\` 下出现
// `.db\comfyhub.db` 只可能来自"该 `storage` 曾作为数据目录"（旧版允许把 `…\storage`
// 选为数据目录，见 pitfalls 第 188 条）。标准布局中 `storage` 只存放产物，不存放库。
//
// 判据集中在此处，界面 / 脚本 / 迁移共用：
//   · 指针目录不存在 → 返回运行根目录，并报告（不得静默创建空库）；
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

/// 本次数据目录的确定方式（界面根据该值生成说明，不在各处重复拼装）。
enum DataDirPick {
  /// 指针（或运行根目录）指向的那一份 —— 显式指定，或身份匹配。
  chosen,

  /// 按"上次用的那份库"的记录切回来的（复制出去的构建 / 换盘 / 目录改名）。
  remembered,
}

/// 一个"看着像数据目录"的候选（真的有一份库才算）。
///
/// ⚠ 候选只用于**报告**，不用于自动选择：库的归属由身份
/// （[DataDirResolution.usedLibraryId]）决定，无法判断时由界面手动选择 ——
/// **不得按大小 / 时间 / 层级猜测**（这类规则只在单台机器上成立）。
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
  /// 只用于**报告**（界面 / 日志显示机器上还有哪些库），不参与挑选。
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

  /// 指针层级存在**另一份有身份的库**（显式指定过），本次尊重指针并提示。
  final bool pointerHasOtherLibrary;

  /// 有记录，但那份库在这台机器上找不到了。
  final bool rememberedMissing;

  /// 指针写的和实际用的不是同一个目录。
  bool get pointerDiffers => pointer != null && !_samePath(pointer!, dataDir);

  /// 这次用的是不是"上次那份库"（没有记录时算 true —— 无从比较，不该吓人）。
  bool get matchesRemembered =>
      knownLibraryId == null || usedLibraryId == knownLibraryId;

  /// 界面提示文案（无异常时为 null，不占用横幅）。
  ///
  /// 顺序按优先级：打开的是另一份库 > 上次那份找不到 > 按记录切回 > 指针失效 > 产物缺失。
  ///
  /// ⚠ "同一棵树里还有别的库"不写入此处：它不属于故障，每次启动显示横幅会成为噪音。
  /// 该信息只出现在设置页的「还发现另一份库」一节，是否切换由界面决定。
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
/// 1. **指针 / 运行根目录** 确定目标位置（`chosen`）；
/// 2. 偏好中存在"上次使用的库"记录（`knownLibraryId`）时：
///    · `chosen` 即该库 → 使用它；
///    · `chosen` 中存在**另一份有身份的库** → 尊重显式指定的位置，同时设置
///      `pointerHasOtherLibrary`（界面提醒并提供换回入口）；
///    · `chosen` 中没有库，或库**没有身份**（旧数据）→ 按记录处理：
///      先检查记录中的路径是否存在，再在候选中按身份查找；找到则使用（`remembered`）。
/// 3. **没有记录时使用 `chosen`**，其他候选只在 `notice` / 设置页中**列出**，由界面选择。
///    ⚠ **不得**按大小 / 修改时间 / 层级猜测"哪份是目标库"：判据只能是**身份**，
///    无法判断时交由界面选择。
/// 4. 候选只包含**同一棵树内**的目录（`chosen` / 根目录 / 它们的 `storage` 层），
///    不做全盘搜索 —— 全盘搜索速度慢，且可能把其他库误认为目标库。
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
      // 指针中是另一份**有身份**的库：该位置由显式指定，尊重它并提示。
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
    // 仅报告，不切换；切换由设置页操作（设置 → 数据目录）
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
