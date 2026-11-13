// 两条"失败不许装成成功"的回归（2026-11-07 的 catch (_) 审计）：
//
//   ① **缓存目录读不出来** ≠ **缓存是空的**：
//      `MediaCache.stats()` 以前两个分支都回 0，设置页显示「已用 0 B」——
//      否则会被理解为未占用空间，实际是目录读取失败。统计结果携带 `error`，卡片如实显示。
//   ② **拉实时进度失败** ≠ **没有任务**：
//      `AiWorkspaceStore.refreshComfyJobs` 以前 `catch (_)` 吞掉，右侧栏那块
//      整块消失（看起来就是"没在跑"）。现在原因记在 `comfyJobsError`，
//      界面据此说"拉不到"，成功一次就清掉。

import 'dart:io';

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/media_cache.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/widgets/media_cache_card.dart';

void main() {
  group('产物缓存：读不动目录时如实说', () {
    test('stats() 带上原因，而不是回一个「空缓存」', () async {
      // 用一个**文件**当缓存根：`_cacheDir()` 建不出目录 → 扫描必然失败
      final scratch = Directory(
        '${Directory.current.path}/build/.scratch/'
        'media-cache-broken-${DateTime.now().microsecondsSinceEpoch}',
      );
      await scratch.create(recursive: true);
      addTearDown(() async {
        if (await scratch.exists()) await scratch.delete(recursive: true);
      });
      final asFile = File('${scratch.path}/not-a-directory');
      await asFile.writeAsString('x');

      // 路径本身是个文件：`_cacheDir()` 建不出目录，扫描必然失败
      final cache = MediaCache(root: Directory(asFile.path), enabled: true);
      final stats = await cache.stats();

      expect(stats.totalBytes, 0, reason: '读不出来时数字只能是 0');
      expect(stats.error, isNotNull, reason: '必须带上原因：否则"列不出目录"会被读成"缓存没占空间"');
    });

    testWidgets('设置页那张卡把「数字不可信」写出来', (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: MediaCacheCard(
              stats: const MediaCacheStats(
                thumbBytes: 0,
                origBytes: 0,
                thumbCount: 0,
                origCount: 0,
                thumbBudget: kThumbCacheBudgetBytes,
                origBudget: kOriginalCacheBudgetBytes,
                error: '拒绝访问',
              ),
              budgetMb: 1536,
              busy: false,
              onBudgetChanged: (_) {},
              onClear: () {},
            ),
          ),
        ),
      );

      expect(find.textContaining('缓存目录读不出来'), findsOneWidget);
      expect(find.textContaining('不可信'), findsOneWidget);
    });
  });

  group('实时进度：拉不到时不许装成「没有任务」', () {
    test('失败记原因，成功一次就清掉', () async {
      var fail = true;
      final api = AiApiClient(
        'http://desk.test',
        client: MockClient((request) async {
          if (request.url.path == '/api/capture/jobs' && !fail) {
            return http.Response(
              '{"queueRunning":0,"queuePending":0,"comfyReachable":true,'
              '"submissions":[]}',
              200,
              headers: {'content-type': 'application/json'},
            );
          }
          return http.Response('backend down', 503);
        }),
      );
      final store = AiWorkspaceStore(api: api);

      await store.refreshComfyJobs();
      expect(
        store.comfyJobsError,
        isNotNull,
        reason: '拉不到要说"拉不到"，不能与"当前没有任务"混成一个样子',
      );

      fail = false;
      await store.refreshComfyJobs();
      expect(store.comfyJobsError, isNull, reason: '成功一次之后旧错误必须清掉');
      expect(store.comfyJobs.comfyReachable, isTrue);
    });
  });
}
