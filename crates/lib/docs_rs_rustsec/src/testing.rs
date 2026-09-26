use crate::{RustsecClient, api::RustsecDatabase};
use anyhow::Result;
use docs_rs_types::KrateName;
use rustsec::{VersionReq, advisory::Informational};
use std::{
    fs, mem,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

static NEXT_ADVISORY_ID: AtomicU32 = AtomicU32::new(1);

fn next_advisory_id() -> String {
    format!(
        "RUSTSEC-2026-{:04}",
        NEXT_ADVISORY_ID.fetch_add(1, Ordering::Relaxed),
    )
}

/// create a dummy rustsec advisory for testing.
#[bon::builder(
    on(_, into),
    finish_fn(name = build)
)]
pub fn advisory(
    #[builder(start_fn)] package: &KrateName,
    #[builder(default = next_advisory_id())] id: String,
    informational: Option<Informational>,
    patched: Option<VersionReq>,
    withdrawn: Option<String>,
) -> rustsec::Advisory {
    serde_json::from_value(serde_json::json!({
        "advisory": {
            "id": id,
            "package": package.as_str(),
            "date": "2026-09-23",
            "title": format!("`{package}` is unmaintained"),
            "description": "Test advisory.",
            "informational": informational,
            "withdrawn": withdrawn,
        },
        "versions": {
            "patched": patched.into_iter().collect::<Vec<_>>(),
        },
    }))
    .expect("valid test advisory")
}

/// create & open a separate rustsec database with the given advisories.
///
/// `rustsec::Database` loads everything into memory when it opens the DB,
/// so it's ok that we write to a `tempdir` here, which will be cleaned up
/// at the end of this method.
pub fn database(
    advisories: impl IntoIterator<Item = rustsec::Advisory>,
) -> Result<RustsecDatabase> {
    let directory = tempfile::tempdir()?;
    fs::create_dir(directory.path().join("crates"))?;
    for mut advisory in advisories {
        let package_dir = directory
            .path()
            .join("crates")
            .join(advisory.metadata.package.as_str());
        fs::create_dir_all(&package_dir)?;

        let title = mem::take(&mut advisory.metadata.title);
        let description = mem::take(&mut advisory.metadata.description);
        let metadata = toml::to_string(&advisory)?;
        fs::write(
            package_dir.join(format!("{}.md", advisory.id())),
            format!("```toml\n{metadata}```\n\n# {title}\n\n{description}\n"),
        )?;
    }
    Ok(RustsecDatabase(Arc::new(rustsec::Database::open(
        directory.path(),
    )?)))
}

/// Create a client backed by test advisories, without starting a fetch task.
pub fn client(advisories: impl IntoIterator<Item = rustsec::Advisory>) -> Result<RustsecClient> {
    Ok(RustsecClient::from_database(database(advisories)?))
}

/// Create an unmaintained advisory for a package.
pub fn unmaintained(package: &KrateName) -> rustsec::Advisory {
    advisory(package)
        .informational(Informational::Unmaintained)
        .build()
}
