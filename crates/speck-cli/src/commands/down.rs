use std::path::Path;

pub async fn run_down(_speck_home: &Path) -> anyhow::Result<()> {
    println!("spk down: send SIGTERM to running 'spk up' process to shut down the VM.");
    println!("Alternatively, Ctrl-C the 'spk up' process.");
    Ok(())
}
