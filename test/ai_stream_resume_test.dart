// 回归（用户 bug 2026-09-28）：
//
// 「当一个对话正在生成答复的时候，如果切到别的窗口再切回去，前面显示的部分答复全部消失」
//
// 根因不是"窗口"，是**流式正文只在内存里**：后端一圈跑完才把正文落库
// （`server-rs/src/executor.rs` 的 `Sink`：流式期间只在内存里攒，收尾时按 ordinal 一次写完），
// 而当时的收尾逻辑是"事件流一断就 `_reloadMessages()`"—— 那一读恰好读到一条**空**消息，
// 界面上已经流出来的字当场被擦干净，而 Run 还在后端好好跑着。
//
// 现在两条防线：
//   ① 断流**不等于**结束：只有收到终态事件（或后端明确说这条 Run 已终态）才收尾；
//      其余情况带 `after=<最后一条事件的 seq>` 重新挂上去，已经流出来的正文一个字不动；
//   ② 切会话再切回来时，内存里那份**正在长的**消息不许被"库里还是空的"覆盖掉。

import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/state/ai_workspace_store.dart';

const _head = '先把本机状态查出来，';
const _tail = '再动手。';

/// 假后端"库"里的消息（与真实后端一致：流式期间助手正文**是空的**）。
List<Map<String, dynamic>> _server = [];

/// 每一次挂事件流时带的 `after`（断言"续传而不是从 0 重放"）。
List<int> _afters = [];

/// 断流之后要不要**接着吐剩下的**（false = 一直断，用来验"如实说明"）。
bool _resumeWorks = true;

/// 第一次挂流吐几段正文就断。
int _breakAfterDeltas = 2;

MockClient _fakeBackend() {
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
      body = {'id': 'c1', 'title': '断流回归', 'messageCount': 0, 'archived': false};
    } else if (path == '/api/ai/conversations') {
      body = [
        {'id': 'c1', 'title': '断流回归', 'messageCount': 0, 'archived': false},
      ];
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = _server;
    } else if (path == '/api/ai/runs/r1') {
      // 断流之后界面会问一次"这次 Run 到底还在不在跑"
      body = {'id': 'r1', 'conversationId': 'c1', 'status': 'running'};
    } else if (path == '/api/ai/conversations/c1/runs') {
      final sent = jsonDecode(request.body) as Map<String, dynamic>;
      _server = [
        {
          'id': 'u1',
          'conversationId': 'c1',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': sent['text'],
          'parts': [
            {'type': 'text', 'text': sent['text']},
          ],
        },
        {
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          'status': 'streaming',
          // ⚠ 后端**这一刻**还没落正文（淡定：真正落库在收尾）
          'text': '',
          'modelId': 'm-text',
          'parts': <Object>[],
        },
      ];
      body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    } else if (path == '/api/ai/runs/r1/events') {
      final after = int.tryParse(request.url.queryParameters['after'] ?? '0') ?? 0;
      _afters.add(after);

      if (after <= _breakAfterDeltas) {
        // 第一段：吐 `_breakAfterDeltas` 个字，然后**连接断掉**（没有终态事件）
        final buf = StringBuffer('id: 1\nevent: run.started\ndata: {"runId":"r1"}\n\n');
        for (var i = 1; i <= _breakAfterDeltas; i++) {
          buf.write('id: ${i + 1}\nevent: text.delta\n');
          buf.write('data: {"messageId":"a1","text":${jsonEncode(_head[i - 1])}}\n\n');
        }
        return http.Response.bytes(
          utf8.encode(buf.toString()),
          200,
          headers: {'content-type': 'text/event-stream; charset=utf-8'},
        );
      }
      if (!_resumeWorks) {
        // 挂上就断，一个字都不给（验"反复断流要如实说明、已经收到的内容留着"）
        return http.Response.bytes(
          utf8.encode(': 空\n\n'),
          200,
          headers: {'content-type': 'text/event-stream; charset=utf-8'},
        );
      }
      // 续传：从断点接着吐，最后给终态
      final rest = _head.substring(_breakAfterDeltas);
      final sse = 'id: ${_breakAfterDeltas + 2}\nevent: text.delta\n'
          'data: {"messageId":"a1","text":${jsonEncode(rest)}}\n\n'
          'id: ${_breakAfterDeltas + 3}\nevent: text.delta\n'
          'data: {"messageId":"a1","text":${jsonEncode(_tail)}}\n\n'
          'id: ${_breakAfterDeltas + 4}\nevent: message.completed\n'
          'data: {"messageId":"a1","text":${jsonEncode('$_head$_tail')},"steps":0,'
          '"parts":[{"type":"text","text":${jsonEncode('$_head$_tail')}}]}\n\n'
          'id: ${_breakAfterDeltas + 5}\nevent: run.completed\ndata: {"runId":"r1"}\n\n';
      // 收尾：后端这时候才把正文写进库（真实行为）
      final a1 = _server.firstWhere((m) => m['id'] == 'a1');
      a1['status'] = 'complete';
      a1['text'] = '$_head$_tail';
      a1['parts'] = [
        {'type': 'text', 'text': '$_head$_tail'},
      ];
      return http.Response.bytes(
        utf8.encode(sse),
        200,
        headers: {'content-type': 'text/event-stream; charset=utf-8'},
      );
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

Future<AiWorkspaceStore> _store() async {
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: _fakeBackend()),
  );
  await store.load();
  return store;
}

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    _server = [];
    _afters = [];
    _resumeWorks = true;
    _breakAfterDeltas = 2;
  });

  test('事件流断在半路：不许当成"这一轮结束"，接着听，已经流出来的正文一个字不丢', () async {
    final store = await _store();
    await store.send('换个人物');

    expect(_afters.length, greaterThanOrEqualTo(2), reason: '断流之后必须重新挂上去（而不是就此收尾）');
    expect(_afters.first, 0);
    expect(_afters[1], greaterThan(0), reason: '续传要带断点序号：从 0 重放会让整段回复又长一遍');

    final reply = store.messages.firstWhere((m) => m.id == 'a1');
    expect(reply.text, '$_head$_tail', reason: '断流前后两段要接起来，一段都不能少');
    expect(reply.status, 'complete');
    expect(store.running, isFalse, reason: '拿到终态事件之后才允许收尾');
  });

  test('后端说这一轮已经终态：不空转，如实收尾（正文以库为准）', () async {
    final store = await _store();
    await store.send('换个人物');
    final reply = store.messages.firstWhere((m) => m.id == 'a1');
    expect(reply.text, contains(_head), reason: '库里那份定稿必须完整落在界面上');
    expect(store.messages.map((m) => m.id).toList(), ['u1', 'a1'],
        reason: '收尾之后不许留下重复气泡');
  });

  test('反复断流也不许把已经收到的正文擦掉，并如实报出来', () async {
    _resumeWorks = false;
    final store = await _store();
    await store.send('换个人物');

    final reply = store.messages.firstWhere((m) => m.id == 'a1');
    expect(reply.text, _head.substring(0, _breakAfterDeltas),
        reason: '一直断流时，已经显示给用户的那半段必须留着');
    expect(store.error, isNotNull, reason: '连不上要如实说，不许假装跑完了');
  });
}
