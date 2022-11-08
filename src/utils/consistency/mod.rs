use self::diff::{Diff, Diffable};
use crate::Index;
use anyhow::Context;
use tracing::info;

mod data;
mod db;
mod diff;
mod index;

pub fn run_check(
    conn: &mut postgres::Client,
    index: &Index,
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    if !dry_run {
        anyhow::bail!("TODO: only a --dry-run synchronization is supported currently");
    }

    info!("Loading data from database...");
    let timer = std::time::Instant::now();
    let db_data =
        self::db::load(conn).context("Loading crate data from database for consistency check")?;
    info!("...loaded in {:?}", timer.elapsed());

    tracing::info!("Loading data from index...");
    let timer = std::time::Instant::now();
    let index_data =
        self::index::load(index).context("Loading crate data from index for consistency check")?;
    info!("...loaded in {:?}", timer.elapsed());

    let diff = db_data.diff(index_data);

    for krate in diff.crates {
        match krate {
            Diff::Both(name, diff) => {
                for release in diff.releases {
                    match release {
                        Diff::Both(_, _) => {}
                        Diff::Left(version, _) => {
                            println!("Release in db not in index: {} {}", name, version);
                        }
                        Diff::Right(version, _) => {
                            println!("Release in index not in db: {} {}", name, version);
                        }
                    }
                }
            }
            Diff::Left(name, _) => {
                println!("Crate in db not in index: {}", name);
            }
            Diff::Right(name, _) => {
                println!("Crate in index not in db: {}", name);
            }
        }
    }

    Ok(())
}
