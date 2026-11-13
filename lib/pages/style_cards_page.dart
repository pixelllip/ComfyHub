import 'package:material_ui/material_ui.dart';

import '../core/ai_api_client.dart';
import 'cards_page.dart';

/// 画风卡页（2026-10-21）。
///
/// 与角色卡**同等地位**：同一个 [CardsPage] 实现，只是初始种类是画风卡。
/// 卡片库入口（可切换）与这个专用入口都能用；测试也可以直接 pump 这一页。
class StyleCardsPage extends StatelessWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const StyleCardsPage({super.key, this.api});

  @override
  Widget build(BuildContext context) =>
      CardsPage(initialKind: CardKind.style, switchable: false, api: api);
}
