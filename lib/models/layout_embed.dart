import 'dart:convert';

import 'layout_spec.dart';

/// AI 在写提示词那一轮给出的**候选布局**（工具 `layout_propose` 的结构化结果）。
///
/// 后端已经把坐标夹回 0~1000、补好 id、清掉坏关系（`ai/layout_proposal.rs`），
/// 这里只负责读。**坏数据返回空表，绝不抛** —— 一张卡片读不出来，不该连累整段历史。
class LayoutCandidate {
  const LayoutCandidate({
    required this.title,
    this.note = '',
    required this.spec,
  });

  /// 一句话标题（"三分法：旅人压左"）。
  final String title;

  /// 这版的构图意图（可空）。
  final String note;

  final LayoutSpec spec;

  /// 从 `tool_result.jsonPayload` 里读候选（键 `layoutTemplates`）。
  ///
  /// 空框的模板会被丢掉：画布打开一张空白布局比不打开更让人困惑。
  static List<LayoutCandidate> listFromPayload(Object? raw) {
    if (raw is! Map) return const [];
    final list = raw['layoutTemplates'];
    if (list is! List) return const [];
    final out = <LayoutCandidate>[];
    for (final item in list) {
      if (item is! Map) continue;
      final specRaw = item['spec'];
      if (specRaw is! Map) continue;
      final spec = LayoutSpec.fromJson(Map<String, Object?>.from(specRaw));
      if (spec.regions.isEmpty) continue;
      final title = item['title']?.toString().trim() ?? '';
      out.add(
        LayoutCandidate(
          title: title.isEmpty ? '布局 ${out.length + 1}' : title,
          note: item['note']?.toString().trim() ?? '',
          spec: spec,
        ),
      );
    }
    return out;
  }
}

/// "把改好的布局提交回工作台"所需要的全部内容。
///
/// 它同时是**画布页的返回值**与**重新提交的消息**：
///
/// * [toMessage] 拼出一段模型读得懂的正文（散文 + 带标记的 JSON）；
/// * [tryParse] 把那条消息读回来 —— 消息气泡因此能显示为布局卡
///   （而不是一屏 JSON），点「再次修改」还能接着改**这一版**。
class LayoutResubmit {
  const LayoutResubmit({required this.spec, this.note = ''});

  /// 消息开头那行标记。**改了它就认不出历史消息**，别动。
  static const String header = '【2D 布局】';

  /// 提交对话框里的默认要求。
  static const String defaultNote = '按这份布局重写提示词，构图以框为准。';

  final LayoutSpec spec;
  final String note;

  /// 拼成一条消息。模型看到的是"要求 + 散文 + 规格 JSON"。
  ///
  /// 散文用 `LayoutSpec.translate()` 的纯函数投影：即使模型读不懂 JSON，
  /// "谁在哪、占多大"也还在正文里。
  String toMessage() {
    final prose = spec.translate().prose;
    final noteText = note.trim().isEmpty ? defaultNote : note.trim();
    final payload = jsonEncode(<String, Object?>{
      'comfyhubLayout': spec.toJson(),
    });
    return '$header$noteText\n\n'
        '这份布局是我在画布上亲手定下的（不是建议稿），请按它重写提示词：'
        '框的位置 / 大小 / 前后关系以规格为准，不要自行移动框。\n\n'
        '$prose\n\n'
        '以下是规格（坐标 0~1000，左上角为原点）：\n'
        '```json\n$payload\n```';
  }

  /// 从一条消息里读回布局（不是这种消息 → null）。
  ///
  /// 只认 [header] + 最后一个 ```json 围栏 + `comfyhubLayout` 键：
  /// 三个条件缺一个都当普通文本，宁可少一张卡，也不误识别手动粘贴的 JSON。
  static LayoutResubmit? tryParse(String text) {
    if (!text.contains(header)) return null;
    final fence = text.lastIndexOf('```json');
    if (fence < 0) return null;
    final end = text.indexOf('```', fence + 7);
    if (end < 0) return null;
    try {
      final decoded = jsonDecode(text.substring(fence + 7, end).trim());
      if (decoded is! Map) return null;
      final specRaw = decoded['comfyhubLayout'];
      if (specRaw is! Map) return null;
      final spec = LayoutSpec.fromJson(Map<String, Object?>.from(specRaw));
      if (spec.regions.isEmpty) return null;
      final head = text.substring(text.indexOf(header) + header.length);
      final note = head.split('\n\n').first.trim();
      return LayoutResubmit(spec: spec, note: note == defaultNote ? '' : note);
    } catch (_) {
      // 手改坏的消息 → 当普通文本，不抛
      return null;
    }
  }
}
