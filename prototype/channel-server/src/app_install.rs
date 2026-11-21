//! Per-user installation for the packaged Windows App.
//!
//! The package is a portable directory. A first GUI launch can copy it to one
//! stable per-user program directory, create the Start Menu shortcut and the
//! uninstall entry, then relaunch there. The updater later replaces exactly that
//! program directory; application data stays in the App data directory outside
//! it, so replacement never touches user data.

use anyhow::{Context, Result};
use std::{
    env,
    ffi::OsString,
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const PROGRAM_DIRECTORY: &str = "app";
const PORTABLE_MARKER: &str = "portable";
const ENTRYPOINT: &str = "piik-app.exe";
const SHORTCUT: &str = "Piik.lnk";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Piik";
const APP_KEY: &str = r"HKCU\Software\Piik";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
}

/// The program directory this executable belongs to.
fn program_directory() -> Result<PathBuf> {
    Ok(env::current_exe()
        .context("Locate the App executable")?
        .parent()
        .context("App directory is missing")?
        .to_path_buf())
}

/// The one stable per-user program directory, outside the application data.
pub fn install_directory() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("No local application data directory")?
        .join("Piik")
        .join(PROGRAM_DIRECTORY))
}

fn data_directory() -> Result<PathBuf> {
    Ok(match env::var_os("PIIK_CHANNEL_APP_DATA_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => crate::app::default_data_directory()
            .context("No local application data directory")?,
    })
}

fn declined_marker() -> Result<PathBuf> {
    let directory = data_directory()?;
    fs::create_dir_all(&directory).context("Create the App data directory")?;
    Ok(directory.join(PORTABLE_MARKER))
}

