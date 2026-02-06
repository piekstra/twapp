pub mod cli;
pub mod gui;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "twapp", version, about = "Manage Claude work sessions")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<cli::Commands>,

    #[command(flatten)]
    pub gui: gui::GuiArgs,
}
