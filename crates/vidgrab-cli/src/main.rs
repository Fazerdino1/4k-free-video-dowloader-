//! Zero-GTK headless CLI entrypoint for servers, Docker containers, Alpine/musl, and ARM boards.

mod server;

use std::process::ExitCode;
use vidgrab_core::cli::{print_usage, try_run_cli_from_args};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        let bin = args.first().map(String::as_str).unwrap_or("vidgrab-cli");
        print_usage(bin, false);
        return ExitCode::SUCCESS;
    }

    if args.get(1).map(String::as_str) == Some("serve") {
        let code = server::run_serve(&args[2..]).await;
        return ExitCode::from(u8::try_from(code.clamp(0, 255)).unwrap_or(1));
    }

    match try_run_cli_from_args(&args).await {
        Some(code) => ExitCode::from(u8::try_from(code.clamp(0, 255)).unwrap_or(1)),
        None => {
            let bin = args.first().map(String::as_str).unwrap_or("vidgrab-cli");
            print_usage(bin, false);
            ExitCode::SUCCESS
        }
    }
}
