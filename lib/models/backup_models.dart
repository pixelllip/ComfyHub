/// 备份与恢复的 JSON 形状（`/api/system/backups*`、`/api/system/restore`）。
///
/// 真源在 `server-rs/src/infra/backup.rs` 与 `server-rs/src/web/backup.rs`。
/// 字段名一处写错**不会报错**，界面上那一格就是空的 —— 所以
/// `test/backup_card_test.dart` 直接拿真实响应的形状喂它。
library;

/// 一份备份（`items[]`）。
class BackupItem {
  const BackupItem({
    required this.name,
    required this.bytes,
    required this.createdAt,
    required this.kind,
  });

  final String name;
  final int bytes;

  /// ISO-8601（UTC），后端按文件 mtime 给。
  final String createdAt;

  /// `auto`（手动 / 自动备份）或 `beforeRestore`（恢复前自动拍的那张保险）。
  final String kind;

  bool get isSafety => kind == 'beforeRestore';

  factory BackupItem.fromJson(Map<String, dynamic> json) => BackupItem(
    name: (json['name'] ?? '') as String,
    bytes: (json['bytes'] as num?)?.toInt() ?? 0,
    createdAt: (json['createdAt'] ?? '') as String,
    kind: (json['kind'] ?? 'auto') as String,
  );
}

/// 上一次恢复的结果 —— 重启之后界面靠它说"到底成没成"。
class LastRestore {
  const LastRestore({
    required this.ok,
    required this.name,
    required this.message,
    required this.at,
  });

  final bool ok;
  final String name;
  final String message;
  final String at;

  factory LastRestore.fromJson(Map<String, dynamic> json) => LastRestore(
    ok: json['ok'] == true,
    name: (json['name'] ?? '') as String,
    message: (json['message'] ?? '') as String,
    at: (json['at'] ?? '') as String,
  );
}

/// `GET /api/system/backups` 的整份响应。
class BackupListing {
  const BackupListing({
    required this.dir,
    required this.items,
    this.pending,
    this.lastRestore,
  });

  /// 备份目录（绝对路径，用来"打开文件夹"）。
  final String dir;
  final List<BackupItem> items;

  /// 已经准备好、等下次启动换上去的那份备份名；`null` = 没有待办。
  final String? pending;

  final LastRestore? lastRestore;

  factory BackupListing.fromJson(Map<String, dynamic> json) => BackupListing(
    dir: (json['dir'] ?? '') as String,
    items: ((json['items'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => BackupItem.fromJson(Map<String, dynamic>.from(e)))
        .toList(growable: false),
    pending: json['pending'] as String?,
    lastRestore: json['lastRestore'] is Map
        ? LastRestore.fromJson(
            Map<String, dynamic>.from(json['lastRestore'] as Map),
          )
        : null,
  );
}

/// `POST /api/system/restore` 的响应：标记已落，**重启后端**之后才真的换库。
class RestorePreparation {
  const RestorePreparation({
    required this.name,
    required this.safety,
    required this.restartRequired,
  });

  final String name;

  /// 恢复前自动拍的那张保险（备份文件名）；`null` = 当时没有库可拍。
  final String? safety;
  final bool restartRequired;

  factory RestorePreparation.fromJson(Map<String, dynamic> json) =>
      RestorePreparation(
        name: (json['name'] ?? '') as String,
        safety: json['safety'] as String?,
        restartRequired: json['restartRequired'] == true,
      );
}
