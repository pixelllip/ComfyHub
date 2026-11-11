// Provider 的额外请求参数（请求头 / 查询参数）在 DTO 上的往返规则：
//
//   1. 后端没配时**不出现**这两个键，读出来按空表处理（不需要第二份默认值）；
//   2. 写了就原样保留，`toJson()` **总是**写成对象（空表写 `{}`）——
//      更新接口把"缺省"解释为"保留原值"，只有显式空对象才能删掉最后一条。

import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/ai_models.dart';

Map<String, dynamic> _row({Map<String, String>? headers, Map<String, String>? query}) => {
  'id': 'opencode-gw',
  'displayName': 'OpenCode',
  'api': 'openai-completions',
  'baseURL': 'https://opencode.ai/zen/v1',
  'endpointTrust': 'public',
  'enabled': true,
  'revision': 3,
  'headers': ?headers,
  'query': ?query,
};

void main() {
  test('没有额外参数时按空表读，toJson 仍写空对象（用于清空）', () {
    final p = AiProvider.fromJson(_row());
    expect(p.headers, isEmpty);
    expect(p.query, isEmpty);
    expect(p.toJson()['headers'], isEmpty, reason: '空对象才是"清空"的显式表达');
    expect(p.toJson()['query'], isEmpty);
  });

  test('额外请求头与查询参数原样往返', () {
    final p = AiProvider.fromJson(
      _row(
        headers: {'x-opencode-session': 'ses_abc', 'X-Client': 'ComfyHub'},
        query: {'api-version': '2024-10-21'},
      ),
    );
    expect(p.headers['x-opencode-session'], 'ses_abc');
    expect(p.headers['X-Client'], 'ComfyHub');
    expect(p.query['api-version'], '2024-10-21');

    final back = AiProvider.fromJson(p.toJson());
    expect(back.headers, p.headers);
    expect(back.query, p.query);
  });

  test('坏形状的额外参数不会把整条 Provider 读挂', () {
    final p = AiProvider.fromJson({
      ..._row(),
      'headers': ['not', 'a', 'map'],
      'query': {'ok': '1', 'nested': {'x': 1}},
    });
    expect(p.headers, isEmpty, reason: '不是对象就当没配');
    expect(p.query, {'ok': '1'}, reason: '只为标量取值，嵌套项丢掉');
  });
}
