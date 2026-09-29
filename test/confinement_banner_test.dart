// 「后端受限」横幅的三条回归：
//
//   1) 不受限 / 问不到时**一个像素都不占**（别制造噪音）
//   2) 受限时**判据与结论一起说**（"因为后端映像在工作区内"），修法取后端给的那一份
//   3) 老后端没有 /api/system/confinement（404）→ 当"不知道"，**不许当成"不受限"也不许报错**
//
// 为什么要有第 2 条：2026-09-29 用户 bug 里，模型把 os error 5 诊断成"input 目录权限不够"，
// 用户去改 ACL —— 真因是"映像在工作区内的后端写不了工作区外"（AGENTS §12）。
// 界面既然要显示，就必须把**判据**显示出来，否则又是一轮"现象对、结论错"。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/widgets/confinement_banner.dart';

const _restricted = {
  'restricted': true,
  'pid': 5516,
  'projectRoot': r'D:\myProject\FlutterProject\viewer',
  'serverImage': r'D:\myProject\FlutterProject\viewer\server-rs\target\release\comfyhub-server-rs.exe',
  'imageInsideProject': true,
  'checkedPaths': ['产物目录：D:\\Comfy-Desktop\\ComfyUI-Shared\\output'],
  'blockedPaths': ['产物目录 D:\\Comfy-Desktop\\ComfyUI-Shared\\output（permissiondenied（os error 5））'],
  'summary': '受限（写不进 …）',
  'remedy': '把整套 ComfyHub 放到**工作区外**再启动',
};

const _free = {
  'restricted': false,
  'imageInsideProject': false,
  'checkedPaths': ['产物目录：D:\\dist\\ComfyHub\\storage'],
  'blockedPaths': <Object>[],
  'summary': '正常（探过 1 处）',
};

ApiClient _client(Object? body, {int status = 200}) {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  return ApiClient(
    settings.baseUrl,
    client: MockClient((_) async => http.Response(
          jsonEncode(body),
          status,
          headers: {'content-type': 'application/json; charset=utf-8'},
        )),
  );
}

void main() {
  testWidgets('不受限 / 问不到时横幅不占位', (tester) async {
    for (final info in <ConfinementInfo?>[
      null,
      ConfinementInfo.fromJson(_free),
    ]) {
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: ConfinementBanner(info: info)),
      ));
      expect(find.textContaining('写不进项目外'), findsNothing);
      expect(find.byIcon(Icons.lock_outline), findsNothing);
    }
  });

  testWidgets('受限时把判据与修法一起说清楚', (tester) async {
    final info = ConfinementInfo.fromJson(_restricted);
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(body: ConfinementBanner(info: info, onOpenSettings: () {})),
    ));

    expect(find.text(info.headline), findsOneWidget);
    // 判据：映像路径 + "在工作区内"（标题里就有，后端映像那一行再点明一次）
    expect(find.textContaining(r'server-rs\target\release'), findsOneWidget);
    expect(find.textContaining('在工作区内 → 受限'), findsOneWidget);
    // 写不进的是哪几个目录（带系统原话）
    expect(find.textContaining('os error 5'), findsOneWidget);
    // 修法取后端给的那一份，界面不另写
    expect(find.textContaining('工作区外'), findsWidgets);

    // 点「去设置」要走回调
    var tapped = false;
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: ConfinementBanner(info: info, onOpenSettings: () => tapped = true),
      ),
    ));
    await tester.tap(find.text('去设置'));
    expect(tapped, isTrue);
  });

  test('模型层：受限与不受限的措辞都要说人话', () {
    final restricted = ConfinementInfo.fromJson(_restricted);
    expect(restricted.headline, contains('工作区内'));
    expect(restricted.detail, contains('os error 5'));

    // 判据为 false（后端不在工作区内却仍写不进）——不许硬说成"被关在项目里"
    final odd = ConfinementInfo.fromJson({
      ..._restricted,
      'imageInsideProject': false,
    });
    expect(odd.headline, '后端写不进项目外的目录');
    expect(odd.headline, isNot(contains('工作区内')));
  });

  test('老后端没有这个端点（404）→ 当"不知道"，不许当成不受限也不许抛', () async {
    expect(await _client({'error': 'not found'}, status: 404).confinement(), isNull);
    expect((await _client(_free).confinement())!.restricted, isFalse);
    expect((await _client(_restricted).confinement())!.restricted, isTrue);
  });
}
