use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;
mod config;
mod docker_client;
mod docker_context;
mod keychain;
mod output;
mod profile;
mod shell;
mod size;
mod theme;

#[derive(Parser)]
#[command(
    name = "spk",
    version,
    about = "Ultra-fast container runtime for Apple Silicon"
)]
struct Cli {
    /// Profile to act on [default: $SPECK_PROFILE, then `spk profile use`, then `default`]
    #[arg(short = 'p', long, global = true, value_name = "NAME")]
    profile: Option<String>,
    /// Output format of status, list, profile ls/current, config, version and doctor
    #[arg(short = 'o', long, global = true, value_enum, default_value_t = output::Output::Plain)]
    output: output::Output,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
enum Commands {
    /// Start the Speck VM and Docker API server
    #[command(visible_alias = "start")]
    Up(UpArgs),
    /// Stop the Speck VM
    Down,
    /// Restart the Speck VM (graceful stop then fresh start)
    Restart,
    /// Pull an image and run a container
    Run(RunArgs),
    /// List running containers
    Ps,
    /// Execute a command in a running container
    Exec(ExecArgs),
    /// Stop Speck (like `colima stop`), or a container when one is named
    Stop(StopArgs),
    /// Remove a container
    Rm(RmArgs),
    /// Build an image from a Dockerfile
    Build(BuildArgs),
    /// Open the interactive dashboard TUI
    Dashboard,
    /// Generate shell completions
    Completion(CompletionArgs),
    /// Print shell environment exports for this session
    Env(EnvArgs),
    /// Initialize Speck shell integration (one-time setup)
    Init(InitArgs),
    /// Run health checks and diagnose Speck configuration
    Doctor(DoctorArgs),
    /// Validate config.yaml and show the effective settings and their sources
    Config,
    /// Edit config.yaml in $VISUAL/$EDITOR; creates a commented template first
    Template,
    /// Show Speck status (like `colima status`)
    Status(StatusArgs),
    /// List Speck instances (like `colima list`)
    List(ListArgs),
    /// Show Speck, runtime and guest versions
    Version,
    /// Delete cached downloads Speck no longer uses (like `colima prune`)
    Prune(PruneArgs),
    /// Show VM boot logs
    Logs(LogsArgs),
    /// Delete the Speck VM with all containers, images and volumes
    Delete(DeleteArgs),
    /// Delete the Speck VM and start a fresh one (keeps downloads and config)
    Recreate(DeleteArgs),
    /// Manage profiles: independent Speck instances (like `colima -p`)
    Profile(ProfileArgs),
}

impl Commands {
    /// Whether the command prints a document for `-o json|yaml`.
    fn supports_output(&self) -> bool {
        match self {
            Commands::Status(_)
            | Commands::List(_)
            | Commands::Config
            | Commands::Version
            | Commands::Doctor(_) => true,
            Commands::Profile(args) => {
                matches!(
                    args.command,
                    ProfileCommand::Ls(_) | ProfileCommand::Current
                )
            }
            _ => false,
        }
    }
}

#[derive(Parser)]
struct ProfileArgs {
    #[command(subcommand)]
    command: ProfileCommand,
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List profiles; `*` marks the current one
    #[command(visible_alias = "list")]
    Ls(ListArgs),
    /// Make a profile the one bare `spk` commands act on
    Use { name: String },
    /// Print the current profile name
    Current,
    /// Create a profile without starting it
    Create {
        name: String,
        #[arg(long)]
        cpus: Option<u64>,
        /// VM memory: 6g, 6144m, 1.5g (a plain number is MiB)
        #[arg(long, value_name = "SIZE", value_parser = crate::size::parse_mib)]
        memory: Option<u64>,
        /// VM data disk: 100g, 1t (a plain number is GiB)
        #[arg(long, value_name = "SIZE", value_parser = crate::size::parse_gib)]
        disk: Option<u64>,
        /// Run without a launchd agent; the profile does not come back after a reboot
        #[arg(long)]
        no_autostart: bool,
    },
    /// Delete a profile with its VM, config and logs
    #[command(visible_alias = "delete")]
    Rm {
        name: String,
        /// Skip the confirmation prompt
        #[arg(long, short)]
        force: bool,
    },
}

#[derive(Parser)]
struct StatusArgs {
    /// Include resources, container counts and versions
    #[arg(long, short)]
    extended: bool,
    /// Print JSON output
    #[arg(long, short)]
    json: bool,
}

#[derive(Parser)]
struct ListArgs {
    /// Print JSON output
    #[arg(long, short)]
    json: bool,
}

#[derive(Parser)]
struct PruneArgs {
    /// Skip the confirmation prompt
    #[arg(long, short)]
    force: bool,
}

#[derive(Parser)]
struct DeleteArgs {
    /// Skip the confirmation prompt
    #[arg(long, short)]
    force: bool,
}

#[derive(Parser)]
struct UpArgs {
    #[arg(long)]
    kernel: Option<PathBuf>,
    #[arg(long)]
    initrd: Option<PathBuf>,
    #[arg(long)]
    rootfs: Option<PathBuf>,
    #[arg(long)]
    data_disk: Option<PathBuf>,
    #[arg(long)]
    cpus: Option<u64>,
    /// VM memory: 6g, 6144m, 1.5g (a plain number is MiB)
    #[arg(long, value_name = "SIZE", value_parser = crate::size::parse_mib)]
    memory: Option<u64>,
    /// VM data disk: 100g, 1t (a plain number is GiB)
    #[arg(long, value_name = "SIZE", value_parser = crate::size::parse_gib)]
    disk: Option<u64>,
    #[arg(long)]
    foreground: bool,
    /// Force re-download of VM assets even if present on disk
    #[arg(long)]
    pull: bool,
    /// Block until the Docker socket responds to GET /_ping with HTTP 200
    #[arg(long)]
    wait: bool,
    /// Timeout in seconds for --wait (default: 300)
    #[arg(long, default_value = "300")]
    timeout: u64,
    /// Run without a launchd agent: the instance does not come back after a
    /// reboot (for CI). Saved to config.yaml as `autostart: false`
    #[arg(long)]
    no_autostart: bool,
    /// Edit config.yaml in $VISUAL/$EDITOR before starting (like `colima start --edit`)
    #[arg(long)]
    edit: bool,
}

#[derive(Parser)]
struct RunArgs {
    image: String,
    #[arg(trailing_var_arg = true)]
    cmd: Vec<String>,
    /// Publish a container port (`8080:80`)
    #[arg(long = "publish")]
    port: Vec<String>,
    #[arg(short = 'v')]
    volume: Vec<String>,
    #[arg(short = 'e')]
    env: Vec<String>,
    #[arg(short = 'd')]
    detach: bool,
}

#[derive(Parser)]
struct ExecArgs {
    container: String,
    #[arg(trailing_var_arg = true)]
    cmd: Vec<String>,
    #[arg(long)]
    tty: bool,
}

#[derive(Parser)]
struct StopArgs {
    /// Container to stop; without it, Speck itself is stopped
    container: Option<String>,
    /// Kill Speck and its VM at once, without a graceful guest shutdown
    #[arg(short, long, conflicts_with = "container")]
    force: bool,
}

#[derive(Parser)]
struct RmArgs {
    container: String,
}

#[derive(Parser)]
struct BuildArgs {
    #[arg(default_value = ".")]
    context: PathBuf,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    dockerfile: Option<String>,
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    no_cache: bool,
}

#[derive(Parser)]
struct CompletionArgs {
    shell: clap_complete::Shell,
}

#[derive(Parser)]
struct EnvArgs {
    /// Target shell syntax for env exports: `posix` (bash/zsh) or `fish`.
    #[arg(long, default_value = "posix", value_parser = ["posix", "fish"])]
    shell: String,
}

#[derive(Parser, Clone)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub command: Option<DoctorSubcommand>,
}

