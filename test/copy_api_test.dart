// AI 文案台客户端（`lib/core/copy_api.dart`）的跨语言契约那一半。
//
// 盯三件事，都是"两边形状对不上也不会报错"的那一类：
//  ① **URL 与 query 拼装**（空筛选不许发出空参；key 必须过 `encodeComponent`）；
//  ② **`{error, detail?}` 错误体**（稳定码与人话都要留得住，与
//     `api_error_envelope_test.dart` 同一套约定）；
//  ③ **`batch` 一定带 `dryRun`**（设计 §4 硬规矩 1：没有预览的批量改文案就是赌博）。
//
// 全部走 `MockClient`：**不连真后端**（后端此刻还没实现）。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/copy_api.dart';

/// 记录每一个请求的假后端 —— 这一组用例断言的就是"发出去的到底长什么样"。
class _Recorder {
  final List<http.Request> requests = [];

  MockClient client(Object? Function(http.Request req) reply) => MockClient((req) async {
        requests.add(req);
        return http.Response(jsonEncode(reply(req)), 200,
            headers: {'content-type': 'application/json; charset=utf-8'});
      });

  http.Request get last => requests.last;

  List<http.Request> get batches =>
      requests.where((r) => r.url.path == '/api/copy/batch').toList();

  Map<String, dynamic> bodyOf(http.Request req) =>
      Map<String, dynamic>.from(jsonDecode(req.body) as Map);
}

