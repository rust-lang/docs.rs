use fetch_from_crate::SourceArchive;
use tokio::io;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (name, version) = match (args.next(), args.next()) {
        (Some(name), Some(version)) => (name, version),
        _ => {
            eprintln!("usage: fetch <name> <version> [file]");
            return;
        }
    };
    let file = args.next().unwrap_or_else(|| "Cargo.toml".to_string());

    let archive = match SourceArchive::load(&name, &version).await {
        Ok(archive) => archive,
        Err(err) => {
            eprintln!("error loading: {:?}", err);
            return;
        }
    };

    let entry = archive
        .by_name(&file)
        .unwrap_or_else(|| panic!("no {file} in archive"));

    if let Err(err) = archive.fetch(entry, &mut io::stdout()).await {
        eprintln!("error fetching: {:?}", err);
    }
}
