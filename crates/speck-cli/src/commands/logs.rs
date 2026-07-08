use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use anyhow::Context as _;

pub struct LogsArgs {
    pub follow: bool,
    pub tail: bool,
}

pub fn run_logs(speck_home: &Path, args: LogsArgs) -> anyhow::Result<()> {
    let log_path = speck_home.join("speck.log");
    if !log_path.exists() {
        anyhow::bail!(
            "no log file found at {} — start Speck first",
            log_path.display()
        );
    }

    if args.tail {
        // Print the last 20 lines of the daemon log.
        let f = std::fs::File::open(&log_path)
            .with_context(|| format!("cannot open {}", log_path.display()))?;
        let reader = BufReader::new(f);
        let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
        let start = lines.len().saturating_sub(20);
        for line in &lines[start..] {
            println!("{line}");
        }
        return Ok(());
    }

    if args.follow {
        // Follow mode: print existing lines then wait for new ones.
        let mut f = std::fs::File::open(&log_path)
            .with_context(|| format!("cannot open {}", log_path.display()))?;
        let metadata = f.metadata()?;
        let file_len = metadata.len();
        if file_len > 0 {
            let mut buf = vec![0u8; file_len as usize];
            use std::io::Read;
            f.read_exact(&mut buf)?;
            print!("{}", String::from_utf8_lossy(&buf));
        }
        let mut last_len = file_len;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let metadata = f.metadata()?;
            let new_len = metadata.len();
            if new_len > last_len {
                let mut buf = vec![0u8; (new_len - last_len) as usize];
                use std::io::Read;
                f.seek(SeekFrom::Start(last_len))?;
                f.read_exact(&mut buf)?;
                print!("{}", String::from_utf8_lossy(&buf));
                use std::io::Write;
                std::io::stdout().flush()?;
                last_len = new_len;
            }
        }
    }

    // Default: print the full daemon log.
    let content = std::fs::read_to_string(&log_path)
        .with_context(|| format!("cannot read {}", log_path.display()))?;
    print!("{content}");
    Ok(())
}
