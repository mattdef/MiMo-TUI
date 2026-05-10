mod client;
mod config;
mod tui;

use std::io::{self, Write};

use anyhow::Result;
use clap::{Parser, Subcommand};
use client::{ChatMessage, MimoClient};
use config::{AppConfig, ConfigOverrides};

#[derive(Debug, Parser)]
#[command(name = "mimo-tui")]
#[command(about = "Terminal UI specialised for Xiaomi MiMo OpenAI-compatible models")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        long,
        help = "Override MiMo API key. Prefer MIMO_API_KEY for regular use."
    )]
    api_key: Option<String>,

    #[arg(
        long,
        help = "Override API base URL, for example https://api.xiaomimimo.com/v1"
    )]
    base_url: Option<String>,

    #[arg(long, help = "Override MiMo model id, for example mimo-v2-flash")]
    model: Option<String>,

    #[arg(long, help = "Override sampling temperature")]
    temperature: Option<f32>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Run a single prompt without opening the TUI")]
    Ask {
        #[arg(help = "Prompt to send to MiMo")]
        prompt: String,
    },
    #[command(about = "Show resolved configuration and credential status")]
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(ConfigOverrides {
        api_key: cli.api_key,
        base_url: cli.base_url,
        model: cli.model,
        temperature: cli.temperature,
    })?;

    match cli.command {
        Some(Command::Ask { prompt }) => ask(config, prompt).await,
        Some(Command::Doctor) => {
            doctor(&config);
            Ok(())
        }
        None => tui::run(config).await,
    }
}

async fn ask(config: AppConfig, prompt: String) -> Result<()> {
    let client = MimoClient::new(&config)?;
    let messages = vec![
        ChatMessage::system(config.system_prompt.clone()),
        ChatMessage::user(prompt),
    ];

    client
        .stream_chat(&messages, |delta| {
            print!("{delta}");
            io::stdout().flush()?;
            Ok(())
        })
        .await?;

    println!();
    Ok(())
}

fn doctor(config: &AppConfig) {
    println!("Config file : {}", config.config_path.display());
    println!("Base URL    : {}", config.base_url);
    println!("Model       : {}", config.model);
    println!("Temperature : {}", config.temperature);
    println!(
        "API key     : {}",
        if config.api_key.is_some() {
            "configured"
        } else {
            "missing (set MIMO_API_KEY or config.toml)"
        }
    );
}
