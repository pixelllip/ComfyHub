// NSFW 模糊选项的回归（用户 2026-09-20 要求：
// 「再加一个 NSFW 模糊选项，开启后 NSFW 内容的预览图模糊显示。这两个选项放一起」）。
//
// ⚠ 同日晚上按用户 bug 重写过一版：「NSFW内容模糊——怎么丝毫不区分正常内容全部模糊呢？」
// 那版的判据只有"用户开着模糊开关"，于是**所有**缩略图都被糊住（连测试图和音频封面）。
// 现在判据是两条：**这一条被标记过 NSFW**（`nsfw: true`）+ 用户开着模糊开关。
//
// 盯住五件事，缺一条这个功能就是"看着开了、其实没用"或"用起来反而更糟"：
//   1) 没标记的内容**一张都不糊**（哪怕开关开着）—— 这是那次 bug 的本身；
//   2) 后端说 `effectiveBlur=false` 时也不打码；
//   3) 标记过的内容**真的被 `ImageFiltered` 糊住**，打开详情（`reveal`）后不再糊；
//   4) 打码层**不吃点击**：点下去要能走到"打开详情"那一层（用户要的是点开就能看）；
//   5) "看过"只记在内存里：关掉 App 再打开（新 store）重新打码。

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

/// 一张图：默认是"被标记为 NSFW"的那一种（`nsfw: true`）。
Widget _wrap(
  NsfwStore store, {
  String key = 'media:1',
  bool nsfw = true,
  VoidCallback? onTap,
}) =>
    ChangeNotifierProvider<NsfwStore>.value(
      value: store,
      child: MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 200,
            height: 200,
            child: GestureDetector(
              onTap: onTap,
              child: NsfwBlur(
                nsfw: nsfw,
                revealKey: key,
                child: Container(key: const ValueKey('content'), color: Colors.red),
              ),
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

  // 用户 bug 的本身：开关一开，**没被标记过的正常内容也被糊了**。
  testWidgets('没标记为 NSFW 的内容：开关开着也一张都不糊（用户 bug）', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(_wrap(store, nsfw: false));
    await tester.pumpAndSettle();

    expect(store.blurEnabled, isTrue, reason: '前提：用户确实开着"预览图模糊"');
    expect(find.byType(ImageFiltered), findsNothing,
        reason: '这一条没被标记过 → 一个像素都不该动（风景图 / 音频封面都在这一类里）');
    expect(find.text('已模糊 · 点击查看'), findsNothing);
  });

  testWidgets('标记为 NSFW + 两个开关都开：图被糊住，打开详情后不再糊', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(store.blurEnabled, isTrue);
    expect(find.byType(ImageFiltered), findsOneWidget);
    expect(find.text('已模糊 · 点击查看'), findsOneWidget);

    // 用户点开详情 = 主动要看（用户 2026-09-20：「打开详情之后就没必要模糊了」）。
    // 详情页不打码，并顺手把"这一条看过了"记下来 —— 回到列表也不再糊。
    store.reveal('media:1');
    await tester.pumpAndSettle();

    expect(find.byType(ImageFiltered), findsNothing, reason: '看过之后就解开这一条');
    expect(find.byKey(const ValueKey('content')), findsOneWidget);
  });

  testWidgets('打码层不吃点击：点下去要能走到"打开详情"那一层', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    var taps = 0;
    await tester.pumpWidget(_wrap(store, onTap: () => taps++));
    await tester.pumpAndSettle();

    expect(find.byType(ImageFiltered), findsOneWidget, reason: '前提：它确实被打码了');
    // 点在打码层正中间（用 tapAt：那层是 IgnorePointer，按 widget 找会报"没点到"）
    await tester.tapAt(tester.getCenter(find.byType(NsfwBlur)));
    await tester.pumpAndSettle();
    expect(taps, 1, reason: '第一下就该是"打开详情"，而不是"第一下只解锁、第二下才打开"');
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
                SizedBox(
                    width: 120,
                    height: 120,
                    child: NsfwBlur(nsfw: true, revealKey: 'media:a', child: const SizedBox())),
                SizedBox(
                    width: 120,
                    height: 120,
                    child: NsfwBlur(nsfw: true, revealKey: 'media:b', child: const SizedBox())),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(ImageFiltered), findsNWidgets(2));

    store.reveal('media:a');
    await tester.pumpAndSettle();

    expect(find.byType(ImageFiltered), findsOneWidget, reason: '只解开看过的那一张');
  });

  testWidgets('关掉打码后重新打开：又是打码状态（"看过"不落盘）', (tester) async {
    final store = await _store(_nsfwBackend(enabled: true, blur: true));
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    store.reveal('media:1');
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
