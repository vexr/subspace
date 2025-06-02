//! Subspace gateway implementation.

#![feature(iterator_try_collect)]

mod commands;
mod node_client;
mod piece_getter;
mod piece_validator;

use crate::commands::{Command, raise_fd_limit, set_exit_on_panic};
use clap::Parser;
use subspace_logging::init_logger;
use tracing::info;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    set_exit_on_panic();
    init_logger();
    raise_fd_limit();

    info!("Subspace Gateway");
    info!("✌️  version {}", env!("CARGO_PKG_VERSION"));
    info!("❤️  by {}", env!("CARGO_PKG_AUTHORS"));

    let command = Command::parse();

    match command {
        Command::Rpc(run_options) => {
            commands::rpc::run(run_options).await?;
        }
        Command::Http(run_options) => {
            commands::http::run(run_options).await?;
        }
    }
    Ok(())
}
