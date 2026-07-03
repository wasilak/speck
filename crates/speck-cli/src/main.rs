use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod commands;
mod config;
mod docker_client;
mod theme;

#[derive(Parser)]
#[command(
    name = "spk",
    version,
    about = "Ultra-fast container runtime for Apple Silicon"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
enum Commands {
    /// Start the Speck VM and Docker API server
    Up(UpArgs),
    /// Stop the Speck VM
    Down,
    /// Pull an image and run a container
    Run(RunArgs),
    /// List running containers
    Ps,
    /// Execute a command in a running container
    Exec(ExecArgs),
    /// Stop a running container
    Stop(StopArgs),
    /// Remove a container
    Rm(RmArgs),
    /// Build an image from a Dockerfile
    Build(BuildArgs),
    /// Open the interactive dashboard TUI
    Dashboard,
    /// Generate shell completions
    Completion(CompletionArgs),
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
    #[arg(long)]
    memory: Option<u64>,
    #[arg(long)]
    disk: Option<u64>,
}

#[derive(Parser)]
struct RunArgs {
    image: String,
    #[arg(trailing_var_arg = true)]
    cmd: Vec<String>,
    #[arg(short = 'p')]
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
    container: String,
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

fn resolve_speck_home() -> PathBuf {
    if let Ok(home) = std::env::var("SPECK_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/speck")
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

#[cfg(test)]
mod tests {
    const MAIN_SOURCE: &str = include_str!("main.rs");
    const CLI_MANIFEST: &str = include_str!("../Cargo.toml");

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let speck_home = resolve_speck_home();

    match cli.command {
        Commands::Up(args) => {
            let (file_config, warnings) = config::load_config_file(&speck_home)?;
            let effective = config::resolve_effective_config(file_config, &args)?;
            init_tracing(&effective.log_level)?;
            for warning in warnings {
                eprintln!("warning: unknown config key: {}", warning.path);
            }
            commands::up::run_up(args, &speck_home, effective).await?
        }
        Commands::Down => {
            init_tracing(&default_tracing_filter())?;
            commands::down::run_down(&speck_home).await?
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
            commands::stop::run_stop(args, &speck_home).await?
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
    }

    Ok(())
}
