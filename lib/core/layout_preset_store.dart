import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import '../models/layout_spec.dart';

/// 一份存下来的布局。
///
/// 存的是**规格**（框、坐标、名称、关系），不是生成好的提示词 ——
/// 提示词是投影，改一次措辞就过期；规格才是能反复用的东西。
class LayoutPreset {
  const LayoutPreset({
    required this.id,
    required this.name,
    required this.updatedAt,
    required this.spec,
  });

  final String id;
  final String name;
  final DateTime updatedAt;
  final LayoutSpec spec;

  Map<String, Object?> toJson() => <String, Object?>{
        'id': id,
        'name': name,
        'updatedAt': updatedAt.toIso8601String(),
        'spec': spec.toJson(),
      };

  /// 读一份存下来的布局；**坏数据返回 null，绝不抛**。
  ///
  /// 这个文件用户可能手改，也可能被老版本写坏。读的时候崩掉的表现是
  /// "整个布局库打不开"，比丢一条严重得多。
  static LayoutPreset? fromJson(Object? raw) {
    if (raw is! Map) return null;
    final map = Map<String, Object?>.from(raw);
    final specRaw = map['spec'];
    if (specRaw is! Map) return null;
    final id = (map['id'] as String?)?.trim();
    if (id == null || id.isEmpty) return null;
    return LayoutPreset(
      id: id,
      name: ((map['name'] as String?) ?? '').trim().isEmpty
          ? '未命名布局'
          : (map['name'] as String).trim(),
      updatedAt: DateTime.tryParse((map['updatedAt'] as String?) ?? '') ??
          DateTime.fromMillisecondsSinceEpoch(0),
      spec: LayoutSpec.fromJson(Map<String, Object?>.from(specRaw)),
    );
  }
}

/// 布局库。落在 `SharedPreferences`（Windows 上就是
/// `<根>\storage\app-support\shared_preferences.json`，见 `portable_paths.dart`）。
///
/// 为什么不是新开一张表：一份布局只有几 KB，没有跨设备同步与查询需求，
/// 而 `SharedPreferences` 在两端都已经接好了落盘位置与迁移。等它真的要
/// 在手机和桌面之间同步时，再搬到 `server-rs` 的库里 —— 那时形状已经稳定了。
class LayoutPresetStore {
  LayoutPresetStore({SharedPreferences? prefs}) : _injected = prefs;

  /// 偏好键。**改名等于清空用户的布局库**，别动。
  static const String prefsKey = 'comfyhub.layoutPresets';

  final SharedPreferences? _injected;
  SharedPreferences? _prefs;

  Future<SharedPreferences> get _store async =>
      _injected ?? (_prefs ??= await SharedPreferences.getInstance());

  /// 按保存时间倒序。
  Future<List<LayoutPreset>> load() async {
    final prefs = await _store;
    final raw = prefs.getString(prefsKey);
    if (raw == null || raw.trim().isEmpty) return <LayoutPreset>[];
    Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } catch (_) {
      return <LayoutPreset>[];
    }
    if (decoded is! List) return <LayoutPreset>[];
    final out = <LayoutPreset>[];
    for (final e in decoded) {
      final p = LayoutPreset.fromJson(e);
      if (p != null) out.add(p);
    }
    out.sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
    return out;
  }

  Future<void> saveAll(List<LayoutPreset> items) async {
    final prefs = await _store;
    await prefs.setString(
      prefsKey,
      jsonEncode([for (final p in items) p.toJson()]),
    );
  }
}
