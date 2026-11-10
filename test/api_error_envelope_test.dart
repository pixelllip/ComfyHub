// 后端错误体的**读取点**（跨语言契约的"客户端那一半"）。
//
// 服务端那一半由 `server-rs/tests/api_test.rs` 盯着（普通错误 `{"error": 人话}`、
// AI 领域错误 `{"error": 稳定码, "detail": 人话}`、`detail` 为空时整个键不出现）。
// 这里盯 Dart 侧怎么读 —— 形状对了两边也不一定对得上，因为**两个客户端的读取顺序不一样**：
//
//   ApiClient._decode   （lib/core/api_client.dart:50）   `error` 当人话、`detail` 当附加说明
//   AiApiClient._decode （lib/core/ai_api_client.dart:26）**先读 `detail`**、再读 `error`
//
// 任意一侧错误时，界面显示 `NOT_FOUND` 这类机器码，且**不会报错**。
// 背景：Rust 侧早先写的是 `{error:"NOT_FOUND", message:"提示词 999 不存在"}`，
// 两个客户端都读不到那句人话（2026-09-24 才对齐成 `{error, detail?}`）。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';

/// 只回一个固定错误体的假后端。
MockClient _failing(int status, Map<String, dynamic> body) => MockClient(
  (_) async => http.Response(
    jsonEncode(body),
    status,
    headers: {'content-type': 'application/json'},
  ),
);

void main() {
  group('ApiClient（画廊 / 提示词那套）', () {
    test('普通错误：人话就在 error 里', () async {
      final api = ApiClient(
        'http://x',
        client: _failing(404, {'error': '提示词 999 不存在'}),
      );
      await expectLater(
        api.getPrompt(999),
        throwsA(
          isA<ApiException>()
              .having((e) => e.statusCode, 'statusCode', 404)
              .having((e) => e.message, 'message', '提示词 999 不存在')
              .having((e) => e.detail, 'detail', null),
        ),
      );
    });

    test('AI 形状（error 是码、detail 是人话）：两个键都留得住', () async {
      final api = ApiClient(
        'http://x',
        client: _failing(400, {'error': 'CONFIG_ERROR', 'detail': '会话不存在: 9'}),
      );
      await expectLater(
        api.getPrompt(9),
        throwsA(
          isA<ApiException>()
              .having((e) => e.message, 'message', 'CONFIG_ERROR')
              .having((e) => e.detail, 'detail', '会话不存在: 9')
              // 展示串顺序为"说明（码）"：说明在前，先显示可理解的内容
              .having(
                (e) => e.toString(),
                'toString',
                'CONFIG_ERROR (会话不存在: 9)',
              ),
        ),
      );
    });

    test('`message` 这个键已经不在了：只剩它会话界面只能看到兜底文案', () async {
      final api = ApiClient(
        'http://x',
        client: _failing(400, {'message': '旧形状'}),
      );
      await expectLater(
        api.getPrompt(1),
        throwsA(
          isA<ApiException>().having((e) => e.message, 'message', 'HTTP 400'),
        ),
      );
    });
  });

  group('AiApiClient（AI 工作台那套）', () {
    test('AI 错误：先读 detail —— 用户看到的是人话而不是 CONFIG_ERROR', () async {
      final api = AiApiClient(
        'http://x',
        client: _failing(400, {
          'error': 'CONFIG_ERROR',
          'detail': '会话不存在: 999999',
        }),
      );
      await expectLater(
        api.listProviders(),
        throwsA(
          isA<AiApiException>()
              .having((e) => e.statusCode, 'statusCode', 400)
              .having((e) => e.message, 'message', '会话不存在: 999999'),
        ),
      );
    });

    test('普通错误（没有 detail）：回落到 error，仍然是人话', () async {
      final api = AiApiClient(
        'http://x',
        client: _failing(400, {'error': '非法的 id'}),
      );
      await expectLater(
        api.listProviders(),
        throwsA(
          isA<AiApiException>().having((e) => e.message, 'message', '非法的 id'),
        ),
      );
    });
  });
}
