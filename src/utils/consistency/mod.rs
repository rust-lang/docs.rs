use self::diff::{Diff, Diffable};
use crate::config::Config;
use failure::ResultExt;

mod data;
mod db;
mod diff;
mod index;

pub fn run_check(
    config: &Config,
    conn: &mut postgres::Client,
    dry_run: bool,
) -> Result<(), failure::Error> {
    if !dry_run {
        failure::bail!("TODO: only a --dry-run synchronization is supported currently");
    }

    log::info!("Loading data from database...");
    let timer = std::time::Instant::now();
    let db_data =
        self::db::load(conn).context("Loading crate data from database for consistency check")?;
    log::info!("...loaded in {:?}", timer.elapsed());

    log::info!("Loading data from index...");
    let timer = std::time::Instant::now();
    let index_data =
        self::index::load(config).context("Loading crate data from index for consistency check")?;
    log::info!("...loaded in {:?}", timer.elapsed());

    let diff = db_data.diff(index_data);

    for krate in diff.crates {
        match krate {
            Diff::Both(id, diff) => {
                for release in diff.releases {
                    match release {
                        Diff::Both(_, _) => {}
                        Diff::Left(version, _) => {
                            log::info!("Release in db not in index: {} {}", id, version);
                        }
                        Diff::Right(version, _) => {
                            log::info!("Release in index not in db: {} {}", id, version);
                        }
                    }
                }
            }
            Diff::Left(id, _) => {
                log::info!("Crate in db not in index: {}", id);
            }
            Diff::Right(id, _) => {
                log::info!("Crate in index not in db: {}", id);
            }
        }
    }

    Ok(())
}
