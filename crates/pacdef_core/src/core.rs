use std::collections::HashSet;
use std::fs::{remove_file, File};
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use anyhow::{anyhow, ensure, Context, Result};
use clap::ArgMatches;

use crate::action::*;
use crate::args;
use crate::backend::{Backend, Backends, ToDoPerBackend};
use crate::cmd::run_edit_command;
use crate::env::get_single_var;
use crate::path::get_pacdef_group_dir;
use crate::review;
use crate::search;
use crate::ui::get_user_confirmation;
use crate::Config;
use crate::Group;

/// Most data that is required during runtime of the program.
pub struct Pacdef {
    args: ArgMatches,
    config: Config,
    groups: HashSet<Group>,
}

impl Pacdef {
    /// Creates a new [`Pacdef`]. `config` should be passed from [`Config::load`], and `args` from
    /// [`args::get`].
    #[must_use]
    pub const fn new(args: ArgMatches, config: Config, groups: HashSet<Group>) -> Self {
        Self {
            args,
            config,
            groups,
        }
    }

    /// Run the action that was provided by the user as first argument.
    ///
    /// For convenience sake, all called functions take a `&self` argument, even if these are not
    /// strictly required.
    ///
    /// # Panics
    ///
    /// Panics if the user passed an unexpected action. This means all fields from `crate::action::Action` must be matched in this function.
    ///
    /// # Errors
    ///
    /// This function propagates errors from the underlying functions.
    #[allow(clippy::unit_arg)]
    pub fn run_action_from_arg(mut self) -> Result<()> {
        match self.args.subcommand() {
            Some((CLEAN, _)) => self.clean_packages(),
            Some((EDIT, args)) => self.edit_group_files(args).context("editing group files"),
            Some((GROUPS, _)) => Ok(self.show_groups()),
            Some((IMPORT, args)) => self.import_groups(args).context("importing groups"),
            Some((NEW, args)) => self.new_groups(args).context("creating new group files"),
            Some((REMOVE, args)) => self.remove_groups(args).context("removing groups"),
            Some((REVIEW, _)) => review::review(self.get_unmanaged_packages(), self.groups)
                .context("review unmanaged packages"),
            Some((SHOW, args)) => self.show_group_content(args).context("showing groups"),
            Some((SEARCH, args)) => {
                search::search_packages(args, &self.groups).context("searching packages")
            }
            Some((SYNC, _)) => self.install_packages(),
            Some((UNMANAGED, _)) => self.show_unmanaged_packages(),
            Some((VERSION, _)) => Ok(self.show_version()),
            Some((_, _)) => panic!(),
            None => {
                unreachable!("argument parser requires some subcommand to return an `ArgMatches`")
            }
        }
    }

    fn get_missing_packages(&mut self) -> ToDoPerBackend {
        let mut to_install = ToDoPerBackend::new();

        for backend in Backends::iter() {
            let mut backend = self.overwrite_values_from_config(backend);

            backend.load(&self.groups);

            match backend.get_missing_packages_sorted() {
                Ok(diff) => to_install.push((backend, diff)),
                Err(error) => show_error(&error, &*backend),
            };
        }

        to_install
    }

    fn overwrite_values_from_config(&mut self, backend: Box<dyn Backend>) -> Box<dyn Backend> {
        if backend.get_section() == "pacman" {
            Box::new(crate::backend::Pacman {
                binary: self.config.aur_helper.clone(),
                aur_rm_args: self.config.aur_rm_args.take(),
                packages: HashSet::new(),
            })
        } else {
            backend
        }
    }

    fn install_packages(&mut self) -> Result<()> {
        let to_install = self.get_missing_packages();

        if to_install.nothing_to_do_for_all_backends() {
            println!("nothing to do");
            return Ok(());
        }

        println!("Would install the following packages:\n");
        to_install.show().context("printing things to do")?;

        println!();
        if !get_user_confirmation()? {
            return Ok(());
        };

        to_install.install_missing_packages()
    }

