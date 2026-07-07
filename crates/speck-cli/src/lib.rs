#![allow(dead_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod commands;
mod config;
pub mod docker_client;
mod shell;
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
    /// Print shell environment exports for this session
    Env(EnvArgs),
    /// Initialize Speck shell integration (one-time setup)
    Init(InitArgs),
    /// Run health checks and diagnose Speck configuration
    Doctor(DoctorArgs),
    /// Show Speck daemon and VM status
    Status,
    /// Show VM boot logs
    Logs(LogsArgs),
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