void main() {
  group('CopyApi：URL 与 query 拼装', () {
    test('catalog 不带筛选时一个 query 键都不发（`?scope=&q=` 与"不筛"是两回事）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://127.0.0.1:8080', client: rec.client((_) => <Object>[]));

      await api.catalog();

      expect(rec.last.method, 'GET');
      expect(rec.last.url.path, '/api/copy/catalog');
      expect(rec.last.url.query, isEmpty);
    });

    test('catalog 的 scope / q / verified 都拼上，值过 URL 编码', () async {
      final rec = _Recorder();
      final api = CopyApi('http://127.0.0.1:8080', client: rec.client((_) => <Object>[]));

      await api.catalog(scope: 'tool', q: '安全 文案', verifiedOnly: true);

      expect(rec.last.url.path, '/api/copy/catalog');
      expect(rec.last.url.queryParameters['scope'], 'tool');
      expect(rec.last.url.queryParameters['q'], '安全 文案');
      expect(rec.last.url.queryParameters['verified'], '1');
      // 中文关键字要编码后再上路（`encodeQueryComponent` 把空格写成 `+`，
      // 这是 query 组件的标准写法；裸空格会被截断）
      expect(rec.last.url.query, contains('q=%E5%AE%89%E5%85%A8+%E6%96%87%E6%A1%88'));
    });

    test('verifiedOnly:false 是「只看待校验」，不是"不筛"（null 才是不筛）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => <Object>[]));

      await api.catalog(verifiedOnly: false);
      expect(rec.last.url.queryParameters['verified'], '0');

      await api.catalog();
      expect(rec.last.url.queryParameters.containsKey('verified'), isFalse);
    });

    test('detail / put / delete / verify 的 key 一律 encodeComponent（含空格与斜杠）', () async {
      const key = 'skill.我 的/skill';
      final encoded = Uri.encodeComponent(key);
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((req) => req.method == 'DELETE' ? null : <String, Object?>{}));

      await api.detail(key);
      expect(rec.last.method, 'GET');
      expect(rec.last.url.toString(), 'http://x/api/copy/$encoded');

      await api.put(key, '新文本');
      expect(rec.last.method, 'PUT');
      expect(rec.last.url.toString(), 'http://x/api/copy/$encoded');

      await api.reset(key);
      expect(rec.last.method, 'DELETE');
      expect(rec.last.url.toString(), 'http://x/api/copy/$encoded');

      await api.verify(key, note: '看过了');
      expect(rec.last.method, 'POST');
      expect(rec.last.url.toString(), 'http://x/api/copy/$encoded/verify');
      expect(rec.bodyOf(rec.last)['note'], '看过了');
    });

    test('put 不带备注时 body 里没有 note 这个键（别把"没提"写成"清空"）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => <String, Object?>{}));

      await api.put('ui.x', '文本');

      final body = rec.bodyOf(rec.last);
      expect(body['text'], '文本');
      expect(body.containsKey('note'), isFalse);
    });

    test('audit 的 limit 进 query，返回体里缺字段就是 null / 空表', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => [
            {'key': 'prompt.s1', 'action': 'put'},
          ]));

      final rows = await api.audit(limit: 7);

      expect(rec.last.url.path, '/api/copy/audit');
      expect(rec.last.url.queryParameters['limit'], '7');
      expect(rows.single.key, 'prompt.s1');
      expect(rows.single.op, 'put');
      expect(rows.single.actor, isNull);
      expect(rows.single.before, isNull);
    });
  });

  group('CopyApi：{error, detail?} 错误体', () {
    MockClient failing(int status, Object body) => MockClient((_) async =>
        http.Response(body is String ? body : jsonEncode(body), status,
            headers: {'content-type': 'application/json'}));

    test('AI 形状：error 是稳定码、detail 是人话 —— 两个键都留得住', () async {
      final api = CopyApi('http://x',
          client: failing(404, {'error': 'NOT_FOUND', 'detail': '文案不存在: tool.x'}));

      await expectLater(
        api.detail('tool.x'),
        throwsA(isA<CopyApiException>()
            .having((e) => e.statusCode, 'statusCode', 404)
            .having((e) => e.error, 'error', 'NOT_FOUND')
            .having((e) => e.detail, 'detail', '文案不存在: tool.x')
            // 展示串取人话：用户先看到能懂的那半句
            .having((e) => e.message, 'message', '文案不存在: tool.x')
            .having((e) => e.toString(), 'toString', '文案不存在: tool.x')),
      );
    });

    test('普通错误：detail 不出现时人话就在 error 里', () async {
      final api = CopyApi('http://x', client: failing(400, {'error': '非法的 key'}));

      await expectLater(
        api.put('bad', 'x'),
        throwsA(isA<CopyApiException>()
            .having((e) => e.error, 'error', '非法的 key')
            .having((e) => e.detail, 'detail', isNull)
            .having((e) => e.message, 'message', '非法的 key')),
      );
    });

    test('非 JSON 的响应（代理页 / 网关）把原文交出去，不吞成一句"请求失败"', () async {
      final api = CopyApi('http://x', client: failing(502, '<html>Bad Gateway</html>'));

      await expectLater(
        api.catalog(),
        throwsA(isA<CopyApiException>()
            .having((e) => e.statusCode, 'statusCode', 502)
            .having((e) => e.error, 'error', contains('Bad Gateway'))),
      );
    });

    test('success 路径不抛错：204/空体也算成功', () async {
      final api = CopyApi('http://x', client: MockClient((_) async => http.Response('', 204)));
      await expectLater(api.reset('ui.x'), completes);
    });
  });

  group('CopyApi：batch（设计 §4）', () {
    test('dryRun 一定在 body 里 —— true 和 false 都要显式发出去', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => {'items': <Object>[], 'changed': 0}));

      await api.batch(keys: const ['prompt.s17'], op: CopyBatchOp.replace, find: 'a', replace: 'b', dryRun: true);
      expect(rec.bodyOf(rec.last)['dryRun'], isTrue);
      expect(rec.bodyOf(rec.last)['op'], 'replace');
      expect(rec.bodyOf(rec.last)['keys'], ['prompt.s17']);

      await api.batch(keys: const ['prompt.s17'], op: CopyBatchOp.replace, find: 'a', replace: 'b', dryRun: false);
      expect(rec.bodyOf(rec.last)['dryRun'], isFalse);
    });

    test('六种 op 的 wire 名就是设计 §4 那六行（改了后端就不认）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => {'items': <Object>[], 'changed': 0}));

      for (final op in CopyBatchOp.values) {
        await api.batch(keys: const ['k'], op: op, dryRun: true);
      }

      expect(rec.batches.map((r) => rec.bodyOf(r)['op']).toList(), [
        'replace',
        'regex_replace',
        'append',
        'prepend',
        'delete_lines',
        'insert_after',
      ]);
    });

    test('没给 find/replace/regex 时一个多余的键都不发', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => {'items': <Object>[], 'changed': 0}));

      await api.batch(keys: const ['k'], op: CopyBatchOp.append, replace: '尾巴', dryRun: true);

      final body = rec.bodyOf(rec.last);
      expect(body['replace'], '尾巴');
      expect(body.containsKey('find'), isFalse);
      expect(body.containsKey('regex'), isFalse);
      // `confirm` 只在真给的时候才发（设计 §4 的 body 里没有这个键）
      expect(body.containsKey('confirm'), isFalse);
    });

    test('confirm 只在 true 时出现（prompt.nsfw 的二次确认落在这里）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => {'items': <Object>[], 'changed': 0}));

      await api.batch(keys: const ['prompt.nsfw'], op: CopyBatchOp.append, replace: 'x', confirm: true, dryRun: false);

      expect(rec.bodyOf(rec.last)['confirm'], isTrue);
    });

    test('changed 优先用后端给的数；没给就按 items 里真的会变的条数出来', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((req) {
        final dry = (jsonDecode(req.body) as Map)['dryRun'] == true;
        return {
          if (!dry) 'changed': 9,
          'items': [
            {'key': 'a', 'changed': true, 'diff': '- 旧\n+ 新'},
            {'key': 'b', 'changed': false},
            {'key': 'c', 'changed': false, 'blocked': '占位符 «MAX_WEB» 被改坏了'},
          ],
        };
      }));

      final dry = await api.batch(keys: const ['a', 'b', 'c'], op: CopyBatchOp.append, replace: 'x', dryRun: true);
      expect(dry.changed, 1, reason: '后端没给 changed 时按 items 数：blocked / 没命中都不算');
      expect(dry.items.last.blocked, contains('«MAX_WEB»'));

      final applied = await api.batch(keys: const ['a'], op: CopyBatchOp.append, replace: 'x', dryRun: false);
      expect(applied.changed, 9, reason: '后端算了就用它的');
    });

    test('清单既认裸数组也认 {items:[]}（后端换个包装不该让整页变空）', () async {
      final rec = _Recorder();
      final api = CopyApi('http://x', client: rec.client((_) => {
            'items': [
              {'key': 'ui.a', 'scope': 'ui', 'current': '覆盖值', 'overridden': true},
            ],
          }));

      final list = await api.catalog(scope: 'ui');
      expect(list.single.key, 'ui.a');
      expect(list.single.currentText, '覆盖值');
    });
  });

  group('CopyEntry / CopyDiff 解析：宽容，但不编默认值', () {
    test('缺字段回 null / 空表（"后端没给"必须与"后端给了空串"分得开）', () {
      final e = CopyEntry.fromJson({'key': 'prompt.s1'});

      expect(e.key, 'prompt.s1');
      expect(e.scope, '');
      expect(e.title, isNull);
      expect(e.defaultText, isNull);
      expect(e.currentText, isNull);
      expect(e.baseHash, isNull);
      expect(e.verifiedAt, isNull);
      expect(e.note, isNull);
      expect(e.refs, isEmpty);
      expect(e.overridden, isFalse);
      expect(e.baseChanged, isFalse);
    });

    test('`default`/`current`（设计 §3）与 `defaultText`/`currentText` 两个名字都认', () {
      final a = CopyEntry.fromJson({'key': 'k', 'default': 'D', 'current': 'C'});
      expect(a.defaultText, 'D');
      expect(a.currentText, 'C');

      final b = CopyEntry.fromJson({'key': 'k', 'defaultText': 'D', 'currentText': 'C'});
      expect(b.defaultText, 'D');
      expect(b.currentText, 'C');
    });

    test('scope 缺了就按 key 的第一段推（分组不该因此多出一个"其他"）', () {
      expect(CopyEntry.fromJson({'key': 'tool.comfy_submit.description'}).scopeOf, 'tool');
      expect(CopyEntry.fromJson({'key': 'prompt.nsfw'}).scopeOf, 'prompt');
      expect(CopyEntry.fromJson({'key': 'ui.a', 'scope': 'ui'}).scopeOf, 'ui');
      expect(CopyEntry.fromJson({'key': 'nokey'}).scopeOf, '');
    });

    test('refs 里的空元素丢掉，非字符串原样留住（不猜结构、不丢信息）', () {
      final e = CopyEntry.fromJson({
        'key': 'k',
        'refs': ['server-rs/src/nsfw_prompt.rs:120', '', '/api/ai/run-kit'],
      });
      expect(e.refs, ['server-rs/src/nsfw_prompt.rs:120', '/api/ai/run-kit']);
    });

    test('CopyDiff 缺字段时 before/after/diff 都是 null，changed 只有 true 才算', () {
      final d = CopyDiff.fromJson({'key': 'k'});
      expect(d.before, isNull);
      expect(d.after, isNull);
      expect(d.diff, isNull);
      expect(d.changed, isFalse);
      expect(d.blocked, isNull);
    });
  });
}
