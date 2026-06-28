use clap::CommandFactory;
use clap_complete::generate;

use crate::Cli;
use crate::CompletionArgs;

pub fn run_completion(args: CompletionArgs) {
    let mut cmd = Cli::command();
    generate(args.shell, &mut cmd, "spk", &mut std::io::stdout());
}
