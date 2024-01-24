use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs::{File, self};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use tempdir::TempDir;
use fs_extra::dir::CopyOptions;
use uuid::Uuid;

// TODO: Implement app-cds https://nipafx.dev/java-application-class-data-sharing/

static DIRECTORIES: Lazy<ProjectDirs> = Lazy::new(|| {
    let directories =
        ProjectDirs::from("", "", "mctest").expect("Failed to load application directory.");
    fs::create_dir_all(directories.cache_dir())
        .expect("Failed to load application cache directory.");
    directories
});

pub struct MinecraftServer {
    dir: TempDir,
    port: u16,
}

impl MinecraftServer {
    pub fn new(version: &str, uuid: Uuid, datapack_path: &Path) -> Result<Self> {
        let server_dir = TempDir::new("mctest")?;
        let port = find_port()?;
        setup_server_dir(version, &server_dir, port, uuid, datapack_path)?;
        Ok(MinecraftServer {
            dir: server_dir,
            port,
        })
    }

    pub fn start(self) -> Result<RunningMinecraftServer> {
        let mut process = Command::new("java")
            .current_dir(&self.dir.path())
            .args(["-Xshare:on", "-jar", "server.jar", "--nogui"])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()?;
        wait_for_load(&mut process)?;
        Ok(RunningMinecraftServer {
            _dir: self.dir,
            process,
            port: self.port,
        })
    }
}

fn find_port() -> Result<u16> {
    let listener = TcpListener::bind(("localhost", 0))?;
    Ok(listener.local_addr()?.port())
}

fn setup_server_dir(version: &str, server_dir: &TempDir, port: u16, uuid: Uuid, datapack_path: &Path) -> Result<()> {
    write_eula(server_dir)?;
    write_server_properties(server_dir, port)?;
    write_ops(server_dir, uuid)?;
    copy_datapack(server_dir, datapack_path)?;
    let jar = retrieve_jar(version)?;
    fs::write(server_dir.path().join("server.jar"), jar)?;
    Ok(())
}

fn write_eula(server_dir: &TempDir) -> Result<()> {
    fs::write(server_dir.path().join("eula.txt"), "eula=true\n")?;
    Ok(())
}

fn write_server_properties(server_dir: &TempDir, port: u16) -> Result<()> {
    let mut content = String::new();
    content.push_str(&format!("server-port={port}\n"));
    content.push_str("online-mode=false\n");
    content.push_str("network-compression-threshold=-1\n");
    content.push_str("enforce-secure-profile=false\n");
    content.push_str("level-type=flat\n");
    content.push_str(r#"generator-settings={"biome":"minecraft:desert","layers":[{"block":"minecraft:bedrock","height":1}, {"block":"minecraft:sandstone","height":15}]}"#);
    fs::write(server_dir.path().join("server.properties"), content)?;
    Ok(())
}

fn write_ops(server_dir: &TempDir, uuid: Uuid) -> Result<()> {
    let mut file = File::create(server_dir.path().join("ops.json"))?;
    writeln!(file, r#"[ {{ "uuid": "{uuid}""#)?;
    writeln!(file, r#"  , "name": "player""#)?;
    writeln!(file, r#"  , "level": 4"#)?;
    writeln!(file, r#"  , "bypassesPlayerLimit": false"#)?;
    writeln!(file, r#"  }}"#)?;
    writeln!(file, r#"]"#)?;
    file.flush()?;
    Ok(())
}

fn copy_datapack(server_dir: &TempDir, datapack_path: &Path) -> Result<()> {
    let path = server_dir.path().join("world/datapacks");
    fs::create_dir_all(&path)?;
    if datapack_path.is_dir() {
        fs_extra::dir::copy(datapack_path, &path, &CopyOptions::default())?;
    } else {
        fs::copy(datapack_path, path.join(datapack_path.file_name().unwrap_or(OsStr::new("pack.zip"))))?;
    }
    Ok(())
}

fn retrieve_jar(version_id: &str) -> Result<Vec<u8>> {
    if let Some(jar) = read_jar_from_cache(version_id) {
        Ok(jar)
    } else {
        let jar = download_jar(version_id)?;
        fs::write(
            DIRECTORIES
                .cache_dir()
                .join(format!("server-{version_id}.jar")),
            &jar,
        )?;
        Ok(jar)
    }
}

fn read_jar_from_cache(version_id: &str) -> Option<Vec<u8>> {
    fs::read(
        DIRECTORIES
            .cache_dir()
            .join(format!("server-{version_id}.jar")),
    )
    .ok()
}

const PISTON_META: &str = "https://piston-meta.mojang.com";
fn download_jar(version_id: &str) -> Result<Vec<u8>> {
    let version_manifest = retrieve_version_manifest()?;
    let versions: &Vec<Value> = version_manifest["versions"]
        .as_array()
        .ok_or(anyhow!("Unexpected version manifest format"))?;
    let version = versions
        .iter()
        .find(|v| v["id"].as_str() == Some(version_id))
        .ok_or(anyhow!("Could not identify version: {}", version_id))?;
    let meta_data_url = version["url"]
        .as_str()
        .ok_or(anyhow!("Unexpected version manifest format"))?;
    let meta_data: Value = reqwest::blocking::get(meta_data_url)?.json()?;
    let jar_path = meta_data["downloads"]["server"]["url"]
        .as_str()
        .ok_or(anyhow!("Unexpected version meta data format"))?;
    Ok(reqwest::blocking::get(jar_path)?.bytes()?.into())
}

fn retrieve_version_manifest() -> Result<Value> {
    Ok(
        reqwest::blocking::get(&format!("{PISTON_META}/mc/game/version_manifest_v2.json"))?
            .json()?,
    )
}

#[cfg(not(debug_assertions))]
fn wait_for_load(process: &mut Child) -> Result<()> {
    let stdout = process
        .stdout
        .take()
        .ok_or(anyhow!("Failed to read stdout of minecraft server"))?;
    let reader = BufReader::new(stdout);

    let regex = Regex::new(".*Done.*")?;
    for line in reader.lines() {
        let line = line?;

        if regex.is_match(&line) {
            break;
        }
    }

    Ok(())
}

#[cfg(debug_assertions)]
fn wait_for_load(process: &mut Child) -> Result<()> {
    use std::sync::mpsc::channel;

    let stdout = process
        .stdout
        .take()
        .ok_or(anyhow!("Failed to read stdout of minecraft server"))?;
    let reader = BufReader::new(stdout);

    let (sender, receiver) = channel();

    std::thread::spawn(move || {
        let regex = Regex::new(".*Done.*").expect("Failed to compile regex");
        for line in reader.lines() {
            if let Ok(line) = line {
                println!("{line}");

                if regex.is_match(&line) {
                    sender.send(()).expect("Failed to send done signal");
                }
            } else {
                println!("Failed to read");
            }
        }
    });

    receiver.recv()?;

    Ok(())
}

pub struct RunningMinecraftServer {
    _dir: TempDir,
    process: Child,
    port: u16,
}

impl RunningMinecraftServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    fn stop(&mut self) {
        // println!("Server stopped");

        if self.try_stop_gracefully().is_err() {
            self.process.kill().ok();
        }
    }

    fn try_stop_gracefully(&mut self) -> Result<()> {
        let mut stdin = self
            .process
            .stdin
            .take()
            .ok_or(anyhow!("Failed to access server stdin"))?;
        writeln!(stdin, "stop")?;
        Ok(())
    }
}

impl Drop for RunningMinecraftServer {
    fn drop(&mut self) {
        self.stop()
    }
}
