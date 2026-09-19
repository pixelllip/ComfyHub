// 回归（用户 bug：429 之类"不可抗力"打断回复后，**已经回复完的内容直接看不到了**）：
//
// 现场（查库实据，会话 9b6d01af 的 seq12）：那次 Run 跑了 4 分钟、成功调了 3 次 comfy_submit，
// 最后一轮撞上 HTTP 429；用户接着点了「重试」，结果整条回复连同工具卡一起消失
// —— 库里那条消息 `text=''`、一个 part 都没有。
//
// 两半根因：
//   · 后端 `resetAssistantForRetry` 把**有内容**的失败回复就地清零（现在只对空壳这么做，
//     有内容的另起一条气泡；端到端防线在 `scripts/e2e-ai-tools-test.ps1` 第 13 幕）；
//   · 前端 `retry()` **无条件就地替换**助手气泡 —— 后端另起一条时，旧气泡里的正文 / 工具卡
//     会被顶掉。现在按后端返回的 `assistantMessageId` 分辨：
//     同一条 id（空壳被就地复用）→ 就地换占位；新 id（原来那条有内容）→ 原样留着、追加新占位。
//
// 这里走的是**真实链路**（`send()` 起一次必然 429 的 Run，再点重试），因为 `_runIds` 是
// 内存里的关联：只有同一次会话里失败的那条回复才带得上 `retryOfRunId`。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/state/ai_workspace_store.dart';

const _ask = '把这一步跑完';
const _partial = '先把本机状态查出来，再动手：';

/// 假后端"库"里的消息（`send()` 时追加，失败时按后端的真实行为落正文 + parts）。
List<Map<String, dynamic>> _server = [];

/// 假后端"库"里的会话。
List<Map<String, dynamic>> _conversations = [];

/// 这次失败要不要真的留下内容（false = 空壳：请求还没开始就被限流）。
bool _failWithContent = true;

/// 重试时后端给的助手消息 id（模拟"有内容 → 另起一条" / "空壳 → 就地复用"）。
String _retryAssistantId = 'a2';

/// SSE 刚订阅那一刻 Store 里的消息（回归断言的关键取样点：占位已放进列表、
/// 而 `_consumeRun` 末尾那次 `_reloadMessages` 还没发生）。
List<AiMessage> _atStreamOpen = [];

