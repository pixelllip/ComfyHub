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
///   · 产物目录（产物 / 缩略图 / 附件 / Skills / 记忆 / MCP 运行时）。
///
/// **源目录的产物层按实际布局认**（`backend_paths.dart` 的 `resolvedStorageDirOf`）：
/// 标准是 `<数据目录>\storage`，旧版把 `storage` 当数据目录会套成
/// `storage\storage`，用户手动挪过则是"扁平"（产物直接在数据目录里）。
/// 搬过去一律写成**标准布局** —— 双层顺手压平，扁平落进目标目录的 `storage\`。
///
/// **不搬**：`tmp`（24 小时就清的中转文件）、`app-support`
/// （偏好已经搬到 `<根>\app-support`，那是程序目录的东西，不跟着数据走）。
///
/// # 三条纪律
///
///  1. **只复制、不删源**：源目录原样留着，用户确认没问题后自己删（界面会问一次）。
///  2. **校验过才落指针**：每个文件都比对字节数；有一个对不上就当场失败、
///     **不写** `data-dir.json` —— 半份数据比不搬更糟。
///  3. **失败 / 取消都如实说**：目标目录里会留下已经拷完的部分，告诉用户它在哪、
///     可以删掉重来，别假装什么都没发生。
///  4. **覆盖旧库之前一定先备份**：目标里已经有一份库（"搬回程序目录"）时，
///     先把它放进 `<根>\backups\`；备份不成就一个字节都不动。
///
/// # 谁停后端
///
/// 这个模块**不碰后端**：调用方（设置页）必须先停掉本地服务，再调 [migrateDataDir]，
/// 成功之后用新的数据目录重新起。活着的 SQLite 连接握着 WAL 与页缓存，
/// 边跑边搬等于搬一份"可能差最后几个事务"的库。
library;

import 'dart:io';

import 'backend_paths.dart';
import 'library_identity.dart';

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
    this.replacesTarget = false,
  });

  final bool ok;

  /// 给用户看的一句人话：不通过时是原因；通过但目标里已经有库时是提醒。
  final String message;

  /// 当前数据目录的规模（给确认弹窗用）。
  final DataDirUsage source;

  /// 目标目录里已经有一份库，搬过去会**覆盖**它（迁移会先把它备份到 `backups`）。
  ///
  /// 2026-11-09：用户把数据搬到别处、又想搬回来时，旧目录里的库还在 ——
  /// 以前一律拒绝并让人去「备份与恢复」，可那份备份根本不存在。现在改成
  /// "先备份、再覆盖"，并在确认弹窗里把这件事说清楚。
  final bool replacesTarget;
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
    this.backupPath,
  });

  final bool ok;

  /// 给用户看的一句人话（失败 / 取消时**必须**说清楚现场）。
  final String message;

  final bool cancelled;

  /// 指针文件写成功 = 下次起后端就会用新目录。
  final bool pointerWritten;

  /// 目标里原来的库备份到了哪（没有覆盖旧库时是 null）。
  final String? backupPath;
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
///
/// 目标里一律写成**标准布局**（`.db\…` 与 `storage\…`），源目录是扁平 / 双层
/// 布局时也先归一到这两条路径上 —— 于是"双层搬一次就压平"、"扁平的产物
/// 搬完会落进 `storage\`"，不用额外分支。
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
      // 库的身份文件（`library.json`）跟着库一起走：搬完还是"同一份库"，
      // 不然下一次启动会以为换了一份（2026-11-10 的身份机制）。
      if (name != dbName &&
          name != '$dbName-wal' &&
          name != '$dbName-shm' &&
          name != kLibraryIdFileName) {
        continue;
      }
      final stat = entity.statSync();
      entries.add(_Entry(['.db', name], entity.path, stat.size));
    }
  }

  // 产物 / AI 数据：跳过 tmp（中转）与 app-support（偏好，已经不在数据目录里了）。
  final storageRoot = resolvedStorageDirOf(dataDir);
  if (_samePath(storageRoot, dataDir)) {
    // 扁平布局：只认那几个"产物"子目录，别把 `.db` / 程序文件也扫进去。
    for (final name in kStorageContentDirs) {
      final dir = Directory([storageRoot, name].join(Platform.pathSeparator));
      if (!dir.existsSync()) continue;
      await _addFiles(dir, ['storage', name], entries);
    }
  } else {
    final storage = Directory(storageRoot);
    if (storage.existsSync()) {
      await for (final entity in storage.list(
        recursive: true,
        followLinks: false,
      )) {
        if (entity is! File) continue;
        final relative = _relativeParts(storageRoot, entity.path);
        if (relative.isNotEmpty && _isTransient(relative.first)) continue;
        final stat = entity.statSync();
        entries.add(_Entry(['storage', ...relative], entity.path, stat.size));
      }
    }
  }

  return entries;
}

