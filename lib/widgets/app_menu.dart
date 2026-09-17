/// 应用统一的下拉 / 按钮菜单（用户 bug ②：**弹出菜单不该挡住页面滚动**）。
///
/// ## 为什么不用 `PopupMenuButton` / `showMenu`
///
/// Material 的 `_PopupMenuRoute` 会铺一层挡住整个窗口的 `ModalBarrier`，而屏障参与
/// 命中测试并吃掉全部指针事件 —— 菜单一开，底下的列表**滚轮与拖动全部失效**，
/// 菜单外的区域也点不动（`test/menu_test.dart` 两条用例把这个行为钉住了）。
///
/// `MenuAnchor`（Material 3）不铺屏障：菜单放在 `OverlayPortal` 里，再用 `TapRegion`
/// 判断"点到外面了"来关闭。指针事件照常落到下面的页面 —— **菜单开着也能滚列表**，
/// 点空白处照样关菜单，正是用户要的效果。
///
/// 右键菜单（`showAppContextMenu`，见 `context_menu.dart`）走同一套机制。
///
/// ```dart
/// AppMenuButton(
///   tooltip: '排序',
///   onSelected: store.setSort,
///   button: (context, controller, isOpen) => Chip(label: Text('最新加入')),
///   options: const [MenuOption(value: 'newest', label: '最新加入')],
/// )
/// ```
library;

import 'package:flutter/material.dart';

/// 菜单里的一项。
///
/// [value] 为 null 且 [label] 为 null 表示"分隔线"（用 [MenuOption.divider]）。
/// [leading] 是可选的自定义前导控件（例如主题色圆点），给了就优先于 [icon]。
@immutable
class MenuOption<T> {
  final T? value;
  final String? label;

  /// 第二行说明（`PopupMenuItem` 里那种 `ListTile(subtitle:)` 的替代）。
  final String? subtitle;

  final IconData? icon;
  final Widget? leading;

  /// 尾部控件：用来标"当前选中"（`Icons.check` / `Icons.radio_button_checked` 等）。
  final IconData? trailingIcon;

  final bool enabled;
  final bool danger;

  /// 在这一项**前面**画一条分隔线（`PopupMenuDivider` 的替代）。
  final bool dividerBefore;

  const MenuOption({
    required this.value,
    required this.label,
    this.subtitle,
    this.icon,
    this.leading,
    this.trailingIcon,
    this.enabled = true,
    this.danger = false,
    this.dividerBefore = false,
  });

  /// 分隔线。
  const MenuOption.divider()
    : value = null,
      label = null,
      subtitle = null,
      icon = null,
      leading = null,
      trailingIcon = null,
      enabled = false,
      danger = false,
      dividerBefore = false;

  bool get isDivider => value == null && label == null;
}

/// 一组菜单项。
typedef MenuSpec<T> = List<MenuOption<T>>;

/// 菜单面板外观：对齐 M2 的观感（M3 的 `MenuStyle` 默认更扁平）。
MenuStyle appMenuStyle(ThemeData theme) => MenuStyle(
  backgroundColor: WidgetStatePropertyAll(
    theme.menuTheme.style?.backgroundColor?.resolve({}),
  ),
  elevation: const WidgetStatePropertyAll(8),
  shadowColor: WidgetStatePropertyAll(theme.colorScheme.shadow),
  padding: const WidgetStatePropertyAll(EdgeInsets.symmetric(vertical: 4)),
  shape: WidgetStatePropertyAll(
    RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
  ),
);

/// [MenuOption] → `MenuAnchor.menuChildren`。
List<Widget> menuChildrenOf<T>(
  BuildContext context, {
  required MenuSpec<T> spec,
  required void Function(T value) onSelected,
}) {
  final theme = Theme.of(context);
  final children = <Widget>[];
  for (final option in spec) {
    if (option.isDivider) {
      children.add(const Divider(height: 9, thickness: 1));
      continue;
    }
    if (option.dividerBefore && children.isNotEmpty) {
      children.add(const Divider(height: 9, thickness: 1));
    }
    final color = option.danger ? theme.colorScheme.error : null;
    children.add(
      MenuItemButton(
        style: MenuItemButton.styleFrom(foregroundColor: color),
        onPressed: option.enabled ? () => onSelected(option.value as T) : null,
        leadingIcon:
            option.leading ??
            (option.icon == null
                ? null
                : Icon(
                    option.icon,
                    size: 18,
                    color: option.enabled
                        ? color ?? theme.colorScheme.onSurfaceVariant
                        : theme.disabledColor,
                  )),
        trailingIcon: option.trailingIcon == null
            ? null
            : Icon(
                option.trailingIcon,
                size: 18,
                color: theme.colorScheme.primary,
              ),
        child: option.subtitle == null
            ? Text(option.label ?? '')
            : Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(option.label ?? ''),
                  const SizedBox(height: 2),
                  Text(
                    option.subtitle!,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ],
              ),
      ),
    );
  }
  return children;
}

/// 按钮 + 下拉菜单：用法等价于 `PopupMenuButton`，但**不吃掉页面滚动**。
class AppMenuButton<T> extends StatefulWidget {
  /// 菜单项；[onSelected] 拿到的就是被点项的 `value`。
  final MenuSpec<T> options;

  /// 按钮本体。[controller] 用来开关菜单，[isOpen] 用来画"展开中"的样子。
  final Widget Function(
    BuildContext context,
    MenuController controller,
    bool isOpen,
  )
  button;

  final void Function(T value) onSelected;
  final String? tooltip;

  /// 相对锚点的偏移（`MenuAnchor.alignmentOffset`）。
  final Offset alignmentOffset;

  const AppMenuButton({
    super.key,
    required this.options,
    required this.button,
    required this.onSelected,
    this.tooltip,
    this.alignmentOffset = Offset.zero,
  });

  @override
  State<AppMenuButton<T>> createState() => _AppMenuButtonState<T>();
}

class _AppMenuButtonState<T> extends State<AppMenuButton<T>> {
  final _controller = MenuController();

  @override
  Widget build(BuildContext context) {
    final anchor = MenuAnchor(
      controller: _controller,
      style: appMenuStyle(Theme.of(context)),
      alignmentOffset: widget.alignmentOffset,
      // 这是"按钮上的菜单"（不是右键菜单）：点按钮本身不算"点外面"，
      // 开/关交给按钮自己的 onPressed（见 _toggle），与 PopupMenuButton 手感一致。
      consumeOutsideTap: false,
      menuChildren: menuChildrenOf<T>(
        context,
        spec: widget.options,
        onSelected: (v) {
          _controller.close();
          widget.onSelected(v);
        },
      ),
      builder: (context, controller, _) =>
          widget.button(context, controller, controller.isOpen),
    );
    final tip = widget.tooltip;
    return tip == null ? anchor : Tooltip(message: tip, child: anchor);
  }
}
