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
///
/// 以前是 12 个写死的 **Material 2 500 色**（`0xFF4CAF50`…）：它们和当前
/// `ColorScheme` 没有关系，动态取色下会撞色，浅色主题下还容易让白字读不清。
///
/// 现在只有**色相**由名字散列决定（360° 色轮，撞色概率更低），饱和度 / 亮度
/// 跟着 [scheme] 的明暗走 —— 深色主题取亮一点、浅色主题取深一点。
Color tagColor(String name, {Color? fallback, ColorScheme? scheme}) {
  if (name.isEmpty) return fallback ?? scheme?.primary ?? const Color(0xFF7C5CFF);
  var hash = 0;
  for (final c in name.codeUnits) {
    hash = (hash * 31 + c) & 0x7fffffff;
  }
  final hue = (hash % 360).toDouble();
  final dark = (scheme?.brightness ?? Brightness.dark) == Brightness.dark;
  return HSVColor.fromAHSV(
    1,
    hue,
    dark ? 0.42 : 0.55,
    dark ? 0.88 : 0.62,
  ).toColor();
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
