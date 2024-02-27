use crate::backend::backend_trait::{Backend, Switches, Text};
use crate::backend::macros::impl_backend_constants;
use crate::{Group, Package};
use anyhow::Context;
use core::panic;
use std::collections::HashSet;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Rustup {
    pub(crate) packages: HashSet<Package>,
}

const BINARY: Text = "rustup";
const SECTION: Text = "rustup";

const SWITCHES_INSTALL: Switches = &["component", "add"];
const SWITCHES_INFO: Switches = &["component", "list", "--installed"];
const SWITCHES_MAKE_DEPENDENCY: Switches = &[];
const SWITCHES_NOCONFIRM: Switches = &[];
const SWITCHES_REMOVE: Switches = &["component", "remove"];

const SUPPORTS_AS_DEPENDENCY: bool = false;

impl Backend for Rustup {
    impl_backend_constants!();

    fn get_all_installed_packages(&self) -> anyhow::Result<HashSet<Package>> {
        let mut toolchains_vec = self
            .run_toolchain_command(&[&"toolchain", &"list"])
            .context("Getting installed toolchains")?;

        let mut toolchains: HashSet<Package> = toolchains_vec
            .iter()
            .map(|name| ["toolchain", name].join("/").into())
            .collect();

        let packages: HashSet<Package> = self
            .run_component_command(
                &[&"component", &"list", &"--installed", &"--toolchain"],
                &mut toolchains_vec,
            )
            .context("Getting installed components")?
            .iter()
            .map(|name| ["component", name].join("/").into())
            .collect();
        toolchains.extend(packages.into_iter());
        Ok(toolchains)
    }

    fn get_explicitly_installed_packages(&self) -> anyhow::Result<HashSet<Package>> {
        self.get_all_installed_packages()
            .context("Getting all installed packages")
    }

    fn make_dependency(&self, _: &[Package]) -> anyhow::Result<std::process::ExitStatus> {
        panic!("Not supported by {}", BINARY)
    }

    fn install_packages(
        &self,
        packages: &[Package],
        _noconfirm: bool,
    ) -> anyhow::Result<std::process::ExitStatus> {
        let mut result: anyhow::Result<std::process::ExitStatus> =
            Ok(std::process::ExitStatus::from_raw(0));
        for p in packages {
            let repo = p
                .repo
                .as_ref()
                .expect("Not specified whether it is a toolchain or a component!");
            if repo == "toolchain" {
                let mut cmd = Command::new(self.get_binary());
                cmd.args(&[&"toolchain", &"install"]);
                cmd.arg(format!("{}", p.name));
                result = cmd.status().context("Installing toolchain {p}");
                if !result.as_ref().is_ok_and(|exit| exit.success()) {
                    return result;
                }
            };
        }
        for p in packages {
            let repo = p
                .repo
                .as_ref()
                .expect("Not specified wether it is a component or a toolchain!");
            if repo == "component" {
                let mut iter = p.name.split('/');
                let toolchain = iter.next().expect("Toolchain not specified!");
                let component = iter.next().expect("Component not specified!");
                let mut cmd = Command::new(self.get_binary());
                cmd.args(&[&"component", &"add"]);
                cmd.args([&"--toolchain", format!("{toolchain}").as_str()]);
                cmd.arg(format!("{component}"));
                result = cmd.status().context("Installing component {p}");
                if !result.as_ref().is_ok_and(|exit| exit.success()) {
                    return result;
                }
            }
        }
        result
    }
}

impl Rustup {
    pub(crate) fn new() -> Self {
        Self {
            packages: HashSet::new(),
        }
    }

    fn run_component_command(
        &self,
        args: &[&str],
        toolchains: &mut Vec<String>,
    ) -> Result<Vec<String>, anyhow::Error> {
        let mut val = Vec::new();
        for toolchain in toolchains {
            let mut cmd = Command::new(self.get_binary());
            cmd.args(args);
            cmd.arg(&toolchain);
            let output = String::from_utf8(cmd.output()?.stdout)?;
            for i in output.lines() {
                let mut it = i.splitn(3, "-");
                let component = it.next().expect("Component name is empty!");
                match component {
                    "cargo" | "rustfmt" | "clippy" | "miri" | "rls" | "rustc" => {
                        val.push([toolchain, component].join("/"));
                    }
                    _ => {
                        let component = [
                            component,
                            it.next().expect("No such component is managed by rustup"),
                        ]
                        .join("-");
                        val.push([toolchain, component.as_str()].join("/"));
                    }
                }
            }
        }
        Ok(val)
    }
    fn run_toolchain_command(&self, args: &[&str]) -> Result<Vec<String>, anyhow::Error> {
        let mut cmd = Command::new(self.get_binary());
        cmd.args(args);
        let output = String::from_utf8(cmd.output()?.stdout)?;
        let mut val = Vec::new();
        for i in output.lines() {
            let mut it = i.splitn(2, "-");
            val.push(it.next().expect("Toolchain name is empty.").to_string());
        }
        Ok(val)
    }
}
