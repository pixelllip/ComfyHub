// `CopyOverrides`：界面注解的离线降级表。
//
// 这一层的**全部价值**就是三条行为，缺一条都会在没网 / 后端没起来的时候现原形：
//  ① 没有覆盖 → 用 Dart 里的默认值；
//  ② 有覆盖 → 用覆盖值；
//  ③ **加载失败 → 仍然用默认值**（离线必须照常显示，不能空一片），
//     而且失败**不许被吞掉**（要有地方能断言到它）。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/copy_api.dart';
import 'package:viewer/core/copy_overrides.dart';

const _key = 'ui.settings.nsfw.note';
const _fallback = '默认那句话（离线也看得见）';

MockClient _catalog(List<Object> entries) => MockClient((req) async {
      expect(req.url.path, '/api/copy/catalog');
      expect(req.url.queryParameters['scope'], 'ui', reason: '这一层只关心 ui.* 的覆盖');
      return http.Response(jsonEncode(entries), 200, headers: {'content-type': 'application/json'});
    });

CopyApi _api(MockClient client) => CopyApi('http://127.0.0.1:8080', client: client);

void main() {
  test('没有覆盖 → 用 fallback（catalog 里的 current 是默认值，不是覆盖）', () async {
    final overrides = CopyOverrides();
    addTearDown(overrides.dispose);

    await overrides.load(_api(_catalog([
      {'key': _key, 'scope': 'ui', 'default': _fallback, 'current': _fallback, 'overridden': false},
    ])));

    expect(overrides.text(_key, _fallback), _fallback);
    expect(overrides.length, 0, reason: '没改过的条目不该被当成覆盖收进来');
    expect(overrides.isLoaded, isTrue);
    expect(overrides.lastError, isNull);
  });

  test('有覆盖 → 用覆盖值', () async {
    final overrides = CopyOverrides();
    addTearDown(overrides.dispose);

    await overrides.load(_api(_catalog([
      {'key': _key, 'scope': 'ui', 'current': '桌面改过的口径', 'overridden': true},
    ])));

    expect(overrides.text(_key, _fallback), '桌面改过的口径');
    expect(overrides.text('ui.other', '别处的默认'), '别处的默认', reason: '没覆盖的 key 照旧');
  });

  test('load 抛异常 → 仍然回退到 fallback，而且错误没被吞掉（onError + lastError）', () async {
    final errors = <Object>[];
    final stacks = <StackTrace>[];
    final overrides = CopyOverrides()
      ..onError = (e, st) {
        errors.add(e);
        stacks.add(st);
      };
    addTearDown(overrides.dispose);

    await overrides.load(_api(MockClient((_) async => throw http.ClientException('连接被拒绝'))));

    expect(overrides.text(_key, _fallback), _fallback, reason: '离线必须照常显示默认值');
    expect(overrides.lastError, isA<http.ClientException>());
    expect(errors, hasLength(1), reason: '失败必须留痕，否则"没覆盖"与"加载炸了"分不出来');
    expect(stacks, hasLength(1));
    expect(overrides.isLoaded, isTrue, reason: '"试过了"这件事本身也要能被界面读到');
  });

  test('失败不会把上一次成功的覆盖退回默认值（一次网络抖动不该改口径）', () async {
    final overrides = CopyOverrides();
    addTearDown(overrides.dispose);

    await overrides.load(_api(_catalog([
      {'key': _key, 'scope': 'ui', 'current': '桌面改过的口径', 'overridden': true},
    ])));
    expect(overrides.text(_key, _fallback), '桌面改过的口径');

    await overrides.load(_api(MockClient((_) async => throw http.ClientException('掉线了'))));

    expect(overrides.text(_key, _fallback), '桌面改过的口径');
    expect(overrides.lastError, isNotNull);
  });
}
