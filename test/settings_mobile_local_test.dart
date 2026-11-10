// 「本机数据与同步」页（`lib/pages/settings_mobile_local_page.dart`）的回归。
//
// 它取代的是桌面端的「本地服务（后端 + 数据库）」——手机上真正拥有的那一套是
// 本机库 + 上行队列 + 桌面连接。
//
// 这里钉两件事：
//   ① 待上行条数**来自本地库**（不出网），同步完要立刻变成 0 并把结果说出来；
//   ② 「从桌面拉取」成功时说的那句**不许**沿用上一条旧提示（会让人以为又推了一次）。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/run_event_source.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/settings_mobile_local_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/state/library_store.dart';

/// 只实现这一页真正会调用的几个方法；其余走 `noSuchMethod`（不会被调到）。
class _FakeLocalSource implements RunEventSource {
  int pending;
  int syncCalls = 0;
  int pullCalls = 0;

  _FakeLocalSource({this.pending = 3});

  @override
  bool get isLocal => true;

  @override
  Future<int> pendingOutbox() async => pending;

  @override
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  }) async {
    syncCalls++;
    final applied = pending;
    pending = 0;
    return (ok: true, applied: applied, pending: 0, error: null);
  }

  @override
  Future<({bool ok, String mode, int applied, int cursor, String? error})>
  pullChanges({String? baseUrl, String? token}) async {
    pullCalls++;
    return (ok: true, mode: 'changes', applied: 2, cursor: 9, error: null);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

Future<void> _pump(
  WidgetTester tester,
  _FakeLocalSource source,
) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = ApiClient(
    settings.baseUrl,
    client: MockClient((_) async => http.Response('{}', 200)),
  );
  final library = LibraryStore(settings, api: api);
  final workspace = AiWorkspaceStore(source: source);
  addTearDown(library.dispose);
  addTearDown(workspace.dispose);

  tester.view.physicalSize = const Size(400, 1800);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<LibraryStore>.value(value: library),
        ChangeNotifierProvider<AiWorkspaceStore>.value(value: workspace),
      ],
      child: const MaterialApp(home: MobileLocalSettingsPage()),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('待上行条数来自本机库；同步完归零并把结果说出来', (tester) async {
    final source = _FakeLocalSource(pending: 3);
    await _pump(tester, source);

    expect(find.text('3 条'), findsOneWidget, reason: '待上行要如实显示本机队列长度');
    expect(tester.takeException(), isNull);

    await tester.tap(find.text('立即同步到桌面'));
    await tester.pumpAndSettle();

    expect(source.syncCalls, 1);
    expect(
      find.text('3 条'),
      findsNothing,
      reason: '推完了队列里就不该还有 3 条',
    );
    expect(
      find.text('0 条'),
      findsNWidgets(2),
      reason: '会话数与待上行都归零（这一页只有这两行是"N 条"）',
    );
    expect(find.textContaining('已把本机 3 条记录同步到桌面'), findsOneWidget);
  });

  testWidgets('从桌面拉取：成功但与上次无关时，不许沿用上一条同步提示', (tester) async {
    final source = _FakeLocalSource(pending: 2);
    await _pump(tester, source);

    await tester.tap(find.text('从桌面拉取'));
    await tester.pumpAndSettle();

    expect(source.pullCalls, 1);
    expect(find.textContaining('已从桌面同步 2 条记录'), findsOneWidget);
    // 上行一句都没发过 —— 这句是"拉"的结果，不是"推"的成功
    expect(source.syncCalls, 0);
  });
}
