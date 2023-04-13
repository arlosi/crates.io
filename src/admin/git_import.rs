use std::{
    fs::File,
    io::{BufRead, BufReader},
    thread,
    time::Duration,
};

use anyhow::Context;
use cargo_registry_index::{Repository, RepositoryConfig};
use diesel::prelude::*;
use indicatif::{ProgressBar, ProgressIterator, ProgressStyle};

use crate::{
    admin::dialoguer,
    db,
    schema::{crates, versions},
};

#[derive(clap::Parser, Debug, Copy, Clone)]
#[command(
    name = "git-import",
    about = "Import missing fields from git into the database"
)]
pub struct Opts {
    /// Time in milliseconds to sleep between crate updates to reduce database load.
    #[arg(long)]
    delay: u64,
}

pub fn run(opts: Opts) -> anyhow::Result<()> {
    let mut conn = db::oneoff_connection().unwrap();
    println!("fetching git repo");
    let config = RepositoryConfig::from_environment();
    let repo = Repository::open(&config)?;
    repo.reset_head()?;
    repo.run_command(std::process::Command::new("git").args(["checkout", "9fb832597eae57a3886ca2467cf6183bae88dd5d"]))?;
    println!("HEAD is at {}", repo.head_oid()?);
    let files = repo.get_files_modified_since(None)?;
    println!("found {} crates", files.len());
    if !dialoguer::confirm("continue?") {
        return Ok(());
    }

    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(ProgressStyle::with_template("{bar:60} ({pos}/{len}, ETA {eta})").unwrap());

    for file in files.iter().progress_with(pb.clone()) {
        thread::sleep(Duration::from_millis(opts.delay));
        let crate_name = file.file_name().unwrap().to_str().unwrap();
        let path = repo.index_file(crate_name);
        if !path.exists() {
            pb.suspend(|| println!("skipping {}", path.display()));
            continue;
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let result = conn.transaction(|conn| -> anyhow::Result<()> {
            for line in reader.lines() {
                let krate: cargo_registry_index::Crate = serde_json::from_str(&line?)?;
                if krate.links.is_some() {
                    let rows = import_data(conn, &krate).with_context(|| {
                        format!("Failed to update crate {}#{}", krate.name, krate.vers)
                    })?;
                    if rows > 0 {
                        pb.suspend(|| println!("edited {rows} rows for {}#{}", krate.name, krate.vers));
                    }
                }
            }
            Ok(())
        });
        if let Err(err) = result {
            pb.suspend(|| println!("{err:?}"));
        }
    }
    println!("completed");

    Ok(())
}

fn import_data(conn: &mut PgConnection, krate: &cargo_registry_index::Crate) -> anyhow::Result<usize> {
    let version_id: i32 = versions::table
        .inner_join(crates::table)
        .filter(crates::name.eq(&krate.name))
        .filter(versions::num.eq(&krate.vers))
        .select(versions::id)
        .first(conn)
        .with_context(|| {
            format!(
                "Failed to find {}#{} in the database",
                krate.name, krate.vers
            )
        })?;

    // Update `links` fields.
    let rows = diesel::update(versions::table)
        .set((
            versions::links.eq(&krate.links),
        ))
        .filter(versions::id.eq(version_id))
        .filter(versions::links.is_null())
        .execute(conn)
        .with_context(|| {
            format!(
                "Failed to update links of {}#{} (id: {version_id})",
                krate.name, krate.vers
            )
        })?;
    Ok(rows)
}
