use std::io::{self, Write};

use anyhow::Result;
use clap::{Parser, Subcommand};
use mimo_agent::run_agent_turn;
use mimo_client::MimoClient;
use mimo_config::{AppConfig, ConfigOverrides, known_mimo_models};
use mimo_protocol::ChatMessage;
use mimo_state::session_store;
use mimo_tools::{ToolContext, ToolRegistryBuilder, default_workspace_root};

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
    #[command(about = "List MiMo models from the API, or local suggestions when offline")]
    Models,
    #[command(about = "List locally saved MiMo-TUI sessions")]
    Sessions,
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
        Some(Command::Doctor) => doctor(&config).await,
        Some(Command::Models) => list_models(&config).await,
        Some(Command::Sessions) => list_sessions(&config),
        None => mimo_tui::run(config).await,
    }
}

async fn ask(config: AppConfig, prompt: String) -> Result<()> {
    let client = MimoClient::new(&config)?;
    let request_messages = vec![
        ChatMessage::system(config.system_prompt.clone()),
        ChatMessage::user(prompt),
    ];
    let workspace = default_workspace_root();
    let tool_context = ToolContext::new(workspace);
    let tool_registry = ToolRegistryBuilder::new().build_all();

    run_agent_turn(
        &client,
        request_messages,
        &tool_registry,
        &tool_context,
        |delta| {
            print!("{delta}");
            io::stdout().flush()?;
            Ok(())
        },
        |_| Ok(()),
        |_invocation| async move {
            // In CLI mode, approve all tools to enable tool execution
            // Read-only tools are already auto-approved, mutating tools are also approved here
            Ok(true)
        },
    )
    .await?;

    println!();
    Ok(())
}

async fn doctor(config: &AppConfig) -> Result<()> {
    println!("Config file      : {}", config.config_path.display());
    println!(
        "Base URL         : {} ({})",
        config.base_url, config.base_url_source
    );
    println!(
        "Model            : {} ({})",
        config.model, config.model_source
    );
    println!(
        "Temperature      : {} ({})",
        config.temperature, config.temperature_source
    );
    println!(
        "System prompt    : {} ({})",
        config
            .system_prompt
            .lines()
            .next()
            .unwrap_or("<empty system prompt>"),
        config.system_prompt_source
    );
    println!(
        "API key          : {} ({})",
        if config.api_key.is_some() {
            "configured"
        } else {
            "missing (set MIMO_API_KEY or config.toml)"
        },
        config.api_key_source
    );

    match MimoClient::new(config) {
        Ok(client) => match client.list_models().await {
            Ok(models) => {
                println!("Models API       : ok ({} models)", models.len());
                if !models.is_empty() {
                    println!(
                        "Model preview    : {}",
                        models.into_iter().take(5).collect::<Vec<_>>().join(", ")
                    );
                }
            }
            Err(error) => println!("Models API       : error ({error})"),
        },
        Err(_) => println!("Models API       : skipped (API key missing)"),
    }

    Ok(())
}

async fn list_models(config: &AppConfig) -> Result<()> {
    match MimoClient::new(config) {
        Ok(client) => match client.list_models().await {
            Ok(models) if !models.is_empty() => {
                for model in models {
                    println!("{model}");
                }
            }
            Ok(_) => {
                for model in known_mimo_models(&config.model) {
                    println!("{model}");
                }
            }
            Err(error) => {
                eprintln!("warning: failed to load models from MiMo API: {error}");
                for model in known_mimo_models(&config.model) {
                    println!("{model}");
                }
            }
        },
        Err(_) => {
            for model in known_mimo_models(&config.model) {
                println!("{model}");
            }
        }
    }
    Ok(())
}

fn list_sessions(config: &AppConfig) -> Result<()> {
    let sessions = session_store::list_sessions(config)?;
    if sessions.is_empty() {
        println!("No saved sessions.");
        return Ok(());
    }

    for session in sessions {
        println!(
            "{} | {} | {} | {}",
            session.title,
            session.model,
            session.mode,
            session.path.display()
        );
    }
    Ok(())
}
