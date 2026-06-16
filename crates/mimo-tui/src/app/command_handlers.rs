use super::*;

impl App {
    pub(crate) fn handle_model_command(&mut self, model: Option<String>) -> Result<()> {
        let Some(model) = model else {
            self.status = format!("Current model: {}", self.config.model);
            return Ok(());
        };
        let Some(model) = normalize_model_name(&model) else {
            self.status =
                "Invalid MiMo model id. Expected auto or a non-empty id starting with mimo-"
                    .to_string();
            return Ok(());
        };
        self.config.set_model(model.clone())?;
        self.status = format!("Model switched to {model}");
        Ok(())
    }

    pub(crate) fn handle_config_command(&mut self, command: ConfigCommand) -> Result<()> {
        match command {
            ConfigCommand::Show => {
                self.push_system_message(self.config_summary());
                self.status = "Configuration shown".to_string();
            }
            ConfigCommand::SetApiKey(api_key) => {
                self.config.set_api_key(Some(api_key))?;
                self.status = "API key saved to config.toml".to_string();
            }
            ConfigCommand::ClearApiKey => {
                self.config.set_api_key(None)?;
                self.status = "API key removed from config.toml".to_string();
            }
            ConfigCommand::SetBaseUrl(base_url) => {
                let Some(base_url) = normalize_base_url(&base_url) else {
                    self.status = "Base URL must be a valid http(s) URL".to_string();
                    return Ok(());
                };
                self.config.set_base_url(base_url.clone())?;
                self.status = format!("Base URL switched to {base_url}");
            }
            ConfigCommand::SetModel(model) => {
                let Some(model) = normalize_model_name(&model) else {
                    self.status =
                        "Invalid MiMo model id. Expected auto or a non-empty id starting with mimo-"
                            .to_string();
                    return Ok(());
                };
                self.config.set_model(model.clone())?;
                self.status = format!("Model saved as {model}");
            }
            ConfigCommand::SetTemperature(value) => match value.parse::<f32>() {
                Ok(temperature) => {
                    self.config.set_temperature(temperature)?;
                    self.status = format!("Temperature saved as {temperature}");
                }
                Err(_) => self.status = "Temperature must be a floating point number".to_string(),
            },
        }
        Ok(())
    }

    pub(crate) fn handle_plan_command(&mut self, command: PlanCommand) {
        match command {
            PlanCommand::Show => {
                self.push_system_message(self.plan_summary());
                self.status = "Plan checklist shown".to_string();
            }
            PlanCommand::Add(text) => {
                self.plan_items.push(PlanItem::new(text.clone()));
                self.status = format!("Added plan item: {text}");
            }
            PlanCommand::Done(index) => {
                if let Some(item) = self.plan_items.get_mut(index.saturating_sub(1)) {
                    item.done = true;
                    self.status = format!("Completed plan item {index}");
                } else {
                    self.status = format!("Unknown plan item {index}");
                }
            }
            PlanCommand::Undo(index) => {
                if let Some(item) = self.plan_items.get_mut(index.saturating_sub(1)) {
                    item.done = false;
                    self.status = format!("Reopened plan item {index}");
                } else {
                    self.status = format!("Unknown plan item {index}");
                }
            }
            PlanCommand::Remove(index) => {
                let index = index.saturating_sub(1);
                if index < self.plan_items.len() {
                    self.plan_items.remove(index);
                    self.status = format!("Removed plan item {}", index + 1);
                } else {
                    self.status = format!("Unknown plan item {}", index + 1);
                }
            }
            PlanCommand::Clear => {
                self.plan_items.clear();
                self.status = "Plan checklist cleared".to_string();
            }
        }
    }

    pub(crate) fn handle_memory_command(&mut self, command: MemoryCommand) -> Result<()> {
        match command {
            MemoryCommand::Show => {
                self.push_system_message(memory_store::show_memory(&self.config)?);
                self.status = "Memory shown".to_string();
            }
            MemoryCommand::Path => {
                self.push_system_message(format!(
                    "Memory path\n\n{}",
                    memory_store::memory_path(&self.config).display()
                ));
                self.status = "Memory path shown".to_string();
            }
            MemoryCommand::Clear => {
                let path = memory_store::clear_memory(&self.config)?;
                self.status = format!("Cleared memory file {}", path.display());
            }
            MemoryCommand::Help => {
                self.push_system_message(
                    "Memory commands\n\n/memory show\n/memory path\n/memory clear\n/note <text>\n/recall <query>\n/compact"
                        .to_string(),
                );
                self.status = "Memory help shown".to_string();
            }
        }
        Ok(())
    }

    pub(crate) fn handle_lsp_command(&mut self, command: LspCommand) -> Result<()> {
        match command {
            LspCommand::Status => {
                self.push_system_message(self.diagnostics_status_message());
                self.status = "Diagnostics status shown".to_string();
            }
            LspCommand::Run => {
                self.start_diagnostics_refresh("manual run".to_string())?;
            }
            LspCommand::Show => {
                self.open_text_pager("Diagnostics", self.diagnostics_detail_message());
                self.status = "Diagnostics opened".to_string();
            }
            LspCommand::Clear => {
                let path = diagnostics_store::clear_snapshot(&self.config)?;
                self.diagnostics = None;
                self.status = format!("Cleared diagnostics snapshot {}", path.display());
            }
            LspCommand::On => {
                self.diagnostics_auto_run = true;
                self.status = "Auto diagnostics enabled".to_string();
            }
            LspCommand::Off => {
                self.diagnostics_auto_run = false;
                self.status = "Auto diagnostics disabled".to_string();
            }
        }
        Ok(())
    }