/// 把 [dir] 下的文件按 [prefix] 前缀收进 [out]（扁平布局专用）。
Future<void> _addFiles(
  Directory dir,
  List<String> prefix,
  List<_Entry> out,
) async {
  await for (final entity in dir.list(recursive: true, followLinks: false)) {
    if (entity is! File) continue;
    final relative = _relativeParts(dir.path, entity.path);
    final stat = entity.statSync();
    out.add(_Entry([...prefix, ...relative], entity.path, stat.size));
  }
}

bool _isTransient(String name) {
  final lower = name.toLowerCase();
  return lower == 'tmp' || lower == 'app-support';
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

/// 目录里有东西就算（读不动当"有"，宁可多问一句也不静默覆盖）。
bool _dirHasContent(String path) {
  try {
    final dir = Directory(path);
    return dir.existsSync() && dir.listSync(followLinks: false).isNotEmpty;
  } catch (_) {
    return true;
  }
}

/// 选中的文件夹是不是就叫 `storage`。
bool isStorageNamedDir(String path) {
  final norm = _normalize(path);
  if (norm.isEmpty) return false;
  return norm.split(Platform.pathSeparator).last.toLowerCase() == 'storage';
}

/// 把用户选中的目录**归一到数据目录**。
///
/// 数据目录 = "库 + 一个 `storage` 子目录"的容器。用户常直接选 `…\storage`
/// （他想要的是"产物放这儿"），照原样用就会得到 `…\storage\storage` ——
/// 2026-11-09 报上来的嵌套 bug 就是这么来的。选中 `storage` 文件夹时按它的
/// 上一级算：产物仍然落在用户点的那个文件夹里，只是不再多套一层。
String normalizeDataDirTarget(String target) {
  final norm = _normalize(target.trim());
  if (norm.isEmpty || !isStorageNamedDir(norm)) return norm;
  final parent = Directory(norm).parent.path;
  return _samePath(parent, norm) ? norm : parent;
}

// ---------------------------------------------------------------------------
//  查
// ---------------------------------------------------------------------------

/// 检查一个目标目录能不能当数据目录。**只读 + 一次写探针**，不动任何数据。
Future<DataDirCheck> checkDataDirTarget({
  required String target,
  required String current,
}) async {
  final trimmed = _normalize(target.trim());
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
      message: '新的位置不能位于当前位置的内部，否则搬运时会把刚复制过去的文件再次复制。',
      source: source,
    );
  }

  final targetDb = File(dataDbFileOf(trimmed)).existsSync();
  final targetStorage = dataStorageDirOf(trimmed);
  final targetHasStorage =
      _dirHasContent(targetStorage) || looksLikeStorageDir(trimmed);
  final targetHasData = targetDb || targetHasStorage;

  final sourceStorage = resolvedStorageDirOf(current);
  // 产物本来就已经在目标目录的 `storage` 里（用户把数据目录选成了 `…\storage`），
  // 或者当前数据目录**就是**目标的 `storage`（双层布局往上压平）：
  // 这次"搬"其实只是把库挪到上一级，产物不用换地方。
  final artifactsInPlace =
      _samePath(targetStorage, sourceStorage) ||
      _samePath(targetStorage, current);

  if (_isInside(current, trimmed)) {
    // 上一级 / 更上一级：只允许"直接上一级且产物已经在位"，或者目标本来是干净的。
    // 更远的上级目录里什么都有（用户自己的文件），不能拿它当数据目录。
    final parent = Directory(_normalize(current)).parent.path;
    if (!_samePath(parent, trimmed)) {
      return DataDirCheck(
        ok: false,
        message:
            '只能向上移动一级。如需向上移动，请选择当前位置的上一级文件夹。',
        source: source,
      );
    }
    if (targetHasData && !artifactsInPlace) {
      return DataDirCheck(
        ok: false,
        message:
            '上一级文件夹中已经存在另一份数据，搬运后会与它混在一起。请先将那份数据移走。',
        source: source,
      );
    }
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
      message: '这个文件夹无法写入（$e）。请选择其他可写入的文件夹。',
      source: source,
    );
  }

  // 目标里已经有一份数据：不是"不能搬"，而是"搬过去要先备份、再覆盖"。
  // 用户 2026-11-09 报的"搬回程序目录被踢去备份与恢复、可我没备份过"就是这里。
  final String message;
  if (targetDb) {
    message =
        '这个位置已经存在一份数据。该数据会先保存到 backups，'
        '然后替换为当前这一份；同名文件会被覆盖。';
  } else if (targetHasStorage) {
    message =
        '这个位置已经存在图片和视频。搬运时会覆盖同名文件，'
        '多余的原文件会保留。';
  } else {
    message = '';
  }

  return DataDirCheck(
    ok: true,
    source: source,
    replacesTarget: targetDb,
    message: message,
  );
}

