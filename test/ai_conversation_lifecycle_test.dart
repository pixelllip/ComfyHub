// 会话生命周期回归（用户报的 bug ①「打开 ComfyHub 冒出好几条新对话」/
// bug ②「输入框里的字切走就没了」）：
//
//   1. `AiWorkspaceStore.load()` 必须**幂等** —— HomeShell 每次切页都会重建
//      AiHomePage，页面重建不能再加载一遍、更不能又新建一条会话；
//   2. 冷启动落在一条**空会话**上，但"空"要连本地草稿一起看：
//      已经有一条干净的空会话就直接用它，历史遗留的多条空壳只留一条；
//   3. 输入框里打了一半的字，切到别的会话再切回来必须还在 ——
//      旧实现里"清空输入框"会被当成用户删字，把刚取回来的草稿覆盖成空串。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

/// 新建出来的会话 id（按顺序）——用于断言"到底建了几条"。
final List<String> created = [];

/// 被删掉的会话 id（按顺序）。
final List<String> deleted = [];

Map<String, dynamic> _conv(String id, {String title = '新对话', int messageCount = 0}) => {
      'id': id,
      'title': title,
      'messageCount': messageCount,
      'archived': false,
    };

MockClient _fakeBackend(
  List<Map<String, dynamic>> conversations, {
  Map<String, List<Map<String, dynamic>>> messages = const {},
}) {
  var seq = 0;
  return MockClient((request) async {
    final path = request.url.path;
    Object body;

    if (path == '/api/ai/providers') {
      body = [
        {
          'id': 'local-gw',
          'displayName': '本机网关',
          'api': 'openai-completions',
          'baseURL': 'http://127.0.0.1:11434/v1',
          'credentialRef': 'LOCAL_KEY',
          'endpointTrust': 'loopback',
          'enabled': true,
          'revision': 1,
          'credential': {'configured': true, 'source': 'managed', 'writable': true},
        }
      ];
    } else if (path == '/api/ai/providers/local-gw/models') {
      body = [
        {
          'providerId': 'local-gw',
          'id': 'm-text',
          'displayName': '纯文本模型',
          'inputModalities': ['text'],
          'tools': true,
          'reasoning': false,
          'thinkingEfforts': <String, String>{},
          'capabilitySource': 'manual',
          'enabled': true,
        }
      ];
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      final id = 'created-${++seq}';
      created.add(id);
      final row = _conv(id);
      conversations.insert(0, row);
      body = row;
    } else if (path == '/api/ai/conversations') {
      body = conversations;
    } else if (RegExp(r'^/api/ai/conversations/[^/]+/messages$').hasMatch(path)) {
      final id = path.split('/')[4];
      body = messages[id] ?? <Object>[];
    } else if (RegExp(r'^/api/ai/conversations/[^/]+/runs$').hasMatch(path)) {
      // 发一条消息：立刻给回 runId，正文与流式收尾由下面那段固定 SSE 负责
      body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    } else if (RegExp(r'^/api/ai/runs/[^/]+/events$').hasMatch(path)) {
      // 冒号后必须有空格，否则客户端会把整行当未知事件丢掉
      final sse = 'id: 1\nevent: run.started\ndata: {"runId":"r1"}\n\n'
          'id: 2\nevent: message.completed\n'
          'data: {"messageId":"a1","text":"收到","steps":0}\n\n'
          'id: 3\nevent: run.completed\ndata: {"runId":"r1"}\n\n';
      return http.Response.bytes(
        utf8.encode(sse),
        200,
        headers: {'content-type': 'text/event-stream; charset=utf-8'},
      );
    } else if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        request.method == 'DELETE') {
      final id = path.split('/').last;
      deleted.add(id);
      conversations.removeWhere((c) => c['id'] == id);
      body = {'deleted': true, 'id': id};
    } else if (path == '/api/capture') {
      body = {
        'enabled': true,
        'comfyUrl': 'http://127.0.0.1:8188',
        'comfyReachable': true,
        'queueRunning': 0,
        'queuePending': 0,
        'capturedRuns': 0,
        'capturedMedia': 0,
        'recent': <Object>[],
      };
    } else {
      body = <Object>[];
    }

    return http.Response(
      jsonEncode(body),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Future<AiWorkspaceStore> _store(
  List<Map<String, dynamic>> conversations, {
  Map<String, List<Map<String, dynamic>>> messages = const {},
}) async {
  final settings = SettingsStore();
  await settings.load();
  return AiWorkspaceStore(
    api: AiApiClient(
      settings.baseUrl,
      client: _fakeBackend(conversations, messages: messages),
    ),
  );
}

Widget _page(AiWorkspaceStore store) {
  final settings = SettingsStore();
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  );
}

String _inputText(WidgetTester tester) => tester.widget<TextField>(find.byType(TextField)).controller!.text;

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    created.clear();
    deleted.clear();
  });

  testWidgets('已经有一条干净的空会话：直接用它，不再新建', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = await _store([_conv('c-empty')]);
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();

    expect(store.conversation?.id, 'c-empty', reason: '空会话可以接着用');
    expect(created, isEmpty, reason: '已经有一条空会话就不该再造一条');
  });

  testWidgets('切到别的功能页再切回来：不会重新加载，也不会再新建会话', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 一条消息都没有、列表里也空 → 第一次进入必须新建一条
    final store = await _store([]);
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();
    expect(created, hasLength(1));
    final first = store.conversation?.id;

    // 模拟 HomeShell：切到设置页再切回来（AiHomePage 会被整个重建）
    await tester.pumpWidget(MaterialApp(home: Scaffold(body: Text('其他页'))));
    await tester.pumpAndSettle();
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();

    expect(created, hasLength(1), reason: '页面重建不该再建一条会话');
    expect(store.conversation?.id, first, reason: '还应该停在原来那条会话上');
  });

  testWidgets('历史遗留的多条空壳：只留最新一条，其余清掉', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 列表按更新时间倒序：c-new 最新
    final store = await _store([
      _conv('c-new', title: '新对话'),
      _conv('c-old', title: '新对话'),
      _conv('c-older', title: '新对话'),
    ]);
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();

    expect(store.conversation?.id, 'c-new');
    expect(deleted, containsAll(['c-old', 'c-older']));
    expect(created, isEmpty);
  });

  testWidgets('打了一半的字：切到别的会话再切回来还在（bug ②）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = await _store(
      [_conv('c-empty'), _conv('c-old', title: '聊过的', messageCount: 2)],
      messages: {
        'c-old': [
          {
            'id': 'u1',
            'conversationId': 'c-old',
            'seq': 1,
            'role': 'user',
            'status': 'complete',
            'text': '你好',
            'parts': <Object>[],
          },
        ],
      },
    );
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();
    expect(store.conversation?.id, 'c-empty');

    await tester.enterText(find.byType(TextField), '切换前打的字');
    await tester.pumpAndSettle();

    await store.openConversation('c-old');
    await tester.pumpAndSettle();
    expect(_inputText(tester), isEmpty, reason: '切走了就不该还显示上一条的字');

    await store.openConversation('c-empty');
    await tester.pumpAndSettle();
    expect(_inputText(tester), '切换前打的字', reason: '草稿必须能取回来');
    expect(deleted, isEmpty, reason: '有草稿的空会话不能被当成垃圾删掉');
  });

  testWidgets('草稿会话不会被自动清理（连删带字一起消失的旧毛病）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = await _store([_conv('c-empty'), _conv('c-other', title: '别的', messageCount: 1)]);
    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '别删我');
    await tester.pumpAndSettle();
    await store.newConversation();
    await tester.pumpAndSettle();

    expect(deleted, isNot(contains('c-empty')), reason: '有草稿的空会话要留着');
    expect((await store.loadDraft('c-empty')).text, '别删我');
    expect(created, hasLength(1), reason: '新建的那条才算新建');
  });

  testWidgets('发出去的话不会再被草稿灌回输入框（用户报的"一条消息复制一遍再发送"）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 会话已经在库里加载好了（页面这次挂上来不会再收到"加载完成"的通知）：
    // 这正是出事的那种状态 —— 页面的"当前草稿挂在哪条会话上"还是空的，
    // 而这条会话的草稿里躺着上一次（重建前那个页面实例）存下来的同一句话。
    final store = await _store([_conv('c-empty')]);
    await store.load();
    await store.saveDraft('c-empty', '这句话只该发一次');

    await tester.pumpWidget(_page(store));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '这句话只该发一次');
    await tester.pumpAndSettle();
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(_inputText(tester), isEmpty,
        reason: '发出去之后输入框必须是空的：同一句话不能被草稿灌回来（那就是同一条消息发两遍）');
    expect((await store.loadDraft('c-empty')).text, isEmpty, reason: '发出去 = 这条会话的草稿作废');
  });
}