#[derive(Subcommand, Clone)]
pub enum DoctorSubcommand {
    /// Trace DNS resolution for a hostname through the full guest DNS path
    Dns { hostname: String },
}

#[derive(Parser)]
pub struct LogsArgs {
    /// Show the last 20 lines of the daemon log
    #[arg(long)]
    pub tail: bool,
    /// Follow new log entries as they are written
    #[arg(long)]
    pub follow: bool,
}

#[derive(Parser, Clone)]
pub struct InitArgs {
    /// Persist the Speck environment block in your shell startup file (opt-in)
    #[arg(long = "set-docker-host")]
    pub set_docker_host: bool,
    /// Override shell detection (bash, zsh, fish)
    #[arg(long, value_parser = ["bash", "zsh", "fish"])]
    pub shell: Option<String>,
}

fn init_tracing(log_level: &str) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(log_level)?)
        .try_init()
        .map_err(|err| anyhow::anyhow!(err))
}

fn default_tracing_filter() -> String {
    std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    const MAIN_SOURCE: &str = include_str!("main.rs");
    const CLI_MANIFEST: &str = include_str!("../Cargo.toml");

    fn production_source() -> &'static str {
        let end = MAIN_SOURCE
            .find("#[cfg(test)]")
            .unwrap_or(MAIN_SOURCE.len());
        &MAIN_SOURCE[..end]
    }

    #[test]
    fn cli_definition_is_consistent() {
        // Catches clashing flags, e.g. a subcommand short option that
        // collides with the global -p/-o.
        use clap::CommandFactory as _;
        super::Cli::command().debug_assert();
    }

    #[test]
    fn up_args_do_not_use_clap_env_annotations() {
        assert!(
            MAIN_SOURCE.contains("mod config;"),
            "main.rs must expose the speck-cli config module"
        );
        assert!(
            MAIN_SOURCE.contains("cpus: Option<u64>"),
            "UpArgs must include --cpus as Option<u64>"
        );
        assert!(
            MAIN_SOURCE.contains("memory: Option<u64>"),
            "UpArgs must include --memory as Option<u64>"
        );
        assert!(
            MAIN_SOURCE.contains("disk: Option<u64>"),
            "UpArgs must include --disk as Option<u64>"
        );
        assert!(
            !MAIN_SOURCE.contains("env = \"SPECK_VM_"),
            "SPECK_VM_* env precedence must be resolved manually, not by clap env annotations"
        );
        assert!(
            CLI_MANIFEST.contains("serde_yaml"),
            "serde_yaml must be a speck-cli dependency"
        );
        assert!(
            CLI_MANIFEST.contains("sysinfo"),
            "sysinfo must be a speck-cli dependency"
        );
    }

    #[test]
    fn main_initializes_tracing_from_effective_log_level_before_run_up() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        let load_config = main_fn
            .find("load_config_file(&speck_home)")
            .expect("Commands::Up must load $SPECK_HOME/config.yaml before dispatch");
        let resolve_config = main_fn
            .find("resolve_effective_config(file_config, &args)")
            .expect("Commands::Up must resolve EffectiveConfig before dispatch");
        let init_tracing = main_fn
            .find("init_tracing(&effective.log_level)")
            .expect("Commands::Up must initialize tracing from EffectiveConfig.log_level");
        let run_up = main_fn
            .find("commands::up::run_up(args, &speck_home, effective).await")
            .expect("Commands::Up must pass pre-resolved EffectiveConfig to run_up");

        assert!(load_config < resolve_config);
        assert!(resolve_config < init_tracing);
        assert!(init_tracing < run_up);
        assert!(
            main_fn.contains("warning: unknown config key:"),
            "unknown config keys must print the exact warning prefix"
        );
    }

    #[test]
    fn up_args_has_foreground_flag() {
        assert!(
            MAIN_SOURCE.contains("foreground: bool"),
            "UpArgs must expose --foreground as a bool flag"
        );
    }

    #[test]
    fn up_args_has_pull_flag() {
        assert!(
            MAIN_SOURCE.contains("Force re-download of VM assets even if present on disk"),
            "UpArgs --pull help text must explain that it refreshes VM assets"
        );
        assert!(
            MAIN_SOURCE.contains("pull: bool"),
            "UpArgs must expose --pull as a bool flag"
        );
    }

    #[test]
    fn env_command_registered() {
        let production = production_source();
        assert!(
            production.contains("Env("),
            "spk env subcommand must be registered as a variant in the Commands enum"
        );
    }

    #[test]
    fn env_command_dispatched() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        assert!(
            main_fn.contains("commands::env::run_env"),
            "the env subcommand dispatch arm must invoke the run_env entrypoint"
        );
    }

    #[test]
    fn env_args_exposes_shell_flag_with_supported_values() {
        let production = production_source();
        assert!(
            production.contains("shell: String"),
            "EnvArgs must carry a String field for the --shell selector"
        );
        assert!(
            production.contains("posix"),
            "EnvArgs --shell must advertise posix as a supported value"
        );
        assert!(
            production.contains("fish"),
            "EnvArgs --shell must advertise fish as a supported value"
        );
    }

    #[test]
    fn main_daemonizes_when_not_foreground() {
        assert!(
            MAIN_SOURCE
                .contains("!args.foreground && std::env::var(\"SPECK_DAEMONIZED\").is_err()"),
            "Commands::Up must guard daemonization on --foreground and SPECK_DAEMONIZED"
        );
        assert!(
            MAIN_SOURCE.contains("commands::up::daemonize(&speck_home, &binary)"),
            "Commands::Up must daemonize before continuing in default mode"
        );
    }

    #[test]
    fn main_uses_daemon_logging_when_speck_daemonized() {
        assert!(
            MAIN_SOURCE.contains("std::env::var(\"SPECK_DAEMONIZED\").is_ok()"),
            "Commands::Up must detect daemon context via SPECK_DAEMONIZED"
        );
        assert!(
            MAIN_SOURCE.contains("commands::logging::init_daemon_logging"),
            "daemon context must initialize the file appender logger"
        );
    }

    #[test]
    fn main_holds_worker_guard_for_lifetime() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        assert!(
            main_fn.contains("let _guard ="),
            "Commands::Up must keep the WorkerGuard binding in scope"
        );
        assert!(
            !main_fn.contains("let _ = guard"),
            "Commands::Up must not drop the WorkerGuard immediately"
        );
    }

    #[test]
    fn init_command_registered() {
        let production = production_source();
        assert!(
            production.contains("Init("),
            "spk init subcommand must be registered as a variant in the Commands enum"
        );
    }

    #[test]
    fn init_command_dispatched() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        assert!(
            main_fn.contains("commands::init::run_init"),
            "the init subcommand dispatch arm must invoke the run_init entrypoint"
        );
    }

    #[test]
    fn init_args_has_set_docker_host_flag() {
        let production = production_source();
        assert!(
            production.contains("set_docker_host"),
            "InitArgs must expose the --set-docker-host opt-in flag as set_docker_host"
        );
    }

    #[test]
    fn up_args_has_wait_flag() {
        assert!(
            MAIN_SOURCE.contains("wait: bool"),
            "UpArgs must expose --wait as a bool flag"
        );
    }

    #[test]
    fn up_args_has_timeout_flag_with_default() {
        assert!(
            MAIN_SOURCE.contains("timeout: u64"),
            "UpArgs must expose --timeout as a u64 field"
        );
        assert!(
            MAIN_SOURCE.contains("default_value = \"300\""),
            "UpArgs --timeout must default to 300 seconds"
        );
    }

    #[test]
    fn main_polls_socket_after_daemonize_when_wait_flag_set() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        let daemonize = main_fn
            .find("commands::up::daemonize(&speck_home, &binary)")
            .expect("Commands::Up must call daemonize");
        let wait_guard = main_fn
            .find("args.wait")
            .expect("Commands::Up must guard socket polling behind args.wait");
        let wait_for_socket = main_fn
            .find("commands::up::wait_for_socket")
            .expect("Commands::Up must call wait_for_socket when --wait is set");

        assert!(
            daemonize < wait_guard,
            "daemonize must happen before wait guard check"
        );
        assert!(
            wait_guard < wait_for_socket,
            "wait guard must wrap wait_for_socket call"
        );
    }

    #[test]
    fn main_exits_nonzero_on_wait_timeout() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        assert!(
            main_fn.contains("std::process::exit(1)"),
            "Commands::Up must exit with code 1 when wait_for_socket times out"
        );
    }

    #[test]
    fn restart_command_registered() {
        let production = production_source();
        assert!(
            production.contains("Restart"),
            "spk restart subcommand must be registered as a variant in the Commands enum"
        );
    }

    #[test]
    fn restart_command_dispatched() {
        let main_fn = &MAIN_SOURCE[MAIN_SOURCE
            .rfind("async fn main()")
            .expect("main.rs must define async main")..];
        assert!(
            main_fn.contains("commands::restart::run_restart"),
            "the restart subcommand dispatch arm must invoke the run_restart entrypoint"
        );
    }
}

