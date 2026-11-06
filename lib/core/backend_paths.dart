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
List<MapEntry<String, String>> serverExeCandidates(String root,
    {Map<String, String>? env}) {
  final sep = Platform.pathSeparator;
  final e = env ?? Platform.environment;
  final out = <MapEntry<String, String>>[];

  final explicit = (e[kServerExeEnv] ?? '').trim();
  if (explicit.isNotEmpty) out.add(MapEntry(kServerExeEnv, explicit));

  try {
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    out.add(MapEntry('App 自带 backend\\', [exeDir, 'backend', kServerExeName].join(sep)));
  } catch (_) {}

  out
    ..add(MapEntry('backend\\', [root, 'backend', kServerExeName].join(sep)))
    ..add(MapEntry('server\\（旧发布包布局）', [root, 'server', kServerExeName].join(sep)))
    ..add(MapEntry('server-rs\\target\\release',
        [root, 'server-rs', 'target', 'release', kServerExeName].join(sep)))
    ..add(MapEntry('server-rs\\target\\debug',
        [root, 'server-rs', 'target', 'debug', kServerExeName].join(sep)));
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
        return BackendLayout(root: root, exe: candidate.value, exeSource: candidate.key);
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
String? readDataDirOverride(String root) {
  try {
    final file = File(dataDirFileOf(root));
    if (!file.existsSync()) return null;
    final value = jsonDecode(file.readAsStringSync());
    if (value is Map) {
      final raw = value['dataDir'];
      if (raw is String && raw.trim().isNotEmpty) return raw.trim();
    }
  } catch (_) {
    // 半截 JSON / 正在被替换：当"这次没读到"，走默认目录
  }
  return null;
}

/// 这次真正用的数据目录：覆盖值优先，否则就是运行根目录。
String dataDirOf(String root) => readDataDirOverride(root) ?? root;

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
String dataStorageDirOf(String dataDir) =>
    [dataDir, 'storage'].join(Platform.pathSeparator);

String runDirOf(String root) =>
    [root, '.run'].join(Platform.pathSeparator);

/// 后端**实际**监听的端口记录（`.run\api.json`），后端自己写的，我们只读。
///
/// 读不到 / 格式坏了都回 null，**绝不抛**（它在启动与退出的关键路径上）。
Map<String, dynamic>? readPortFile(String root) {
  try {
    final file = File([runDirOf(root), 'api.json'].join(Platform.pathSeparator));
    if (!file.existsSync()) return null;
    final value = jsonDecode(file.readAsStringSync());
    if (value is Map) return value.cast<String, dynamic>();
  } catch (_) {
    // 文件正在被后端原子替换（写 .tmp → rename）/ 半截 JSON：都当"这次没读到"
  }
  return null;
}
