# 由 `windows\CMakeLists.txt` 在 **release 装配阶段** install(SCRIPT) 进来。
#
# 干五件事（做完之后 `build\windows\x64\runner\Release\` 本身就是完整发布包）：
#
#   ① `cargo build --release`：**构建 release 时后端一起更新到最新**（用户要求）；
#   ② 把后端 exe 装到 `<bundle>\backend\`（App 自己的资产，viewer.exe 同级），
#      并把后端**运行期真的要用的**几样一起装上：`scripts\video-poster.ps1`
#      （抽视频封面，`poster.rs` 会起它）、`db\`（DDL 副本，给人看）、`skills\builtin`；
#   ③ 把 viewer.exe 与后端 exe 的**完整性标签**设回 Medium；
#   ④ 把上一份构建留在产物里的**运行期数据**（`.db` / `storage` / `.run` /
#      `app-support`）挪进 `_leftover-data\` —— 不删，只是别让复制出去的构建把它
#      当成"用户的库"；
#   ⑤ 把源码树里的 `data-dir.json`（数据目录指针）带进产物，复制出去还是同一份数据。
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

# ④ 产物目录里**不该有运行期数据**（2026-11-10）
#
# 用户 2026-11-10 报的 bug：把 Release 构建（除了 storage）复制到别处打开，对话只剩
# 几条旧的、角色卡不是现在这版、画廊全是加载失败。真因是**上一份构建在这个目录里
# 跑过**，于是 `.db\comfyhub.db`、`storage\`、`.run\`、`app-support\` 都被留在了产物
# 里；复制到别处之后那个目录自己成了根，App 就照着这份旧库开了。
#
# ⚠ **不删**，只挪到 `_leftover-data\`：
#   本机实测（2026-11-10）那份 `Release\storage\media` 里**有 30 张图在用户的数据目录
#   里根本不存在** —— 它们是更早那版库的独有内容。构建步骤没有资格判断"这份数据还要
#   不要"，删掉就真没了。挪到不叫 `.db` / `storage` 的目录之后，App 不会再把它当数据
#   目录，复制出去的构建也就不会"变成另一份库"（这是这次事故的根）。
foreach(_junk ".db" "storage" ".run" "app-support")
  if(EXISTS "${_bundle}/${_junk}")
    file(MAKE_DIRECTORY "${_bundle}/_leftover-data")
    if(EXISTS "${_bundle}/_leftover-data/${_junk}")
      # 上次已经挪过一份同名的：**不覆盖**（两份都是数据，谁都可能是唯一副本）
      message(WARNING
        "ComfyHub: 产物里又出现 ${_junk}，但 _leftover-data/${_junk} 已经有一份了 —— 留在原处不动。"
        "复制这个目录之前先确认它是不是你要的数据。")
    else()
      file(RENAME "${_bundle}/${_junk}" "${_bundle}/_leftover-data/${_junk}")
      message(STATUS "ComfyHub: 产物里的运行期残留已挪到 _leftover-data/${_junk}（没删）")
    endif()
  endif()
endforeach()

# ⑤ 数据目录指针跟着走：`data-dir.json` 有就带进产物
#
# 复制出去的构建要还是**同一份数据**（用户就是这么用发布包的）。指针在别的机器上
# 指不到东西时，App 会回落到运行根目录，并把这件事说出来（backend_paths.dart 的
# `resolveDataDir`）。源码树里没设过指针时，产物里那份旧的也要清掉。
if(EXISTS "${_root}/data-dir.json")
  file(INSTALL "${_root}/data-dir.json" DESTINATION "${_bundle}")
  message(STATUS "ComfyHub: 数据目录指针已带进产物（data-dir.json）")
elseif(EXISTS "${_bundle}/data-dir.json")
  file(REMOVE "${_bundle}/data-dir.json")
  message(STATUS "ComfyHub: 源码树没设数据目录指针，清掉产物里那份旧的")
endif()
