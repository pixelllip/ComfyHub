/// 数据目录迁移（2026-11-06）。
///
/// 用户要把库 + 产物搬到别的盘（C: 满了 / 想放移动硬盘），这里负责三件事：
/// **量**（多大）、**查**（目标能不能用）、**搬**（复制 + 校验 + 落指针）。
///
/// # 搬什么
///
/// 只搬数据目录里的两样东西（与 `backend_paths.dart` 的约定一致）：
///
///   · `.db\comfyhub.db`（连同 `-wal` / `-shm`：后端停掉之后它们应当已经收尾，
///     但"应当"不是判据 —— 文件在就一起搬，缺了下次打开库会少最后几个事务）；
///   · `storage\`（产物 / 缩略图 / 附件 / Skills / 记忆 / MCP 运行时）。
///
/// **不搬**：`storage\tmp`（24 小时就清的中转文件）、`storage\app-support`
/// （偏好已经搬到 `<根>\app-support`，那是程序目录的东西，不跟着数据走）。
///
/// # 三条纪律
///
///  1. **只复制、不删源**：源目录原样留着，用户确认没问题后自己删（界面会问一次）。
///  2. **校验过才落指针**：每个文件都比对字节数；有一个对不上就当场失败、
///     **不写** `data-dir.json` —— 半份数据比不搬更糟。
///  3. **失败 / 取消都如实说**：目标目录里会留下已经拷完的部分，告诉用户它在哪、
///     可以删掉重来，别假装什么都没发生。
///
/// # 谁停后端
///
/// 这个模块**不碰后端**：调用方（设置页）必须先停掉本地服务，再调 [migrateDataDir]，
/// 成功之后用新的数据目录重新起。活着的 SQLite 连接握着 WAL 与页缓存，
/// 边跑边搬等于搬一份"可能差最后几个事务"的库。
library;

import 'dart:io';

import 'backend_paths.dart';

/// 一份数据目录的规模。
class DataDirUsage {
  const DataDirUsage({required this.bytes, required this.files});

  final int bytes;
  final int files;

  static const empty = DataDirUsage(bytes: 0, files: 0);

  @override
  String toString() => '$files 个文件 / $bytes 字节';
}

/// 目标目录检查的结果。
class DataDirCheck {
  const DataDirCheck({
    required this.ok,
    this.message = '',
    this.source = DataDirUsage.empty,
  });

  final bool ok;

  /// 不通过时给用户看的一句人话（通过时为空）。
  final String message;

  /// 当前数据目录的规模（给确认弹窗用）。
  final DataDirUsage source;
}

/// 迁移进度（界面画进度条用）。
class MigrationProgress {
  const MigrationProgress({
    required this.copiedFiles,
    required this.totalFiles,
    required this.copiedBytes,
    required this.totalBytes,
    required this.currentPath,
  });

  final int copiedFiles;
  final int totalFiles;
  final int copiedBytes;
  final int totalBytes;
  final String currentPath;

  double get fraction {
    if (totalBytes > 0) {
      return (copiedBytes / totalBytes).clamp(0.0, 1.0);
    }
    if (totalFiles > 0) return (copiedFiles / totalFiles).clamp(0.0, 1.0);
    return 1;
  }
}

/// 迁移结果。
class MigrationResult {
  const MigrationResult({
    required this.ok,
    required this.message,
    this.cancelled = false,
    this.pointerWritten = false,
  });

  final bool ok;

  /// 给用户看的一句人话（失败 / 取消时**必须**说清楚现场）。
  final String message;

  final bool cancelled;

  /// 指针文件写成功 = 下次起后端就会用新目录。
  final bool pointerWritten;
}

// ---------------------------------------------------------------------------
//  量
// ---------------------------------------------------------------------------

/// 算数据目录的大小（不含 `tmp` / `app-support`，与迁移搬的东西一致）。
Future<DataDirUsage> measureDataDir(String dataDir) async {
  final entries = await _collectEntries(dataDir);
  var bytes = 0;
  for (final entry in entries) {
    bytes += entry.length;
  }
  return DataDirUsage(bytes: bytes, files: entries.length);
}

class _Entry {
  _Entry(this.relative, this.source, this.length);

  /// 相对数据目录的路径分段（例如 `storage\media\a.png`）。
  final List<String> relative;
  final String source;
  final int length;
}