fn is_installed() -> bool {
    let Ok(executable) = env::current_exe() else {
        return false;
    };
    let Ok(directory) = install_directory() else {
        return false;
    };
    same_file(&executable, &directory.join(ENTRYPOINT))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// A package carries its immutable build identity beside the executable; a
/// development build in `target/` does not.
fn is_package() -> bool {
    program_directory()
        .map(|directory| {
            directory.join("REVISION").is_file() && directory.join("web").join("index.html").is_file()
        })
        .unwrap_or(false)
}

fn version() -> String {
    option_env!("PIIK_BUILD_VERSION")
        .unwrap_or("development")
        .to_owned()
}

/// Handle install and uninstall modes before any App runtime starts.
///
/// `Ok(true)` means this process only prepared the installation or removal and
/// must exit; `Ok(false)` continues the ordinary App startup.
pub fn entry() -> Result<bool> {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let has = |name: &str| arguments.iter().any(|argument| argument.to_str() == Some(name));

    if has("--uninstall") {
        uninstall()?;
        return Ok(true);
    }
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument.to_str() == Some("--uninstall-worker"))
    {
        let directory = arguments
            .get(index + 1)
            .map(PathBuf::from)
            .context("--uninstall-worker requires a program directory")?;
        let parent = arguments
            .get(index + 2)
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u32>().ok());
        // A directory cannot be removed while it is a process working directory.
        let _ = env::set_current_dir(env::temp_dir());
        wait_for_parent(parent);
        let mut removed = false;
        for _ in 0..40 {
            match fs::remove_dir_all(&directory) {
                Ok(()) => {
                    removed = true;
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(250)),
            }
        }
        if !removed {
            crate::app_windows::report_error(
                "无法完成卸载：请关闭正在运行的 Piik 后重试。\n\nCould not finish uninstalling: close Piik and try again.",
            );
        }
        if let Ok(worker) = env::current_exe() {
            let _ = Command::new("cmd.exe")
                .args([
                    "/d",
                    "/c",
                    &format!("ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", worker.display()),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
        return Ok(true);
    }

    if is_installed() {
        return Ok(false);
    }
    let explicit = has("--install");
    if !explicit {
        // Automated package checks and the terminal entry must never block on a
        // dialog; only a first interactive launch of a real package may ask.
        if has("--portable")
            || has("--browser")
            || env::var_os("PIIK_CHANNEL_APP_NO_BROWSER").is_some()
            || !is_package()
            || declined_marker()?.is_file()
        {
            return Ok(false);
        }
        let install = crate::app_windows::confirm(
            "安装 Piik 到本机？\n\n将创建开始菜单快捷方式和卸载项；聊天等数据仍保存在本机用户目录。\n\nInstall Piik for this user?",
        );
        if !install {
            let _ = fs::write(declined_marker()?, b"");
            return Ok(false);
        }
    }
    install(&arguments)?;
    Ok(true)
}

fn install(arguments: &[OsString]) -> Result<()> {
    let source = program_directory()?;
    let target = install_directory()?;
    fs::create_dir_all(target.parent().context("Install directory has no parent")?)?;
    if target.exists() {
        fs::remove_dir_all(&target).with_context(|| {
            format!(
                "Replace the previous installation at {}; close any running Piik first",
                target.display()
            )
        })?;
    }
    copy_tree(&source, &target)?;
    let executable = target.join(ENTRYPOINT);
    anyhow::ensure!(executable.is_file(), "Installed App entrypoint is missing");
    register(&executable, &target)?;
    create_shortcut(&executable, &target)?;
    launch(&executable, &target, arguments)
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source).with_context(|| format!("Read {}", source.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &destination)
                .with_context(|| format!("Copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn register(executable: &Path, directory: &Path) -> Result<()> {
    let executable = executable.to_string_lossy().into_owned();
    let directory = directory.to_string_lossy().into_owned();
    let version = version();
    let uninstall = format!("\"{executable}\" --uninstall");
    reg(&["add", UNINSTALL_KEY, "/v", "DisplayName", "/t", "REG_SZ", "/d", "Piik", "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "DisplayVersion", "/t", "REG_SZ", "/d", &version, "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "Publisher", "/t", "REG_SZ", "/d", "Piik", "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "DisplayIcon", "/t", "REG_SZ", "/d", &executable, "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "InstallLocation", "/t", "REG_SZ", "/d", &directory, "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "UninstallString", "/t", "REG_SZ", "/d", &uninstall, "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "NoModify", "/t", "REG_DWORD", "/d", "1", "/f"])?;
    reg(&["add", UNINSTALL_KEY, "/v", "NoRepair", "/t", "REG_DWORD", "/d", "1", "/f"])?;
    reg(&["add", APP_KEY, "/v", "InstallDirectory", "/t", "REG_SZ", "/d", &directory, "/f"])?;
    reg(&["add", APP_KEY, "/v", "Version", "/t", "REG_SZ", "/d", &version, "/f"])?;
    Ok(())
}

fn reg(arguments: &[&str]) -> Result<()> {
    let status = Command::new("reg.exe")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("Run reg.exe")?;
    anyhow::ensure!(status.success(), "reg.exe {} failed", arguments.join(" "));
    Ok(())
}

fn create_shortcut(executable: &Path, directory: &Path) -> Result<()> {
    let programs = PathBuf::from(env::var_os("APPDATA").context("APPDATA is missing")?)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    fs::create_dir_all(&programs).context("Create the Start Menu directory")?;
    let shortcut = programs.join(SHORTCUT);
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{}';$s.Description='Piik';$s.Save()",
        quote(&shortcut),
        quote(executable),
        quote(directory),
        quote(executable),
    );
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("Run PowerShell")?;
    anyhow::ensure!(status.success(), "Start Menu shortcut could not be created");
    Ok(())
}

fn quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn launch(executable: &Path, directory: &Path, arguments: &[OsString]) -> Result<()> {
    let forwarded: Vec<&OsString> = arguments
        .iter()
        .filter(|argument| !matches!(argument.to_str(), Some("--install" | "--portable")))
        .collect();
    Command::new(executable)
        .args(forwarded)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("Start the installed App")?;
    Ok(())
}

fn uninstall() -> Result<()> {
    if let Some(appdata) = env::var_os("APPDATA") {
        let shortcut = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join(SHORTCUT);
        let _ = fs::remove_file(shortcut);
    }
    let _ = reg(&["delete", UNINSTALL_KEY, "/f"]);
    let _ = reg(&["delete", APP_KEY, "/f"]);
    let directory = install_directory()?;
    if directory.is_dir() {
        // The running image cannot remove its own directory; one copied helper
        // waits for this process, then retires the installed program tree.
        let worker = env::temp_dir().join(format!("piik-uninstall-{}.exe", std::process::id()));
        fs::copy(env::current_exe()?, &worker).context("Stage the uninstall worker")?;
        Command::new(&worker)
            .arg("--uninstall-worker")
            .arg(&directory)
            .arg(std::process::id().to_string())
            .current_dir(env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("Start the uninstall worker")?;
    }
    Ok(())
}

fn wait_for_parent(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    for _ in 0..120 {
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        if handle == 0 {
            return;
        }
        let result = unsafe { WaitForSingleObject(handle, 250) };
        unsafe { CloseHandle(handle) };
        if result == 0 {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}
