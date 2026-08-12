use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codex_ble_bridge::ble;
use codex_ble_bridge::config::{self, AppPaths};
use codex_ble_bridge::{hooks, installer, service};

#[derive(Parser, Debug)]
#[command(version, about = "Codex tasks on StickS3 over Bluetooth LE")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 在前台运行桥接服务
    Run,
    /// 扫描并保存所有处于配对模式的设备
    Pair {
        #[arg(long, default_value_t = 12)]
        seconds: u64,
    },
    /// 列出已保存的设备
    Devices,
    /// 从桥接服务中忘记一个或全部设备
    Forget {
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// 安装 Codex Hooks 和 macOS 登录服务
    Install {
        #[arg(long)]
        no_start: bool,
    },
    /// 移除登录服务和本项目添加的 Hooks
    Uninstall {
        /// 同时删除设备记录和日志
        #[arg(long)]
        all_data: bool,
    },
    /// 清理停止服务后残留的运行期文件
    Clean {
        /// 同时删除设备记录和日志
        #[arg(long)]
        all_data: bool,
    },
    /// 检查 Codex、配置和安装状态
    Doctor,
    /// Codex Hook 的快速事件转发入口
    #[command(hide = true)]
    Hook,
    /// 显示 macOS 菜单栏控制器
    #[command(hide = true)]
    Menu,
}

fn main() {
    let cli = Cli::parse();
    if cli.command.is_none() || matches!(cli.command, Some(Commands::Menu)) {
        #[cfg(target_os = "macos")]
        if let Err(error) = codex_ble_bridge::menu_bar::run() {
            eprintln!("错误: {error:#}");
            std::process::exit(1);
        }
        #[cfg(not(target_os = "macos"))]
        eprintln!("菜单栏只支持 macOS");
        return;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create Tokio runtime");
    if let Err(error) = runtime.block_on(execute(cli.command.unwrap())) {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}

async fn execute(command: Commands) -> Result<()> {
    let paths = AppPaths::discover()?;
    match command {
        Commands::Run => service::run(paths).await,
        Commands::Pair { seconds } => {
            paths.ensure()?;
            let devices = ble::pair_visible(&paths, seconds).await?;
            for device in &devices {
                println!("已配对: {}  {}", device.name, device.id);
            }
            Ok(())
        }
        Commands::Devices => {
            let settings = config::load(&paths)?;
            if settings.devices.is_empty() {
                println!("尚未保存设备");
            } else {
                for device in settings.devices {
                    println!("{}\t{}", device.id, device.name);
                }
            }
            Ok(())
        }
        Commands::Forget { id, all } => {
            let mut settings = config::load(&paths)?;
            if all {
                settings.devices.clear();
            } else if let Some(id) = id {
                settings.devices.retain(|device| device.id != id);
            } else {
                anyhow::bail!("请提供设备 ID，或使用 --all")
            }
            config::save(&paths, &settings)?;
            println!("桥接服务的设备记录已更新");
            Ok(())
        }
        Commands::Install { no_start } => {
            let binary = std::env::current_exe()?;
            let agent = installer::install(&binary, &paths, !no_start)?;
            println!("已安装: {}", agent.display());
            println!("请在 Codex 中打开 /hooks，确认信任 Codex Beacon Hook。");
            Ok(())
        }
        Commands::Uninstall { all_data } => {
            installer::uninstall()?;
            let removed = service::clean(&paths, all_data)?;
            println!(
                "已移除登录服务、Codex Hooks 和 {} 个本地项目。",
                removed.len()
            );
            if !all_data {
                println!("设备记录和日志仍保留；如需删除请使用 uninstall --all-data。");
            }
            Ok(())
        }
        Commands::Clean { all_data } => {
            let removed = service::clean(&paths, all_data)?;
            if removed.is_empty() {
                println!("没有需要清理的文件");
            } else {
                for path in removed {
                    println!("已删除: {}", path.display());
                }
            }
            Ok(())
        }
        Commands::Doctor => doctor(&paths),
        Commands::Hook => hooks::forward_stdin(&paths),
        Commands::Menu => unreachable!(),
    }
}

fn doctor(paths: &AppPaths) -> Result<()> {
    println!("bridge: {}", std::env::current_exe()?.display());
    match config::find_codex_binary() {
        Ok(path) => println!("codex:  {}", path.display()),
        Err(error) => println!("codex:  未找到 ({error})"),
    }
    println!("config: {}", paths.settings.display());
    println!("socket: {}", paths.socket.display());
    let settings = config::load(paths).context("load settings")?;
    println!("devices: {}", settings.devices.len());
    Ok(())
}
