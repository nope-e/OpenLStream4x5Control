mod cli;

use clap::Parser;
use cli::{Cli, execute, render_error, render_report};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(&cli.command) {
        Ok(report) => match render_report(&report, cli.json) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("failed to render command output: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            let rendered = render_error(&error, cli.json);
            if cli.json {
                println!("{rendered}");
            } else {
                eprintln!("{rendered}");
            }
            ExitCode::from(2)
        }
    }
}
