use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use docs_rs_builder::{Config, PackageKind, RustwideBuilder, queue_builder};
use docs_rs_config::AppConfig as _;
use docs_rs_context::Context;
use docs_rs_database::service_config::{ConfigName, get_config};
use docs_rs_env_vars::maybe_env;
use docs_rs_types::{KrateName, Version};
use std::{path::PathBuf, sync::Arc};
use tokio::runtime;

fn main() -> Result<()> {
    let logging_config = docs_rs_logging::Config::from_environment()?;
    docs_rs_builder::logging::init(&logging_config);
    let _guard =
        docs_rs_logging::init_with_config(&logging_config).context("error initializing logging")?;

    if let Err(err) = CommandLine::parse().handle_args() {
        eprintln!("error running builder: {:?}", err);
        drop(_guard);
        std::process::exit(1);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
enum CommandLine {
    /// Run a build-server
    Start,

    Build {
        #[command(subcommand)]
        subcommand: BuildSubcommand,
    },
}
impl CommandLine {
    fn handle_args(self) -> Result<()> {
        let runtime = runtime::Builder::new_multi_thread().enable_all().build()?;
        let config = Arc::new(Config::from_environment()?);
        let ctx = runtime.block_on(async {
            Context::builder()
                .with_runtime()
                .await?
                .with_meter_provider()?
                .with_pool()
                .await?
                .with_storage()
                .await?
                .with_maybe_cdn()?
                .with_build_queue()?
                .with_registry_api()?
                .with_repository_stats()?
                .with_build_limits()?
                .build()
        })?;

        match self {
            Self::Start => {
                queue_builder(&ctx, &config, RustwideBuilder::init(config.clone(), &ctx)?)?;
            }
            Self::Build { subcommand } => subcommand.handle_args(ctx, config)?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum BuildSubcommand {
    /// Builds documentation for a crate
    Crate {
        /// Crate name
        #[arg(name = "CRATE_NAME", requires("CRATE_VERSION"))]
        crate_name: Option<KrateName>,

        /// Version of crate
        #[arg(name = "CRATE_VERSION")]
        crate_version: Option<Version>,

        /// Build a crate at a specific path
        #[arg(short = 'l', long = "local", conflicts_with_all(&["CRATE_NAME", "CRATE_VERSION"]))]
        local: Option<PathBuf>,
    },

    /// update the currently installed rustup toolchain
    UpdateToolchain {
        /// Update the toolchain only if no toolchain is currently installed
        #[arg(name = "ONLY_FIRST_TIME", long = "only-first-time")]
        only_first_time: bool,
    },

    /// Adds essential files for the installed version of rustc
    AddEssentialFiles,
}

impl BuildSubcommand {
    fn handle_args(self, ctx: Context, config: Arc<Config>) -> Result<()> {
        let rustwide_builder =
            || -> Result<RustwideBuilder> { RustwideBuilder::init(config.clone(), &ctx) };

        match self {
            Self::Crate {
                crate_name,
                crate_version,
                local,
            } => {
                let mut builder = rustwide_builder()?;

                builder.update_toolchain_and_add_essential_files()?;

                if let Some(path) = local {
                    builder
                        .build_local_package(&path)
                        .context("Building documentation failed")?;
                } else {
                    if maybe_env::<String>("REGISTRY_URL")?.is_some() {
                        bail!("we temporarily don't support custom registries in this commmand.");
                    }

                    builder
                        .build_package(
                            &crate_name
                                .with_context(|| anyhow!("must specify name if not local"))?,
                            &crate_version
                                .with_context(|| anyhow!("must specify version if not local"))?,
                            PackageKind::CratesIo,
                            // registry_url
                            //     .map(|s| PackageKind::Registry(s.as_str()))
                            //     .unwrap_or(PackageKind::CratesIo
                            true,
                        )
                        .context("Building documentation failed")?;
                }
            }

            Self::UpdateToolchain { only_first_time } => {
                let rustc_version = ctx.runtime().block_on({
                    let pool = ctx.pool()?;
                    async move {
                        let mut conn = pool
                            .get_async()
                            .await
                            .context("failed to get a database connection")?;

                        get_config::<String>(&mut conn, ConfigName::RustcVersion).await
                    }
                })?;
                if only_first_time && rustc_version.is_some() {
                    println!("update-toolchain was already called in the past, exiting");
                    return Ok(());
                }

                rustwide_builder()?.update_toolchain_and_add_essential_files()?;
            }

            Self::AddEssentialFiles => {
                rustwide_builder()?
                    .add_essential_files()
                    .context("failed to add essential files")?;
            }
        }

        Ok(())
    }
}
