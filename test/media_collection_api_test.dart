// 产物归属（集合）的 API 形状（2026-10-18 手动集合）。
//
// 为什么单独钉这一条：`clearCollection` 与 `collectionId` 在 `updateMedia` 里是
// **互斥**的（clear 优先），而且它们决定了两件在界面上一模一样、语义完全不同的事：
//   · `clearCollection: true` = 用户**移出**集合 → 后端同时打 `collection_opt_out = 1`；
//   · `collectionId: N`      = 用户**移进**集合 → 后端清掉那个标记。
// 写错任何一边的症状都是静默的：图还在那儿，只是"下次启动又回到原组"。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/models/models.dart';

Map<String, dynamic> _mediaJson() => {
  'id': 7,
  'kind': 'IMAGE',
  'title': 'a.png',
  'originalName': 'a.png',
  'storedName': 'a.png',
  'mimeType': 'image/png',
  'sizeBytes': 1,
  'favorite': false,
  'source': 'ComfyUI',
  'createdAt': '2026-10-18T01:00:00.000Z',
};

void main() {
  late Map<String, dynamic> lastBody;

  ApiClient client() => ApiClient(
    'http://127.0.0.1:1',
    client: MockClient((request) async {
      if (request.method == 'PATCH') {
        lastBody = Map<String, dynamic>.from(jsonDecode(request.body) as Map);
      }
      return http.Response(
        jsonEncode(_mediaJson()),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    }),
  );

  setUp(() => lastBody = {});

  test('移出集合：发 clearCollection，**不发** collectionId', () async {
    await client().updateMedia(7, clearCollection: true);
    expect(lastBody['clearCollection'], isTrue);
    expect(
      lastBody.containsKey('collectionId'),
      isFalse,
      reason: '两个一起发语义就含糊了 —— 后端以 clear 优先，但请求里不该同时出现',
    );
  });

  test('移进集合：发 collectionId，**不发** clearCollection', () async {
    await client().updateMedia(7, collectionId: 3);
    expect(lastBody['collectionId'], 3);
    expect(lastBody.containsKey('clearCollection'), isFalse);
  });

  test('两个都给时以"移出"为准（与 clearPrompt / promptId 同一套规矩）', () async {
    await client().updateMedia(7, collectionId: 3, clearCollection: true);
    expect(lastBody['clearCollection'], isTrue);
    expect(lastBody.containsKey('collectionId'), isFalse);
  });

  test('什么都不给时不动归属（这一列一个字都不许碰）', () async {
    await client().updateMedia(7, favorite: true);
    expect(lastBody.containsKey('clearCollection'), isFalse);
    expect(lastBody.containsKey('collectionId'), isFalse);
  });

  test('列集合：解析手动 / 算法的两条，缺 title 不许报错', () async {
    final api = ApiClient(
      'http://127.0.0.1:1',
      client: MockClient((request) async {
        expect(request.url.path, '/api/media/collections');
        return http.Response(
          jsonEncode([
            {
              'id': 5,
              'kind': 'IMAGE',
              'origin': 'manual',
              'title': '精选',
              'size': 3,
            },
            // 算法分出来的**没有 title 这个键**（后端 skip_serializing_if）
            {'id': 9, 'kind': 'VIDEO', 'origin': 'auto', 'size': 2},
          ]),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }),
    );
    final rows = await api.listCollections();
    expect(rows, hasLength(2));
    expect(rows.first.isManual, isTrue);
    expect(rows.first.label, '精选');
    expect(rows.last.isManual, isFalse);
    expect(rows.last.label, '算法分组（2 张）', reason: '算法分组没有名字，界面要如实说');
  });

  test('新建集合：发 mediaIds（+ 可选 title），空名字不发 title', () async {
    late Map<String, dynamic> body;
    final api = ApiClient(
      'http://127.0.0.1:1',
      client: MockClient((request) async {
        expect(request.method, 'POST');
        expect(request.url.path, '/api/media/collections');
        body = Map<String, dynamic>.from(jsonDecode(request.body) as Map);
        return http.Response(
          jsonEncode({
            'id': 11,
            'kind': 'IMAGE',
            'origin': 'manual',
            'title': 'A',
            'size': 2,
          }),
          201,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }),
    );

    final created = await api.createCollection([3, 4], title: 'A');
    expect(body['mediaIds'], [3, 4]);
    expect(body['title'], 'A');
    expect(created.id, 11);
    expect(created.isManual, isTrue);

    await api.createCollection([5], title: '   ');
    expect(body.containsKey('title'), isFalse, reason: '空白名字等于没有名字，别发一个空串过去');
  });

  test('一个集合只装一种：accepts 判据大小写不敏感', () {
    const image = MediaCollection(
      id: 1,
      kind: 'IMAGE',
      origin: 'manual',
      size: 1,
    );
    expect(image.accepts('IMAGE'), isTrue);
    expect(image.accepts('image'), isTrue);
    expect(image.accepts('VIDEO'), isFalse, reason: '图像集合装不下视频 —— 目标要置灰');
  });

  test('mediaPath：磁盘路径由后端给，前端不拼', () async {
    final api = ApiClient(
      'http://127.0.0.1:1',
      client: MockClient((request) async {
        expect(request.url.path, '/api/media/7/path');
        return http.Response(
          jsonEncode({'path': r'D:\storage\media\abc.png'}),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }),
    );
    expect(await api.mediaPath(7), r'D:\storage\media\abc.png');
  });

  test('重新自动整理：POST 到 regroup，回条数', () async {
    final api = ApiClient(
      'http://127.0.0.1:1',
      client: MockClient((request) async {
        expect(request.method, 'POST');
        expect(request.url.path, '/api/media/collections/regroup');
        return http.Response(
          jsonEncode({'regrouped': 12}),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }),
    );
    expect(await api.regroupCollections(), 12);
  });
}