// ---------------------------------------------------------------------------
//  搬
// ---------------------------------------------------------------------------

/// 把数据从 [source] 复制到 [target]，校验通过后把 `<根>\data-dir.json` 指过去。
///
/// **调用前必须先停掉本地服务**（见文件头）。[isCancelled] 每搬一个文件问一次，
/// 返回 true 就停手（已经拷完的部分留在目标目录，不写指针）。
///
/// 目标目录里已经有一份库时（"搬回程序目录"最常见）：**先把它备份到
/// `<根>\backups\`，再覆盖**。以前这里直接拒绝，用户被指去「备份与恢复」，
/// 可那次搬家根本没做过备份 —— 2026-11-09 卡住的现场。
Future<MigrationResult> migrateDataDir({
  required String source,
  required String target,
  required String root,
  void Function(MigrationProgress progress)? onProgress,
  bool Function()? isCancelled,
}) async {
  if (_samePath(source, target)) {
    return const MigrationResult(ok: false, message: '这就是当前的存放位置，无需搬运。');
  }
  if (isCancelled?.call() == true) {
    return const MigrationResult(
      ok: false,
      cancelled: true,
      message: '已取消。原来的内容没有被修改。',
    );
  }

  final List<_Entry> entries;
  try {
    entries = await _collectEntries(source);
  } catch (e) {
    return MigrationResult(
      ok: false,
      message: '无法读取当前的存放位置（$e）。原来的内容没有被修改。',
    );
  }

  // 有未完成的库恢复：那份标记里写的是**旧路径**，搬走之后重启会找不到 tmp。
  // 让用户先把恢复做完（重启一次后端就会应用），再来搬家。
  final pendingRestore = File('${dataDbFileOf(source)}.restore-pending.json');
  if (pendingRestore.existsSync()) {
    return const MigrationResult(
      ok: false,
      message: '上一次恢复尚未完成。请先重新启动程序，让它完成后再搬运。',
    );
  }

  // 目标里已经有一份库：先备份再让开。**备份失败就什么都不动** ——
  // 覆盖一份没备份的库是不可接受的。
  String? backupPath;
  if (File(dataDbFileOf(target)).existsSync()) {
    try {
      backupPath = _backupTargetDb(root, target);
    } catch (e) {
      return MigrationResult(
        ok: false,
        message: '新的位置中存在一份数据，但保存备份失败（$e）。原来的内容没有被修改。',
      );
    }
    // 备份已经落盘、还没删目标那份库：这时取消仍然算"什么都没动"。
    if (isCancelled?.call() == true) {
      return MigrationResult(
        ok: false,
        cancelled: true,
        message: '已取消。原来的内容没有被修改。新位置原有的数据已保存到 $backupPath。',
        backupPath: backupPath,
      );
    }
    try {
      for (final name in [
        'comfyhub.db-wal',
        'comfyhub.db-shm',
        'comfyhub.db',
      ]) {
        final file = File(
          [dataDbParentOf(target), name].join(Platform.pathSeparator),
        );
        if (file.existsSync()) file.deleteSync();
      }
    } catch (e) {
      return MigrationResult(
        ok: false,
        message:
            '新位置原有的数据已保存到 $backupPath，但无法把它从原处移除（$e）。'
            '原来的内容没有被修改，可以重试。',
      );
    }
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
          message:
              '已取消。原来的内容没有被修改。新位置中保留了部分已复制的文件，可以整体删除后重试。'
              '${_backupNote(backupPath)}',
          backupPath: backupPath,
        );
      }
      final destination = [
        target,
        ...entry.relative,
      ].join(Platform.pathSeparator);
      // 产物本来就在目标位置（把 `…\storage` 选成数据目录后再搬到上一级）：
      // 源和目标是同一个文件，拷自己会报错，跳过即可。
      if (_samePath(entry.source, destination)) {
        copiedFiles++;
        copiedBytes += entry.length;
        report(entry.relative.join(Platform.pathSeparator));
        continue;
      }
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
      message:
          '复制过程中失败（$e）。原来的内容没有被修改。新位置中的副本不完整，可以删除后重试。'
          '${_backupNote(backupPath)}',
      backupPath: backupPath,
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
        message:
            '核对未通过：${entry.relative.join('\\')} 没有被复制过去。原来的内容没有被修改。'
            '${_backupNote(backupPath)}',
        backupPath: backupPath,
      );
    }
    final size = file.lengthSync();
    if (size != entry.length) {
      return MigrationResult(
        ok: false,
        message:
            '核对未通过：${entry.relative.join('\\')} 的大小不一致'
            '（应为 ${entry.length} 字节，实际为 $size）。原来的内容没有被修改。'
            '${_backupNote(backupPath)}',
        backupPath: backupPath,
      );
    }
  }

  if (!writeDataDirOverride(root, target)) {
    return MigrationResult(
      ok: false,
      message:
          '内容已复制并核对完成，但无法记录新的存放位置（<根>\\data-dir.json）。'
          '当前仍使用原来的位置，可以重试。${_backupNote(backupPath)}',
      backupPath: backupPath,
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
    message: '搬运完成，共 $totalFiles 个文件。${_backupNote(backupPath)}',
    backupPath: backupPath,
  );
}

