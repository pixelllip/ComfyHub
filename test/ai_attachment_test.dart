// AI 附件（M3）的界面回归：
//
//   · 选好文件后**先上传**再进托盘，托盘里图片显示缩略图、视频显示预览帧（同一张 /thumb 接口）；
//   · 发送时把 attachmentIds 交给后端（准入判定在创建 Run 之前，不通过不会有上游请求）；
//   · 上传失败 / 被准入拦下都要**说清楚是哪个文件、为什么**，绝不静默丢弃；
//   · 用户消息气泡里也能看到自己发了哪些附件。

import 'dart:convert';
import 'dart:io';

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

/// 每次创建 Run 的请求体（按顺序）。
final List<Map<String, dynamic>> runBodies = [];

/// 上传接口收到的请求次数（断言"选一次只传一次"）。
int uploadCalls = 0;

String _sse(int seq, String type, String dataJson) => 'id: $seq\nevent: $type\ndata: $dataJson\n\n';

http.Response _sseResponse(String body) => http.Response.bytes(
      utf8.encode(body),
      200,
      headers: {'content-type': 'text/event-stream; charset=utf-8'},
    );

/// 假后端：
///  - 上传返回 [uploadItems]（模拟后端按签名判定后的结果）；
///  - 预检返回 [preflightAllowed]；
///  - `/thumb` 回一张 1×1 PNG（真实图片解码在 widget 测试里不可靠，用例只断言 URL）。
MockClient _backend({
  required List<Map<String, dynamic>> uploadItems,
  List<Map<String, dynamic>> uploadFailed = const [],
  int uploadStatus = 201,
  String uploadError = '无法识别文件 a.bin 的真实类型',
  bool preflightAllowed = true,
  List<Map<String, dynamic>> preflightItems = const [],
}) {
  var runCount = 0;
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    int status = 200;

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
          'id': 'm-vision',
          'displayName': '看得见图的模型',
          'inputModalities': ['text', 'image'],
          'attachmentTransports': {
            'image': ['inline_base64']
          },
          'tools': true,
          'reasoning': false,
          'capabilitySource': 'manual',
          'enabled': true,
        }
      ];
    } else if (path == '/api/ai/attachments' && request.method == 'POST') {
      uploadCalls++;
      if (uploadStatus >= 400) {
        status = uploadStatus;
        body = {'code': 'UNSUPPORTED_CONTENT', 'message': uploadError};
      } else {
        body = {'items': uploadItems, 'failed': uploadFailed};
      }
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      body = {
        'id': 'c1',
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-vision',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = <Object>[];
    } else if (path == '/api/ai/preflight') {
      body = {'allowed': preflightAllowed, 'blockers': const [], 'items': preflightItems};
    } else if (path == '/api/ai/conversations/c1/runs') {
      runBodies.add(jsonDecode(request.body) as Map<String, dynamic>);
      runCount++;
      body = {'runId': 'r$runCount', 'assistantMessageId': 'a$runCount', 'userMessageId': 'u$runCount'};
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      final runId = path.split('/')[4];
      return _sseResponse(
        _sse(1, 'run.started', '{"runId":"$runId"}') +
            _sse(2, 'text.delta', '{"messageId":"a$runCount","text":"我看到了"}') +
            _sse(3, 'message.completed', '{"messageId":"a$runCount","text":"我看到了"}') +
            _sse(4, 'run.completed', '{"runId":"$runId"}'),
      );
    } else if (path == '/api/ai/conversations/c1/messages') {
      // 与真实后端一致：用户消息里带着 attachment 有序块（缩略图就靠它渲染）
      body = [
        {
          'id': 'u1',
          'conversationId': 'c1',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '看看这张图',
          'parts': [
            {'type': 'text', 'text': '看看这张图'},
            for (final item in uploadItems)
              {'type': 'attachment', 'text': item['name'], 'attachmentId': item['id']},
          ],
        },
        {
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          'status': 'complete',
          'text': '我看到了',
          'parts': <Object>[],
        },
      ];
    } else {
      body = <Object>[];
    }
    return http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Future<AiWorkspaceStore> _pump(
  WidgetTester tester, {
  required MockClient client,
}) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(api: AiApiClient(settings.baseUrl, client: client));
  await tester.pumpWidget(MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  ));
  await tester.pumpAndSettle();
  return store;
}

/// 真建一个临时文件：上传走 `MultipartFile.fromPath`，路径必须真实存在。
({String name, String path}) _tempFile(String name) {
  final dir = Directory.systemTemp.createTempSync('ai-attach-test');
  addTearDown(() => dir.deleteSync(recursive: true));
  final f = File('${dir.path}${Platform.pathSeparator}$name');
  f.writeAsBytesSync(List<int>.filled(64, 7));
  return (name: name, path: f.path);
}

Map<String, dynamic> _dto(String id, String name, String kind, {int size = 2048}) => {
      'id': id,
      'name': name,
      'kind': kind,
      'modality': kind,
      'mimeType': kind == 'image' ? 'image/png' : 'video/mp4',
      'sizeBytes': size,
    };

/// 找到托盘 / 气泡里指向某个附件的网络图片。
///
/// 注意：`Image.network(url, cacheWidth: …)` 会把 provider 包成 `ResizeImage`
/// （解码上限就是这么实现的），所以这里要先拆一层再看 URL。
Iterable<String> thumbUrls(WidgetTester tester) => tester
    .widgetList<Image>(find.byType(Image))
    .map((w) => w.image)
    .map((p) => p is ResizeImage ? p.imageProvider : p)
    .whereType<NetworkImage>()
    .map((n) => n.url);

