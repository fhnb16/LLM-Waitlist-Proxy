//! `lmq-server` — сервер очереди без окна: консоль, служба Windows, systemd.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use lmq_core::config::{self, AppPaths, CliOverrides, ClientCfg, PathOverrides, Secrets};
use lmq_core::crypto::Cipher;
use lmq_core::{Core, StartOptions};

#[derive(Parser)]
#[command(name = "lmq-server", version, about = "LMQ — сервер очереди для UniversalBot (протокол queue-server.md)")]
struct Cli {
    #[command(flatten)]
    paths: PathArgs,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Args, Clone, Default)]
struct PathArgs {
    /// Общая папка для настроек и данных (как LMQ_HOME).
    #[arg(long, global = true, env = "LMQ_HOME")]
    home: Option<PathBuf>,
    /// Папка настроек (settings.yaml, secrets.bin, профили).
    #[arg(long, global = true, env = "LMQ_CONFIG_DIR")]
    config_dir: Option<PathBuf>,
    /// Папка данных (БД, журналы).
    #[arg(long, global = true, env = "LMQ_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Профиль настроек.
    #[arg(long, global = true)]
    profile: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Запустить сервер (по умолчанию).
    Run(RunArgs),
    /// Проверить конфигурацию и выйти.
    CheckConfig,
    /// Показать, где лежат настройки и данные.
    Paths,
    /// Клиенты API (ключи для бота).
    #[command(subcommand)]
    Client(ClientCmd),
    /// Токен администратора веб-панели.
    #[command(subcommand)]
    AdminToken(AdminCmd),
    /// Сгенерировать значение для SECRET_KEY.
    GenKey,
    /// Служба Windows.
    #[command(subcommand)]
    Service(ServiceCmd),
}

#[derive(Args, Clone, Default)]
struct RunArgs {
    /// Адрес клиентского API (перекрывает LISTEN).
    #[arg(long)]
    listen: Option<String>,
    /// Адрес панели (перекрывает ADMIN_LISTEN; пустая строка — без панели).
    #[arg(long)]
    admin_listen: Option<String>,
    #[arg(long)]
    log_level: Option<String>,
    /// Не запускать движки с autostart.
    #[arg(long)]
    no_engines: bool,
}

#[derive(Subcommand)]
enum ClientCmd {
    /// Создать клиента и показать его ключ (один раз).
    Add {
        name: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Список клиентов.
    List,
    /// Удалить клиента.
    Remove { name: String },
    /// Выдать новый ключ.
    Rotate { name: String },
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Новый токен администратора (старый перестанет действовать).
    Reset,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Установить службу (нужны права администратора).
    Install {
        /// Папка настроек и данных службы.
        #[arg(long, default_value = default_service_home())]
        service_home: PathBuf,
    },
    /// Удалить службу.
    Uninstall,
    /// Точка входа службы (вызывает диспетчер служб).
    Run,
}

fn default_service_home() -> &'static str {
    if cfg!(windows) { "C:\\ProgramData\\LMQ" } else { "/var/lib/lmq" }
}

fn overrides(p: &PathArgs) -> PathOverrides {
    PathOverrides { home: p.home.clone(), config_dir: p.config_dir.clone(), data_dir: p.data_dir.clone() }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => run_blocking(&cli.paths, RunArgs::default()),
        Some(Cmd::Run(a)) => run_blocking(&cli.paths, a),
        Some(Cmd::CheckConfig) => check_config(&cli.paths),
        Some(Cmd::Paths) => show_paths(&cli.paths),
        Some(Cmd::Client(c)) => client_cmd(&cli.paths, c),
        Some(Cmd::AdminToken(AdminCmd::Reset)) => admin_reset(&cli.paths),
        Some(Cmd::GenKey) => {
            println!("{}", lmq_core::util::random_token(32));
            Ok(())
        }
        Some(Cmd::Service(s)) => service::handle(s, &cli.paths),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("lmq").build()?)
}

fn start_options(p: &PathArgs, a: &RunArgs) -> StartOptions {
    StartOptions {
        paths: overrides(p),
        profile: p.profile.clone(),
        cli: CliOverrides { listen: a.listen.clone(), admin_listen: a.admin_listen.clone(), log_level: a.log_level.clone(), ..Default::default() },
        init_logging: true,
        console_log: true,
        listeners: true,
        autostart_engines: !a.no_engines,
        ..Default::default()
    }
}

