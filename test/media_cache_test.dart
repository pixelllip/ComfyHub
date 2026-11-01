import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/media_cache.dart';

void main() {
  late Directory root;

  setUp(() async {
    root = Directory(
      '${Directory.current.path}/build/.scratch/media-cache-${DateTime.now().microsecondsSinceEpoch}',
    );
    await root.create(recursive: true);
  });

  tearDown(() async {
    if (await root.exists()) await root.delete(recursive: true);
  });

  test('缓存成功落盘；第二次命中不再发请求', () async {
    var hits = 0;
    final api = ApiClient(
      'http://desk.test',
      client: MockClient((request) async {
        hits++;
        expect(request.url.path, '/api/media/7/file');
        return http.Response.bytes([1, 2, 3, 4], 200);
      }),
    );
    final cache = MediaCache(root: root, enabled: true);

    final first = await cache.ensure(
      7,
      url: '/api/media/7/file',
      thumb: false,
      api: api,
    );
    expect(first, isNotNull);
    expect(await first!.readAsBytes(), [1, 2, 3, 4]);

    final second = await cache.ensure(
      7,
      url: '/api/media/7/file',
      thumb: false,
      api: api,
    );
    expect(second?.path, first.path);
    expect(hits, 1, reason: '命中磁盘缓存后不许再打一次网络');
  });

  test('下载失败返回 null，且不落下半截文件', () async {
    final api = ApiClient(
      'http://desk.test',
      client: MockClient((request) async => http.Response('boom', 500)),
    );
    final cache = MediaCache(root: root, enabled: true);

    expect(
      await cache.ensure(8, url: '/api/media/8/file', thumb: false, api: api),
      isNull,
    );
    expect(await cache.cached(8, thumb: false), isNull);
  });

  test('缩略图与原件分开存（不会拿 512px 缩略图当详情大图）', () async {
    final api = ApiClient(
      'http://desk.test',
      client: MockClient((request) async {
        final bytes = request.url.path.endsWith('/thumb') ? [9] : [1, 1, 1];
        return http.Response.bytes(bytes, 200);
      }),
    );
    final cache = MediaCache(root: root, enabled: true);

    final thumb = await cache.ensure(
      9,
      url: '/api/media/9/thumb',
      thumb: true,
      api: api,
    );
    final original = await cache.ensure(
      9,
      url: '/api/media/9/file',
      thumb: false,
      api: api,
    );
    expect(thumb!.path, isNot(original!.path));
    expect(await thumb.readAsBytes(), [9]);
    expect(await original.readAsBytes(), [1, 1, 1]);
  });
}