/// 覆盖旧库之后给用户的一句提醒（没覆盖时为空）。
String _backupNote(String? backupPath) =>
    backupPath == null ? '' : '新位置原有的数据保存在 $backupPath。';

/// 把目标目录里现成的库文件（含 `-wal` / `-shm`）备份到 `<根>\backups\`。
String _backupTargetDb(String root, String target) {
  final stamp = _timestamp();
  final backups = Directory([root, 'backups'].join(Platform.pathSeparator));
  var backup = Directory(
    [backups.path, 'data-dir-$stamp'].join(Platform.pathSeparator),
  );
  var suffix = 1;
  while (backup.existsSync()) {
    backup = Directory(
      [
        backups.path,
        'data-dir-$stamp-${suffix++}',
      ].join(Platform.pathSeparator),
    );
  }
  backup.createSync(recursive: true);

  final dbDir = dataDbParentOf(target);
  for (final name in ['comfyhub.db', 'comfyhub.db-wal', 'comfyhub.db-shm']) {
    final file = File([dbDir, name].join(Platform.pathSeparator));
    if (!file.existsSync()) continue;
    file.copySync([backup.path, name].join(Platform.pathSeparator));
  }
  return backup.path;
}

String _timestamp() {
  final now = DateTime.now();
  String two(int value) => value.toString().padLeft(2, '0');
  final ms = now.millisecond.toString().padLeft(3, '0');
  return '${now.year}${two(now.month)}${two(now.day)}-'
      '${two(now.hour)}${two(now.minute)}${two(now.second)}-$ms';
}

// ---------------------------------------------------------------------------
//  删旧数据（用户确认之后才调）
// ---------------------------------------------------------------------------

/// 删掉旧数据目录里的数据（`.db\` 与 `storage\`），**不碰程序目录里的任何东西**。
///
/// [newStorageDir] 是迁移后的产物目录。搬到上一级时旧数据目录**就是**新产物目录
/// （`…\storage` 原样留着），这时只删它里面的旧 `.db`，产物一个都不许动。
///
/// 永远不会删 [oldDataDir] 本身之外的路径：`backups\` / `.run\` / `comfyui\` /
/// `skills\builtin\` 都不在删除范围里。删不掉的文件如实列出。
///
/// ⚠ **`storage\app-support` 一定保留**：新版本的偏好已经搬到 `<根>\app-support`，
/// 但"刚升级、还没重启"的进程可能仍把它当活文件在用（热重载不会重跑 `main()`）；
/// 删掉它 = 下次启动读不到偏好。它只有几十 KB，留着不碍事。
Future<({int freedBytes, int deletedFiles, List<String> failed})> deleteOldData(
  String oldDataDir, {
  String? newStorageDir,
}) async {
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

  final canonicalStorage = dataStorageDirOf(oldDataDir);
  // 旧数据目录本身就是新产物目录（搬到上一级）：产物原地留着，只清旧库。
  final oldIsNewStorage =
      newStorageDir != null && _samePath(oldDataDir, newStorageDir);
  final dirs = <String>[
    dataDbParentOf(oldDataDir),
    if (newStorageDir == null || !_samePath(canonicalStorage, newStorageDir))
      canonicalStorage,
    // 扁平布局：产物直接放在数据目录里，旧的那份要清掉（但也别碰新产物目录）。
    if (!oldIsNewStorage && isFlatStorageLayout(oldDataDir))
      for (final name in [...kStorageContentDirs, 'tmp'])
        [oldDataDir, name].join(Platform.pathSeparator),
  ];

  for (final dirPath in dirs) {
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
    final keepAppSupport =
        _samePath(dirPath, canonicalStorage) &&
        Directory('$canonicalStorage${Platform.pathSeparator}app-support')
            .existsSync();
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
