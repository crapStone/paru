use crate::config::{Config, LocalRepos, Sign};
use crate::exec;

use std::env::current_exe;
use std::ffi::OsStr;
use std::fs::read_link;
use std::path::{Path, PathBuf};
use std::process::Command;

use alpm::{AlpmListMut, Db};
use anyhow::{Context, Result};
use nix::unistd::{self, User};

pub fn add<P: AsRef<Path>, S: AsRef<OsStr>>(
    config: &Config,
    path: P,
    name: &str,
    pkgs: &[S],
) -> Result<()> {
    let db = path.as_ref().join(format!("{}.db", name));
    let name = if db.exists() {
        if pkgs.is_empty() {
            return Ok(());
        }
        read_link(db)?
    } else if !name.contains(".db.") {
        PathBuf::from(format!("{}.db.tar.gz", name))
    } else {
        PathBuf::from(name)
    };

    let path = path.as_ref();
    let file = path.join(&name);

    let user = unistd::getuid();
    let group = unistd::getgid();

    if !path.exists() {
        exec::command(
            &config.sudo_bin,
            ".",
            &[
                OsStr::new("install"),
                OsStr::new("-dm755"),
                OsStr::new("-o"),
                user.to_string().as_ref(),
                OsStr::new("-g"),
                group.to_string().as_ref(),
                path.as_os_str(),
            ],
        )?;
    }

    let mut args = vec![OsStr::new("-R"), file.as_os_str()];
    let pkgs = pkgs
        .iter()
        .map(|p| path.join(Path::new(p.as_ref()).file_name().unwrap()))
        .collect::<Vec<_>>();

    if config.sign_db != Sign::No {
        args.push("-s".as_ref());
        if let Sign::Key(ref k) = config.sign_db {
            args.push("-k".as_ref());
            args.push(k.as_ref());
        }
    }

    args.extend(pkgs.iter().map(|p| p.as_os_str()));
    let err = exec::command("repo-add", ".", &args);

    let user = User::from_uid(user).unwrap().unwrap();

    if err.is_err() {
        eprintln!(
            "Could not add packages to repo:
    paru now expects local repos to be writable as your user:
    You should chown/chmod your repos to be writable by you:
    chown -R {}: {}",
            user.name,
            path.display()
        );
    }

    err
}

pub fn remove<P: AsRef<Path>, S: AsRef<OsStr>>(
    config: &Config,
    path: P,
    name: &str,
    pkgs: &[S],
) -> Result<()> {
    let path = path.as_ref();
    let db = path.join(format!("{}.db", name));
    if pkgs.is_empty() || !db.exists() {
        return Ok(());
    }

    let name = read_link(db)?;
    let file = path.join(&name);

    let mut args = vec![file.as_os_str()];

    if config.sign_db != Sign::No {
        args.push("-s".as_ref());
        if let Sign::Key(ref k) = config.sign_db {
            args.push("-k".as_ref());
            args.push(k.as_ref());
        }
    }

    args.extend(pkgs.iter().map(|p| p.as_ref()));
    exec::command("repo-remove", ".", &args)?;

    Ok(())
}

pub fn init<P: AsRef<Path>>(config: &Config, path: P, name: &str) -> Result<()> {
    let pkgs: &[&str] = &[];
    add(config, path, name, pkgs)
}

fn is_configured_local_db(config: &Config, db: &Db) -> bool {
    match config.repos {
        LocalRepos::None => false,
        LocalRepos::Default => is_local_db(db),
        LocalRepos::Repo(ref r) => is_local_db(db) && r.iter().any(|r| *r == db.name()),
    }
}

pub fn file<'a>(repo: &Db<'a>) -> Option<&'a str> {
    repo.servers()
        .first()
        .map(|s| s.trim_start_matches("file://"))
}

pub fn all_files(config: &Config) -> Vec<String> {
    config
        .alpm
        .syncdbs()
        .iter()
        .flat_map(|db| db.servers())
        .filter(|f| f.starts_with("file://"))
        .map(|s| s.trim_start_matches("file://").to_string())
        .collect()
}

fn is_local_db(db: &alpm::Db) -> bool {
    !db.servers().is_empty() && db.servers().iter().all(|s| s.starts_with("file://"))
}

pub fn repo_aur_dbs(config: &Config) -> (AlpmListMut<Db>, AlpmListMut<Db>) {
    let dbs = config.alpm.syncdbs();
    let mut aur = dbs.to_list_mut();
    let mut repo = dbs.to_list_mut();
    aur.retain(|db| is_configured_local_db(config, db));
    repo.retain(|db| !is_configured_local_db(config, db));
    (repo, aur)
}

pub fn refresh<S: AsRef<OsStr>>(config: &mut Config, repos: &[S]) -> Result<i32> {
    let exe = current_exe().context("failed to get current exe")?;
    let c = config.color;
    if !nix::unistd::getuid().is_root() {
        let mut cmd = Command::new(&config.sudo_bin);

        cmd.arg(exe);

        if let Some(ref conf) = config.pacman_conf {
            cmd.arg("--config").arg(conf);
        }

        cmd.arg("--dbpath")
            .arg(config.alpm.dbpath())
            .arg("-Lu")
            .args(repos);

        let status = cmd.spawn()?.wait()?;

        return Ok(status.code().unwrap_or(1));
    }

    let mut dbs = config.alpm.syncdbs_mut().to_list_mut();
    dbs.retain(|db| is_local_db(db));

    if !repos.is_empty() {
        dbs.retain(|db| repos.iter().any(|r| r.as_ref() == db.name()));
    }

    println!(
        "{} {}",
        c.action.paint("::"),
        c.bold.paint("syncing local databases...")
    );

    if !dbs.is_empty() {
        dbs.update(false)?;
    } else {
        println!("  nothing to do");
    }

    Ok(0)
}
