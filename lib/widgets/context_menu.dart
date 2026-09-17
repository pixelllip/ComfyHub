import 'dart:async';

import 'package:flutter/material.dart';

import 'app_menu.dart';

/// 右键上下文菜单（替代 `showMenu` / `showContextMenuAt`）。
///
/// 用法：
/// ```dart
/// // ① 页面根部挂一个 ContextMenuScope（一个页面一个就够，别每行挂一个）
/// ContextMenuScope(child: ...页面内容...)
///
/// // ② 右键时
/// final action = await showAppContextMenu<String>(
///   context,
///   globalPosition: d.globalPosition,
///   buildOptions: (context) => [
///     MenuOption(value: 'link', icon: Icons.link, label: '关联提示词…'),
///   ],
/// );
/// if (action == 'link') { ... }
/// ```
///
/// ## 为什么不用 `showMenu` / `showContextMenuAt`（用户 bug ②）
///
/// `showMenu` 推的是 `_PopupMenuRoute`：它会铺一层铺满窗口的 `ModalBarrier`，
/// 而屏障的 `RawGestureDetector(behavior: HitTestBehavior.opaque)` 在命中测试里
/// **第一个命中并终止整条路径**（`_RenderTheater.hitTestChildren` 遇到第一个命中就停），
/// 于是菜单外的页面既收不到滚轮信号、也收不到拖动 —— 菜单一开，整页滚不动。
/// 上游 [flutter/flutter#90223] 至今未修，也没有任何公开开关能关掉这层屏障。
///
/// 这里改用 Material 3 的 `MenuAnchor`：它**不铺屏障**，菜单放在 `OverlayPortal` 里，
/// 靠 `TapRegion` 判断"点到外面了"来关闭；命中测试只在菜单面板矩形内成立，其余区域
/// 照常落到下面的页面 —— 所以**菜单开着也能滚列表、也能点列表项**。
///
/// [flutter/flutter#90223]: https://github.com/flutter/flutter/issues/90223
class ContextMenuScope extends StatefulWidget {
  final Widget child;

  const ContextMenuScope({super.key, required this.child});

  /// 从 [context] 往上找**最近**的 host（找不到返回 null）。
  ///
  /// 两个坑，缺一不可：
  ///  1. 右键的调用点常常拿的是**页面自己的** `context`（`State.context`，例如
  ///     `_GalleryPageState.context`），而页面 `build` 里 `return ContextMenuScope(...)`
  ///     意味着这个 context 在 Scope **外面** —— 但它同时又是 Scope 所在元素的**祖先**，
  ///     所以"向上找 InheritedWidget"这条路是通的（[InheritedWidget] 的查找本来就走祖先链）。
  ///  2. 有些右键来自**另一条路由**里的组件（弹窗 / 底部面板用的是 root navigator），
  ///     祖先链上确实没有 Scope；这时退回"已挂载 Scope 注册表"，挑 context 落在它里面的那个。
  static ContextMenuScopeState? maybeOf(BuildContext context) {
    final viaTree = context
        .findAncestorWidgetOfExactType<_ContextMenuHostScope>()
        ?.scope;
    if (viaTree != null) return viaTree;

    // 兜底：注册表里挑"其元素是 context 祖先"的那个（注册顺序=挂载顺序，倒着找即最近）
    final target = context as Element?;
    if (target == null) return null;
    for (final scope in _mounted.reversed) {
      final element = scope.context as Element?;
      if (element == null) continue;
      if (identical(element, target)) return scope;
      var hit = false;
      target.visitAncestorElements((ancestor) {
        if (identical(ancestor, element)) {
          hit = true;
          return false;
        }
        return true;
      });
      if (hit) return scope;
    }
    return null;
  }

  /// 已挂载的 Scope（挂载顺序）。页面不多（一页一个），线性扫足够。
  static final List<ContextMenuScopeState> _mounted = [];

  @override
  State<ContextMenuScope> createState() => ContextMenuScopeState();
}

/// 把 host 挂进树里的 InheritedWidget：让任意后代（哪怕用的是页面自己的
/// `State.context`）都能用 `findAncestorWidgetOfExactType` 找到它。
class _ContextMenuHostScope extends InheritedWidget {
  final ContextMenuScopeState scope;

  const _ContextMenuHostScope({required this.scope, required super.child});

  @override
  bool updateShouldNotify(_ContextMenuHostScope oldWidget) =>
      !identical(oldWidget.scope, scope);
}

/// 公开给 `showAppContextMenu` 用（它要能调 [show]）。
class ContextMenuScopeState extends State<ContextMenuScope> {
  ContextMenuRequest? _request;

  /// 菜单面板和它的"点外面检测"要算同一个 `TapRegion` 组，
  /// `onTapOutside` 才不会把菜单内部的点击也当成"点外面"。
  final _groupId = Object();

  /// 菜单挂在 **root overlay** 的 `OverlayPortal` 里。
  ///
  /// 为什么不用 `MenuAnchor` 的 `MenuController.open(position:)`：
  ///   · 它的 `position` 是"相对锚点子树左上角"（`_MenuLayout._positionChild` 里算
  ///     `menuPosition + anchorRect.topLeft`），为了右键坐标还得反算一次锚点原点；
  ///   · 更麻烦的是它"点外面关掉"依赖 `TapRegion` 的注册表，而我们的菜单挂在
  ///     `OverlayPortal` 里时**拿不到那个注册表** —— 实测点外面菜单不关。
  /// 换成自己摆一个 `OverlayPortal` + `TapRegion(onTapOutside:)`：坐标就是 Overlay 坐标，
  /// 关闭也由我们自己的回调负责（实测可靠）。
  final _overlay = OverlayPortalController();

