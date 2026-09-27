/// 运行期的「界面注解」查表层（`docs/copy-review-design.md` §2 / §7 C4）。
///
/// # 为什么要有这一层
///
/// 设置页里那些解释"这个开关干什么、密钥怎么存、预算怎么算"的话，**默认值必须留在
/// Dart 字面量里**（用户离线、后端没起来、手机上没配对桌面 —— 界面照样要能看）；
/// 但它们同时是"承诺性文案"，两端口径不能各说各话（R13 最怕的漂移）。
///
/// 于是分成两半：
///  * **默认值**留在调用点的字面量里 —— 永远可用；
///  * **覆盖值**在桌面（`copy_overrides` 表）里 —— 在线时用它，离线时当它不存在。
///
/// ```dart
/// final text = context.watch<CopyOverrides>().text('ui.settings.nsfw.note', '默认那句话');
/// ```
library;

import 'package:flutter/foundation.dart';

import 'copy_api.dart';

class CopyOverrides extends ChangeNotifier {
  final Map<String, String> _text = {};

  Object? _lastError;
  StackTrace? _lastStack;
  bool _loaded = false;

  /// 加载失败时的回调（默认什么都不做）。
  ///
  /// **为什么必须有**：离线时界面要静默用默认值（不能让后端不在就空一片），
  /// 但"失败"这件事本身不能被吞掉。没有这个钩子，"加载成功但一条覆盖都没有"
  /// 与"加载炸了"在外部**完全无法区分** —— 用例只能靠猜。
  void Function(Object error, StackTrace stack)? onError;

  /// 当前生效的覆盖值（`ui.*` 里的那几条）。只读视图，别拿它当写入口。
  Map<String, String> get overrides => Map.unmodifiable(_text);

  Object? get lastError => _lastError;

  /// 失败时的调用栈（日志里要用；界面不用）。
  StackTrace? get lastStack => _lastStack;

  /// 至少成功拉过一次（失败重试也算 —— 判据是"拉过了"而不是"拉到了"）。
  bool get isLoaded => _loaded;

  int get length => _text.length;

  /// 拉一次 `ui.*` 覆盖层。**不抛异常**：这就是"离线必须照常显示"的落点。
  ///
  /// 失败时**保留上一次成功的那一份**（一次网络抖动不该把已经生效的覆盖退回默认值），
  /// 并且把原因留在 [lastError] + 交给 [onError]。
  Future<void> load(CopyApi api) async {
    try {
      final entries = await api.catalog(scope: 'ui');
      final next = <String, String>{};
      for (final e in entries) {
        // 只认**真的被改过**的那几条：`current` 在没有覆盖时是默认值，
        // 把它当成覆盖会让下一版的默认值永远推不下来（默认值在 Dart 里才是真源）。
        final text = e.currentText;
        if (e.overridden && text != null) next[e.key] = text;
      }
      _text
        ..clear()
        ..addAll(next);
      _lastError = null;
      _lastStack = null;
    } catch (e, st) {
      _lastError = e;
      _lastStack = st;
      onError?.call(e, st);
    } finally {
      _loaded = true;
      // 无论成败都通知：失败也要让界面知道"试过了"（例如把"桌面没连上"标出来）。
      notifyListeners();
    }
  }

  /// 有覆盖用覆盖，没有（或还没加载 / 加载失败）用 [fallback]。
  ///
  /// 空串是**合法覆盖**（用户就是要把这句话删掉），所以判据是 `containsKey` 而不是
  /// "非空"—— `??` 已经天然做到了这一点，别改成 `isEmpty ? fallback : ...`。
  String text(String key, String fallback) => _text[key] ?? fallback;
}
