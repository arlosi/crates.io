use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use cargo_registry_index;
use diesel::prelude::*;
use indicatif::{ProgressBar, ProgressIterator, ProgressStyle};

use crate::{admin::dialoguer, db, models::Crate, schema::crates};

#[derive(clap::Parser, Debug, Copy, Clone)]
#[clap(
    name = "regenerate-index",
    about = "Re-create the index from the database"
)]
pub struct Opts {
    /// Time in milliseconds to sleep between crate updates to reduce database load.
    #[clap(long)]
    delay: u64,
}

fn walkdir(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_str().unwrap();
        if file_name.starts_with(".") || file_name == "config.json" {
            continue;
        }
        let meta = entry.metadata()?;
        if meta.is_file() {
            files.push(entry.path())
        } else if meta.is_dir() {
            walkdir(&entry.path(), files)?;
        }
    }
    Ok(())
}

pub fn normalize(files: &Vec<PathBuf>) -> anyhow::Result<()> {
    let num_files = files.len();
    for (i, file) in files.iter().enumerate() {
        if i % 50 == 0 {
            info!(num_files, i, ?file);
        }

        let path = file;
        let mut body: Vec<u8> = Vec::new();
        let file = File::open(&file)?;
        let reader = BufReader::new(file);
        let mut versions = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            let mut krate: cargo_registry_index::Crate = serde_json::from_str(&line)?;
            krate.deps.sort();
            versions.push(krate);
        }
        for version in versions {
            serde_json::to_writer(&mut body, &version).unwrap();
            body.push(b'\n');
        }
        fs::write(path, body)?;
    }
    Ok(())
}

pub fn run(opts: Opts) -> anyhow::Result<()> {
    let mut conn = db::oneoff_connection().unwrap();
    let path = Path::new("/tmp/gitwbXzm7");
    let mut files = Vec::new();
    walkdir(path, &mut files)?;

    println!("found {} crates", files.len());

    if dialoguer::confirm("phase 1 - normalize?") {
        normalize(&files)?
    }

    if dialoguer::confirm("phase 2 - regenerate index from database, continue?") {
        let pb = ProgressBar::new(files.len() as u64);
        pb.set_style(ProgressStyle::with_template("{bar:60} ({pos}/{len}, ETA {eta})").unwrap());

        let crate_list: Vec<String> = crates::table.select(crates::name).load(&mut conn)?;
        for crate_name in crate_list.iter().progress_with(pb) {
            thread::sleep(Duration::from_millis(opts.delay));

            let db_crate: Crate = Crate::by_exact_name(crate_name).first(&mut conn)?;
            let metadata = db_crate.index_metadata(&mut conn)?;
            let path = path.join(cargo_registry_index::Repository::relative_index_file(
                crate_name,
            ));
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(path, metadata)?;
        }
    }
    Ok(())
}
