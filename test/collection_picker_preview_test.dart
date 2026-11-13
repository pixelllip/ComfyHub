import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/media_thumb.dart';

/// 集合需要提供预览图。
///
/// 一排「未命名集合（3 张）」长得一模一样；选择器必须把后端给的
/// `coverMediaId` 画成缩略图，否则根本分不出要移进哪一个。
void main() {
  testWidgets('移到集合：有封面的行显示缩略图，没有封面的退回图标', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();

    final api = ApiClient(
      settings.baseUrl,
      client: MockClient((request) async {
        if (request.url.path == '/api/media/collections') {
          return http.Response(
            jsonEncode([
              {
                'id': 5,
                'kind': 'IMAGE',
                'origin': 'manual',
                'title': '未命名集合',
                'size': 3,
                'coverMediaId': 7,
              },
              {
                'id': 6,
                'kind': 'IMAGE',
                'origin': 'manual',
                'title': '还没封面的集合',
                'size': 1,
              },
            ]),
            200,
            headers: {'content-type': 'application/json; charset=utf-8'},
          );
        }
        return http.Response('[]', 404);
      }),
    );
    final store = LibraryStore(settings, api: api);
    addTearDown(store.dispose);

    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: Center(
                child: TextButton(
                  onPressed: () => showCollectionPicker(
                    context,
                    mediaKind: 'IMAGE',
                    count: 2,
                  ),
                  child: const Text('打开'),
                ),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('打开'));
    await tester.pumpAndSettle();

    expect(find.text('未命名集合'), findsOneWidget);
    expect(find.text('还没封面的集合'), findsOneWidget);
    expect(
      find.byType(CachedThumbImage),
      findsOneWidget,
      reason: '有 coverMediaId 的那一行要显示预览，没有的退回图标',
    );
  });
}
