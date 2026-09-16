// 画廊分页的回归测试（bug 清单第 2 条）：
//
//   删除数量大于单页承载时，执行删除会导致提示"画廊为空"，即便其实没有删完 ——
//   刷新本页才恢复。
//
// 成因：删除前停在最后一页，删完之后这个页码已经不存在了，后端对越界页码返回空数组。
// 期望：`LibraryStore.refreshMedia()` 自己回到最后一页，用户感觉不到。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/state/library_store.dart';

/// 假后端：库里始终有 [total] 个产物，**每页 3 个**（真实每页 24，这里缩小到 3 才能
/// 用小数据量造出"多页 + 删完当前页"的场景）；对越界页码返回空 items，
/// 但 `total` / `pages` 仍然如实给出（真实 `MediaRepo` 就是这个行为）。
class _FakeLibrary {
  _FakeLibrary({required this.total});

  static const size = 3;

  int total;
  final List<String> requests = [];

  int get pages => total == 0 ? 0 : ((total + size - 1) ~/ size);

  List<Map<String, dynamic>> _items(int page) {
    final start = (page - 1) * size;
    if (start >= total) return const [];
    final end = (start + size).clamp(0, total);
    return [
      for (var i = start; i < end; i++)
        {
          'id': i + 1,
          'kind': 'IMAGE',
          'title': 'shot-${i + 1}.png',
          'originalName': 'shot-${i + 1}.png',
          'storedName': 's${i + 1}.png',
          'mimeType': 'image/png',
          'sizeBytes': 1024,
          'favorite': false,
          'source': 'ComfyUI',
          'createdAt': '2026-09-16T01:00:00.000Z',
        },
    ];
  }

  MockClient client() => MockClient((request) async {
        final path = request.url.path;
        final page = int.tryParse(request.url.queryParameters['page'] ?? '1') ?? 1;
        requests.add('${request.method} $path?page=$page');

        Object? body;
        var status = 200;
        if (path == '/api/media') {
          body = {
            'items': _items(page),
            'total': total,
            'page': page,
            'size': size,
            'pages': pages,
          };
        } else if (RegExp(r'^/api/media/\d+$').hasMatch(path)) {
          total -= 1; // 后端真的删掉了
          body = {'deleted': true, 'id': int.parse(path.split('/').last)};
        } else if (path == '/api/tags' || path == '/api/tags/categories') {
          body = <Object>[];
        } else if (path == '/api/stats') {
          body = {
            'prompts': 0,
            'media': total,
            'tags': 0,
            'favoritePrompts': 0,
            'favoriteMedia': 0,
            'byKind': <String, int>{},
            'byMediaKind': <String, int>{},
          };
        } else if (path == '/api/prompts') {
          body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
        } else {
          status = 404;
          body = {'error': 'not found: $path'};
        }

        return http.Response(
          jsonEncode(body),
          status,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      });
}

Future<(LibraryStore, _FakeLibrary)> _makeStore(int total) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final fake = _FakeLibrary(total: total);
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: fake.client()));
  return (store, fake);
}

void main() {
  test('删完当前页之后自动回到最后一页，而不是显示空画廊', () async {
    final (store, fake) = await _makeStore(4); // 每页 3 条 → 第 1 页 3 条，第 2 页 1 条

    await store.refreshMedia();
    expect(store.media.pages, 2);

    store.setMediaPage(2);
    await store.refreshMedia();
    expect(store.media.items.map((m) => m.id), [4]);

    // 删掉第 2 页上唯一那个产物 → 只剩 3 条、只剩 1 页
    await store.deleteMedia(4);

    expect(store.mediaPage, 1, reason: '越界页码应该被收敛回最后一页');
    expect(store.media.items, isNotEmpty, reason: '库里还有 3 条，不能显示成空画廊');
    expect(store.media.total, 3);
    expect(store.media.items.length, 3);
    expect(fake.requests.where((r) => r.contains('page=2')).length, greaterThanOrEqualTo(2),
        reason: '越界时应该重发一次（第一次发现越界，第二次取回正确的一页）');
  });

  test('删除后确实一条都不剩时，才允许显示空画廊', () async {
    final (store, fake) = await _makeStore(2);

    await store.refreshMedia();
    expect(store.media.items, hasLength(2));

    await store.deleteMedia(1);
    await store.deleteMedia(2);

    expect(store.media.total, 0);
    expect(store.media.pages, 0);
    expect(store.media.items, isEmpty);
    expect(store.mediaPage, 1, reason: '页码永远不该变成 0 或负数');
    expect(fake.total, 0);
  });

  test('当前页还有内容时不做多余请求', () async {
    final (store, fake) = await _makeStore(4);

    await store.refreshMedia();
    final before = fake.requests.length;
    await store.refreshMedia();

    expect(fake.requests.length - before, 1, reason: '正常情况下一次刷新只发一个请求');
    expect(store.media.items, hasLength(3));
  });
}
