use std::net::SocketAddr;
use std::path::PathBuf;

struct Args {
    data_dir: Option<PathBuf>,
    listen: SocketAddr,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let base_dir = match args.data_dir {
        Some(path) => path,
        None => {
            let cwd = std::env::current_dir()?;
            agent_teams::team_mode::data_dir::resolve_default_base_dir(&cwd)
        }
    };

    agent_teams::team_mode::data_dir::ensure_scaffold(&base_dir)?;
    eprintln!(
        "team_mode_web listening on http://{} using data dir {}",
        args.listen,
        base_dir.display()
    );
    agent_teams::team_mode_web::serve(base_dir, args.listen)?;
    Ok(())
}

impl Args {
    fn parse() -> Self {
        let mut data_dir = None;
        let mut listen = "127.0.0.1:8787"
            .parse::<SocketAddr>()
            .expect("default listen address is valid");
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    let Some(value) = args.next() else {
                        eprintln!("--data-dir requires a path");
                        std::process::exit(2);
                    };
                    data_dir = Some(PathBuf::from(value));
                }
                "--listen" => {
                    let Some(value) = args.next() else {
                        eprintln!("--listen requires an address, e.g. 127.0.0.1:8787");
                        std::process::exit(2);
                    };
                    listen = value.parse().unwrap_or_else(|err| {
                        eprintln!("invalid --listen value '{value}': {err}");
                        std::process::exit(2);
                    });
                }
                "--help" | "-h" => {
                    println!(
                        "team_mode_web\n\nUSAGE:\n  team_mode_web [--data-dir PATH] [--listen ADDR]\n\nDEFAULTS:\n  --listen 127.0.0.1:8787"
                    );
                    std::process::exit(0);
                }
                other => {
                    eprintln!("unknown argument: {other}");
                    std::process::exit(2);
                }
            }
        }
        Self { data_dir, listen }
    }
}
