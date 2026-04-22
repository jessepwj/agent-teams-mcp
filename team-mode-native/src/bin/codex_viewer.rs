use std::time::Duration;

use clap::Parser;
use team_mode_native::viewer::{codex_event_log_path, read_tail_lines, render_codex_event_line};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    data_dir: std::path::PathBuf,
    #[arg(long)]
    member_id: String,
    #[arg(long, default_value_t = 200)]
    lines: usize,
    #[arg(long)]
    follow: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = codex_event_log_path(&args.data_dir, &args.member_id);
    let mut printed = 0_usize;

    loop {
        match read_tail_lines(&path, args.lines) {
            Ok(lines) => {
                for line in lines.iter().skip(printed.saturating_sub(lines.len())) {
                    println!("{}", render_codex_event_line(line));
                }
                printed = lines.len();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("event log not found: {}", path.display());
            }
            Err(error) => return Err(error.into()),
        }

        if !args.follow {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}