/// 列出"迁移要搬的文件"。读不动的目录**直接报错**，不静默跳过 ——
/// 少搬一个目录，用户看到的就是"产物少了一批"。
Future<List<_Entry>> _collectEntries(String dataDir) async {
  final entries = <_Entry>[];

  // 库：只认 comfyhub.db 与它的 -wal / -shm（同目录里还有 restore 标记 / 保险备份，
  // 那些属于恢复流程，不跟着搬）。
  final dbDir = Directory(dataDbParentOf(dataDir));
  if (dbDir.existsSync()) {
    final dbName = File(dataDbFileOf(dataDir)).uri.pathSegments.last;
    for (final entity in dbDir.listSync(followLinks: false)) {
      if (entity is! File) continue;
      final name = entity.uri.pathSegments.last;
      if (name != dbName && name != '$dbName-wal' && name != '$dbName-shm') {
        continue;
      }
      final stat = entity.statSync();
      entries.add(_Entry(['.db', name], entity.path, stat.size));
    }
  }

  // 产物 / AI 数据：跳过 tmp（中转）与 app-support（偏好，已经不在数据目录里了）。
  final storage = Directory(dataStorageDirOf(dataDir));
  if (storage.existsSync()) {
    await for (final entity in storage.list(
      recursive: true,
      followLinks: false,
    )) {
      if (entity is! File) continue;
      final relative = _relativeParts(dataDir, entity.path);
      if (relative.length >= 2 &&
          (relative[1].toLowerCase() == 'tmp' ||
              relative[1].toLowerCase() == 'app-support')) {
        continue;
      }
      final stat = entity.statSync();
      entries.add(_Entry(relative, entity.path, stat.size));
    }
  }

  return entries;
}

/// `D:\data\storage\media\a.png` → `['storage', 'media', 'a.png']`
List<String> _relativeParts(String base, String path) {
  final basePath = _normalize(base);
  final full = _normalize(path);
  if (full == basePath) return const [];
  final prefix = basePath.endsWith(Platform.pathSeparator)
      ? basePath
      : '$basePath${Platform.pathSeparator}';
  final rest = full.startsWith(prefix) ? full.substring(prefix.length) : full;
  return rest
      .split(Platform.pathSeparator)
      .where((part) => part.isNotEmpty)
      .toList();
}

String _normalize(String path) {
  var value = path.replaceAll('/', Platform.pathSeparator);
  while (value.length > 1 && value.endsWith(Platform.pathSeparator)) {
    value = value.substring(0, value.length - 1);
  }
  return value;
}

bool _samePath(String a, String b) =>
    _normalize(a).toLowerCase() == _normalize(b).toLowerCase();

bool _isInside(String child, String parent) {
  final c = _normalize(child).toLowerCase();
  final p = _normalize(parent).toLowerCase();
  return c.startsWith(
    p.endsWith(Platform.pathSeparator) ? p : '$p${Platform.pathSeparator}',
  );
}

// ---------------------------------------------------------------------------
//  查
// ---------------------------------------------------------------------------

