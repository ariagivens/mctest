mod minecraft_client;
mod minecraft_server;
mod test;

use std::path::PathBuf;
use clap::Parser;
use anyhow::Result;
use uuid::Uuid;

use minecraft_client::MinecraftClient;
use minecraft_server::MinecraftServer;
use test::run_tests;

#[derive(Parser)]
struct Args {
    datapack_path: PathBuf
}

// Java incorrectly builds V3 uuids by ignoring the need for a namespace.
// We have to follow what it does to stay compatible.
// Algorithm from https://gist.github.com/yushijinhun/69f68397c5bb5bee76e80d192295f6e0
fn offline_player_uuid(name: &str) -> Uuid {
    let mut hash: [u8; 16] = md5::compute(format!("OfflinePlayer:{name}")).into();
    hash[6] = hash[6] & 0x0f | 0x30; // Set version to 3
    hash[8] = hash[8] & 0x3f | 0x80; // Set variant to IETF
    Uuid::from_bytes(hash)
}

fn main() -> Result<()> {
    let Args { datapack_path } = Args::parse();
    let uuid = offline_player_uuid("player");
    let server = MinecraftServer::new("1.20.2", uuid, &datapack_path)?;
    let server = server.start()?;
    let client = MinecraftClient::new("player", uuid);
    let (reader, writer) = client.connect_to(&server)?.split();
    
    println!("TAP version 14");
    run_tests(reader, writer)?;

    Ok(())
}