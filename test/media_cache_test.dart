import 'dart:async';
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

  // ---------------------------------------------------------------------------
  //  上限 / LRU / 清理（缓存需设置上限）
  // ---------------------------------------------------------------------------

  /// 造一个"每次回 [size] 字节"的下载器，并按请求顺序返回不同的填充字节，
  /// 便于断言"留下的是哪一条"。
  ApiClient sizedApi(int size, {void Function()? onRequest}) => ApiClient(
    'http://desk.test',
    client: MockClient((request) async {
      onRequest?.call();
      expect(size, greaterThan(0));
      return http.Response.bytes(List<int>.filled(size, size % 251), 200);
    }),
  );

  test('超上限按最久未用淘汰；命中过的那一条留下来', () async {
    final api = sizedApi(40);
    final cache = MediaCache(
      root: root,
      enabled: true,
      thumbBudget: 100,
      origBudget: 100,
    );

    Future<bool> onDisk(int id) async =>
        (await cache.fileFor(id, thumb: false)).exists();

    // 三条 40 字节：写第三条时总量 120 > 100 → 淘汰到 80（只够留两条）
    await cache.ensure(1, url: '/api/media/1/file', thumb: false, api: api);
    await cache.ensure(2, url: '/api/media/2/file', thumb: false, api: api);
    await cache.ensure(3, url: '/api/media/3/file', thumb: false, api: api);

    expect(await onDisk(1), isFalse, reason: '最旧的该被淘汰');
    expect(await onDisk(2), isTrue);
    expect(await onDisk(3), isTrue);

    // 第 2 条刚被访问过（LRU 里挪到最新）→ 再写第 4 条时该淘汰第 3 条。
    // ⚠ 这里用 `fileFor(...).exists()` 而不是 `cached()` 看第 3 条：
    //    后者本身算一次访问，会把断言对象挪到最新，把用例自己搞坏。
    await cache.cached(2, thumb: false);
    await cache.ensure(4, url: '/api/media/4/file', thumb: false, api: api);
    expect(await onDisk(2), isTrue, reason: '命中过的条目不该被当成最久未用');
    expect(await onDisk(3), isFalse);
    expect(await onDisk(4), isTrue);
  });

  test('单条比整档预算还大：不缓存，也不留下 .part 残片', () async {
    final api = sizedApi(200);
    final cache = MediaCache(
      root: root,
      enabled: true,
      thumbBudget: 100,
      origBudget: 100,
    );

    expect(
      await cache.ensure(31, url: '/api/media/31/file', thumb: false, api: api),
      isNull,
      reason: '一条就超预算，留在盘上只会把别人全挤掉',
    );
    expect(await cache.cached(31, thumb: false), isNull);
    final leftovers = await root
        .list()
        .where((e) => e.path.endsWith('.part'))
        .toList();
    expect(leftovers, isEmpty);
    final stats = await cache.stats();
    expect(stats.origBytes, 0);
  });

  test('下载中途断流：当场清掉 .part，也不进统计', () async {
    final api = ApiClient(
      'http://desk.test',
      client: MockClient.streaming((request, bodyStream) async {
        final controller = StreamController<List<int>>();
        controller.add([1, 2, 3]);
        controller.addError(const SocketException('断流'));
        controller.close();
        return http.StreamedResponse(controller.stream, 200);
      }),
    );
    final cache = MediaCache(root: root, enabled: true);

    expect(
      await cache.ensure(71, url: '/api/media/71/file', thumb: false, api: api),
      isNull,
    );
    final leftovers = await root
        .list()
        .where((e) => e.path.endsWith('.part'))
        .toList();
    expect(leftovers, isEmpty, reason: '残片不该留到下一次扫描（那是白白占空间）');
    expect((await cache.stats()).origBytes, 0);
  });

  test('统计与清理：清理后占用归零、文件确实没了', () async {
    final api = sizedApi(30);
    final cache = MediaCache(
      root: root,
      enabled: true,
      thumbBudget: 1000,
      origBudget: 1000,
    );
    await cache.ensure(41, url: '/api/media/41/thumb', thumb: true, api: api);
    await cache.ensure(42, url: '/api/media/42/file', thumb: false, api: api);

    var stats = await cache.stats();
    expect(stats.thumbBytes, 30);
    expect(stats.thumbCount, 1);
    expect(stats.origBytes, 30);
    expect(stats.origCount, 1);

    expect(await cache.clear(), 60);
    stats = await cache.stats();
    expect(stats.totalBytes, 0);
    expect(await cache.cached(41, thumb: true), isNull);
    expect(await cache.cached(42, thumb: false), isNull);
  });

  test('预算下限有夹子（别让人把上限设成"等于关掉缓存"）', () async {
    final cache = MediaCache(
      root: root,
      enabled: true,
      thumbBudget: 1000,
      origBudget: 1000,
    );
    cache.setOriginalBudget(1);
    expect(cache.originalBudgetBytes, 64 * 1024 * 1024);
    cache.setOriginalBudget(2048 * 1024 * 1024);
    expect(cache.originalBudgetBytes, 2048 * 1024 * 1024);
    // 缩小预算会同时触发一次 trim（此处磁盘为空，执行通过即可）
    await cache.trim();
  });

  test('过期的 .part 残片在扫描时清掉，新鲜的留着', () async {
    final stale = File('${root.path}/media-61.orig.part');
    await stale.writeAsBytes([1, 2, 3]);
    await stale.setLastModified(
      DateTime.now().subtract(const Duration(hours: 2)),
    );
    final fresh = File('${root.path}/media-62.orig.part');
    await fresh.writeAsBytes([4, 5, 6]);

    final cache = MediaCache(root: root, enabled: true);
    await cache.stats();

    expect(await stale.exists(), isFalse, reason: '两小时前的残片是上一个进程留下的');
    expect(await fresh.exists(), isTrue, reason: '刚写的 .part 可能是一次正在进行的下载');
  });

  test('自动缓存判据：大视频不在打开详情时下，小图和未知大小按平台兜底', () {
    expect(
      MediaCache.shouldAutoCache(sizeBytes: 300 * 1024 * 1024, streaming: true),
      isFalse,
    );
    expect(
      MediaCache.shouldAutoCache(sizeBytes: 100 * 1024 * 1024, streaming: true),
      isTrue,
    );
    expect(
      MediaCache.shouldAutoCache(sizeBytes: 0, streaming: true),
      isFalse,
      reason: '不知道大小的视频不自动下（详情页能在线播）',
    );
    expect(
      MediaCache.shouldAutoCache(sizeBytes: 0, streaming: false),
      isTrue,
      reason: '图片就算没给大小也照常缓存',
    );
  });
}