/// 检查一个目标目录能不能当数据目录。**只读 + 一次写探针**，不动任何数据。
Future<DataDirCheck> checkDataDirTarget({
  required String target,
  required String current,
}) async {
  final trimmed = target.trim();
  if (trimmed.isEmpty) {
    return const DataDirCheck(ok: false, message: '先选一个目录。');
  }
  final source = await measureDataDir(current);

  if (_samePath(trimmed, current)) {
    return DataDirCheck(
      ok: false,
      message: '数据已经在这个目录里了，不用再搬一次。',
      source: source,
    );
  }
  if (_isInside(trimmed, current)) {
    return DataDirCheck(
      ok: false,
      message: '新目录不能放在当前数据目录里面，会一边搬一边把新文件也搬进去。',
      source: source,
    );
  }
  if (_isInside(current, trimmed)) {
    return DataDirCheck(
      ok: false,
      message: '新目录不能是当前数据目录的上一级，会一边搬一边把旧目录也搬进去。',
      source: source,
    );
  }

  // 目标里已经有一份 ComfyHub 数据：这不是"迁移"，是"换一份库"。
  // 允许它等于让用户以为在搬数据、实际切到了另一份库（产物还在旧目录）。
  final targetHasDb = File(dataDbFileOf(trimmed)).existsSync();
  final targetStorage = Directory(dataStorageDirOf(trimmed));
  final targetHasStorage =
      targetStorage.existsSync() &&
      targetStorage.listSync(followLinks: false).isNotEmpty;
  if (targetHasDb || targetHasStorage) {
    return DataDirCheck(
      ok: false,
      message:
          '这个目录里已经有一份 ComfyHub 的数据了。换过去等于换一份库，'
          '不是搬家 —— 要恢复旧数据请用「本地服务 → 备份与恢复」。',
      source: source,
    );
  }

  // 写探针：建目录 + 写一个文件再删掉（建不出目录 / 只读盘当场说清楚）。
  try {
    final dir = Directory(trimmed);
    if (!dir.existsSync()) dir.createSync(recursive: true);
    final probe = File(
      '${dir.path}${Platform.pathSeparator}.comfyhub-write-test',
    );
    probe.writeAsStringSync('ok');
    probe.deleteSync();
  } catch (e) {
    return DataDirCheck(
      ok: false,
      message: '这个目录写不进去（$e）。换一个能写的目录，或先处理磁盘权限。',
      source: source,
    );
  }

  // 目标就是运行根目录 = "搬回默认位置"，允许；但根目录里若有 `.db` / `storage`
  // 残留（上面已经拦过），这里只剩正常的程序文件，不拦。

  return DataDirCheck(ok: true, source: source);
}

// ---------------------------------------------------------------------------
//  搬
// ---------------------------------------------------------------------------

/// 把数据从 [source] 复制到 [target]，校验通过后把 `<根>\data-dir.json` 指过去。
///
/// **调用前必须先停掉本地服务**（见文件头）。[isCancelled] 每搬一个文件问一次，
/// 返回 true 就停手（已经拷完的部分留在目标目录，不写指针）。
Future<MigrationResult> migrateDataDir({
  required String source,
  required String target,
  required String root,
  void Function(MigrationProgress progress)? onProgress,
  bool Function()? isCancelled,
}) async {
  final List<_Entry> entries;
  try {
    entries = await _collectEntries(source);
  } catch (e) {
    return MigrationResult(ok: false, message: '读当前数据目录失败（$e）。旧数据没动。');
  }

  // 有未完成的库恢复：那份标记里写的是**旧路径**，搬走之后重启会找不到 tmp。
  // 让用户先把恢复做完（重启一次后端就会应用），再来搬家。
  final pendingRestore = File('${dataDbFileOf(source)}.restore-pending.json');
  if (pendingRestore.existsSync()) {
    return const MigrationResult(
      ok: false,
      message: '还有一次库恢复没完成。先重启一次本地服务让它做完，再来改数据目录。',
    );
  }

  final totalBytes = entries.fold<int>(0, (sum, e) => sum + e.length);
  final totalFiles = entries.length;

  var copiedFiles = 0;
  var copiedBytes = 0;
  var lastReport = DateTime.fromMillisecondsSinceEpoch(0);

  void report(String current) {
    final now = DateTime.now();
    // 文件多的时候别每一帧都 setState：100ms 一次足够画进度条
    if (now.difference(lastReport).inMilliseconds < 100 &&
        copiedFiles != totalFiles) {
      return;
    }
    lastReport = now;
    onProgress?.call(
      MigrationProgress(
        copiedFiles: copiedFiles,
        totalFiles: totalFiles,
        copiedBytes: copiedBytes,
        totalBytes: totalBytes,
        currentPath: current,
      ),
    );
  }

  try {
    for (final entry in entries) {
      if (isCancelled?.call() == true) {
        return MigrationResult(
          ok: false,
          cancelled: true,
          message: '已取消。旧数据没动；新目录里留着一部分拷完的文件，可以整个删掉重来。',
        );
      }
      final destination = [
        target,
        ...entry.relative,
      ].join(Platform.pathSeparator);
      final parent = Directory(File(destination).parent.path);
      if (!parent.existsSync()) parent.createSync(recursive: true);
      File(entry.source).copySync(destination);
      copiedFiles++;
      copiedBytes += entry.length;
      report(entry.relative.join(Platform.pathSeparator));
    }
  } catch (e) {
    return MigrationResult(
      ok: false,
      message: '复制到一半失败了（$e）。旧数据没动；新目录里的副本不完整，可以删掉重来。',
    );
  }

  // 校验：逐个文件比字节数。有一个对不上就不落指针 —— 半份数据比不搬更糟。
  for (final entry in entries) {
    final destination = [
      target,
      ...entry.relative,
    ].join(Platform.pathSeparator);
    final file = File(destination);
    if (!file.existsSync()) {
      return MigrationResult(
        ok: false,
        message: '校验没通过：${entry.relative.join('\\')} 没拷过去。旧数据没动。',
      );
    }
    final size = file.lengthSync();
    if (size != entry.length) {
      return MigrationResult(
        ok: false,
        message:
            '校验没通过：${entry.relative.join('\\')} 大小对不上'
            '（应该是 ${entry.length} 字节，实际 $size）。旧数据没动。',
      );
    }
  }

  if (!writeDataDirOverride(root, target)) {
    return const MigrationResult(
      ok: false,
      message:
          '数据已经复制并校验好了，但"记住新位置"没写成（<根>\\data-dir.json）。'
          '旧目录还在用，可以重试一次。',
    );
  }

  onProgress?.call(
    MigrationProgress(
      copiedFiles: copiedFiles,
      totalFiles: totalFiles,
      copiedBytes: copiedBytes,
      totalBytes: totalBytes,
      currentPath: '',
    ),
  );

  return MigrationResult(
    ok: true,
    pointerWritten: true,
    message: '搬完了，共 $totalFiles 个文件。',
  );
}