fn run_blocking(p: &PathArgs, a: RunArgs) -> Result<()> {
    runtime()?.block_on(async move {
        let core = Core::start(start_options(p, &a)).await?;
        announce(&core).await;
        shutdown_signal().await;
        core.shutdown().await;
        Ok(())
    })
}

async fn announce(core: &Core) {
    let b = core.wait_bound(std::time::Duration::from_secs(3)).await;
    let eff = core.app.effective();
    eprintln!("LMQ {} — настройки: {}", lmq_core::VERSION, eff.settings_path.display());
    match (b.client, &b.client_error) {
        (Some(a), _) => eprintln!("  API:    http://{a}/v1/chat/completions"),
        (None, Some(e)) => eprintln!("  API:    ОШИБКА — {e}"),
        _ => {}
    }
    match (b.admin, &b.admin_error) {
        (Some(a), _) => eprintln!("  Панель: http://{a}/ui/"),
        (None, Some(e)) => eprintln!("  Панель: ОШИБКА — {e}"),
        _ => eprintln!("  Панель: выключена (ADMIN_LISTEN пуст)"),
    }
    if let Some(t) = &core.first_admin_token {
        eprintln!("\n  Токен администратора (показывается один раз): {t}");
        eprintln!("  Новый токен: lmq-server admin-token reset\n");
    }
    if core.app.settings().clients.is_empty() {
        eprintln!("  Клиентов API нет. Создать ключ для бота: lmq-server client add bot");
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct Offline {
    paths: AppPaths,
    cipher: Cipher,
    profile: String,
}

fn offline(p: &PathArgs) -> Result<Offline> {
    let paths = AppPaths::resolve(&overrides(p))?;
    paths.ensure()?;
    let secrets = Secrets::load_or_create(&paths.secrets_file)?;
    let profile = p
        .profile
        .clone()
        .or_else(|| std::fs::read_to_string(paths.active_profile_file()).ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    Ok(Offline { cipher: Cipher::new(&secrets.master_key), paths, profile })
}

fn check_config(p: &PathArgs) -> Result<()> {
    let o = offline(p)?;
    let eff = config::build_effective(&o.paths, &o.profile, &o.cipher, &CliOverrides::default(), &config::system_env)?;
    println!("OK: {}", eff.settings_path.display());
    println!("  моделей: {}, клиентов: {}, движков: {}", eff.settings.models.len(), eff.settings.clients.len(), eff.settings.engines.len());
    if !eff.locked.is_empty() {
        println!("  из окружения: {}", eff.locked.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

fn show_paths(p: &PathArgs) -> Result<()> {
    let o = offline(p)?;
    println!("настройки: {}", o.paths.settings_file(&o.profile).display());
    println!("секреты:   {}", o.paths.secrets_file.display());
    println!("данные:    {}", o.paths.data_dir.display());
    println!("журналы:   {}", o.paths.log_dir.display());
    println!("профиль:   {}{}", o.profile, if o.paths.portable { " (портативный режим)" } else { "" });
    Ok(())
}

fn client_cmd(p: &PathArgs, c: ClientCmd) -> Result<()> {
    let o = offline(p)?;
    let path = o.paths.settings_file(&o.profile);
    let mut s = config::load_file(&path, &o.cipher)?;
    match c {
        ClientCmd::List => {
            if s.clients.is_empty() {
                println!("клиентов нет");
            }
            for c in &s.clients {
                println!("{}\t{}\t{}", c.name, if c.enabled { "включён" } else { "выключен" }, c.note);
            }
            return Ok(());
        }
        ClientCmd::Add { name, note } => {
            if s.clients.iter().any(|c| c.name == name) {
                bail!("клиент '{name}' уже есть");
            }
            let key = format!("lmq_{}", lmq_core::util::random_token(24));
            s.clients.push(ClientCfg { name: name.clone(), key_hash: config::hash_key(&key), enabled: true, created_at: lmq_core::util::now_ms(), note });
            config::save_file(&path, &s, &o.cipher)?;
            println!("Клиент '{name}' создан. Ключ (показывается один раз):\n{key}");
            println!("Если сервер запущен, перезапустите его или создайте клиента в панели.");
        }
        ClientCmd::Remove { name } => {
            let before = s.clients.len();
            s.clients.retain(|c| c.name != name);
            if s.clients.len() == before {
                bail!("клиента '{name}' нет");
            }
            config::save_file(&path, &s, &o.cipher)?;
            println!("Клиент '{name}' удалён.");
        }
        ClientCmd::Rotate { name } => {
            let key = format!("lmq_{}", lmq_core::util::random_token(24));
            let c = s.clients.iter_mut().find(|c| c.name == name).with_context(|| format!("клиента '{name}' нет"))?;
            c.key_hash = config::hash_key(&key);
            config::save_file(&path, &s, &o.cipher)?;
            println!("Новый ключ клиента '{name}':\n{key}");
        }
    }
    Ok(())
}

fn admin_reset(p: &PathArgs) -> Result<()> {
    let o = offline(p)?;
    let token = lmq_core::util::random_token(24);
    Secrets::save_admin_hash(&o.paths.secrets_file, Some(lmq_core::util::hash_admin(&token)))?;
    println!("Новый токен администратора:\n{token}");
    println!("Работающий сервер примет его после перезапуска.");
    Ok(())
}

#[cfg(windows)]
mod service {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::{PathArgs, RunArgs, ServiceCmd};

    const NAME: &str = "LMQ";

    pub fn handle(cmd: ServiceCmd, paths: &PathArgs) -> Result<()> {
        match cmd {
            ServiceCmd::Install { service_home } => install(service_home),
            ServiceCmd::Uninstall => uninstall(),
            ServiceCmd::Run => {
                if let Some(h) = &paths.home {
                    // SAFETY: до запуска потоков.
                    unsafe { std::env::set_var("LMQ_HOME", h) };
                }
                service_dispatcher::start(NAME, ffi_service_main).context("service dispatcher")
            }
        }
    }

    fn install(home: PathBuf) -> Result<()> {
        std::fs::create_dir_all(&home)?;
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
            .context("нужны права администратора")?;
        let exe = std::env::current_exe()?;
        let info = ServiceInfo {
            name: OsString::from(NAME),
            display_name: OsString::from("LMQ queue server"),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: vec![OsString::from("--home"), home.clone().into_os_string(), OsString::from("service"), OsString::from("run")],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        let svc = manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?;
        svc.set_description("Очередь заданий для локальных моделей (UniversalBot)")?;
        println!("Служба {NAME} установлена. Данные: {}", home.display());
        println!("Запуск: sc start {NAME}. Токен панели: lmq-server --home \"{}\" admin-token reset", home.display());
        Ok(())
    }

    fn uninstall() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).context("нужны права администратора")?;
        let svc = manager.open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)?;
        if svc.query_status()?.current_state != ServiceState::Stopped {
            let _ = svc.stop();
            std::thread::sleep(Duration::from_secs(2));
        }
        svc.delete()?;
        println!("Служба {NAME} удалена.");
        Ok(())
    }

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        let _ = run_service();
    }

    fn run_service() -> Result<()> {
        let rt = super::runtime()?;
        let stop = tokio_util_token();
        let stop2 = stop.clone();
        let handler = move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop2.notify_one();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status = service_control_handler::register(NAME, handler)?;
        let set = |state, accept| {
            let _ = status.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            });
        };
        set(ServiceState::StartPending, ServiceControlAccept::empty());
        let paths = PathArgs { home: std::env::var_os("LMQ_HOME").map(PathBuf::from), ..Default::default() };
        let res = rt.block_on(async {
            let mut opts = super::start_options(&paths, &RunArgs::default());
            opts.console_log = false;
            let core = lmq_core::Core::start(opts).await?;
            set(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN);
            stop.notified().await;
            set(ServiceState::StopPending, ServiceControlAccept::empty());
            core.shutdown().await;
            Ok::<_, anyhow::Error>(())
        });
        set(ServiceState::Stopped, ServiceControlAccept::empty());
        res
    }

    fn tokio_util_token() -> std::sync::Arc<tokio::sync::Notify> {
        std::sync::Arc::new(tokio::sync::Notify::new())
    }
}

#[cfg(not(windows))]
mod service {
    use anyhow::{Result, bail};

    use super::{PathArgs, ServiceCmd};

    pub fn handle(_cmd: ServiceCmd, _paths: &PathArgs) -> Result<()> {
        bail!("службы Windows недоступны на этой ОС; используйте systemd (packaging/debian/lmq.service)")
    }
}