  @override
  void initState() {
    super.initState();
    ContextMenuScope._mounted.add(this);
  }

  @override
  void dispose() {
    ContextMenuScope._mounted.remove(this);
    // 页面销毁时菜单还开着：让 await 的那一侧拿到 null，别永远挂着
    _request?.completer.complete(null);
    _request = null;
    super.dispose();
  }

  /// 弹出菜单并等待选择结果（点外面 / 菜单自己关掉 / 页面销毁 → null）。
  Future<T?> show<T>(ContextMenuRequest<T> request) {
    // 同一时刻只允许一个右键菜单：先前那个按"取消"处理
    _request?.completer.complete(null);
    _request = request;
    setState(() {});
    _overlay.show();
    return request.completer.future;
  }

  void _pick(Object? value) {
    final pending = _request;
    if (pending == null) return;
    _request = null;
    _overlay.hide();
    setState(() {});
    // 先拆掉菜单再完成 Future：await 之后的代码可能又弹一个菜单
    pending.completer.complete(value);
  }

  @override
  Widget build(BuildContext context) {
    return OverlayPortal(
      controller: _overlay,
      overlayChildBuilder: (context) {
        final pending = _request;
        if (pending == null) return const SizedBox.shrink();
        return _buildOverlay(context, pending);
      },
      // _ContextMenuHostScope 必须包在**子树的根**上：这样页面里任意深度的后代
      // （包括页面自己那个 `State.context`）都能往上找到它。
      child: _ContextMenuHostScope(scope: this, child: widget.child),
    );
  }

  Widget _buildOverlay(BuildContext context, ContextMenuRequest pending) {
    final theme = Theme.of(context);
    // 菜单尺寸按屏幕边界夹一下：点得太靠右下角时不能跑出窗口
    final screen = MediaQuery.sizeOf(context);
    final estimated = pending.estimatedSize();
    final maxLeft = (screen.width - estimated.width - 8).clamp(8.0, screen.width);
    final maxTop = (screen.height - estimated.height - 8).clamp(8.0, screen.height);
    final left = pending.globalPosition.dx.clamp(8.0, maxLeft);
    final top = pending.globalPosition.dy.clamp(8.0, maxTop);

    return TapRegion(
      groupId: _groupId,
      // 点菜单外面 → 关掉并按"没选"处理（等价于 showMenu 的 barrierDismissible）
      onTapOutside: (_) => _pick(null),
      child: Stack(
        children: [
          Positioned(
            left: left,
            top: top,
            child: TapRegion(
              groupId: _groupId,
              child: Material(
                elevation: 8,
                borderRadius: BorderRadius.circular(8),
                clipBehavior: Clip.antiAlias,
                color: theme.menuTheme.style?.backgroundColor?.resolve({}) ?? theme.canvasColor,
                child: IntrinsicWidth(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: pending.childrenOf(context, onPick: _pick),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// 一次右键菜单请求：位置 + 菜单项 + 结果通道。
///
/// [buildOptions] 在菜单真正渲染的那一帧才执行，所以拿到的永远是**当前**状态
/// （"关联提示词"还是"更换关联提示词"取决于此刻有没有关联）。
class ContextMenuRequest<T> {
  /// 右键点的**全局**坐标。
  final Offset globalPosition;

  final MenuSpec<T> Function(BuildContext context) buildOptions;
  final Completer<T?> completer = Completer<T?>();

  ContextMenuRequest({
    required this.globalPosition,
    required this.buildOptions,
  });

  /// 估一下菜单有多大，只用于"别跑出屏幕"的夹取（真实尺寸由 `IntrinsicWidth` 决定）。
  /// 刻意**不**调用 [buildOptions]：那个构造器可能要读 `Theme`。
  Size estimatedSize() => const Size(240, 240);

  List<Widget> childrenOf(
    BuildContext context, {
    required void Function(Object?) onPick,
  }) {
    final theme = Theme.of(context);
    final children = <Widget>[];
    for (final option in buildOptions(context)) {
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
          style: MenuItemButton.styleFrom(
            foregroundColor: color,
            minimumSize: const Size(0, 38),
            padding: const EdgeInsets.symmetric(horizontal: 12),
            visualDensity: VisualDensity.compact,
          ),
          onPressed: option.enabled ? () => onPick(option.value) : null,
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
          child: Text(option.label ?? ''),
        ),
      );
    }
    return children;
  }
}

/// 在**全局坐标** [globalPosition] 处弹出右键菜单。
///
/// 返回被点项的值；点菜单外面 / 页面被销毁返回 null（调用方按"什么都没做"处理）。
Future<T?> showAppContextMenu<T>(
  BuildContext context, {
  required Offset globalPosition,
  required MenuSpec<T> Function(BuildContext context) buildOptions,
}) {
  final host = ContextMenuScope.maybeOf(context);
  if (host == null) {
    // 页面忘了挂 ContextMenuScope：接线错误直接报出来，比"菜单不弹"好查
    throw FlutterError(
      'showAppContextMenu 需要在页面上挂一个 ContextMenuScope（见 lib/widgets/context_menu.dart 顶部说明）。',
    );
  }
  return host.show<T>(
    ContextMenuRequest<T>(
      globalPosition: globalPosition,
      buildOptions: buildOptions,
    ),
  );
}
