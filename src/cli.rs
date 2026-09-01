use crate::vault::{EntryPatch, Vault};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "relic",
    version,
    about = "Never fade away — local-first knowledge for every agent"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new knowledge vault
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Add a knowledge entry
    Add {
        title: String,
        #[arg(short, long, default_value = "")]
        content: String,
        #[arg(long, default_value = "knowledge")]
        kind: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value_t = 0.7)]
        confidence: f64,
        #[arg(long, default_value = "")]
        source_agent: String,
    },
    /// Update selected fields on an existing entry
    Update {
        entry_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(short, long)]
        content: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        confidence: Option<f64>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        source_agents: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        links: Option<Vec<String>>,
    },
    /// Mark an older entry as superseded by a newer entry
    Supersede {
        old_entry_id: String,
        new_entry_id: String,
    },
    /// Print an entry by ID
    Get { entry_id: String },
    /// List entries
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        status: Option<String>,
    },
    /// Full-text search the vault
    Search {
        query: String,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Rebuild the local search index
    Reindex,
    /// Create a reflection draft
    Reflect {
        #[arg(long, default_value = "weekly")]
        period: String,
    },
    /// Show vault statistics
    Stats,
    /// Check vault health
    Doctor,
    /// Run the Model Context Protocol server over standard input/output
    Mcp {
        /// Relic vault exposed to connected agents
        #[arg(long)]
        vault: PathBuf,
        /// Agent name recorded when a client does not supply source_agent
        #[arg(long, default_value = "mcp-agent")]
        source_agent: String,
    },
}

impl Cli {
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Init { path } => {
                let vault = Vault::init(&path.canonicalize().unwrap_or(path))?;
                println!("Initialized Relic vault at {}", vault.root.display());
            }
            Command::Add {
                title,
                content,
                kind,
                tags,
                confidence,
                source_agent,
            } => {
                let vault = current_vault()?;
                let entry =
                    vault.create(&title, &content, &kind, tags, confidence, &source_agent)?;
                println!(
                    "{}\t{}",
                    entry.meta.id,
                    entry.path.strip_prefix(&vault.root)?.display()
                );
            }
            Command::Update {
                entry_id,
                title,
                content,
                kind,
                status,
                confidence,
                tags,
                source_agents,
                links,
            } => {
                let entry = current_vault()?.update(
                    &entry_id,
                    EntryPatch {
                        title,
                        content,
                        kind,
                        status,
                        confidence,
                        tags,
                        source_agents,
                        links,
                    },
                )?;
                println!(
                    "{}\t{}\t{}",
                    entry.meta.id, entry.meta.status, entry.meta.title
                );
            }
            Command::Supersede {
                old_entry_id,
                new_entry_id,
            } => {
                let (old, new) = current_vault()?.supersede(&old_entry_id, &new_entry_id)?;
                println!("{}\tsuperseded by\t{}", old.meta.id, new.meta.id);
            }
            Command::Get { entry_id } => println!("{}", current_vault()?.get(&entry_id)?.render()?),
            Command::List { kind, status } => {
                for entry in current_vault()?.entries()?.into_iter().filter(|entry| {
                    kind.as_ref().is_none_or(|v| &entry.meta.kind == v)
                        && status.as_ref().is_none_or(|v| &entry.meta.status == v)
                }) {
                    println!(
                        "{}\t{:.2}\t{}\t{}",
                        entry.meta.id, entry.meta.confidence, entry.meta.status, entry.meta.title
                    );
                }
            }
            Command::Search { query, limit } => {
                for hit in current_vault()?.search(&query, limit)? {
                    println!(
                        "{}  {:.2}  {}\n  {}",
                        hit.id,
                        hit.confidence,
                        hit.title,
                        hit.excerpt.replace('\n', " ")
                    );
                }
            }
            Command::Reindex => println!("Indexed {} entries", current_vault()?.reindex()?),
            Command::Reflect { period } => println!(
                "Created {}",
                current_vault()?.create_reflection(&period)?.display()
            ),
            Command::Stats => {
                let entries = current_vault()?.entries()?;
                let active = entries.iter().filter(|e| e.meta.status == "active").count();
                let avg = if entries.is_empty() {
                    0.0
                } else {
                    entries.iter().map(|e| e.meta.confidence).sum::<f64>() / entries.len() as f64
                };
                println!(
                    "entries: {}\nactive: {}\naverage confidence: {:.2}",
                    entries.len(),
                    active,
                    avg
                );
            }
            Command::Doctor => {
                let vault = current_vault()?;
                let entries = vault.entries()?;
                vault.reindex()?;
                println!(
                    "Vault healthy: {} valid entries; index rebuilt",
                    entries.len()
                );
            }
            Command::Mcp {
                vault,
                source_agent,
            } => {
                let vault = Vault::discover(&vault.canonicalize().unwrap_or(vault))?;
                crate::mcp::serve_with_source_agent(vault, &source_agent)?;
            }
        }
        Ok(())
    }
}

fn current_vault() -> Result<Vault> {
    Vault::discover(&env::current_dir()?)
}
