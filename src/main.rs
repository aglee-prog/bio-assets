use anyhow::{Context, Result, ensure};
use bio_assets::{
    importers::{self, Network},
    index::Library,
    server,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use std::{fs::OpenOptions, net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "BIO_ASSETS_DATA",
        default_value = "storage"
    )]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Serve the offline MCP endpoint over Streamable HTTP.
    Serve {
        #[arg(long, env = "BIO_ASSETS_BIND", default_value = "0.0.0.0:8092")]
        bind: SocketAddr,
    },
    /// Explicitly import a source. Downloads are cached for resumability.
    Import(ImportArgs),
    /// Refresh a source, including original downloads. Existing IDs are retained.
    Update(ImportArgs),
    /// Show the local index size without network access.
    Status,
}
#[derive(Clone, Copy, ValueEnum)]
enum Source {
    Bioicons,
    Niaid,
    Scidraw,
}
impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Bioicons => "bioicons",
            Self::Niaid => "niaid",
            Self::Scidraw => "scidraw",
        }
    }
}
#[derive(Args)]
struct ImportArgs {
    source: Source,
    /// Return a nonzero exit status if any asset is skipped.
    #[arg(long)]
    strict: bool,
    /// Import a local JSON manifest and relative SVG paths (all sources).
    #[arg(long, conflicts_with = "from")]
    manifest: Option<PathBuf>,
    /// Import a local Bioicons Git checkout, without downloading.
    #[arg(long)]
    from: Option<PathBuf>,
    /// Optional cap for smoke tests; omitted means the complete collection.
    #[arg(long,value_parser=clap::value_parser!(usize))]
    limit: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bio_assets=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let library = Library::new(cli.data_dir);
    match cli.command {
        Commands::Serve { bind } => {
            library.initialize()?;
            server::serve(library, bind).await?;
        }
        Commands::Status => println!("{}", serde_json::json!({"assets":library.count()?})),
        Commands::Import(args) => run_import(&library, args, false).await?,
        Commands::Update(args) => run_import(&library, args, true).await?,
    }
    Ok(())
}
async fn run_import(library: &Library, args: ImportArgs, update: bool) -> Result<()> {
    library.initialize()?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(library.root.join("data/import.lock"))?;
    lock.try_lock_exclusive()
        .context("another import is already running")?;
    ensure!(args.limit != Some(0), "limit must be positive");
    ensure!(
        args.from.is_none() || matches!(args.source, Source::Bioicons),
        "--from is only supported for Bioicons; use --manifest for other local exports"
    );
    let report = if let Some(path) = args.manifest {
        importers::import_manifest(library, args.source.name(), &path, args.limit)?
    } else {
        match args.source {
            Source::Bioicons => {
                importers::bioicons::import(library, args.from.as_deref(), update, args.limit)?
            }
            Source::Niaid => {
                importers::niaid::import(library, &Network::new()?, args.limit, update).await?
            }
            Source::Scidraw => {
                importers::scidraw::import(library, &Network::new()?, args.limit, update).await?
            }
        }
    };
    report.save(library, args.source.name())?;
    report.check_outcome(args.strict)?;
    Ok(())
}
