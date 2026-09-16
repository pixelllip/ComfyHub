// 设置页的两条回归：
//
//   1) 宽窗口按「宽度 / 550」分列（以前一路单列往下滚）
//   2) 开关行（「开启自动捕获」这些）不能顶到卡片边缘 —— 它们用的是
//      contentPadding 为零的 SwitchListTile，必须自己套一层内边距

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/settings_page.dart';
import 'package:viewer/state/library_store.dart';

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;

    if (path == '/api/capture/config') {
      body = {
        'enabled': true,
        'comfyUrl': 'http://127.0.0.1:8188',
        'pollSeconds': 4,
        'autoTag': 'ComfyUI',
        'maxPerPoll': 20,
        'downloadFallback': true,
      };
    } else if (path == '/api/capture/status') {
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
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': 0,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': <String, int>{},
        'byMediaKind': <String, int>{},
      };
    } else if (path == '/api/prompts' || path == '/api/media') {
      body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
    } else {
      status = 404;
      body = {'error': 'not found: $path'};
    }

    return http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

void main() {
  testWidgets('设置页：宽窗口分列，开关行左右都留出了内边距', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
    final launcher = BackendLauncher(settings);
    addTearDown(store.dispose);
    addTearDown(launcher.dispose);
    await store.refreshAll();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
          ChangeNotifierProvider<LibraryStore>.value(value: store),
        ],
        child: const MaterialApp(home: SettingsPage()),
      ),
    );
    await tester.pumpAndSettle();

    // 1) 分列：自动捕获排到了右边那一列
    final left = tester.getTopLeft(find.text('本地服务（MySQL + 后端）'));
    final right = tester.getTopLeft(find.text('ComfyUI 自动捕获'));
    expect(right.dx, greaterThan(left.dx + 100), reason: '宽窗口应该分两列');

    // 2) 开关行不顶到卡片边缘（16 卡片内边距 + 10 自己套的内边距）
    final tile = find
        .ancestor(of: find.text('开启自动捕获'), matching: find.byType(SwitchListTile))
        .first;
    final card = find.ancestor(of: find.text('开启自动捕获'), matching: find.byType(Card)).first;
    final tileRect = tester.getRect(tile);
    final cardRect = tester.getRect(card);
    expect(tileRect.left - cardRect.left, greaterThanOrEqualTo(20),
        reason: '左侧要有留白，不能贴到卡片边缘');
    expect(cardRect.right - tileRect.right, greaterThanOrEqualTo(20),
        reason: '右侧（开关那一侧）也要有留白');
  });

  // 「AI 模型与凭据」是这一页最常去的入口，必须排在前面的列里，
  // 不能压在「ComfyUI 自动捕获 / 库统计」下面（用户反馈过要往下翻很久）。
  testWidgets('设置页：AI 模型与凭据排在 ComfyUI 自动捕获前面', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
    final launcher = BackendLauncher(settings);
    addTearDown(store.dispose);
    addTearDown(launcher.dispose);
    await store.refreshAll();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
          ChangeNotifierProvider<LibraryStore>.value(value: store),
        ],
        child: const MaterialApp(home: SettingsPage()),
      ),
    );
    await tester.pumpAndSettle();

    final local = tester.getTopLeft(find.text('本地服务（MySQL + 后端）'));
    final ai = tester.getTopLeft(find.text('AI 模型与凭据'));
    final capture = tester.getTopLeft(find.text('ComfyUI 自动捕获'));

    // 排在「自动捕获」前面（同一列更靠上，或者前一个列）
    final beforeCapture = ai.dx < capture.dx - 100 || ai.dy < capture.dy;
    expect(beforeCapture, isTrue,
        reason: 'AI 入口不能排在 ComfyUI 自动捕获之后（实际 ai=$ai capture=$capture）');
    expect(ai.dy, greaterThan(local.dy - 1), reason: '本地服务仍然排在最前');
  });
}