void _failAssistant({required bool withContent}) {
  final a1 = _server.firstWhere((m) => m['id'] == 'a1');
  a1['status'] = 'failed';
  a1['text'] = withContent ? _partial : '';
  a1['parts'] = withContent
      ? [
          {'type': 'text', 'text': _partial},
          {
            'type': 'tool_call',
            'toolCallId': 'call_1',
            'jsonPayload': {'name': 'comfy_get_status', 'arguments': '{}'},
          },
          {'type': 'tool_result', 'toolCallId': 'call_1', 'text': '{"queueRunning":0}'},
        ]
      : <Object>[];
}

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
      final conv = {
        'id': 'c1',
        'title': '重试回归',
        'messageCount': 0,
        'archived': false,
      };
      _conversations = [conv];
      body = conv;
    } else if (path == '/api/ai/conversations') {
      body = _conversations;
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = _server;
    } else if (path == '/api/ai/conversations/c1/runs') {
      final sent = jsonDecode(request.body) as Map<String, dynamic>;
      if (sent['retryOfRunId'] == null) {
        // 第一次提问：追加用户消息 + 空的助手占位（与后端一致）
        _server = [
          ..._server,
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
            'text': '',
            'modelId': 'm-text',
            'parts': <Object>[],
          },
        ];
        body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
      } else {
        // 重试：后端**另起一条**（原来那条有内容）或**就地复用**（空壳）
        if (_retryAssistantId != 'a1') {
          _server = [
            ..._server,
            {
              'id': _retryAssistantId,
              'conversationId': 'c1',
              'seq': _server.length + 1,
              'role': 'assistant',
              'status': 'streaming',
              'text': '',
              'modelId': 'm-text',
              'parts': <Object>[],
            },
          ];
        } else {
          _failAssistant(withContent: false);
        }
        body = {
          'runId': 'r2',
          'assistantMessageId': _retryAssistantId,
          'userMessageId': 'u1',
        };
      }
    } else if (path == '/api/ai/runs/r1/events') {
      // 第一次：吐半段正文，然后被 429 打断（空壳那一幕连正文都还没有）
      _failAssistant(withContent: _failWithContent);
      final delta = _failWithContent
          ? 'id: 2\nevent: text.delta\ndata: {"messageId":"a1","text":${jsonEncode(_partial)}}\n\n'
          : '';
      final sse = 'id: 1\nevent: run.started\ndata: {"runId":"r1"}\n\n'
          '$delta'
          'id: 3\nevent: run.failed\ndata: {"runId":"r1","code":"RATE_LIMIT",'
          '"message":"被限流（HTTP 429）"}\n\n';
      return http.Response.bytes(
        utf8.encode(sse),
        200,
        headers: {'content-type': 'text/event-stream; charset=utf-8'},
      );
    } else if (path == '/api/ai/runs/r2/events') {
      // 取样点：`retry()` 刚把新的流式占位放进列表，`_reloadMessages` 还没跑
      _atStreamOpen = List<AiMessage>.of(_storeRef!.messages);
      final sse = 'id: 1\nevent: run.started\ndata: {"runId":"r2"}\n\n'
          'id: 2\nevent: text.delta\ndata: {"messageId":"$_retryAssistantId","text":"接上"}\n\n'
          'id: 3\nevent: message.completed\n'
          'data: {"messageId":"$_retryAssistantId","text":"接上没说完的部分。","steps":0}\n\n'
          'id: 4\nevent: run.completed\ndata: {"runId":"r2"}\n\n';
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

AiWorkspaceStore? _storeRef;

Future<AiWorkspaceStore> _store() async {
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: _fakeBackend()),
  );
  _storeRef = store;
  await store.load();
  return store;
}

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    _server = [];
    _conversations = [];
    _failWithContent = true;
    _retryAssistantId = 'a2';
    _atStreamOpen = [];
    _storeRef = null;
  });

  /// 走一遍真实链路：发一条消息 → 被 429 打断 → 点重试。
  /// [expectContent] 为 true 时顺带断言"已经说出去的那半段还在界面上"。
  Future<AiWorkspaceStore> sendThenRetry({bool expectContent = true}) async {
    final store = await _store();
    await store.send(_ask);
    final failed = store.messages.firstWhere((m) => m.id == 'a1');
    expect(failed.status, 'failed');
    if (expectContent) {
      // 用户 bug 的另一半：失败之后内容不能凭空消失
      expect(failed.text, contains(_partial), reason: '失败之后已经说出去的正文必须还在');
      expect(failed.parts.length, 3, reason: '正文 / 工具卡 / 工具结果都要在（缺了界面上就看不见）');
    } else {
      expect(failed.text, isEmpty, reason: '这一幕是"一句话都没说就被限流"的空壳');
    }

    await store.retry('a1');
    return store;
  }

  test('有内容的失败回复：重试另起一条气泡，原来那条原样留着', () async {
    await sendThenRetry();

    expect(_atStreamOpen.map((m) => m.id).toList(), ['u1', 'a1', 'a2'],
        reason: '重试不许把有内容的失败回复从界面上顶掉（用户 bug：429 之后看不到已回复的内容）');
    final old = _atStreamOpen.firstWhere((m) => m.id == 'a1');
    expect(old.status, 'failed', reason: '旧气泡如实留在失败上（带「重试」按钮）');
    expect(old.text, contains(_partial), reason: '已经说出去的正文必须还在');
    expect(old.parts.length, 3, reason: '工具卡也随之留着');
  });

  test('撞上 429 时如果还没说出任何内容：重试就地复用那条空壳，不新增气泡', () async {
    _failWithContent = false;
    _retryAssistantId = 'a1'; // 空壳 → 后端就地重置复用同一条
    await sendThenRetry(expectContent: false);

    expect(_atStreamOpen.map((m) => m.id).toList(), ['u1', 'a1'],
        reason: '空壳就地复用，会话里不许冒出多余气泡');
    expect(_atStreamOpen.firstWhere((m) => m.id == 'a1').status, 'streaming');
  });
}
