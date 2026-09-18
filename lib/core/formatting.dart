import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

final _dateFmt = DateFormat('yyyy-MM-dd HH:mm');
final _dateShortFmt = DateFormat('MM-dd HH:mm');

String formatDateTime(DateTime? dt) => dt == null ? '—' : _dateFmt.format(dt);
String formatDateShort(DateTime? dt) => dt == null ? '—' : _dateShortFmt.format(dt);

String formatSize(int bytes) {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var value = bytes.toDouble();
  var i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return '${value.toStringAsFixed(value >= 100 || i == 0 ? 0 : 1)} ${units[i]}';
}

String formatDuration(int? ms) {
  if (ms == null || ms <= 0) return '';
  final total = ms ~/ 1000;
  final h = total ~/ 3600;
  final m = (total % 3600) ~/ 60;
  final s = total % 60;
  final mm = m.toString().padLeft(2, '0');
  final ss = s.toString().padLeft(2, '0');
  return h > 0 ? '$h:$mm:$ss' : '$m:$ss';
}

String relativeTime(DateTime? dt) {
  if (dt == null) return '—';
  // 后端时间戳是 UTC（`Instant.toString()`）；不转本地的话，最上面那句"绝对时间"
  // 会按 UTC 时钟显示，整体偏一个时差。
  final local = dt.toLocal();
  final diff = DateTime.now().difference(local);
  if (diff.inSeconds < 60) return '刚刚';
  if (diff.inMinutes < 60) return '${diff.inMinutes} 分钟前';
  if (diff.inHours < 24) return '${diff.inHours} 小时前';
  if (diff.inDays < 30) return '${diff.inDays} 天前';
  return _dateFmt.format(local);
}

/// 把标签名字符串映射成稳定颜色，保证同名标签颜色一致。
Color tagColor(String name, {Color? fallback}) {
  const palette = [
    Color(0xFF4CAF50),
    Color(0xFF2196F3),
    Color(0xFF9C27B0),
    Color(0xFFE91E63),
    Color(0xFFFF9800),
    Color(0xFF00BCD4),
    Color(0xFF795548),
    Color(0xFF3F51B5),
    Color(0xFF8BC34A),
    Color(0xFFF44336),
    Color(0xFF009688),
    Color(0xFF673AB7),
  ];
  if (name.isEmpty) return fallback ?? palette.first;
  var hash = 0;
  for (final c in name.codeUnits) {
    hash = (hash * 31 + c) & 0x7fffffff;
  }
  return palette[hash % palette.length];
}

Color? parseHexColor(String? hex) {
  if (hex == null) return null;
  var v = hex.trim().replaceFirst('#', '');
  if (v.length == 6) v = 'FF$v';
  if (v.length != 8) return null;
  final parsed = int.tryParse(v, radix: 16);
  return parsed == null ? null : Color(parsed);
}

/// 提示词太长时截断展示
String ellipsis(String text, int max) {
  final t = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  if (t.length <= max) return t;
  return '${t.substring(0, max)}…';
}
