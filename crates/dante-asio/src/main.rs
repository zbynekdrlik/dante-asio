// SPDX-License-Identifier: GPL-3.0-or-later

//! `dante-asio run --instance <n> --config <device.toml> --ip <address>`:
//! one instance's Dante device and its shared memory for the driver.
//! `dante-asio register --instance <n> --dll <path>` and
//! `dante-asio unregister --instance <n>`: the driver's registry entries
//! (administrator).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: dante-asio run --instance <n> --config <device.toml> --ip <address>\n       \
         dante-asio register --instance <n> --dll <path>\n       \
         dante-asio unregister --instance <n>"
    );
    ExitCode::from(2)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        return usage();
    };
    let Some(instance) = flag(&args, "--instance").and_then(|n| n.parse::<u32>().ok()) else {
        return usage();
    };
    let result: Result<(), String> = match cmd.as_str() {
        "register" => match flag(&args, "--dll")
            .and_then(|dll| dante_asio::register::entries(instance, &dll))
        {
            Some(entries) => apply(&entries),
            None => return usage(),
        },
        "unregister" => match dante_asio::register::keys(instance) {
            Some(keys) => remove(&keys),
            None => return usage(),
        },
        "run" => {
            let (Some(config), Some(ip)) = (
                flag(&args, "--config"),
                flag(&args, "--ip").and_then(|ip| ip.parse().ok()),
            ) else {
                return usage();
            };
            run(instance, config.into(), ip)
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dante-asio: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn apply(entries: &[dante_asio::register::Entry]) -> Result<(), String> {
    dante_asio::register::apply(entries).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn remove(keys: &[String]) -> Result<(), String> {
    dante_asio::register::remove(keys).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn run(instance: u32, config: std::path::PathBuf, ip: std::net::Ipv4Addr) -> Result<(), String> {
    dante_asio::run::run(instance, config, ip)
}

#[cfg(not(windows))]
fn apply(_entries: &[dante_asio::register::Entry]) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn remove(_keys: &[String]) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn run(_instance: u32, _config: std::path::PathBuf, _ip: std::net::Ipv4Addr) -> Result<(), String> {
    Err("Windows only".into())
}
