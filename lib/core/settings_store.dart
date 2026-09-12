import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// 全局设置（后端地址、自动启动、MySQL 数据目录等），持久化在本地。
class SettingsStore extends ChangeNotifier {
  static const _kBaseUrl = 'comfyhub.baseUrl';
  static const _kPageSize = 'comfyhub.pageSize';
  static const _kAutoStartBackend = 'comfyhub.autoStartBackend';
  static const _kStopServicesOnExit = 'comfyhub.stopServicesOnExit';
  static const _kProjectRoot = 'comfyhub.projectRoot';
  static const _kMysqlDataDir = 'comfyhub.mysqlDataDir';

  String _baseUrl = defaultBaseUrl();
  int _pageSize = 24;
  bool _autoStartBackend = true;
  bool _stopServicesOnExit = true;
  String? _projectRoot;
  String? _mysqlDataDir;
  bool _loaded = false;

  String get baseUrl => _baseUrl;
  int get pageSize => _pageSize;

  /// App 启动时是否自动把 MySQL + 后端拉起来
  bool get autoStartBackend => _autoStartBackend;

  /// 关闭 App 时是否把**这次由 App 启动的**本地服务一并停掉（谁起的谁关）。
  /// 关掉的话服务会常驻，下次开 App 就是热启动。
  bool get stopServicesOnExit => _stopServicesOnExit;

  /// 项目根目录（放着 scripts\comfyhub.ps1 的那个目录）。为空表示自动探测。
  String? get projectRoot => _projectRoot;

  /// 本地 MySQL 实例目录。为空表示用脚本默认值（<项目>\.mysql）。
  String? get mysqlDataDir => _mysqlDataDir;

  bool get loaded => _loaded;

  static String defaultBaseUrl() {
    if (kIsWeb) return 'http://127.0.0.1:8080';
    try {
      // Android 模拟器访问宿主机需要用 10.0.2.2
      if (Platform.isAndroid) return 'http://10.0.2.2:8080';
    } catch (_) {
      // 平台判断在个别环境下会抛异常，忽略即可
    }
    return 'http://127.0.0.1:8080';
  }

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    _baseUrl = prefs.getString(_kBaseUrl) ?? defaultBaseUrl();
    _pageSize = prefs.getInt(_kPageSize) ?? 24;
    _autoStartBackend = prefs.getBool(_kAutoStartBackend) ?? true;
    _stopServicesOnExit = prefs.getBool(_kStopServicesOnExit) ?? true;
    _projectRoot = prefs.getString(_kProjectRoot);
    _mysqlDataDir = prefs.getString(_kMysqlDataDir);
    _loaded = true;
    notifyListeners();
  }

  Future<void> setBaseUrl(String value) async {
    var v = value.trim();
    if (v.isEmpty) v = defaultBaseUrl();
    if (!v.startsWith('http://') && !v.startsWith('https://')) v = 'http://$v';
    while (v.endsWith('/')) {
      v = v.substring(0, v.length - 1);
    }
    _baseUrl = v;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kBaseUrl, v);
  }

  Future<void> setPageSize(int value) async {
    _pageSize = value.clamp(8, 120);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kPageSize, _pageSize);
  }

  Future<void> setAutoStartBackend(bool value) async {
    _autoStartBackend = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kAutoStartBackend, value);
  }

  Future<void> setStopServicesOnExit(bool value) async {
    _stopServicesOnExit = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kStopServicesOnExit, value);
  }

  Future<void> setProjectRoot(String? value) async {
    final v = value?.trim();
    _projectRoot = (v == null || v.isEmpty) ? null : v;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    if (_projectRoot == null) {
      await prefs.remove(_kProjectRoot);
    } else {
      await prefs.setString(_kProjectRoot, _projectRoot!);
    }
  }

  Future<void> setMysqlDataDir(String? value) async {
    final v = value?.trim();
    _mysqlDataDir = (v == null || v.isEmpty) ? null : v;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    if (_mysqlDataDir == null) {
      await prefs.remove(_kMysqlDataDir);
    } else {
      await prefs.setString(_kMysqlDataDir, _mysqlDataDir!);
    }
  }
}
