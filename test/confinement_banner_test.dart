import 'dart:convert';

import 'package:material_ui/material_ui.dart';
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
  'imageInsideProjectRoot': true,
  'checkedPaths': ['产物目录：D:\\Comfy-Desktop\\ComfyUI-Shared\\output'],
  'blockedPaths': [
    '产物目录 D:\\Comfy-Desktop\\ComfyUI-Shared\\output（permissiondenied（os error 5））',
  ],
  'summary': '受限（写不进 …）',
  'remedy': '把整套 ComfyHub 放到**工作区外**再启动',
};

const _free = {
  'restricted': false,
  'imageInsideProjectRoot': true,
  'checkedPaths': ['产物目录：D:\\dist\\ComfyHub\\storage'],
  'blockedPaths': <Object>[],
  'summary': '正常（探过 1 处）',
};

ApiClient _client(Object? body, {int status = 200}) {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  return ApiClient(
    settings.baseUrl,
    client: MockClient(
      (_) async => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json; charset=utf-8'},
      ),
    ),
  );
}

void main() {
  testWidgets('不受限 / 问不到时横幅不占位', (tester) async {
    for (final info in <ConfinementInfo?>[
      null,
      // ⚠ 发布包的真实回包：映像**也在**运行根目录里（`<包根>\server\…`），
      //   但那不是被沙箱框住的地方 —— 所以判据为真、受限为假时**绝不能**亮横幅。
      ConfinementInfo.fromJson(_free),
    ]) {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: ConfinementBanner(info: info)),
        ),
      );
      expect(find.textContaining('写不进项目外'), findsNothing);
      expect(find.byIcon(Icons.lock_outline), findsNothing);
    }
  });

  testWidgets('受限时把判据与修法一起说清楚', (tester) async {
    final info = ConfinementInfo.fromJson(_restricted);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ConfinementBanner(info: info, onOpenSettings: () {}),
        ),
      ),
    );

    expect(find.text(info.headline), findsOneWidget);
    // 判据：映像路径 + "在运行根目录内"
    expect(find.textContaining(r'server-rs\target\release'), findsOneWidget);
    expect(find.textContaining('在运行根目录内'), findsOneWidget);
    // 写不进的是哪几个目录（带系统原话）
    expect(find.textContaining('os error 5'), findsOneWidget);
    // 修法取后端给的那一份，界面不另写
    expect(find.textContaining('工作区外'), findsWidgets);

    // 点「去设置」要走回调
    var tapped = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ConfinementBanner(
            info: info,
            onOpenSettings: () => tapped = true,
          ),
        ),
      ),
    );
    await tester.tap(find.text('去设置'));
    expect(tapped, isTrue);
  });

  test('模型层：受限与不受限的措辞都要说人话', () {
    final restricted = ConfinementInfo.fromJson(_restricted);
    expect(restricted.headline, contains('运行根目录'));
    expect(restricted.detail, contains('os error 5'));

    // 判据为 false（映像不在运行根目录里却仍写不进）——不许硬说成"落在根目录里"
    final odd = ConfinementInfo.fromJson({
      ..._restricted,
      'imageInsideProjectRoot': false,
    });
    expect(odd.headline, '后端写不进项目外的目录');
    expect(odd.headline, isNot(contains('运行根目录')));
  });

  test('老后端没有这个端点（404）→ 当"不知道"，不许当成不受限也不许抛', () async {
    expect(
      await _client({'error': 'not found'}, status: 404).confinement(),
      isNull,
    );
    expect((await _client(_free).confinement())!.restricted, isFalse);
    expect((await _client(_restricted).confinement())!.restricted, isTrue);
  });
}
