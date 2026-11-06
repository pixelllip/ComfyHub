/// 存储占用 / 孤儿文件 / 事件归档的 JSON 形状（`/api/system/storage*`）。
///
/// 真源在 `server-rs/src/infra/storage.rs`、`server-rs/src/ai/runs.rs` 与
/// `server-rs/src/web/storage.rs`。字段名一处写错**不会报错**，界面上那一格就是空的。
library;

/// 一个目录的占用。
class StorageDir {
  const StorageDir({
    required this.key,
    required this.label,
    required this.path,
    required this.bytes,
    required this.files,
  });

  final String key;
  final String label;
  final String path;
  final int bytes;
  final int files;

  factory StorageDir.fromJson(Map<String, dynamic> json) => StorageDir(
    key: (json['key'] ?? '') as String,
    label: (json['label'] ?? '') as String,
    path: (json['path'] ?? '') as String,
    bytes: (json['bytes'] as num?)?.toInt() ?? 0,
    files: (json['files'] as num?)?.toInt() ?? 0,
  );
}

/// `ai_run_events` 的规模。
class EventStats {
  const EventStats({required this.rows, this.oldest, this.newest});

  final int rows;
  final String? oldest;
  final String? newest;

  factory EventStats.fromJson(Map<String, dynamic> json) => EventStats(
    rows: (json['rows'] as num?)?.toInt() ?? 0,
    oldest: json['oldest'] as String?,
    newest: json['newest'] as String?,
  );
}

/// `GET /api/system/storage` 的整份响应。
class StorageUsage {
  const StorageUsage({
    required this.root,
    required this.dirs,
    required this.totalBytes,
    required this.events,
  });

  final String root;
  final List<StorageDir> dirs;
  final int totalBytes;
  final EventStats events;

  factory StorageUsage.fromJson(Map<String, dynamic> json) => StorageUsage(
    root: (json['root'] ?? '') as String,
    dirs: ((json['dirs'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => StorageDir.fromJson(Map<String, dynamic>.from(e)))
        .toList(growable: false),
    totalBytes: (json['totalBytes'] as num?)?.toInt() ?? 0,
    events: json['events'] is Map
        ? EventStats.fromJson(Map<String, dynamic>.from(json['events'] as Map))
        : const EventStats(rows: 0),
  );
}

/// 一个孤儿文件（磁盘上有、库里没有对应行）。
class OrphanFile {
  const OrphanFile({
    required this.kind,
    required this.name,
    required this.bytes,
    required this.modifiedAt,
  });

  /// `media` / `thumb` / `attachment` / `attachmentThumb`
  final String kind;
  final String name;
  final int bytes;
  final String modifiedAt;

  factory OrphanFile.fromJson(Map<String, dynamic> json) => OrphanFile(
    kind: (json['kind'] ?? '') as String,
    name: (json['name'] ?? '') as String,
    bytes: (json['bytes'] as num?)?.toInt() ?? 0,
    modifiedAt: (json['modifiedAt'] ?? '') as String,
  );
}

/// `POST /api/system/storage/orphans`（`apply=false`）的结果。
class OrphanScan {
  const OrphanScan({
    required this.items,
    required this.count,
    required this.bytes,
    required this.truncated,
  });

  final List<OrphanFile> items;
  final int count;
  final int bytes;

  /// 超过 1000 条时只列前 1000 条；`count` / `bytes` 仍然是全量。
  final bool truncated;

  factory OrphanScan.fromJson(Map<String, dynamic> json) => OrphanScan(
    items: ((json['items'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => OrphanFile.fromJson(Map<String, dynamic>.from(e)))
        .toList(growable: false),
    count: (json['count'] as num?)?.toInt() ?? 0,
    bytes: (json['bytes'] as num?)?.toInt() ?? 0,
    truncated: json['truncated'] == true,
  );
}

/// 删完之后回给界面的账。
class CleanReport {
  const CleanReport({
    required this.deleted,
    required this.bytes,
    required this.failed,
    required this.remaining,
  });

  final int deleted;
  final int bytes;
  final List<FailedFile> failed;

  /// 这次没删完的（超过 1000 条或删失败），> 0 就提示再点一次。
  final int remaining;

  factory CleanReport.fromJson(Map<String, dynamic> json) => CleanReport(
    deleted: (json['deleted'] as num?)?.toInt() ?? 0,
    bytes: (json['bytes'] as num?)?.toInt() ?? 0,
    failed: ((json['failed'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => FailedFile.fromJson(Map<String, dynamic>.from(e)))
        .toList(growable: false),
    remaining: (json['remaining'] as num?)?.toInt() ?? 0,
  );
}

class FailedFile {
  const FailedFile({required this.name, required this.error});

  final String name;
  final String error;

  factory FailedFile.fromJson(Map<String, dynamic> json) => FailedFile(
    name: (json['name'] ?? '') as String,
    error: (json['error'] ?? '') as String,
  );
}

/// `POST /api/system/events/archive` 的结果。
class EventArchiveReport {
  const EventArchiveReport({
    required this.archived,
    required this.bytes,
    required this.file,
    required this.cutoff,
    required this.remaining,
  });

  final int archived;
  final int bytes;

  /// 归档文件（绝对路径）；`archived == 0` 时是空串。
  final String file;
  final String cutoff;

  /// 还有多少条同样够老、这次没装下的。
  final int remaining;

  factory EventArchiveReport.fromJson(Map<String, dynamic> json) =>
      EventArchiveReport(
        archived: (json['archived'] as num?)?.toInt() ?? 0,
        bytes: (json['bytes'] as num?)?.toInt() ?? 0,
        file: (json['file'] ?? '') as String,
        cutoff: (json['cutoff'] ?? '') as String,
        remaining: (json['remaining'] as num?)?.toInt() ?? 0,
      );
}
