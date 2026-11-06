//! 产物**集合**在前端这一侧的两条规则。
//!
//! 判定本身**完全在后端**（`server-rs/src/media/collection.rs`），前端一个字都不算 ——
//! 这里只钉两件事：
//! 1. DTO 里那几个字段怎么解析（`collectionId` / `collectionSize` / `collectionPreview`）；
//! 2. `isCollection` 的判据（**成员多于一个**才算一组），它是"要不要叠放封面"的唯一开关。
//!
//! 为什么值得单独测：这两条错了不会报错，只会让画廊把单张产物显示成"一组"（角标写 1），
//! 或者把真的同批产物拆成散图 —— 两种都只能靠肉眼发现。

import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/models.dart';

Map<String, dynamic> _json({
  int id = 1,
  Object? collectionId,
  Object? collectionSize,
  Object? collectionPreview,
}) => {
  'id': id,
  'kind': 'IMAGE',
  'title': 't',
  'originalName': 'a.png',
  'storedName': 'a.png',
  'sizeBytes': 0,
  'favorite': false,
  'nsfw': false,
  'hasWorkflow': false,
  'fileUrl': '/api/media/$id/file',
  'thumbUrl': '/api/media/$id/thumb',
  'collectionId': ?collectionId,
  'collectionSize': ?collectionSize,
  'collectionPreview': ?collectionPreview,
};

void main() {
  test('没进组的产物：collectionSize 退回 1，不算一组', () {
    final m = MediaAsset.fromJson(_json());
    expect(m.collectionId, isNull);
    expect(m.collectionSize, 1, reason: '后端保证 ≥1；键缺失时也要退回 1，别误判成一组');
    expect(m.collectionPreview, isEmpty);
    expect(m.isCollection, isFalse);
  });

  test('只有自己一个的集合：collectionId 有值但不算一组', () {
    final m = MediaAsset.fromJson(
      _json(collectionId: 7, collectionSize: 1, collectionPreview: [1]),
    );
    expect(m.collectionId, 7);
    expect(m.isCollection, isFalse, reason: '一张的"组"按普通格子显示，不叠放、不显示角标');
  });

  test('成员多于一个：算一组，且解析出可叠的封面 id', () {
    final m = MediaAsset.fromJson(
      _json(
        collectionId: 7,
        collectionSize: 4,
        collectionPreview: [11, 12, 13],
      ),
    );
    expect(m.isCollection, isTrue);
    expect(m.collectionSize, 4);
    expect(m.collectionPreview, [
      11,
      12,
      13,
    ], reason: '后端只给头三张，总数靠 collectionSize');
  });

  test('collectionId 有、size 缺失：不许当成一组（宁可少叠，不可误叠）', () {
    final m = MediaAsset.fromJson(_json(collectionId: 7));
    expect(m.collectionId, 7);
    expect(m.collectionSize, 1);
    expect(m.isCollection, isFalse);
  });

  test('collectionPreview 里的脏值不会让解析炸掉', () {
    final m = MediaAsset.fromJson(
      _json(collectionId: 7, collectionSize: 2, collectionPreview: []),
    );
    expect(m.collectionPreview, isEmpty);
    expect(m.isCollection, isTrue, reason: 'size>1 就是一组，封面拿不到时退化成单图渲染');
  });
}
