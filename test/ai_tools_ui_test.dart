// AI 工具 / Skills 接线回归（M4 / M5）：
//
//   (a) 右侧栏渲染后端返回的 skills，删除走 DELETE /api/ai/skills/{name}；
//   (b) tool.requested / tool.completed 事件在助手气泡里生成工具卡（含结果预览）；
//   (c) approval=pending 时出「批准 / 拒绝」，点批准 POST 到
//       /api/ai/tool-calls/{callId}/approve；
//   (d) 模型选择器是"搜索框 + 懒构建列表"，不是把 69 个模型一次全建出来；
//   (e) 工具权限页显示生效的白名单与逐工具权限，改动走 PUT /api/ai/tools/policy。
//
// 假后端沿用 test/ai_home_test.dart 的写法（MockClient + 手拼 SSE），不另造一套。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';
import 'package:viewer/pages/ai_tools_settings_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

/// 记录这次用例里后端收到的请求，用来断言"界面上的动作真的打到了对的接口"。
class _Recorder {
  final List<String> calls = [];
  final List<Map<String, dynamic>> policyPuts = [];

  /// 内置目录 sync 的请求体（用来断言"确认之前没有偷偷改库"）。
  final List<Map<String, dynamic>> syncBodies = [];

  /// 可变的 skills 列表：删除之后 fake 要真的少一个（否则刷新又回来了）
  List<Map<String, dynamic>> skills = [];

  /// 长期记忆（M6）：PUT 整篇 / POST 追加都要真的改掉这份内容
  String memory = '';

  /// 追加过的记忆条目（按顺序），用来断言"「添加」真的发出去了"
  final List<String> memoryAdded = [];

  /// 下一次「重新扫描」会登记几个（模拟往投放口里拷了东西）
  int rescanRegistered = 0;

  String? get lastPolicyPut => policyPuts.isEmpty ? null : jsonEncode(policyPuts.last);
}

int _entryCount(String content) =>
    content.split('\n').where((l) => l.trim().isNotEmpty).length;

/// 按真实 SSE 格式拼事件（冒号后必须有空格）。
String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

http.Response _sseResponse(String body) => http.Response.bytes(
      utf8.encode(body),
      200,
      headers: {'content-type': 'text/event-stream; charset=utf-8'},
    );

Map<String, dynamic> _skill(
  String name, {
  String description = '',
  String source = 'user',
  String? validationError,
  String? conflict,
}) =>
    {
      'name': name,
      'description': description,
      'source': source,
      'enabled': true,
      'userInvocable': true,
      'modelInvocable': true,
      'sizeBytes': 1024,
      'fileCount': 1,
      'validationError': ?validationError,
      'conflict': ?conflict,
    };

Map<String, dynamic> _tool(String name, {String category = 'files', String access = 'ask'}) => {
      'name': name,
      'description': '$name 的说明',
      'category': category,
      'mutating': category == 'files',
      'access': access,
      'overridden': false,
    };

