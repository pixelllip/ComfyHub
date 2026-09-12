// ComfyHub 的单元测试。
//
// 这里只测不依赖后端的纯逻辑（模型解析 / 格式化），
// 需要真实 MySQL + 后端的端到端验证请用 scripts\autorun-app.ps1。

import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/core/formatting.dart';
import 'package:viewer/models/models.dart';

void main() {
  group('模型解析', () {
    test('Prompt.fromJson 能正确解析后端返回的字段', () {
      final p = Prompt.fromJson({
        'id': 7,
        'title': '雨夜霓虹街道',
        'kind': 'IMAGE',
        'positivePrompt': 'cyberpunk city street at night',
        'negativePrompt': 'lowres, blurry',
        'checkpoint': 'sd_xl_base_1.0.safetensors',
        'steps': 32,
        'cfgScale': 7.5,
        'seed': 884213771,
        'width': 1216,
        'height': 832,
        'favorite': true,
        'createdAt': '2026-09-11T01:04:51.489Z',
        'loras': [
          {'name': 'detail-tweaker', 'weight': 0.8},
        ],
        'tags': [
          {'id': 3, 'name': '赛博朋克', 'useCount': 2},
        ],
        'mediaCount': 4,
      });

      expect(p.id, 7);
      expect(p.kind, PromptKind.image);
      expect(p.steps, 32);
      expect(p.cfgScale, 7.5);
      expect(p.favorite, isTrue);
      expect(p.loras.single.name, 'detail-tweaker');
      expect(p.loras.single.weight, 0.8);
      expect(p.tags.single.name, '赛博朋克');
      expect(p.mediaCount, 4);
      expect(p.createdAt, isNotNull);
    });

    test('MediaAsset.fromJson 能解析关联提示词摘要与标签', () {
      final m = MediaAsset.fromJson({
        'id': 1,
        'promptId': 7,
        'kind': 'IMAGE',
        'title': 'neon-street.png',
        'originalName': 'neon-street.png',
        'storedName': 'abc.png',
        'sizeBytes': 253976,
        'width': 1216,
        'height': 832,
        'promptTitle': '雨夜霓虹街道',
        'promptPositive': 'cyberpunk city street at night',
        'promptTags': ['赛博朋克', '夜景'],
        'fileUrl': '/api/media/1/file',
        'thumbUrl': '/api/media/1/thumb',
      });

      expect(m.kind, MediaKind.image);
      expect(m.hasPrompt, isTrue);
      expect(m.promptTitle, '雨夜霓虹街道');
      expect(m.promptTags, contains('赛博朋克'));
      expect(m.thumbUrl, '/api/media/1/thumb');
    });

    test('Paged.fromJson 解析分页信息', () {
      final page = Paged<Prompt>.fromJson(
        {
          'items': <Map<String, dynamic>>[],
          'total': 42,
          'page': 2,
          'size': 20,
          'pages': 3,
        },
        Prompt.fromJson,
      );
      expect(page.total, 42);
      expect(page.pages, 3);
      expect(page.items, isEmpty);
    });
  });

  group('格式化', () {
    test('formatSize', () {
      expect(formatSize(0), '0 B');
      expect(formatSize(512), '512 B');
      expect(formatSize(2048), '2.0 KB');
      // 超过 100 时不再保留小数位
      expect(formatSize(253976), '248 KB');
      expect(formatSize(5 * 1024 * 1024), '5.0 MB');
    });

    test('formatDuration', () {
      expect(formatDuration(null), '');
      expect(formatDuration(0), '');
      expect(formatDuration(9500), '0:09');
      expect(formatDuration(65000), '1:05');
      expect(formatDuration(3725000), '1:02:05');
    });

    test('ellipsis 会压缩空白并截断', () {
      expect(ellipsis('a  b\n c', 10), 'a b c');
      expect(ellipsis('abcdefghij', 4), 'abcd…');
    });

    test('parseHexColor 支持 #RRGGBB 与 #AARRGGBB', () {
      expect(parseHexColor('#FF0000'), isNotNull);
      expect(parseHexColor('FF0000FF'), isNotNull);
      expect(parseHexColor('nope'), isNull);
      expect(parseHexColor(null), isNull);
    });

    test('tagColor 同名标签颜色稳定', () {
      expect(tagColor('赛博朋克'), tagColor('赛博朋克'));
    });
  });
}
