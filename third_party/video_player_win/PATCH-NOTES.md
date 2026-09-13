# 本地补丁说明（third_party/video_player_win）

这是 [`video_player_win`](https://pub.dev/packages/video_player_win) **3.2.2** 的本地副本，
`pubspec.yaml` 通过 path 依赖指向它。上游代码本身没动，只打了**一处**补丁。

## 补了什么问题

**Windows 上打开视频只有声音、画面全黑。**

链路是这样的：

1. Flutter 3.4x 在 Windows 上**默认启用 Impeller**（日志里能看到
   `Using the Impeller rendering backend (OpenGLESSDF)`）；
2. Impeller 建外部纹理时，尺寸取自 `FlutterDesktopGpuSurfaceDescriptor`
   的 **`visible_width` / `visible_height`**（见引擎 `embedder_external_texture_gl.cc`
   的 `ResolveTextureImpeller()`）；
3. 上游插件在 `MyPlayerInternal::initTexture()` 里只填了 `width` / `height`，
   `visible_*` 一直是结构体 memset 出来的 **0**；
4. 于是纹理是 0×0，`TextureGLES::WrapTexture()` 返回空 →
   引擎刷 `Could not create external texture` → 画面全黑。
   Media Foundation 的音频跟纹理无关，照放不误。

Skia 后端（`ResolveTextureSkia()`）在尺寸为 0 时会退回用「控件尺寸」，
所以同一份插件在 Skia 下是正常的 —— 这就是为什么以前没事、升级 Flutter 之后才炸。

## 改动点

`windows/video_player_win_plugin.cpp` 的 `initTexture()`，两行，搜索 `ComfyHub 补丁`：

```cpp
texture_buffer.visible_width = desc.Width;
texture_buffer.visible_height = desc.Height;
```

## 维护方式

- **不要**把它换回 pub.dev 版本，除非上游已经修了这个字段（发行说明里没写就是没修）。
- 上游修好后：删掉本目录、`pubspec.yaml` 改回 `video_player_win: ^x.y.z`、
  跑一次 `flutter pub get`，并同步删掉 `analysis_options.yaml` 里的 `third_party/**` 排除项
  和 README 第 12 / 13 节里的相关说明。
- 同步上游新版本时：整包替换，然后把这处补丁重新加上（改动就两行）。
- 只改了 `windows/` 下的原生代码，Dart 侧与上游完全一致。
