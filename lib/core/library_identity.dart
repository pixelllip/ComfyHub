/// **库的身份**：让"这次打开的是不是同一份库"成为可查询的事实。
///
/// 同一个库搬到任何位置都保持同一身份；不是同一份库时必须报告，不按目录名、
/// 层级或库大小推断。
///
/// # 身份存在两处
///
/// | 落点 | 内容 | 跟随对象 |
/// | --- | --- | --- |
/// | `<数据目录>\.db\library.json` | `{"id":"<uuid>","createdAt":"…"}` | 跟随库：复制、搬家、迁移都携带 |
/// | 偏好 `comfyhub.library.last` | `{id, path, seenAt}` | 跟随程序：记录上次使用的库与位置（`SettingsStore.lastLibrary`） |
///
/// # 三条约束
///
/// 1. **只给实际使用的库写身份**：给残留库写身份会使其成为有正式身份的一份库，
///    后续判定会优先选中它；
/// 2. **写入失败必须报告**（`debugPrint` + 返回值），不得返回成功；
/// 3. **读取失败不等于没有身份**：文件损坏或 JSON 不完整时按"本次没有身份"处理，
///    同时记录一行日志。
library;

import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter/foundation.dart';

/// 身份文件的名字（放在数据目录的 `.db\` 里，与库文件同处）。
const String kLibraryIdFileName = 'library.json';

/// 偏好里记"上次用的是哪份库"的键。
const String kLastLibraryPrefsKey = 'comfyhub.library.last';

String libraryIdFileOf(String dataDir) =>
    [dataDir, '.db', kLibraryIdFileName].join(Platform.pathSeparator);

/// 一份库的引用（身份 + 位置 + 上次打开的时间）。
@immutable
class LibraryRef {
  const LibraryRef({
    required this.id,
    required this.path,
    required this.seenAt,
    this.sizeBytes = 0,
  });

  final String id;
  final String path;
  final DateTime seenAt;
  final int sizeBytes;

  bool get usable => id.isNotEmpty && path.isNotEmpty;

  Map<String, Object?> toJson() => <String, Object?>{
    'id': id,
    'path': path,
    'seenAt': seenAt.toIso8601String(),
    'sizeBytes': sizeBytes,
  };

  /// 宽容解析：缺字段 / 脏值回 `null`（调用方当"没有记录"处理，不抛）。
  static LibraryRef? fromJson(Object? raw) {
    if (raw is! Map) return null;
    final id = raw['id'];
    final path = raw['path'];
    if (id is! String || id.isEmpty || path is! String || path.isEmpty) {
      return null;
    }
    final seen = raw['seenAt'];
    DateTime seenAt;
    if (seen is String) {
      seenAt =
          DateTime.tryParse(seen) ?? DateTime.fromMillisecondsSinceEpoch(0);
    } else {
      seenAt = DateTime.fromMillisecondsSinceEpoch(0);
    }
    final size = raw['sizeBytes'];
    return LibraryRef(
      id: id,
      path: path,
      seenAt: seenAt,
      sizeBytes: size is int ? size : 0,
    );
  }

  @override
  String toString() => 'LibraryRef($id @ $path)';
}

/// 读这个数据目录里那份库的身份；没有 / 读不懂都回 null（读不懂会留日志）。
String? readLibraryId(String dataDir) {
  if (dataDir.isEmpty) return null;
  final file = File(libraryIdFileOf(dataDir));
  try {
    if (!file.existsSync()) return null;
    final value = jsonDecode(file.readAsStringSync());
    if (value is Map) {
      final id = value['id'];
      if (id is String && id.trim().isNotEmpty) return id.trim();
    }
    debugPrint('[library] ${file.path} 里没有可用的 id，按"这次没有身份"处理');
  } catch (e) {
    debugPrint('[library] 身份文件读不动（${file.path}）：$e');
  }
  return null;
}

/// 给这个数据目录补一个身份（已有就原样返回）。
///
/// 返回新 / 原有的 id；写不进去回空串并留日志（**别装成成功**）。
String ensureLibraryId(String dataDir) {
  final existing = readLibraryId(dataDir);
  if (existing != null) return existing;
  if (dataDir.isEmpty) return '';
  final file = File(libraryIdFileOf(dataDir));
  final id = _newUuid();
  try {
    file.parent.createSync(recursive: true);
    // 先写 .tmp 再 rename：中途崩掉只会留下半截临时文件，读不到 = "这次没有身份"，
    // 不会把库指到一个坏掉的身份上（与 data-dir.json 同一个写法）。
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode(<String, Object?>{
        'id': id,
        'createdAt': DateTime.now().toIso8601String(),
      }),
    );
    tmp.renameSync(file.path);
    return id;
  } catch (e) {
    debugPrint('[library] 身份文件写不进去（${file.path}）：$e');
    return '';
  }
}

/// 给这个数据目录拍一张"库引用"（没有库就回 null）。
///
/// ⚠ **只在真的用到这份库时调用**（见文件头纪律 1）。
LibraryRef? snapshotLibrary(String dataDir) {
  if (dataDir.isEmpty) return null;
  final db = File([dataDir, '.db', 'comfyhub.db'].join(Platform.pathSeparator));
  try {
    if (!db.existsSync()) return null;
  } catch (_) {
    return null;
  }
  final id = ensureLibraryId(dataDir);
  if (id.isEmpty) return null;
  return LibraryRef(
    id: id,
    path: dataDir,
    seenAt: DateTime.now(),
    sizeBytes: _lengthOf(db),
  );
}

int _lengthOf(File file) {
  try {
    return file.lengthSync();
  } catch (_) {
    return 0;
  }
}

/// 生成一个 uuid v4（不引第三方包：只要够唯一就行）。
String _newUuid() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
  final hex = bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
  return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-'
      '${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}';
}