    pub(crate) fn handle_review_command(&mut self, command: ReviewCommand) -> Result<()> {
        self.push_system_message(self.review_context(command)?);
        self.status = "Review context shown".to_string();
        Ok(())
    }

    pub(crate) fn start_diagnostics_refresh(&mut self, trigger: String) -> Result<()> {
        if self.diagnostics_task.is_some() {
            self.status = "Diagnostics are already running".to_string();
            return Ok(());
        }
        let Some(event_tx) = self.event_tx.clone() else {
            bail!("event channel is unavailable");
        };
        let workspace_root = self.tool_context.workspace_root.clone();
        let config = self.config.clone();
        self.status = format!("Running diagnostics ({trigger})...");
        self.diagnostics_task = Some(tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let snapshot = capture_workspace_diagnostics(&workspace_root)?;
                diagnostics_store::save_snapshot(&config, &snapshot)?;
                Ok::<_, anyhow::Error>(snapshot)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = event_tx.send(AppEvent::DiagnosticsFinished(result));
        }));
        Ok(())
    }

    pub(crate) fn diagnostics_status_message(&self) -> String {
        let path = diagnostics_store::diagnostics_path(&self.config);
        let mut output = format!(
            "Diagnostics status\n\nAuto run : {}\nRunning  : {}\nPath     : {}\n",
            if self.diagnostics_auto_run {
                "on"
            } else {
                "off"
            },
            if self.diagnostics_task.is_some() {
                "yes"
            } else {
                "no"
            },
            path.display()
        );
        match &self.diagnostics {
            Some(snapshot) => {
                output.push_str(&format!(
                    "\nLast run : {}\nStatus   : {}\nSummary  : {}\nErrors   : {}\nWarnings : {}",
                    snapshot.updated_at_epoch,
                    snapshot.status,
                    snapshot.summary,
                    snapshot.error_count,
                    snapshot.warning_count
                ));
            }
            None => output.push_str("\nNo cached diagnostics yet."),
        }
        output
    }

    pub(crate) fn diagnostics_detail_message(&self) -> String {
        let mut output = String::from("Diagnostics\n\n");
        match &self.diagnostics {
            Some(snapshot) => {
                output.push_str(&format!(
                    "Command : {}\nStatus  : {}\nSummary : {}\nErrors  : {}\nWarnings: {}\nUpdated : {}\n\n{}",
                    snapshot.command,
                    snapshot.status,
                    snapshot.summary,
                    snapshot.error_count,
                    snapshot.warning_count,
                    snapshot.updated_at_epoch,
                    snapshot.output
                ));
            }
            None => output.push_str("No cached diagnostics yet. Run /lsp run first."),
        }
        output
    }

    pub(crate) fn review_context(&self, command: ReviewCommand) -> Result<String> {
        let git_status = run_workspace_command(
            &self.tool_context.workspace_root,
            "git",
            &["--no-pager", "status", "--short", "--branch"],
        )?;
        let (scope, args) = match command {
            ReviewCommand::Workspace => (
                "workspace".to_string(),
                vec![
                    "--no-pager".to_string(),
                    "diff".to_string(),
                    "--stat".to_string(),
                    "--summary".to_string(),
                ],
            ),
            ReviewCommand::Staged => (
                "staged".to_string(),
                vec![
                    "--no-pager".to_string(),
                    "diff".to_string(),
                    "--cached".to_string(),
                    "--stat".to_string(),
                    "--summary".to_string(),
                ],
            ),
            ReviewCommand::Path(path) => {
                let resolved = self.tool_context.resolve_path(&path)?;
                (
                    path,
                    vec![
                        "--no-pager".to_string(),
                        "diff".to_string(),
                        "--stat".to_string(),
                        "--summary".to_string(),
                        "--".to_string(),
                        relative_path_display(&self.tool_context.workspace_root, &resolved),
                    ],
                )
            }
        };
        let diff_args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let diff = run_workspace_command(&self.tool_context.workspace_root, "git", &diff_args)?;
        let mut output = format!(
            "Review context\n\nScope: {scope}\n\nGit status\n\n{}\n\nDiff summary\n\n{}",
            git_status.trim(),
            if diff.trim().is_empty() {
                "No diff for this scope."
            } else {
                diff.trim()
            }
        );
        output.push_str("\n\nDiagnostics\n\n");
        match &self.diagnostics {
            Some(snapshot) => {
                output.push_str(&format!(
                    "{}\n\n{}",
                    snapshot.summary,
                    first_non_empty_lines(&snapshot.output, 24)
                ));
            }
            None => output.push_str("No cached diagnostics. Run /lsp run."),
        }
        Ok(output)
    }
}
