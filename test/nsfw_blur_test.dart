// NSFW 模糊选项的回归（用户 2026-09-20 要求：
// 「再加一个 NSFW 模糊选项，开启后 NSFW 内容的预览图模糊显示。这两个选项放一起」）。
//
// 盯住三件事，缺一条这个功能就是"看着开了、其实没用"：
//   1) 后端说 `effectiveBlur=false` 时**不打码**（没开 NSFW 的人不能被糊一脸）；
//   2) 后端说开了时**画面真的被 `ImageFiltered` 糊住**，而且点一下能解开那一张；
//   3) "点了看过"只记在内存里：关掉 App 再打开（新 store）重新打码。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/state/nsfw_store.dart';
import 'package:viewer/widgets/nsfw_blur.dart';

/// 一个只回答 `/api/ai/nsfw` 的后端。
MockClient _nsfwBackend({
  required bool enabled,
  required bool blur,
  void Function(Map<String, dynamic> body)? onWrite,
}) {
  return MockClient((request) async {
    if (request.method == 'PUT') {
      final body = jsonDecode(request.body) as Map<String, dynamic>;
      onWrite?.call(body);
      final e = body['enabled'] == true;
      final b = body['blur'] == true;
      return http.Response(
        jsonEncode({'enabled': e, 'blur': b, 'effectiveBlur': e && b, 'promptVersion': 'nsfw-v1'}),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    }
    return http.Response(
      jsonEncode({
        'enabled': enabled,
        'blur': blur,
        'effectiveBlur': enabled && blur,
        'promptVersion': 'nsfw-v1',
      }),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Future<NsfwStore> _store(MockClient client) async {
  final store = NsfwStore(api: AiApiClient('http://127.0.0.1:8080', client: client));
  addTearDown(store.dispose);
  await store.load(silent: true);
  return store;
}

Widget _wrap(NsfwStore store, {String key = 'http://x/1'}) => ChangeNotifierProvider<NsfwStore>.value(
      value: store,
      child: MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 200,
            height: 200,
            child: NsfwBlur(
              blurKey: key,
              child: Container(key: const ValueKey('content'), color: Colors.red),
            ),
          ),
        ),
      ),
    );

void main() {
  testWidgets('两个开关都关：原图直接显示，没有模糊层', (tester) async {
    final store = await _store(_nsfwBackend(enabled: false, blur: false));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(store.blurEnabled, isFalse);
    expect(find.byType(ImageFiltered), findsNothing);
    expect(find.text('已模糊 · 点击查看'), findsNothing);
  });

  testWidgets('只开 NSFW 模式、没开模糊：照样不打码', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: false));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(store.nsfwEnabled, isTrue);
    expect(store.blurEnabled, isFalse, reason: '模糊是从属于 NSFW 的第二个开关');
    expect(find.byType(ImageFiltered), findsNothing);
  });

  testWidgets('两个都开：图被糊住，点一下解开这一张', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(store.blurEnabled, isTrue);
    expect(find.byType(ImageFiltered), findsOneWidget);
    expect(find.text('已模糊 · 点击查看'), findsOneWidget);

    await tester.tap(find.text('已模糊 · 点击查看'));
    await tester.pumpAndSettle();

    expect(find.byType(ImageFiltered), findsNothing, reason: '点过之后就解开这一张');
    expect(find.byKey(const ValueKey('content')), findsOneWidget);
  });

  testWidgets('解开只针对那一张：别的图仍然糊着', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(
      ChangeNotifierProvider<NsfwStore>.value(
        value: store,
        child: MaterialApp(
          home: Scaffold(
            body: Column(
              children: [
                SizedBox(width: 120, height: 120, child: NsfwBlur(blurKey: 'a', child: const SizedBox())),
                SizedBox(width: 120, height: 120, child: NsfwBlur(blurKey: 'b', child: const SizedBox())),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(ImageFiltered), findsNWidgets(2));

    await tester.tap(find.text('已模糊 · 点击查看').first);
    await tester.pumpAndSettle();

    expect(find.byType(ImageFiltered), findsOneWidget, reason: '只解开点过的那一张');
  });

  testWidgets('关掉打码后重新打开：又是打码状态（"看过"不落盘）', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await tester.tap(find.text('已模糊 · 点击查看'));
    await tester.pumpAndSettle();
    expect(find.byType(ImageFiltered), findsNothing);

    // 关掉模糊：清掉"看过"的记录
    await store.save(enabled: true, blur: false);
    await tester.pumpAndSettle();
    expect(store.blurEnabled, isFalse);

    // 再打开：同一张图重新打码
    await store.save(enabled: true, blur: true);
    await tester.pumpAndSettle();
    expect(store.blurEnabled, isTrue);
    expect(find.byType(ImageFiltered), findsOneWidget,
        reason: '重新开启后不能还是"看过"的状态');
  });

  testWidgets('两个开关的从属关系：写回后端时能各自独立控制', (tester) async {
    final writes = <Map<String, dynamic>>[];
    final store = await _store(
      _nsfwBackend(enabled: false, blur: false, onWrite: writes.add),
    );

    await store.save(enabled: true, blur: false);
    expect(store.nsfwEnabled, isTrue);
    expect(store.blurEnabled, isFalse);

    await store.save(enabled: true, blur: true);
    expect(store.blurEnabled, isTrue);

    expect(writes, [
      {'enabled': true, 'blur': false},
      {'enabled': true, 'blur': true},
    ]);
  });

  testWidgets('后端读不到时按"不打码"处理（不能把应用糊成一片）', (tester) async {
    final client = MockClient((request) async => http.Response('boom', 500));
    final store = NsfwStore(api: AiApiClient('http://127.0.0.1:8080', client: client));
    addTearDown(store.dispose);
    await store.load(silent: true);

    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(store.error, isNotNull);
    expect(store.blurEnabled, isFalse);
    expect(find.byType(ImageFiltered), findsNothing);
  });
}
