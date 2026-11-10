/// **库的身份**（2026-11-10）：让"这次打开的是不是同一份库"变成能问出来的事实。
///
/// # 一句话
///
/// 同一个库不管搬到哪都是同一个库；**不是那个库，就必须说出来** —— 不靠目录名、
/// 不靠层级、不靠"谁的库更大"去猜。
///
/// # 为什么（2026-11-10 那次事故）
///
/// 用户把 Release 构建复制到别处打开：对话只剩几条旧的、角色卡是旧版、画廊全是
/// 加载失败，而磁盘上一个字节都没丢。真因是**两处都"看起来像数据目录"**：
/// `D:\dist\ComfyHub\.db`（上一份构建残留的旧库）和
/// `D:\dist\ComfyHub\storage\.db`（真正在用的库）。任何按路径 / 层级 / 大小的启发式
/// 都可能在别的机器、别的搬家方式下选错 —— 而且选错时一声不响。
///
/// # 身份存在哪（两处，各有各的用途）
///
/// | 落点 | 内容 | 跟着谁走 |
/// | --- | --- | --- |
/// | `<数据目录>\.db\library.json` | `{"id":"<uuid>","createdAt":"…"}` | **跟着库走**：复制 / 搬家 / 迁移都带着它，所以"这是不是同一份库"问它就够了 |
/// | 偏好 `comfyhub.library.last` | `{id, path, seenAt}` | 跟着**程序**走：记住上次用的是哪一份、在哪（`SettingsStore.lastLibrary`） |
///
/// # 三条纪律
///
/// 1. **只给"真的在用"的库写身份**：给残留库写身份等于把它变成"有正式身份的一份库"，
///    下次就会压过记录（见 `backend_paths.dart` 的判定），那是搬起石头砸自己的脚；
/// 2. **写不进去要说出来**（`debugPrint` + 返回值），别装成写成功了 —— 下次启动
///    就会因为"没有身份"退回启发式，而没人知道为什么；
/// 3. **读不懂 ≠ 没有**：文件损坏 / 半截 JSON 时当"这次没有身份"处理，但留一行日志。
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
