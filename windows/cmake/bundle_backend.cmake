# 由 `windows\CMakeLists.txt` 在 **release 装配阶段** install(SCRIPT) 进来。
#
# 干三件事（做完之后 `build\windows\x64\runner\Release\` 本身就是完整发布包）：
#
#   ① `cargo build --release`：**构建 release 时后端一起更新到最新**（用户要求）；
#   ② 把后端 exe 装到 `<bundle>\backend\`（App 自己的资产，viewer.exe 同级），
#      并把后端**运行期真的要用的**几样一起装上：`scripts\video-poster.ps1`
#      （抽视频封面，`poster.rs` 会起它）、`db\`（DDL 副本，给人看）、`skills\builtin`；
#   ③ 把 viewer.exe 与后端 exe 的**完整性标签**设回 Medium。
#
# # 为什么必须有 ③（2026-09-30 用户实测的两条事实）
#
#   · 写限制是 **完整性级别（IL）** 判的：exe 上带着 `Low` 标签 → 进程就是 Low →
#     Low 写不了 %TEMP% / ComfyUI 目录这些 Medium 位置（默认 NO_WRITE_UP）；
#   · Windows 取 **min(父进程, 映像)**：链路上**任何一环**是 Low，子进程照样掉回 Low
#     —— 而"在工作区里构建出来的产物普遍带 Low 标签"，`viewer.exe` 也不例外。
#
# 所以两个 exe 都要归一。⚠ 这一步在**受限会话里构建**时改不动（Low 写不了向上的
# 标签，icacls 会失败）—— 那时脚本只**警告**不中断构建，修法是"换个不受限的终端
# 重新构建一次"，或在运行时把整个产物拷到工作区外（AGENTS §12）。
#
# 变量（由 windows\CMakeLists.txt 用 install(CODE) 提前设好 —— **别在这里直接读
# CMAKE_INSTALL_PREFIX**：它的真值是生成器表达式 `$<TARGET_FILE_DIR:viewer>`，
# install 阶段读到的会是那串没展开的原字符串）：
#   COMFYHUB_SRC_ROOT    —— 源码树根目录
#   COMFYHUB_BUNDLE_DIR  —— 装配目标（就是 viewer.exe 所在的 bundle 目录）

set(_root "${COMFYHUB_SRC_ROOT}")
set(_bundle "${COMFYHUB_BUNDLE_DIR}")
set(_manifest "${_root}/server-rs/Cargo.toml")
set(_exe "${_root}/server-rs/target/release/comfyhub-server-rs.exe")

message(STATUS "ComfyHub: cargo build --release（release 构建会把后端一起更新到最新）")
execute_process(
  COMMAND cargo build --release --manifest-path "${_manifest}"
  RESULT_VARIABLE _rc)
if(NOT _rc STREQUAL "0")
  message(FATAL_ERROR
    "ComfyHub: cargo build --release 失败（结果：${_rc}）。\n"
    "   · 后端正在运行时，Windows 锁着那个 exe —— 先把它停掉再构建\n"
    "     （报错长这样：failed to remove file ...comfyhub-server-rs.exe / 拒绝访问 (os error 5)）；\n"
    "   · 机器上没装 Rust 工具链的话：https://rustup.rs")
endif()
if(NOT EXISTS "${_exe}")
  message(FATAL_ERROR "ComfyHub: cargo 说成功了，却找不到 ${_exe}")
endif()

file(INSTALL "${_exe}" DESTINATION "${_bundle}/backend")
message(STATUS "ComfyHub: 后端已装为 App 自己的资产 —— ${_bundle}/backend/comfyhub-server-rs.exe")

# 后端运行期会用到的只读件（缺了不影响启动，但视频封面 / 内置 skill 会少一块）
if(EXISTS "${_root}/scripts/video-poster.ps1")
  file(INSTALL "${_root}/scripts/video-poster.ps1" DESTINATION "${_bundle}/scripts")
endif()
if(EXISTS "${_root}/db/schema.sqlite.sql")
  file(INSTALL "${_root}/db/schema.sqlite.sql" DESTINATION "${_bundle}/db")
endif()
if(EXISTS "${_root}/skills/builtin")
  file(INSTALL "${_root}/skills/builtin" DESTINATION "${_bundle}/skills")
endif()

# ③ 完整性标签归一（best-effort：失败只警告 —— 受限会话里构建时必然失败）
foreach(_target "${_bundle}/viewer.exe" "${_bundle}/backend/comfyhub-server-rs.exe")
  if(EXISTS "${_target}")
    execute_process(
      COMMAND icacls "${_target}" /setintegritylevel Medium
      RESULT_VARIABLE _il
      OUTPUT_VARIABLE _il_out
      ERROR_VARIABLE _il_out)
    if(_il STREQUAL "0")
      message(STATUS "ComfyHub: 完整性标签已归一为 Medium —— ${_target}")
    else()
      message(WARNING
        "ComfyHub: 没能把完整性标签设回 Medium（${_target}）：${_il_out}\n"
        "   在受限会话（工作区写沙箱）里构建时必然如此。若后端报「写不进 ComfyUI 目录」，\n"
        "   把整个产物拷到工作区外再跑，或换个不受限的终端重新构建一次（见 AGENTS 第 12 节）。")
    endif()
  endif()
endforeach()