// ---------------------------------------------------------------------------
//  删旧数据（用户确认之后才调）
// ---------------------------------------------------------------------------

/// 删掉旧数据目录里的数据（`.db\` 与 `storage\`），**不碰程序目录里的任何东西**。
///
/// 永远不会删 [oldDataDir] 本身之外的路径：`backups\` / `.run\` / `comfyui\` /
/// `skills\builtin\` 都不在删除范围里。删不掉的文件如实列出。
///
/// ⚠ **`storage\app-support` 一定保留**：新版本的偏好已经搬到 `<根>\app-support`，
/// 但"刚升级、还没重启"的进程可能仍把它当活文件在用（热重载不会重跑 `main()`）；
/// 删掉它 = 下次启动读不到偏好。它只有几十 KB，留着不碍事。
Future<({int freedBytes, int deletedFiles, List<String> failed})> deleteOldData(
  String oldDataDir,
) async {
  var bytes = 0;
  var files = 0;
  final failed = <String>[];

  Future<void> removeFile(File file) async {
    try {
      final size = file.lengthSync();
      await file.delete();
      bytes += size;
      files++;
    } catch (e) {
      failed.add('${file.path}：$e');
    }
  }

  for (final dirPath in [
    dataDbParentOf(oldDataDir),
    dataStorageDirOf(oldDataDir),
  ]) {
    final dir = Directory(dirPath);
    if (!dir.existsSync()) continue;
    await for (final entity in dir.list(recursive: true, followLinks: false)) {
      if (entity is! File) continue;
      final relative = _relativeParts(oldDataDir, entity.path);
      if (relative.length >= 2 &&
          relative[0].toLowerCase() == 'storage' &&
          relative[1].toLowerCase() == 'app-support') {
        continue;
      }
      await removeFile(entity);
    }
    // 还有 app-support 时不能用 recursive（会把保留的那份也删掉）；
    // 这时目录删不掉是预期内的，不算失败。
    final keepAppSupport = Directory(
      '${dataStorageDirOf(oldDataDir)}${Platform.pathSeparator}app-support',
    ).existsSync();
    try {
      await dir.delete(recursive: !keepAppSupport);
    } catch (e) {
      if (keepAppSupport) continue;
      // 有文件删不掉（被杀毒软件占着）时整个目录删不掉：如实记下来，别假装成功
      if (failed.isEmpty) failed.add('${dir.path}：$e');
    }
  }

  // 旧数据目录如果是"专门建来放数据的"（不是运行根目录），删空之后把它也收掉。
  // 判据：它下面已经没有任何东西了 —— 有程序文件（comfyui / backups…）就留着。
  try {
    final dir = Directory(oldDataDir);
    if (dir.existsSync() && dir.listSync(followLinks: false).isEmpty) {
      dir.deleteSync();
    }
  } catch (_) {
    // 删不掉空壳不算失败
  }

  return (freedBytes: bytes, deletedFiles: files, failed: failed);
}
