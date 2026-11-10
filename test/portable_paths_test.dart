// portable_paths：偏好落盘位置**一律**在自身文件夹内（`<项目根>\storage\app-support`），
// 并把 `%APPDATA%` 里已有的那份一次性导入进来。
//
// 2026-09-28 原本是"默认位置写得进就一切照旧、写不进才重定向"；2026-09-29 把方向反过来了
// （理由见 portable_paths.dart 的文件头）：偏好是 ComfyHub 自己产生的内容，
// 默认就该在自身文件夹内，而且那个"探针能不能建文件"的判据本身就不可靠 ——
// 它取决于进程的映像位置（AGENTS §12），会让同一条逻辑在源码树/发布包给出相反结论。
//
// 回归盯两件事：
//   1. WorkspacePathProvider 真的把 app-support 指到指定目录；
//   2. installPortablePaths 在任何机器上都不抛（它跑在 main() 最前面，抛了就白屏），
//      且**重复调用是幂等的**（导入只执行一次，不会覆盖新保存的值）。
//
// 原来那条 dirWritable 的用例已经删掉：生产路径不再有"探针"这个概念，
// 留着它等于给"退回探针判据"留后门。
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/core/portable_paths.dart';

void main() {
  test('WorkspacePathProvider 返回指定目录', () async {
    final provider = WorkspacePathProvider(r'D:\somewhere\app-support');
    expect(
      await provider.getApplicationSupportPath(),
      r'D:\somewhere\app-support',
    );
  });

  test('installPortablePaths 不抛（main() 最前面跑，抛了就白屏）', () async {
    await expectLater(installPortablePaths(), completes);
  });

  test('重复调用是幂等的（导入只做一次，不会把新值冲掉）', () async {
    await installPortablePaths();
    await installPortablePaths();
    await expectLater(installPortablePaths(), completes);
  });
}
