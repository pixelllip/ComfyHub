//! 视频封面 / 预览帧（P4 第 51 步）—— Kotlin `MediaFiles.writeVideoPoster` + 两条路由的移植。
//!
//! # 为什么是"起一个脚本"而不是 ffmpeg
//!
//! ffmpeg 不是每台机器都有，但 **Windows 资源管理器的缩略图管线**（Media Foundation）
//! 对所有能播的视频都能出图 —— `scripts\video-poster.ps1` 就是干这个的
//! （Shell 的 `IShellItemImageFactory` 取缩略图 + WPF 的 `PngBitmapEncoder` 存 PNG）。
//! Rust 侧只负责"找到脚本、起子进程、看结果"，**不重写抽帧逻辑**：
//! 重写一份 = 两套行为，而用户看到的是"画廊里的视频有封面、聊天里的没有"。
//!
//! # 两条纪律
//!
//!  1. **抽不出来就如实回 204**（不是破图、也不是 500）：播放器转圈，界面按既有约定退化。
//!  2. **缓存复用**：`<storage>/thumbs/<id>.poster.png` 已存在就直接发（`-Force` 只在显式要求时用）。
//!
//! # 两种布局都认
//!
//! `<根>\scripts\video-poster.ps1`（源码树与发布包同构）—— 根由 storage 往上找，
//! 与 `crate::attachments` / `crate::comfy_dirs` 用的是同一个 [`crate::routes::AppState::project_root`]。

use std::path::{Path, PathBuf};

/// 抽帧脚本相对项目根的位置。
pub const POSTER_SCRIPT: &str = r"scripts\video-poster.ps1";
/// 脚本超时（Kotlin `waitFor(60, SECONDS)`）。
pub const POSTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 封面最长边（Kotlin 默认 640）。
pub const POSTER_SIZE: u32 = 640;

/// 找到抽帧脚本（两种布局同构：都是 `<根>\scripts\video-poster.ps1`）。
pub fn locate_poster_script(project_root: &Path) -> Option<PathBuf> {
    let path = project_root.join(POSTER_SCRIPT);
    path.is_file().then_some(path)
}

/// `pwsh` 在哪（找不到就回 `"pwsh"`，让系统 PATH 自己去找）。
pub fn locate_pwsh() -> String {
    for candidate in [
        r"C:\Program Files\PowerShell\7\pwsh.exe",
        r"C:\Program Files\PowerShell\7-preview\pwsh.exe",
    ] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "pwsh".to_string()
}

/// 抽一帧存到 `dest`；返回是否真的拿到了封面。
///
/// 失败一律**回 false**（并把半份文件删掉）：调用方据此回 204。
/// 参数与脚本的 `-Source / -Dest / -Size / -Force` 一一对应。
pub fn write_video_poster(project_root: &Path, source: &Path, dest: &Path, size: u32) -> bool {
    let Some(script) = locate_poster_script(project_root) else {
        eprintln!("[poster] 找不到 {POSTER_SCRIPT}，跳过封面抽取");
        return false;
    };
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(dest);

    let mut command = std::process::Command::new(locate_pwsh());
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-File")
        .arg(&script)
        .arg("-Source")
        .arg(source)
        .arg("-Dest")
        .arg(dest)
        .arg("-Size")
        .arg(size.to_string())
        .arg("-Force");
    // 静默：不弹控制台窗口（AGENTS §2）
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(e) => {
            eprintln!("[poster] 起 pwsh 失败：{e}");
            let _ = std::fs::remove_file(dest);
            return false;
        }
    };
    let ok = dest.is_file() && std::fs::metadata(dest).map(|m| m.len() > 0).unwrap_or(false);
    if !ok {
        // 脚本自己会打印 `FAIL <原因>`；把它带回日志（排错就靠这一行）
        let text = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "[poster] 抽帧失败 {}：{}",
            source.display(),
            text.trim().chars().take(200).collect::<String>()
        );
        let _ = std::fs::remove_file(dest);
    }
    ok
}

/// 封面的缓存路径（`<storage>/thumbs/<id>.poster.png`，与图片缩略图 `*.jpg` 分开存）。
pub fn poster_path(media_dir: &Path, media_id: i64) -> PathBuf {
    media_dir
        .parent()
        .unwrap_or(media_dir)
        .join("thumbs")
        .join(format!("{media_id}.poster.png"))
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
