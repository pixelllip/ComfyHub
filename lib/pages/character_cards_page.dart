import 'package:material_ui/material_ui.dart';

import '../core/ai_api_client.dart';
import 'cards_page.dart';

/// 角色卡页（O8，2026-09-26；2026-10-21 并入**卡片库**）。
///
/// 真正的实现是 [CardsPage]（与画风卡共用同一套编辑器 / 示例管理）；
/// 这个类保留原来的名字，给既有用例 / 深链接一个稳定落点。「预设库 → 卡片库」
/// 入口现在直接开可切换的 [CardsPage]，不经过这里。
/// `switchable: false`：从该入口进入只显示角色卡，顶部切换器不切换到另一类卡
/// （卡片库入口使用可切换的 [CardsPage]）。
class CharacterCardsPage extends StatelessWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const CharacterCardsPage({super.key, this.api});

  @override
  Widget build(BuildContext context) =>
      CardsPage(initialKind: CardKind.character, switchable: false, api: api);
}
