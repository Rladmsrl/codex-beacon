use crate::config::{self, AppPaths};
use crate::{installer, service};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

const ID_PAIR: &str = "pair";
const ID_DEVICES: &str = "devices";
const ID_FORGET_ALL: &str = "forget_all";
const ID_START: &str = "start";
const ID_RESTART: &str = "restart";
const ID_STOP: &str = "stop";
const ID_REINSTALL: &str = "reinstall";
const ID_CLEAN: &str = "clean";
const ID_LOGS: &str = "logs";
const ID_QUIT: &str = "quit";
const ID_STOP_QUIT: &str = "stop_quit";
const ID_UNINSTALL: &str = "uninstall";

pub fn run() -> Result<()> {
    let paths = AppPaths::discover()?;
    paths.ensure()?;
    let binary = std::env::current_exe()?;
    let mut builder = EventLoop::builder();
    builder.with_activation_policy(ActivationPolicy::Accessory);
    let event_loop = builder.build()?;
    let mut app = MenuBarApp::new(paths, binary);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct MenuBarApp {
    paths: AppPaths,
    binary: PathBuf,
    tray: Option<TrayIcon>,
    service_status: Option<MenuItem>,
    device_status: Option<MenuItem>,
    start: Option<MenuItem>,
    stop: Option<MenuItem>,
    last_refresh: Instant,
    setup_started: bool,
}

impl MenuBarApp {
    fn new(paths: AppPaths, binary: PathBuf) -> Self {
        Self {
            paths,
            binary,
            tray: None,
            service_status: None,
            device_status: None,
            start: None,
            stop: None,
            last_refresh: Instant::now() - Duration::from_secs(10),
            setup_started: false,
        }
    }

    fn create_tray(&mut self) -> Result<()> {
        let menu = Menu::new();
        let service_status = MenuItem::with_id("service_status", "● 服务状态检查中", false, None);
        let device_status = MenuItem::with_id("device_status", "已配对设备：0", false, None);
        let pair = MenuItem::with_id(ID_PAIR, "配对新显示器…", true, None);
        let devices = MenuItem::with_id(ID_DEVICES, "查看已配对设备", true, None);
        let forget_all = MenuItem::with_id(ID_FORGET_ALL, "忘记全部显示器…", true, None);
        let start = MenuItem::with_id(ID_START, "启动服务", true, None);
        let restart = MenuItem::with_id(ID_RESTART, "重启服务", true, None);
        let stop = MenuItem::with_id(ID_STOP, "停止服务", true, None);
        let reinstall = MenuItem::with_id(ID_REINSTALL, "重新安装服务与 Codex Hooks", true, None);
        let clean = MenuItem::with_id(ID_CLEAN, "清理运行期文件并重启", true, None);
        let logs = MenuItem::with_id(ID_LOGS, "打开日志与配置目录", true, None);
        let quit = MenuItem::with_id(ID_QUIT, "退出菜单栏（服务继续）", true, None);
        let stop_quit = MenuItem::with_id(ID_STOP_QUIT, "停止服务并退出菜单栏", true, None);
        let uninstall = MenuItem::with_id(ID_UNINSTALL, "卸载服务与 Codex Hooks…", true, None);
        let separators = [
            PredefinedMenuItem::separator(),
            PredefinedMenuItem::separator(),
            PredefinedMenuItem::separator(),
            PredefinedMenuItem::separator(),
        ];

        menu.append_items(&[
            &service_status,
            &device_status,
            &separators[0],
            &pair,
            &devices,
            &forget_all,
            &separators[1],
            &start,
            &restart,
            &stop,
            &reinstall,
            &separators[2],
            &clean,
            &logs,
            &separators[3],
            &quit,
            &stop_quit,
            &uninstall,
        ])?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Codex Beacon")
            .with_icon(menu_icon()?)
            .with_icon_as_template(true)
            .with_menu(Box::new(menu))
            .build()?;
        self.tray = Some(tray);
        self.service_status = Some(service_status);
        self.device_status = Some(device_status);
        self.start = Some(start);
        self.stop = Some(stop);
        self.refresh();
        Ok(())
    }

    fn begin_setup(&mut self) {
        if self.setup_started {
            return;
        }
        self.setup_started = true;
        if std::env::var_os("CODEX_BLE_SKIP_INSTALL").is_some() {
            return;
        }
        let binary = self.binary.clone();
        let paths = self.paths.clone();
        thread::spawn(move || match installer::install(&binary, &paths, true) {
            Ok(_) => installer::notify("菜单栏和后台桥接服务已经启动"),
            Err(error) => installer::notify(&format!("安装后台服务失败：{error}")),
        });
    }

    fn refresh(&mut self) {
        let running = service::is_running(&self.paths);
        let devices = config::load(&self.paths)
            .map(|settings| settings.devices.len())
            .unwrap_or(0);
        if let Some(item) = &self.service_status {
            item.set_text(if running {
                "● 服务运行中"
            } else {
                "○ 服务已停止"
            });
        }
        if let Some(item) = &self.device_status {
            item.set_text(format!("已配对设备：{devices}"));
        }
        if let Some(item) = &self.start {
            item.set_enabled(!running);
        }
        if let Some(item) = &self.stop {
            item.set_enabled(running);
        }
        if let Some(tray) = &self.tray {
            let _ = tray.set_tooltip(Some(if running {
                "Codex Beacon — 服务运行中"
            } else {
                "Codex Beacon — 服务已停止"
            }));
        }
        self.last_refresh = Instant::now();
    }

    fn handle_menu(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        match id {
            ID_PAIR => self.spawn_pair(),
            ID_DEVICES => self.show_devices(),
            ID_FORGET_ALL => self.forget_all(),
            ID_START => spawn_action(|| installer::start_service(), "服务已启动"),
            ID_RESTART => spawn_action(|| installer::restart_service(), "服务已重启"),
            ID_STOP => spawn_action(|| installer::stop_service(), "服务已停止"),
            ID_REINSTALL => {
                let binary = self.binary.clone();
                let paths = self.paths.clone();
                spawn_action(
                    move || installer::install(&binary, &paths, true).map(|_| ()),
                    "服务与 Codex Hooks 已重新安装",
                );
            }
            ID_CLEAN => self.clean_and_restart(),
            ID_LOGS => {
                let _ = Command::new("open").arg(&self.paths.support).spawn();
            }
            ID_QUIT => event_loop.exit(),
            ID_STOP_QUIT => {
                let _ = installer::stop_service();
                event_loop.exit();
            }
            ID_UNINSTALL => {
                if confirm(
                    "卸载 Codex Beacon？",
                    "将停止后台服务并移除登录项和 Codex Hooks，设备记录与日志会保留。",
                ) {
                    let binary = self.binary.clone();
                    let _ = Command::new(binary).arg("uninstall").spawn();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn spawn_pair(&self) {
        let binary = self.binary.clone();
        installer::notify("请让 StickS3 进入配对模式，正在扫描 12 秒…");
        thread::spawn(move || {
            let result = Command::new(binary)
                .arg("pair")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output();
            match result {
                Ok(output) if output.status.success() => installer::notify("显示器配对完成"),
                Ok(output) => installer::notify(&format!(
                    "配对失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => installer::notify(&format!("无法启动配对：{error}")),
            }
        });
    }

    fn show_devices(&self) {
        let message = match config::load(&self.paths) {
            Ok(settings) if settings.devices.is_empty() => "尚未配对显示器".to_owned(),
            Ok(settings) => settings
                .devices
                .iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>()
                .join("、"),
            Err(error) => format!("读取设备失败：{error}"),
        };
        installer::notify(&message);
    }

    fn forget_all(&self) {
        if !confirm(
            "忘记全部显示器？",
            "只删除 Mac 端设备记录，不删除 StickS3 或 macOS 系统保存的 BLE bond。",
        ) {
            return;
        }
        let binary = self.binary.clone();
        thread::spawn(move || {
            let status = Command::new(binary)
                .args(["forget", "--all"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => installer::notify("已忘记全部显示器"),
                _ => installer::notify("删除设备记录失败"),
            }
        });
    }

    fn clean_and_restart(&self) {
        let paths = self.paths.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                installer::stop_service()?;
                thread::sleep(Duration::from_millis(250));
                service::clean(&paths, false)?;
                installer::start_service()?;
                Ok(())
            })();
            match result {
                Ok(()) => installer::notify("运行期文件已清理，服务已重启"),
                Err(error) => installer::notify(&format!("清理失败：{error}")),
            }
        });
    }
}

impl ApplicationHandler for MenuBarApp {
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            if let Err(error) = self.create_tray() {
                eprintln!("create menu bar: {error:#}");
            }
            self.begin_setup();
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.refresh();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_secs(1),
        ));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            self.handle_menu(event.id().as_ref(), event_loop);
        }
        if self.last_refresh.elapsed() >= Duration::from_secs(1) {
            self.refresh();
        }
    }
}

fn spawn_action<F>(action: F, success: &'static str)
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    thread::spawn(move || match action() {
        Ok(()) => installer::notify(success),
        Err(error) => installer::notify(&format!("操作失败：{error}")),
    });
}

fn menu_icon() -> Result<Icon> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 22;
    let mut rgba = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for y in 2..20 {
        for x in 2..20 {
            let border = x == 2 || x == 19 || y == 2 || y == 19;
            let c = ((x == 6 || y == 6 || y == 15) && x >= 6 && x <= 11)
                || (x == 6 && y >= 6 && y <= 15);
            let diagonal = x >= 13 && x <= 17 && (x + y == 22 || x == y - 1);
            if border || c || diagonal {
                let offset = ((y * WIDTH + x) * 4) as usize;
                rgba[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, WIDTH, HEIGHT).context("create menu bar icon")
}

fn confirm(title: &str, message: &str) -> bool {
    let script = format!(
        "display alert {:?} message {:?} as warning buttons {{\"取消\", \"继续\"}} default button \"继续\" cancel button \"取消\"",
        title, message
    );
    Command::new("osascript")
        .args(["-e", &script])
        .status()
        .is_ok_and(|status| status.success())
}
