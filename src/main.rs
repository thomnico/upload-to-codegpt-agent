use keyring::Entry;
use reqwest;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use toml;

struct FileInfo {
    last_modified: SystemTime,
    plug_id: Option<String>,
}

#[derive(Deserialize)]
struct Config {
    directories: Vec<String>,
    file_types: Vec<String>,
}

fn get_api_key() -> Result<String, Box<dyn std::error::Error>> {
    let entry = Entry::new("codegpt", "api_key")?;
    match entry.get_password() {
        Ok(password) => Ok(password),
        Err(_) => {
            eprintln!("API key not found in keyring. Please set it first.");
            Err("API key not found".into())
        }
    }
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_str = fs::read_to_string("config.toml")?;
    Ok(toml::from_str(&config_str)?)
}

fn is_source_file(path: &Path, file_types: &[String]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| file_types.contains(&ext.to_string()))
        .unwrap_or(false)
}

fn scan_directory(
    dir: &Path,
    files: &mut Vec<PathBuf>,
    file_types: &[String],
) -> std::io::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                scan_directory(&path, files, file_types)?;
            } else if is_source_file(&path, file_types) {
                files.push(path);
            }
        }
    }
    Ok(())
}

async fn upload_and_plug_file(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    filename: &str,
    content: &str,
    plug_id: Option<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let upload_response = client
        .post(format!("{}/agents/files", base_url))
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&json!({
            "name": filename,
            "content": content
        }))
        .send()
        .await?;

    println!("Uploaded {}: {:?}", filename, upload_response.status());

    let file_id = upload_response.json::<serde_json::Value>().await?["id"]
        .as_str()
        .unwrap()
        .to_string();

    let plug_response = if let Some(existing_plug_id) = plug_id {
        client
            .put(format!("{}/agents/plugs/{}", base_url, existing_plug_id))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&json!({
                "file_id": file_id
            }))
            .send()
            .await?
    } else {
        client
            .post(format!("{}/agents/plugs", base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&json!({
                "name": filename,
                "file_id": file_id
            }))
            .send()
            .await?
    };

    println!("Plugged {}: {:?}", filename, plug_response.status());

    Ok(plug_response.json::<serde_json::Value>().await?["id"]
        .as_str()
        .unwrap()
        .to_string())
}

async fn upload_modified_files(
    directories: &[String],
    api_key: &str,
    last_check: &mut HashMap<String, FileInfo>,
    file_types: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let base_url = "https://api.codegpt.co/v1";
    let mut files = Vec::new();

    for dir in directories {
        scan_directory(Path::new(dir), &mut files, file_types)?;
    }

    for path in files {
        let filename = path.to_str().unwrap().to_string();
        let metadata = fs::metadata(&path)?;
        let modified = metadata.modified()?;

        if !last_check.contains_key(&filename) || last_check[&filename].last_modified < modified {
            let content = fs::read_to_string(&path)?;

            let plug_id = upload_and_plug_file(
                &client,
                base_url,
                api_key,
                &filename,
                &content,
                last_check
                    .get(&filename)
                    .and_then(|info| info.plug_id.clone()),
            )
            .await?;

            last_check.insert(
                filename,
                FileInfo {
                    last_modified: modified,
                    plug_id: Some(plug_id),
                },
            );
        }
    }

    Ok(())
}


use tokio::time::{sleep, Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config()?;
    let api_key = get_api_key()?;
    let mut last_check: HashMap<String, FileInfo> = HashMap::new();
    loop {
        match upload_modified_files(
            &config.directories,
            &api_key,
            &mut last_check,
            &config.file_types,
        )
        .await
        {
            Ok(_) => {
                sleep(Duration::from_secs(60)).await; // Check every minute
            }
            Err(e) => {
                eprintln!("Error occurred: {:?}", e);
                // Optionally, add a delay before retrying or break the loop
                sleep(Duration::from_secs(10)).await;
                // If you want to exit on error, uncomment the next line:
                // return Err(e);
            }
        }
    }
}