/// Let `spk status -o json | head -1` end quietly, like other CLI tools.
/// Rust ignores SIGPIPE, so a closed pipe made `println!` panic. Only for
/// commands that print reports; the daemon must keep ignoring SIGPIPE.
fn restore_sigpipe() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: called once at startup, before any threads write to stdout.
    unsafe { signal(SIGPIPE, SIG_DFL) };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::ensure!(
        cli.output.is_plain() || cli.command.supports_output(),
        "-o json|yaml is supported only by status, list, profile ls, profile current, \
         config, version and doctor"
    );
    let format = cli.output;
    if cli.command.supports_output()
        || matches!(
            cli.command,
            Commands::Logs(_) | Commands::Completion(_) | Commands::Env(_)
        )
    {
        restore_sigpipe();
    }
    let profile = profile::select(cli.profile.as_deref())?;
    if profile.managed() {
        // Child `spk` processes (restart, recreate) act on the same profile.
        unsafe { std::env::set_var("SPECK_PROFILE", &profile.name) };
    }
    let speck_home = profile.home.clone();
    let _ = std::fs::create_dir_all(&speck_home);

    match cli.command {
        Commands::Up(args) => {
            let sock_path = speck_home.join("speck.sock");
            unsafe { std::env::set_var("SPECK_SOCK", sock_path.to_string_lossy().to_string()) };
            if !args.foreground && std::env::var("SPECK_DAEMONIZED").is_err() {
                // The launchd agent belongs to the default SPECK_HOME; starting
                // another instance through it would replace the default one.
                anyhow::ensure!(
                    profile.managed(),
                    "a custom SPECK_HOME runs in the foreground only: use `spk up --foreground`"
                );
                profile::check_socket_path(&sock_path)?;
                if args.edit && !commands::template::edit_config(&speck_home, profile)? {
                    std::process::exit(1);
                }
                if let Some(warning) = docker_context::docker_host_override_warning(&speck_home) {
                    eprintln!("warning: {warning}");
                }
                let mut saved = config::persist_vm_overrides(&speck_home, &args)?;
                let (file_config, _) = config::load_config_file(&speck_home)?;
                if args.no_autostart && file_config.autostart != Some(false) {
                    config::persist_autostart(&speck_home, false)?;
                    saved.push("autostart=false".into());
                }
                let autostart = file_config.autostart.unwrap_or(true) && !args.no_autostart;
                for saved in &saved {
                    eprintln!(
                        "saved {saved} to {}",
                        speck_home.join("config.yaml").display()
                    );
                }
                // Like `colima start`: starting a running instance does
                // nothing. Re-registering the agent would stop the daemon.
                let mut detached = None;
                if commands::up::launchd_job_loaded()
                    || commands::up::detached_daemon_running(&speck_home)
                {
                    println!("Speck{} is already running.", profile.display_suffix());
                    if !saved.is_empty() {
                        println!("Run `spk restart` to apply the new settings.");
                    }
                } else {
                    let binary = std::env::current_exe().context("cannot find own binary")?;
                    if autostart {
                        commands::up::daemonize(&speck_home, &binary)?;
                    } else {
                        detached = Some(commands::up::start_detached(&speck_home, &binary)?);
                    }
                }
                if args.wait {
                    let sock = speck_home.join("speck.sock");
                    let ready = match detached {
                        Some(child) => {
                            commands::up::wait_until_ready(&sock, args.timeout, Some(child)).await
                        }
                        None => commands::up::wait_for_socket(&sock, args.timeout).await,
                    };
                    if let Err(e) = ready {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            let (file_config, warnings) = config::load_config_file(&speck_home)?;
            let effective = config::resolve_effective_config(file_config, &args)?;
            let _guard = if std::env::var("SPECK_DAEMONIZED").is_ok() {
                Some(commands::logging::init_daemon_logging(
                    &speck_home,
                    &effective.log_level,
                )?)
            } else {
                init_tracing(&effective.log_level)?;
                None
            };
            for warning in warnings {
                eprintln!("warning: unknown config key: {}", warning.path);
            }
            commands::up::run_up(args, &speck_home, effective).await?
        }
        Commands::Down => {
            init_tracing(&default_tracing_filter())?;
            commands::down::run_down(&speck_home).await?;
            docker_context::deselect();
        }
        Commands::Restart => {
            init_tracing(&default_tracing_filter())?;
            commands::restart::run_restart(&speck_home).await?
        }
        Commands::Run(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::run::run_run(args, &speck_home).await?
        }
        Commands::Ps => {
            init_tracing(&default_tracing_filter())?;
            commands::ps::run_ps(&speck_home).await?
        }
        Commands::Exec(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::exec::run_exec(args, &speck_home).await?
        }
        Commands::Stop(args) => {
            init_tracing(&default_tracing_filter())?;
            match args.container {
                Some(container) => commands::stop::run_stop(&container, &speck_home).await?,
                None if args.force => {
                    commands::down::run_force_stop(&speck_home).await?;
                    docker_context::deselect();
                }
                None => {
                    commands::down::run_down(&speck_home).await?;
                    docker_context::deselect();
                }
            }
        }
        Commands::Delete(args) => {
            init_tracing(&default_tracing_filter())?;
            if commands::delete::run_delete(args.force, &speck_home, true).await?
                && profile.is_default()
            {
                println!("Run `spk start` to create a fresh one.");
            }
        }
        Commands::Recreate(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::recreate::run_recreate(args.force, &speck_home).await?
        }
        Commands::Rm(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::rm::run_rm(args, &speck_home).await?
        }
        Commands::Build(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::build::run_build(args, &speck_home).await?
        }
        Commands::Dashboard => {
            init_tracing(&default_tracing_filter())?;
            commands::dashboard::run_dashboard(&speck_home).await?
        }
        Commands::Completion(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::completion::run_completion(args)
        }
        Commands::Env(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::env::run_env(args, &speck_home)
        }
        Commands::Init(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::init::run_init(args, &speck_home).await?
        }
        Commands::Doctor(args) => {
            init_tracing(&default_tracing_filter())?;
            let code = commands::doctor::run_doctor(&speck_home, args, format).await?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Commands::Template => {
            let code = commands::template::run_template(&speck_home, profile)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Commands::Config => {
            let code = commands::config_cmd::run_config(&speck_home, format)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Commands::Status(args) => {
            init_tracing(&default_tracing_filter())?;
            let code =
                commands::status::run_status(args.extended, format.or_json(args.json)).await?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Commands::List(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::status::run_list(format.or_json(args.json)).await?
        }
        Commands::Version => {
            init_tracing(&default_tracing_filter())?;
            commands::status::run_version(format).await?
        }
        Commands::Prune(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::prune::run_prune(args.force, &speck_home)?
        }
        Commands::Profile(args) => match args.command {
            ProfileCommand::Ls(list) => {
                commands::status::run_list(format.or_json(list.json)).await?
            }
            ProfileCommand::Use { name } => commands::profile_cmd::run_use(&name)?,
            ProfileCommand::Current => commands::profile_cmd::run_current(format)?,
            ProfileCommand::Create {
                name,
                cpus,
                memory,
                disk,
                no_autostart,
            } => commands::profile_cmd::run_create(&name, cpus, memory, disk, no_autostart)?,
            ProfileCommand::Rm { name, force } => {
                let code = commands::profile_cmd::run_rm(&name, force).await?;
                if code != 0 {
                    std::process::exit(code);
                }
            }
        },
        Commands::Logs(args) => {
            init_tracing(&default_tracing_filter())?;
            commands::logs::run_logs(
                &speck_home,
                commands::logs::LogsArgs {
                    follow: args.follow,
                    tail: args.tail,
                },
            )?
        }
    }

    Ok(())
}
