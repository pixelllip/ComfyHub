// 测试里的「系统选文件 / 选目录」替身。
//
// 为什么要它：`file_picker` 13.x 的 `FilePicker.pickFiles` / `getDirectoryPath`
// 是静态方法，内部走 `FilePickerPlatform.instance`。真机上那是 FFI 弹框
// （`windows_file_picker`），`flutter test` 里没有窗口可弹 —— 不换掉它，
// 一调就挂在 MissingPlugin / FFI 上。
//
// 用法：`final picker = FakeFilePicker(...)..install();`，`tearDown(picker.restore)`。
// 换掉的是平台实例本身，所以被测代码（`upload_sheet.dart` /
// `settings_comfy_capture_page.dart` / `settings_data_dir_page.dart`）一个字都不用改。

import 'dart:async';

import 'package:file_picker/file_picker.dart';

/// 一个只实现「上传面板真正用到的那几个成员」的 [PlatformFile]。
///
/// `path` 由基类从 [uri] 推出来（`file://` 才算本地文件），
/// `lengthSync()` 直接给构造时写好的大小。`xFile` / 读字节在测试里用不到，
/// 真被调到说明代码换了读取方式 —— 当场炸，比静默返回空数据好。
final class FakePlatformFile extends PlatformFile {
  FakePlatformFile(this.name, this.uri, {this.size});

  /// 本地文件的快捷构造（对应真机上 picker 返回的那种）。
  factory FakePlatformFile.local(String path, {int? size}) => FakePlatformFile(
    _basename(path),
    Uri.file(path, windows: true),
    size: size,
  );

  @override
  final String name;

  @override
  final Uri uri;

  /// `null` = 真机 picker 没顺手给大小（内部会退回读文件）。
  final int? size;

  @override
  int? lengthSync() => size;

  @override
  Future<int?> length() async => size;

  @override
  Never get xFile => throw UnsupportedError('测试里不读文件内容');

  @override
  Future<Never> readAsBytes() async => throw UnsupportedError('测试里不读文件内容');

  @override
  Stream<Never> readAsByteStream() =>
      Stream<Never>.error(UnsupportedError('测试里不读文件内容'));

  static String _basename(String path) {
    final cut = path.lastIndexOf(RegExp(r'[\\/]'));
    return cut < 0 ? path : path.substring(cut + 1);
  }
}

/// 可编程的 [FilePickerPlatform]：返回什么由用例说了算。
final class FakeFilePicker extends FilePickerPlatform {
  FakeFilePicker({this.directory, this.files = const []});

  /// 下一次 `getDirectoryPath` 返回什么；`null` = 取消选择。
  String? directory;

  /// 下一次 `pickFiles` 返回什么；空列表 = 取消选择。
  List<PlatformFile> files;

  int directoryCalls = 0;
  int pickFilesCalls = 0;

  FilePickerPlatform? _previous;

  /// 把自己装成平台实现（保留原来那个，[restore] 时装回去）。
  void install() {
    _previous = FilePickerPlatform.instance;
    FilePickerPlatform.instance = this;
  }

  void restore() {
    final previous = _previous;
    if (previous != null) FilePickerPlatform.instance = previous;
    _previous = null;
  }

  @override
  Future<List<PlatformFile>> pickFiles({
    String? dialogTitle,
    String? initialDirectory,
    FileType type = FileType.any,
    List<String>? allowedExtensions,
    Function(FilePickerStatus)? onFileLoading,
    int compressionQuality = 0,
    AndroidOptions androidOptions = const AndroidOptions(),
    DarwinOptions darwinOptions = const DarwinOptions(),
    WindowsOptions windowsOptions = const WindowsOptions(),
    LinuxOptions linuxOptions = const LinuxOptions(),
    WebOptions webOptions = const WebOptions(),
  }) async {
    pickFilesCalls++;
    return files;
  }

  @override
  Future<String?> getDirectoryPath({
    String? dialogTitle,
    String? initialDirectory,
    AndroidOptions androidOptions = const AndroidOptions(),
    WindowsOptions windowsOptions = const WindowsOptions(),
    LinuxOptions linuxOptions = const LinuxOptions(),
    WebOptions webOptions = const WebOptions(),
  }) async {
    directoryCalls++;
    return directory;
  }
}