    #[allow(clippy::unused_self)]
    fn edit_group_files(&self, groups: &ArgMatches) -> Result<()> {
        let group_dir = crate::path::get_pacdef_group_dir()?;

        let files: Vec<_> = groups
            .get_many::<String>("group")
            .context("getting group from args")?
            .map(|file| {
                let mut buf = group_dir.clone();
                buf.push(file);
                buf
            })
            .collect();

        for file in &files {
            ensure!(
                file.exists(),
                "group file {} not found",
                file.to_string_lossy()
            );
        }

        let success = run_edit_command(&files)
            .context("running editor")?
            .success();

        ensure!(success, "editor exited with error");
        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn show_version(self) {
        println!("{}", get_version_string());
    }

    fn show_unmanaged_packages(mut self) -> Result<()> {
        let unmanaged_per_backend = &self.get_unmanaged_packages();

        unmanaged_per_backend
            .show()
            .context("printing things to do")
    }

    fn get_unmanaged_packages(&mut self) -> ToDoPerBackend {
        let mut result = ToDoPerBackend::new();

        for backend in Backends::iter() {
            let mut backend = self.overwrite_values_from_config(backend);

            backend.load(&self.groups);

            match backend.get_unmanaged_packages_sorted() {
                Ok(unmanaged) => result.push((backend, unmanaged)),
                Err(error) => show_error(&error, &*backend),
            };
        }
        result
    }

    fn show_groups(self) {
        let mut vec: Vec<_> = self.groups.iter().collect();
        vec.sort_unstable();
        for g in vec {
            println!("{}", g.name);
        }
    }

    fn clean_packages(mut self) -> Result<()> {
        let to_remove = self.get_unmanaged_packages();

        if to_remove.nothing_to_do_for_all_backends() {
            println!("nothing to do");
            return Ok(());
        }

        println!("Would remove the following packages:\n");
        to_remove.show().context("printing things to do")?;

        println!();
        if !get_user_confirmation()? {
            return Ok(());
        };

        to_remove.remove_unmanaged_packages()
    }

    fn show_group_content(&self, groups: &ArgMatches) -> Result<()> {
        let mut iter = groups
            .get_many::<String>("group")
            .context("getting groups from args")?
            .peekable();

        let show_more_than_one_group = iter.size_hint().0 > 1;

        while let Some(arg_group) = iter.next() {
            let group = self
                .groups
                .iter()
                .find(|g| g.name == *arg_group)
                .ok_or_else(|| anyhow!("group {} not found", *arg_group))?;

            if show_more_than_one_group {
                let name = &group.name;
                println!("{name}");
                for _ in 0..name.len() {
                    print!("-");
                }
                println!();
            }

            println!("{group}");
            if iter.peek().is_some() {
                println!();
            }
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn import_groups(&self, args: &ArgMatches) -> Result<()> {
        let files = args::get_absolutized_file_paths(args)?;
        let groups_dir = get_pacdef_group_dir()?;

        for target in files {
            let target_name = target
                .file_name()
                .context("path should not end in '..'")?
                .to_str()
                .context("filename is not valid UTF-8")?;

            if !target.exists() {
                eprintln!("file {target_name} does not exist, skipping");
                continue;
            }

            let mut link = groups_dir.clone();
            link.push(target_name);

            if link.exists() {
                eprintln!("group {target_name} already exists, skipping");
            } else {
                symlink(target, link)?;
            }
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn remove_groups(&self, arg_match: &ArgMatches) -> Result<()> {
        let paths = get_assumed_group_file_names(arg_match)?;

        for file in &paths {
            ensure!(file.exists(), "did not find the group under {file:?}");
        }

        for file in paths {
            remove_file(file)?;
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn new_groups(&self, arg: &ArgMatches) -> Result<()> {
        let paths = get_assumed_group_file_names(arg)?;

        for file in &paths {
            ensure!(!file.exists(), "group already exists under {file:?}");
        }

        for file in &paths {
            File::create(file)?;
        }

        if arg.get_flag("edit") {
            let success = run_edit_command(&paths)
                .context("running editor")?
                .success();

            ensure!(success, "editor exited with error");
        }

        Ok(())
    }
}

fn get_assumed_group_file_names(arg_match: &ArgMatches) -> Result<Vec<PathBuf>> {
    let groups_dir = get_pacdef_group_dir()?;

    let paths: Vec<_> = arg_match
        .get_many::<String>("groups")
        .context("getting groups from args")?
        .map(|s| {
            let mut possible_group_file = groups_dir.clone();
            possible_group_file.push(s);
            possible_group_file
        })
        .collect();

    Ok(paths)
}

#[allow(clippy::option_if_let_else)]
fn show_error(error: &anyhow::Error, backend: &dyn Backend) {
    let section = backend.get_section();
    match get_single_var("RUST_BACKTRACE") {
        Some(s) => {
            if s == "1" || s == "full" {
                eprintln!("WARNING: skipping backend '{section}':");
                for err in error.chain() {
                    eprintln!("  {err}");
                }
            }
        }
        None => eprintln!("WARNING: skipping backend '{section}': {error}"),
    }
}

pub const fn get_version_string() -> &'static str {
    concat!(
        "pacdef, version: ",
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("GIT_HASH"),
        ")",
    )
}
