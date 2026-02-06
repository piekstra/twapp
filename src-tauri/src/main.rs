// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

fn main() {
    let cli = twapp_lib::Cli::parse();
    match cli.command {
        Some(cmd) => {
            let code = twapp_lib::cli::run(cmd);
            std::process::exit(code);
        }
        None => {
            twapp_lib::gui::run(cli.gui);
        }
    }
}