MockClient _backend(
  _Recorder rec, {
  int modelCount = 1,
  String sse = '',
  List<Map<String, dynamic>> assistantParts = const [],
  String assistantText = '好的，我看一下文件。',
  bool approvalAccepted = true,
  List<Map<String, dynamic>>? tools,
  Map<String, dynamic>? policy,
  /// `GET /api/capture/jobs` 的返回值（右侧栏实时进度）
  Map<String, dynamic>? comfyJobs,
}) {
  return MockClient((request) async {
    final path = request.url.path;
    rec.calls.add('${request.method} $path');
    Object body;

    if (path == '/api/capture/jobs') {
      body = comfyJobs ??
          {
            'queueRunning': 0,
            'queuePending': 0,
            'comfyReachable': true,
            'submissions': <Object>[],
          };
    } else if (path == '/api/ai/providers') {
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
        for (var i = 0; i < modelCount; i++)
          {
            'providerId': 'local-gw',
            'id': 'model-${i.toString().padLeft(2, '0')}',
            'displayName': '模型 ${i.toString().padLeft(2, '0')}',
            'inputModalities': ['text', if (i.isEven) 'image'],
            'tools': true,
            'reasoning': i.isEven,
            'capabilitySource': 'manual',
            'enabled': true,
          }
      ];
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      body = {
        'id': 'c1',
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'model-00',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = <Object>[];
    } else if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        request.method == 'DELETE') {
      body = {'deleted': true, 'id': path.split('/').last};
    } else if (path == '/api/ai/conversations/c1/runs') {
      body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      return _sseResponse(sse);
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = [
        {
          'id': 'u1',
          'conversationId': 'c1',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '你好',
          'parts': <Object>[],
        },
        {
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          // 后端落库后的状态：给了 parts 就是写完了（complete）；
          // 没有 parts 说明这一轮还没收尾（例如工具卡还在等批准），仍是 streaming
          'status': assistantParts.isEmpty ? 'streaming' : 'complete',
          'text': assistantText,
          'parts': assistantParts,
        },
      ];
    } else if (path == '/api/ai/skills' && request.method == 'GET') {
      body = rec.skills;
    } else if (path.startsWith('/api/ai/skills/') && request.method == 'DELETE') {
      final name = Uri.decodeComponent(path.split('/').last);
      rec.skills = rec.skills.where((s) => s['name'] != name).toList();
      body = {'deleted': true, 'id': name};
    } else if (path == '/api/ai/skills/roots') {
      // 投放口路径由后端算好（两种运行布局都对），界面只负责显示
      body = {
        'userRoot': r'D:\ComfyHub\storage\ai\skills',
        'builtinRoot': r'D:\ComfyHub\skills\builtin',
        'userRootExists': true,
      };
    } else if (path == '/api/ai/skills/rescan') {
      // 投放口里新拷进来的东西在这里被自动登记
      body = {
        'registered': rec.rescanRegistered,
        'names': rec.rescanRegistered > 0 ? const ['dropped-in'] : const <String>[],
        'errors': const <String>[],
        'skills': rec.skills,
      };
    } else if (path == '/api/ai/memory' && request.method == 'GET') {
      body = {'content': rec.memory, 'path': r'D:\ComfyHub\storage\ai\memory.md', 'entryCount': _entryCount(rec.memory), 'maxChars': 8000};
    } else if (path == '/api/ai/memory' && request.method == 'PUT') {
      rec.memory = (jsonDecode(request.body) as Map)['content'].toString();
      body = {'content': rec.memory, 'path': r'D:\ComfyHub\storage\ai\memory.md', 'entryCount': _entryCount(rec.memory), 'maxChars': 8000};
    } else if (path == '/api/ai/memory/entries' && request.method == 'POST') {
      final entry = (jsonDecode(request.body) as Map)['content'].toString();
      rec.memoryAdded.add(entry);
      rec.memory = rec.memory.isEmpty ? '- $entry' : '${rec.memory}\n- $entry';
      body = {'content': rec.memory, 'path': r'D:\ComfyHub\storage\ai\memory.md', 'entryCount': _entryCount(rec.memory), 'maxChars': 8000};
    } else if (path == '/api/ai/memory' && request.method == 'DELETE') {
      rec.memory = '';
      body = {'content': '', 'path': r'D:\ComfyHub\storage\ai\memory.md', 'entryCount': 0, 'maxChars': 8000};
    } else if (path == '/api/ai/tools') {
      body = tools ??
          [
            _tool('read_file', category: 'files', access: 'ask'),
            _tool('load_skill', category: 'skill', access: 'allow'),
          ];
    } else if (path == '/api/ai/tools/policy' && request.method == 'PUT') {
      final sent = jsonDecode(request.body) as Map<String, dynamic>;
      rec.policyPuts.add(sent);
      body = {...(policy ?? _defaultPolicy()), ...sent};
    } else if (path == '/api/ai/tools/policy') {
      body = policy ?? _defaultPolicy();
    } else if (path.startsWith('/api/ai/tool-calls/')) {
      final segments = path.split('/');
      body = {
        'callId': segments[4],
        'approved': segments.last == 'approve',
        'accepted': approvalAccepted,
      };
    } else if (path == '/api/ai/preflight') {
      body = {'allowed': true, 'blockers': <String>[]};
    } else if (path.startsWith('/api/capture')) {
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

/// AI 设置页的假后端（只覆盖「内置模型目录」卡片要用的那几个接口）。
MockClient _providerBackend(
  _Recorder rec, {
  List<String> divergent = const ['claude-sonnet-5', 'gpt-5'],
  int missing = 3,
  String? statusError,
}) {
  var synced = false;
  Map<String, dynamic> status({
    List<String>? divergent,
    int added = 0,
    int updated = 0,
    int kept = 67,
  }) =>
      {
        'mode': 'add-missing',
        'version': '2026-09b-dsh',
        'providerId': 'command-code-goat',
        'added': added,
        'updated': updated,
        'kept': kept,
        'divergent': divergent ?? const <String>[],
        'modelCount': 69,
        'providerCreated': false,
        'error': statusError,
      };

  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path == '/api/ai/builtin/status') {
      body = status(divergent: synced ? const [] : divergent, kept: synced ? 69 : 67);
    } else if (path == '/api/ai/builtin/sync') {
      final sent = jsonDecode(request.body) as Map<String, dynamic>;
      rec.syncBodies.add(sent);
      synced = true;
      body = sent['mode'] == 'add-missing'
          ? status(divergent: divergent, added: missing, kept: 66)
          : status(updated: divergent.length);
    } else if (path == '/api/ai/providers') {
      body = [
        {
          'id': 'command-code-goat',
          'displayName': '内置网关',
          'api': 'openai-completions',
          'baseURL': 'http://127.0.0.1:11434/v1',
          'credentialRef': 'GOAT_KEY',
          'endpointTrust': 'loopback',
          'enabled': true,
          'revision': 1,
          'credential': {'configured': false, 'source': 'none', 'writable': true},
        }
      ];
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

Map<String, dynamic> _defaultPolicy() => {      'writeRoots': [r'D:\myProject\FlutterProject\viewer\comfyui'],
      'readRoots': [r'D:\myProject\FlutterProject\viewer'],
      'overrides': <String, String>{},
      'maxToolSteps': 8,
      'maxCallsPerRun': 16,
      'maxReadBytes': 262144,
      'maxWriteBytes': 262144,
      'defaultWriteRoot': r'D:\myProject\FlutterProject\viewer\comfyui',
    };

Future<AiWorkspaceStore> _store(
  _Recorder rec, {
  int modelCount = 1,
  String sse = '',
  String assistantText = '好的，我看一下文件。',
}) async {
  SharedPreferences.setMockInitialValues({});
  final store = AiWorkspaceStore(
    api: AiApiClient('http://127.0.0.1:8080',
        client: _backend(rec, modelCount: modelCount, sse: sse, assistantText: assistantText)),
  );
  return store;
}

Future<Widget> _homePage(
  _Recorder rec, {
  int modelCount = 1,
  String sse = '',
  List<Map<String, dynamic>> assistantParts = const [],
  Map<String, dynamic>? comfyJobs,
}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(
      settings.baseUrl,
      client: _backend(
        rec,
        modelCount: modelCount,
        sse: sse,
        assistantParts: assistantParts,
        comfyJobs: comfyJobs,
      ),
    ),
  );
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  );
}

void main() {
  // 纯 dart 用例也要用 SharedPreferences 的 mock（草稿 / 上次模型记录）
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('(a) 右侧栏列出后端的 skills：来源徽标 + 删除打到 DELETE /api/ai/skills/{name}', (tester) async {
    tester.view.physicalSize = const Size(1600, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder()
      ..skills = [
        _skill('anima-prompt', description: '把需求转成 Anima 优化提示词', source: 'builtin'),
        _skill('my-skill', description: '我自己写的', source: 'user'),
        _skill('broken-skill', description: '格式不对', validationError: 'frontmatter 缺少 description'),
      ];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 用户"其他建议"第 2 条：列表**默认折叠**（十几个 skill 不该把右侧栏占满）
    expect(find.text('anima-prompt'), findsNothing);
    expect(find.textContaining('列表已折叠（3 个）'), findsOneWidget);

    // 展开 → 出现搜索框 + 全部条目
    await tester.tap(find.text('Skills'));
    await tester.pumpAndSettle();
    expect(find.text('搜索 skill（名称 / 说明）'), findsOneWidget);

    // 后端返回什么就显示什么（不是硬编码的 7 条占位目录）
    expect(find.text('anima-prompt'), findsOneWidget);
    expect(find.text('my-skill'), findsOneWidget);
    expect(find.text('broken-skill'), findsOneWidget);
    expect(find.text('内置'), findsOneWidget);
    expect(find.text('用户'), findsNWidgets(2));
    // 不合法的那条要标出来（不静默忽略）
    expect(find.textContaining('格式不合法'), findsOneWidget);

    // 搜索：只留命中的那条（描述也参与匹配）
    await tester.enterText(find.byType(TextField).last, '格式不对');
    await tester.pumpAndSettle();
    expect(find.text('broken-skill'), findsOneWidget);
    expect(find.text('my-skill'), findsNothing);

    await tester.enterText(find.byType(TextField).last, '');
    await tester.pumpAndSettle();

    // 内置的没有删除按钮，用户来源的才有
    expect(find.byTooltip('删除 Skill'), findsNWidgets(2));

    await tester.tap(find.byTooltip('删除 Skill').last);
    await tester.pumpAndSettle();
    expect(find.textContaining('删除 Skill「broken-skill」？'), findsOneWidget);

    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();

    expect(rec.calls, contains('DELETE /api/ai/skills/broken-skill'));
    expect(find.text('broken-skill'), findsNothing, reason: '删完要重新拉列表，不能还留在界面上');
    expect(find.text('my-skill'), findsOneWidget);
  });

  testWidgets('(a2) skills 投放口：显示后端给的绝对路径，没有「从 DSH 导入」按钮', (tester) async {
    tester.view.physicalSize = const Size(1600, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder()..skills = [_skill('anima-prompt', source: 'builtin')];
    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 用户明确要求去掉那个按钮
    expect(find.text('从 DSH 导入'), findsNothing);
    expect(rec.calls.any((c) => c.contains('import-dsh')), isFalse);

    // 投放口路径要能看见（用户得知道往哪儿拷），并有打开 / 复制两个入口
    expect(find.textContaining(r'D:\ComfyHub\storage\ai\skills'), findsWidgets);
    expect(find.text('打开文件夹'), findsOneWidget);
    expect(find.text('复制路径'), findsOneWidget);

    // 往目录里拷了东西之后：点刷新 = 重新扫描 + 自动登记
    rec.rescanRegistered = 1;
    await tester.tap(find.byTooltip('重新扫描投放口（自动登记新拷进来的 skill）'));
    await tester.pumpAndSettle();
    expect(rec.calls, contains('POST /api/ai/skills/rescan'));
    expect(find.textContaining('自动登记 1 个'), findsWidgets, reason: '要如实说这次自动登记了什么');
  });

  testWidgets('(a3) 长期记忆：面板显示条数与预览，编辑弹窗能改 / 加一条 / 清空', (tester) async {
    tester.view.physicalSize = const Size(1600, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder()..memory = '- 用户偏好 4:3 画幅\n- 出图统一用 Anima';
    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    expect(find.text('长期记忆'), findsOneWidget);
    expect(find.text('2 条'), findsOneWidget);
    expect(find.textContaining('用户偏好 4:3 画幅'), findsWidgets, reason: '第一条记忆要做预览');

    // 打开编辑器 → 改内容 → 保存 → PUT 出去的正文就是改过的
    await tester.tap(find.byTooltip('查看 / 编辑长期记忆'));
    await tester.pumpAndSettle();
    expect(find.textContaining('memory.md'), findsWidgets, reason: '要告诉用户真源文件在哪');

    await tester.enterText(
      find.widgetWithText(TextField, '- 用户偏好 4:3 画幅\n- 出图统一用 Anima'),
      '- 只保留这一条',
    );
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();

    expect(rec.calls, contains('PUT /api/ai/memory'));
    expect(rec.memory, '- 只保留这一条');
    expect(find.text('1 条'), findsOneWidget, reason: '面板要跟着刷新');

    // 再加一条：走 POST /memory/entries
    await tester.tap(find.byTooltip('查看 / 编辑长期记忆'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, '例如：交付一律 16:9、带字幕'), '交付 16:9');
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();
    expect(rec.memoryAdded, ['交付 16:9']);
    expect(rec.memory, contains('交付 16:9'));

    // 编辑器里的正文要跟着更新（不然用户会以为没加上）
    expect(find.textContaining('交付 16:9'), findsWidgets);

    // 清空要先确认
    await tester.tap(find.text('清空'));
    await tester.pumpAndSettle();
    expect(find.text('清空长期记忆？'), findsOneWidget);
    await tester.tap(find.text('清空').last);
    await tester.pumpAndSettle();
    expect(rec.calls, contains('DELETE /api/ai/memory'));
    expect(rec.memory, isEmpty);
  });

  testWidgets('(b) tool.requested / tool.completed 事件生成可见的工具卡（预览默认折叠）', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":"好的，"}') +
        _sse(
            4,
            'tool.requested',
            '{"runId":"r1","callId":"call-1","name":"read_file",'
                '"arguments":"{\\"path\\":\\"a.md\\"}","approval":"not_required"}') +
        _sse(5, 'tool.started', '{"callId":"call-1","name":"read_file"}') +
        _sse(
            6,
            'tool.completed',
            '{"callId":"call-1","name":"read_file","elapsedMs":42,"preview":"# 标题\\n正文"}') +
        _sse(
            7,
            'message.completed',
            '{"messageId":"a1","text":"好的，我看一下文件。","steps":1,"parts":['
                '{"type":"text","text":"好的，我看一下文件。"},'
                '{"type":"tool_call","toolCallId":"call-1","jsonPayload":{"name":"read_file","arguments":"{\\"path\\":\\"a.md\\"}"}},'
                '{"type":"tool_result","toolCallId":"call-1","text":"# 标题\\n正文","jsonPayload":{"name":"read_file","ok":true,"elapsedMs":42,"approval":"approved"}}'
                ']}') +
        _sse(8, 'run.completed', '{"runId":"r1"}');

    await tester.pumpWidget(await _homePage(
      rec,
      sse: sse,
      assistantParts: [
        {'type': 'text', 'text': '好的，我看一下文件。'},
        {
          'type': 'tool_call',
          'toolCallId': 'call-1',
          'jsonPayload': {'name': 'read_file', 'arguments': '{"path":"a.md"}'},
        },
        {
          'type': 'tool_result',
          'toolCallId': 'call-1',
          'text': '# 标题\n正文',
          'jsonPayload': {
            'name': 'read_file',
            'ok': true,
            'elapsedMs': 42,
            'approval': 'approved',
          },
        },
      ],
    ));
    await tester.pumpAndSettle();

    // 发一条消息把 Run 跑起来，SSE 里才有工具事件
    await tester.enterText(find.byType(TextField).first, '看看 a.md');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 工具卡：名字 + 状态 + 耗时
    expect(find.text('read_file'), findsOneWidget);
    expect(find.text('已完成'), findsOneWidget);
    expect(find.text('42 ms'), findsOneWidget);
    // 结果预览默认折叠
    expect(find.textContaining('# 标题'), findsNothing);

    await tester.tap(find.byTooltip('展开 read_file 的结果'));
    await tester.pumpAndSettle();
    expect(find.textContaining('# 标题'), findsWidgets);
  });

  testWidgets('(e3) 回复末尾贴出「生成的产物」画廊入口卡（用户建议 ⑤）', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":"跑好了。"}') +
        _sse(
            4,
            'tool.requested',
            '{"callId":"call-s","name":"comfy_submit","arguments":"{\\"promptId\\":7}",'
                '"approval":"approved"}') +
        _sse(5, 'tool.started', '{"callId":"call-s","name":"comfy_submit"}') +
        _sse(
            6,
            'tool.completed',
            '{"callId":"call-s","name":"comfy_submit","elapsedMs":9000,"preview":"完成",'
                '"result":{"promptId":7,"status":"success","mediaIds":[31,32],"mediaCount":2}}') +
        _sse(7, 'message.completed', '{"messageId":"a1","text":"跑好了。","steps":1}') +
        _sse(8, 'run.completed', '{"runId":"r1"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '帮我跑一张');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 后端确认入库了两个产物：回复末尾要有入口卡（不是靠模型在正文里嘴上说）
    expect(find.text('生成的产物（2）'), findsOneWidget);
    expect(find.text('查看详情'), findsOneWidget);
  });

  testWidgets('(e4) 右侧栏显示 ComfyUI 实时进度（用户"其他建议"第 1 条）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(await _homePage(
      rec,
      comfyJobs: {
        'queueRunning': 1,
        'queuePending': 2,
        'comfyReachable': true,
        'runningLabel': '雨夜霓虹',
        'submissions': [
          {
            'promptId': 'p-1',
            'submittedBy': 'ai',
            'title': '赛博朋克少女',
            'status': 'running',
            'elapsedMs': 12000,
            'mediaIds': <Object>[],
          },
        ],
      },
    ));
    await tester.pumpAndSettle();

    expect(find.text('实时进度'), findsOneWidget);
    expect(find.textContaining('正在生成：雨夜霓虹'), findsOneWidget);
    expect(find.textContaining('赛博朋克少女'), findsWidgets);
    // 进度是真的打到了后端那个接口（不是界面自己编的）
    expect(rec.calls.any((c) => c.contains('/api/capture/jobs')), isTrue);
  });

  testWidgets('(c) approval=pending 时出「批准 / 拒绝」，批准 POST /api/ai/tool-calls/{id}/approve', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(
            3,
            'tool.requested',
            '{"runId":"r1","callId":"call-9","name":"write_file",'
                '"arguments":"{\\"path\\":\\"x.txt\\"}","approval":"pending"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '写个文件');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.text('write_file'), findsOneWidget);
    expect(find.text('待批准'), findsOneWidget);
    expect(find.text('批准'), findsOneWidget);
    expect(find.text('拒绝'), findsOneWidget);
    expect(find.textContaining('在等你的批准'), findsOneWidget);

    await tester.tap(find.text('批准'));
    // 批准后卡片进入「运行中」（真的在转圈），所以这里不能用 pumpAndSettle
    await tester.pump(const Duration(milliseconds: 400));

    expect(rec.calls, contains('POST /api/ai/tool-calls/call-9/approve'));
    expect(find.text('运行中'), findsOneWidget, reason: '批准生效后就该进入运行中');
  });

  testWidgets('(c2) 点晚了（accepted=false）如实告知，不假装成功', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(
            3,
            'tool.requested',
            '{"runId":"r1","callId":"call-9","name":"write_file","arguments":"{}","approval":"pending"}');

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = AiWorkspaceStore(
      api: AiApiClient(settings.baseUrl,
          client: _backend(rec, sse: sse, approvalAccepted: false)),
    );
    await tester.pumpWidget(MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
      ],
      child: const MaterialApp(home: AiHomePage()),
    ));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '写个文件');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('批准'));
    await tester.pumpAndSettle();

    expect(store.notice, contains('没有生效'));
    expect(find.textContaining('没有生效'), findsOneWidget);
  });

  testWidgets('(b2) reasoning.delta 累积成折叠的「思考过程」', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    // 思考正文写得比折叠摘要（160 字）长：这样才能断言"折叠时只给摘要、展开后才有全文"
    final filler = '先看需求，再定风格。' * 20;
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'reasoning.delta', '{"messageId":"a1","text":"$filler"}') +
        _sse(4, 'reasoning.delta', '{"messageId":"a1","text":"尾巴结论。"}') +
        _sse(5, 'text.delta', '{"messageId":"a1","text":"好的"}') +
        _sse(6, 'message.completed', '{"messageId":"a1","text":"好的","steps":0}') +
        _sse(7, 'run.completed', '{"runId":"r1"}');
    // 后端落库后的有序块：思考/正文按流顺序各一块（与 message.completed 之后库里的一致）
    final parts = [
      {'type': 'reasoning', 'text': '$filler尾巴结论。'},
      {'type': 'text', 'text': '好的'},
    ];

    await tester.pumpWidget(await _homePage(rec, sse: sse, assistantParts: parts));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '帮我想想');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.text('思考过程'), findsOneWidget);
    // 默认折叠：只给一段摘要（开头能看到），完整正文铺不开（摘要限 160 字 + 省略号）
    expect(find.textContaining('先看需求'), findsWidgets);
    expect(find.textContaining('尾巴结论'), findsNothing);
    await tester.tap(find.text('思考过程'));
    await tester.pumpAndSettle();
    expect(find.textContaining('尾巴结论。'), findsOneWidget);
  });

  testWidgets('(b3) 思考过程在**流式生成中**也是折叠的（用户要求默认自动折叠）', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final filler = '先看需求，再定风格。' * 20;
    // 只有思考增量、没有 run.completed：停在"正在生成"这一刻
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'reasoning.delta', '{"messageId":"a1","text":"$filler尾巴结论。"}') +
        _sse(4, 'text.delta', '{"messageId":"a1","text":"好的"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '帮我想想');
    await tester.tap(find.text('发送'));
    // 只推进有限的帧：Run 停在流式过程中
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 40));

    expect(find.text('思考中…'), findsOneWidget, reason: '流式中要有明确的进行中标记');
    expect(find.textContaining('尾巴结论'), findsNothing,
        reason: '流式生成中也不能自动展开整段思考（用户要求默认自动折叠）');
    // 但用户点一下还是能看到全文
    await tester.tap(find.text('思考过程'));
    await tester.pumpAndSettle();
    expect(find.textContaining('尾巴结论。'), findsOneWidget);
  });

  testWidgets('(d) 模型选择器懒构建并可按搜索过滤', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(await _homePage(rec, modelCount: 69));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.models.length, 69);

    // 打开选择器（按钮上就是当前模型名）
    await tester.tap(find.text('模型 00').first);
    await tester.pumpAndSettle();
    expect(find.textContaining('共 69 个模型'), findsOneWidget);

    // 懒构建：69 个模型不能一次全建出来（否则每行 6 个徽标，弹出就卡）
    final built = find.descendant(of: find.byType(Dialog), matching: find.byType(ListTile));
    expect(built.evaluate().length, lessThan(69));

    // 搜索过滤
    await tester.enterText(
      find.descendant(of: find.byType(Dialog), matching: find.byType(TextField)),
      '模型 42',
    );
    await tester.pumpAndSettle();
    // 只认对话框里的那几行：聊天框和右侧栏也会显示当前模型名
    final inDialog = find.byType(Dialog);
    expect(find.descendant(of: inDialog, matching: find.widgetWithText(ListTile, '模型 42')),
        findsOneWidget);
    expect(find.descendant(of: inDialog, matching: find.text('模型 00')), findsNothing);

    await tester.tap(find.descendant(of: inDialog, matching: find.widgetWithText(ListTile, '模型 42')));
    await tester.pumpAndSettle();
    expect(store.selectedModel?.id, 'model-42');
  });

  testWidgets('(e) 工具权限页：默认策略说清楚，逐工具权限改动走 PUT /api/ai/tools/policy', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(MaterialApp(
      home: AiToolsSettingsPage(
        api: AiApiClient('http://127.0.0.1:8080', client: _backend(rec)),
      ),
    ));
    await tester.pumpAndSettle();

    // 默认状态要大声、明确
    expect(find.textContaining('其它位置一律拒绝'), findsOneWidget);
    expect(find.textContaining(r'viewer\comfyui'), findsWidgets);
    expect(find.text('写白名单'), findsOneWidget);
    expect(find.text('读白名单'), findsOneWidget);
    expect(find.text('read_file'), findsOneWidget);
    expect(find.text('load_skill'), findsOneWidget);

    // 把 read_file 改成「拒绝」→ 立刻 PUT
    final dropdown = find.byType(DropdownButton<String>).first;
    await tester.ensureVisible(dropdown);
    await tester.pumpAndSettle();
    await tester.tap(dropdown);
    await tester.pumpAndSettle();
    await tester.tap(find.text('拒绝').last);
    await tester.pumpAndSettle();

    expect(rec.lastPolicyPut, contains('"overrides"'));
    expect(rec.lastPolicyPut, contains('"read_file":"deny"'));
  });

  testWidgets('(e2) 加一个写目录会 PUT writeRoots', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(MaterialApp(
      home: AiToolsSettingsPage(
        api: AiApiClient('http://127.0.0.1:8080', client: _backend(rec)),
      ),
    ));
    await tester.pumpAndSettle();

    final field = find.widgetWithText(TextField, '要放行的目录（绝对路径）').first;
    await tester.ensureVisible(field);
    await tester.pumpAndSettle();
    await tester.enterText(field, r'D:\out');
    await tester.pumpAndSettle();

    final add = find.widgetWithText(FilledButton, '添加').first;
    await tester.ensureVisible(add);
    await tester.pumpAndSettle();
    await tester.tap(add);
    await tester.pumpAndSettle();

    expect(rec.lastPolicyPut, contains(r'D:\\out'));
    expect(rec.lastPolicyPut, contains('"writeRoots"'));
  });

  testWidgets('(f) 内置目录卡片：分歧数醒目 + 先预览再确认才对齐能力', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(MaterialApp(
      home: AiProviderSettingsPage(
        api: AiApiClient('http://127.0.0.1:8080', client: _providerBackend(rec)),
      ),
    ));
    await tester.pumpAndSettle();

    expect(find.text('内置模型目录'), findsOneWidget);
    expect(find.textContaining('冻结副本'), findsOneWidget);
    expect(find.text('版本 2026-09b-dsh'), findsOneWidget);
    expect(find.text('目录模型 69'), findsOneWidget);
    expect(find.textContaining('有 2 个模型的能力声明与内置目录不同'), findsOneWidget);

    // 点对齐：先 GET 预览 → 弹确认框；**这一步绝不能已经改库**
    await tester.tap(find.text('用内置目录对齐模型能力'));
    await tester.pumpAndSettle();
    expect(find.textContaining('重写 2 个模型的能力声明'), findsOneWidget);
    expect(find.textContaining('不会新增或删除模型'), findsOneWidget);
    expect(rec.syncBodies, isEmpty, reason: '确认之前不能已经发过 sync');

    await tester.tap(find.text('对齐'));
    await tester.pumpAndSettle();

    expect(rec.syncBodies.single['mode'], 'refresh-capabilities');
    expect(find.textContaining('已对齐 2 个模型的能力声明'), findsOneWidget);
    // 对齐完分歧清零
    expect(find.textContaining('能力声明一致'), findsOneWidget);
  });

  testWidgets('(f2) 补齐缺失模型是纯新增，直接 POST add-missing', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(MaterialApp(
      home: AiProviderSettingsPage(
        api: AiApiClient('http://127.0.0.1:8080', client: _providerBackend(rec, divergent: const [])),
      ),
    ));
    await tester.pumpAndSettle();

    await tester.tap(find.text('补齐缺失模型'));
    await tester.pumpAndSettle();

    expect(find.byType(AlertDialog), findsNothing, reason: '纯新增不需要确认框');
    expect(rec.syncBodies.single['mode'], 'add-missing');
    expect(find.textContaining('已补齐 3 个缺失模型'), findsOneWidget);
  });

  testWidgets('(f3) 内置目录读取失败要如实显示原因', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(MaterialApp(
      home: AiProviderSettingsPage(
        api: AiApiClient('http://127.0.0.1:8080', client: _providerBackend(rec, statusError: '目录缺失')),
      ),
    ));
    await tester.pumpAndSettle();

    expect(find.textContaining('目录缺失'), findsOneWidget);
  });

  testWidgets('(g) 输入 / 弹出 Skill 菜单并写入 /skill-name', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder()
      ..skills = [
        _skill('anima-prompt', description: '把需求转成 Anima 优化提示词'),
        _skill('my-skill', description: '我自己写的'),
      ];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 还没打 `/` 时不显示菜单
    expect(find.text('/anima-prompt'), findsNothing);

    await tester.enterText(find.byType(TextField).first, '/');
    await tester.pumpAndSettle();
    expect(find.text('/anima-prompt'), findsOneWidget);
    expect(find.text('/my-skill'), findsOneWidget);

    // 打一半要按查询词过滤
    await tester.enterText(find.byType(TextField).first, '/anima');
    await tester.pumpAndSettle();
    expect(find.text('/anima-prompt'), findsOneWidget);
    expect(find.text('/my-skill'), findsNothing);

    await tester.tap(find.text('/anima-prompt'));
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(find.byType(TextField).first).controller?.text, '/anima-prompt ');
    expect(find.text('/anima-prompt'), findsNothing, reason: '选完菜单要收起来');
  });

  testWidgets('(h) 点发送后输入框立刻清空，不用等模型整轮跑完', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 刻意**不给任何 SSE 事件**：这一次 Run 会一直挂着，
    // 正好用来证明"清空发生在等待之前"——以前 `await store.send()` 才 clear，
    // 用户会在整个生成过程里看到自己的话还留在输入框（看起来像点了没反应）。
    final rec = _Recorder();
    await tester.pumpWidget(await _homePage(rec, sse: ''));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '帮我画一张赛博朋克少女');
    await tester.pump();
    // 发送按钮上还有刚输入的文字（发送前的基线）
    expect(find.text('发送'), findsOneWidget);
    await tester.tap(find.text('发送'));
    await tester.pump(); // 只推进一帧：这一次 Run 还没跑完

    final field = tester.widget<TextField>(find.byType(TextField).first);
    expect(field.controller?.text, isEmpty, reason: '发送后输入框必须马上清空');
    // Run 真的发出去了（不是"点了没反应"，也不是"清空了却没发"）
    expect(rec.calls.any((c) => c.contains('/runs')), isTrue, reason: 'Run 真的发出去了');
  });

  test('逐字回包只替换那一条消息：流式期间不能重建整段历史（O(n) 热点的回归）', () async {
    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'text.delta', '{"messageId":"a1","text":"你"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":"好"}') +
        _sse(4, 'text.delta', '{"messageId":"a1","text":"呀"}') +
        _sse(5, 'message.completed', '{"messageId":"a1","text":"你好呀","steps":0}') +
        _sse(6, 'run.completed', '{"runId":"r1"}');
    final store = await _store(rec, sse: sse, assistantText: '你好呀');

    // 流式期间每一帧的用户消息实例：之前 `messages.map().toList()` 会每帧重造一个
    final userInstances = <AiMessage>{};
    store.addListener(() {
      if (store.sending && store.messages.length >= 2) {
        userInstances.add(store.messages.first);
      }
    });

    await store.load();
    await store.send('你好');

    expect(store.messages.length, 2);
    expect(store.messages.last.text, '你好呀');
    expect(userInstances.length, 1,
        reason: '逐字回包只该替换变化的那一条，其余 AiMessage 对象必须原样复用');
  });

  // --- 用户建议 ⑤ / bug：产物预览 ------------------------------------------

  testWidgets('产物卡的预览图走画廊接口（/api/media/{id}/thumb），不是附件接口', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":"跑好了。"}') +
        _sse(
            4,
            'tool.completed',
            '{"callId":"call-s","name":"comfy_submit","elapsedMs":900,"preview":"完成",'
                '"result":{"promptId":7,"status":"success","mediaIds":[31],"mediaCount":1}}') +
        _sse(5, 'message.completed', '{"messageId":"a1","text":"跑好了。","steps":1}') +
        _sse(6, 'run.completed', '{"runId":"r1"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '帮我跑一张');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.text('生成的产物（1）'), findsOneWidget);

    // 两套 id 空间不同：媒体 id 塞进附件接口一定 404，界面就是"预览图不可用"（用户报的 bug）
    final urls = tester
        .widgetList<Image>(find.byType(Image))
        .map((w) => w.image)
        .whereType<NetworkImage>()
        .map((p) => p.url)
        .toList();
    expect(urls.any((u) => u.contains('/api/media/31/thumb')), isTrue,
        reason: '产物缩略图必须走画廊接口，实际：$urls');
    expect(urls.any((u) => u.contains('/api/ai/attachments/')), isFalse,
        reason: '不能把媒体 id 当附件 id 用，实际：$urls');
  });

  testWidgets('权限档可在输入区切换，切到完全权限会写回后端（PUT permissionMode）', (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 默认就是「询问」
    expect(find.text('询问'), findsOneWidget);

    await tester.tap(find.text('询问'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('完全权限').last);
    await tester.pumpAndSettle();

    expect(rec.policyPuts.length, 1, reason: '切档必须真的写回后端');
    expect(rec.policyPuts.last['permissionMode'], 'full');
    // 界面上要如实显示新档位（而且给出"目录范围没放宽"的说明）
    expect(find.text('完全权限'), findsWidgets);
    expect(find.textContaining('不再等你批准'), findsWidgets);
  });

  testWidgets('(b4) 工具卡按 parts 的真实顺序插在正文之间（不重排到最后）', (tester) async {
    tester.view.physicalSize = const Size(1200, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    // 一轮里：思考 → 正文 → 工具 → 正文。
    // 界面上工具卡必须夹在两段正文**中间**（用户要求：完全按照消息获取顺序来）。
    final parts = [
      {'type': 'reasoning', 'text': '先想一想'},
      {'type': 'text', 'text': '前半句正文'},
      {
        'type': 'tool_call',
        'toolCallId': 'c1',
        'jsonPayload': {'name': 'read_file', 'arguments': '{"path":"a.txt"}'},
      },
      {
        'type': 'tool_result',
        'toolCallId': 'c1',
        'text': '文件内容',
        'jsonPayload': {'name': 'read_file', 'ok': true, 'elapsedMs': 12},
      },
      {'type': 'text', 'text': '后半句正文'},
    ];
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":"前半句正文"}') +
        _sse(
            4,
            'message.completed',
            '{"messageId":"a1","text":"前半句正文后半句正文","steps":1,'
                '"parts":${jsonEncode(parts)}}') +
        _sse(5, 'run.completed', '{"runId":"r1"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '看看这个文件');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.text('read_file'), findsWidgets, reason: '工具卡要渲染出来');
    final first = tester.getTopLeft(find.textContaining('前半句正文')).dy;
    final card = tester.getTopLeft(find.text('read_file').first).dy;
    final last = tester.getTopLeft(find.textContaining('后半句正文')).dy;
    expect(card, greaterThan(first), reason: '工具卡不能跑到第一段正文前面');
    expect(card, lessThan(last), reason: '工具卡必须夹在正文之间，不能被重排到最后');
  });

  testWidgets('用户翻上去时不自动跟随，右下角给「回到最新消息」（用户建议 ③④）', (tester) async {
    tester.view.physicalSize = const Size(1000, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final rec = _Recorder();
    // 一条足够长的回复：列表一定会超出视口，才谈得上"滚动位置"
    final long = List.generate(120, (i) => '第 $i 行内容，用来把列表撑长。').join('\n\n');
    final sse = _sse(1, 'run.started', '{"runId":"r1"}') +
        _sse(2, 'message.started', '{"messageId":"a1"}') +
        _sse(3, 'text.delta', '{"messageId":"a1","text":${jsonEncode(long)}}') +
        _sse(4, 'message.completed', '{"messageId":"a1","text":${jsonEncode(long)},"steps":0}') +
        _sse(5, 'run.completed', '{"runId":"r1"}');

    await tester.pumpWidget(await _homePage(rec, sse: sse));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '给我一段长文');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 贴底时不该有这个按钮
    expect(find.byTooltip('回到最新消息'), findsNothing);

    // 往上翻：按钮出现，而且**不会**被自动跟随拽回底部
    await tester.drag(find.byType(ListView).first, const Offset(0, 900));
    await tester.pumpAndSettle();
    expect(find.byTooltip('回到最新消息'), findsOneWidget);

    final scrollable = find.byType(Scrollable).first;
    final offsetAfterDrag = tester.widget<Scrollable>(scrollable).controller!.offset;
    expect(offsetAfterDrag, lessThan(tester.widget<Scrollable>(scrollable).controller!.position.maxScrollExtent));

    // 点一下回到最新消息：按钮消失，并且真的到了底部
    await tester.tap(find.byTooltip('回到最新消息'));
    await tester.pumpAndSettle();
    expect(find.byTooltip('回到最新消息'), findsNothing);
    final controller = tester.widget<Scrollable>(scrollable).controller!;
    expect(controller.offset, closeTo(controller.position.maxScrollExtent, 1));
  });
}