void main() {
  setUp(() {
    runBodies.clear();
    uploadCalls = 0;
  });

  testWidgets('上传图片后托盘显示缩略图，发送时带上 attachmentIds', (tester) async {
    final store = await _pump(tester, client: _backend(uploadItems: [_dto('att-1', 'cat.png', 'image')]));

    await tester.runAsync(() => store.attachFiles([_tempFile('cat.png')]));
    await tester.pumpAndSettle();

    expect(uploadCalls, 1);
    expect(store.attachments.single.id, 'att-1');
    // 图片：缩略图来自后端 /thumb（不是原图）
    expect(
      thumbUrls(tester).any((u) => u.endsWith('/api/ai/attachments/att-1/thumb')),
      isTrue,
      reason: '图片附件在托盘里要显示缩略图',
    );

    await tester.enterText(find.byType(TextField), '看看这张图');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(runBodies.single['attachmentIds'], ['att-1']);
    // 发出去之后托盘清空，但气泡里还能看到自己发了什么
    expect(store.attachments, isEmpty);
    expect(
      thumbUrls(tester).any((u) => u.endsWith('/api/ai/attachments/att-1/thumb')),
      isTrue,
      reason: '用户消息气泡里也要显示附件缩略图',
    );
  });

  testWidgets('视频附件显示预览帧 + 播放角标（同一张 /thumb 接口）', (tester) async {
    final store = await _pump(tester, client: _backend(uploadItems: [_dto('vid-1', 'clip.mp4', 'video')]));

    await tester.runAsync(() => store.attachFiles([_tempFile('clip.mp4')]));
    await tester.pumpAndSettle();

    expect(
      thumbUrls(tester).any((u) => u.endsWith('/api/ai/attachments/vid-1/thumb')),
      isTrue,
      reason: '视频要取预览帧当封面',
    );
    expect(find.byIcon(Icons.play_circle_fill), findsOneWidget, reason: '要有播放角标，别让人以为是静态图');
  });

  testWidgets('音频 / 文档没有画面：显示文件图标而不是破图', (tester) async {
    final store = await _pump(
      tester,
      client: _backend(uploadItems: [
        _dto('doc-1', 'brief.pdf', 'document'),
        _dto('aud-1', 'voice.mp3', 'audio'),
      ]),
    );

    await tester.runAsync(() => store.attachFiles([_tempFile('brief.pdf')]));
    await tester.pumpAndSettle();

    // 音频 / 文档没有画面：托盘里落回文件图标（缩略图请求 204 → errorBuilder / loadingBuilder）
    expect(find.byIcon(Icons.description_outlined), findsOneWidget);
    expect(find.byIcon(Icons.play_circle_fill), findsNothing);
    expect(find.byIcon(Icons.image_outlined), findsNothing);
  });

  testWidgets('被准入拦下的附件：说明原因并禁用发送', (tester) async {
    final store = await _pump(
      tester,
      client: _backend(
        uploadItems: [_dto('att-9', 'huge.png', 'image')],
        preflightAllowed: false,
        preflightItems: [
          {
            'index': 0,
            'name': 'huge.png',
            'allowed': false,
            'blockers': ['文件 huge.png 超过内联发送上限（8 MB），请先压缩或换一张更小的图'],
            'attachmentId': 'att-9',
          }
        ],
      ),
    );

    await tester.runAsync(() => store.attachFiles([_tempFile('huge.png')]));
    await tester.pumpAndSettle();

    expect(store.preflight?.allowed, isFalse);
    expect(store.preflight?.blockersAt(0).single, contains('内联发送上限'));

    final send = tester.widget<FilledButton>(
      find.ancestor(of: find.text('发送'), matching: find.byType(FilledButton)),
    );
    expect(send.onPressed, isNull, reason: '准入不通过时不允许发送');

    // 点了也只是给出阻断理由，不会真的建 Run
    await tester.enterText(find.byType(TextField), '这张图');
    await store.send('这张图');
    await tester.pumpAndSettle();
    expect(runBodies, isEmpty);
    expect(store.notice, contains('已阻断发送'));
  });

  testWidgets('上传失败：如实报错，附件不进托盘（也不会把文字发出去）', (tester) async {
    final store = await _pump(tester, client: _backend(uploadItems: const [], uploadStatus: 400));

    await tester.runAsync(() => store.attachFiles([_tempFile('a.bin')]));
    await tester.pumpAndSettle();

    expect(store.attachments, isEmpty);
    expect(store.error, contains('附件上传失败'));
    expect(store.error, contains('无法识别文件 a.bin 的真实类型'));
  });

  testWidgets('部分成功：成功的进托盘，失败的逐个列出来', (tester) async {
    final store = await _pump(
      tester,
      client: _backend(
        uploadItems: [_dto('att-2', 'ok.png', 'image')],
        uploadFailed: [
          {'fileName': 'bad.bin', 'reason': '无法识别文件 bad.bin 的真实类型'},
        ],
      ),
    );

    await tester.runAsync(() => store.attachFiles([_tempFile('ok.png'), _tempFile('bad.bin')]));
    await tester.pumpAndSettle();

    expect(store.attachments.single.id, 'att-2');
    expect(store.notice, contains('bad.bin'));
    expect(store.notice, contains('无法识别'));
  });
}
