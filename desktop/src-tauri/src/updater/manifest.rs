use super::model::{ClientManifest, TorrentFile};
use reqwest::{blocking::Client, redirect::Policy, Url};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, io::Read, time::Duration};

pub const CLIENT_TORRENT_URL: &str = "https://dl.octowow.st/download/client.torrent";
const UPDATER_USER_AGENT: &str = "OctoUpdater/1.3.1";
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const ALLOWED_MANIFEST_HOSTS: &[&str] = &["dl.octowow.st"];

pub struct FetchedManifest {
    pub manifest: ClientManifest,
    pub raw: Vec<u8>,
}

pub fn fetch_client_manifest() -> Result<FetchedManifest, String> {
    fetch_manifest(CLIENT_TORRENT_URL)
}

fn fetch_manifest(url: &str) -> Result<FetchedManifest, String> {
    validate_manifest_url(
        &Url::parse(url).map_err(|error| format!("Invalid manifest URL: {error}"))?,
    )?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(UPDATER_USER_AGENT)
        .redirect(Policy::custom(|attempt| {
            match validate_manifest_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(message) => attempt.error(message),
            }
        }))
        .build()
        .map_err(|error| format!("Could not build secure HTTP client: {error}"))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(describe_request_error)?
        .error_for_status()
        .map_err(|error| format!("Client manifest server returned an error: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err("Client manifest exceeds the 8 MiB safety limit.".into());
    }

    let mut raw = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|error| format!("Could not read client manifest: {error}"))?;
        if count == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..count]);
        if raw.len() > MAX_MANIFEST_BYTES {
            return Err("Client manifest exceeds the 8 MiB safety limit.".into());
        }
    }

    let files = parse_torrent(&raw)?;
    let total_bytes = files
        .iter()
        .try_fold(0_u64, |total, file| total.checked_add(file.length))
        .ok_or_else(|| "Client manifest total size overflows a 64-bit integer.".to_string())?;
    let sha256 = format!("{:x}", Sha256::digest(&raw));
    Ok(FetchedManifest {
        manifest: ClientManifest {
            source_url: url.into(),
            sha256,
            total_bytes,
            files,
        },
        raw,
    })
}

fn describe_request_error(error: reqwest::Error) -> String {
    let category = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else if error.is_request() {
        "could not send the request"
    } else {
        "request failed"
    };

    let mut details = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        details.push_str(": ");
        details.push_str(&cause.to_string());
        source = cause.source();
    }
    format!("Could not retrieve client manifest ({category}): {details}")
}

fn validate_manifest_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err("Refusing non-HTTPS client manifest URL.".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !ALLOWED_MANIFEST_HOSTS.contains(&host.as_str()) {
        return Err(format!(
            "Refusing client manifest from unexpected host: {host}"
        ));
    }
    Ok(())
}

#[derive(Debug)]
enum BValue {
    Integer(i64),
    Bytes(Vec<u8>),
    List(Vec<BValue>),
    Dictionary(BTreeMap<Vec<u8>, BValue>),
}

fn parse_torrent(raw: &[u8]) -> Result<Vec<TorrentFile>, String> {
    let mut offset = 0;
    let root = parse_value(raw, &mut offset, 0)?;
    if offset != raw.len() {
        return Err("Client manifest contains trailing bencode data.".into());
    }
    let root = dictionary(&root, "torrent root")?;
    let info = dictionary(required(root, b"info", "torrent root")?, "torrent info")?;

    let files = match info.get(b"files".as_slice()) {
        Some(BValue::List(entries)) => entries
            .iter()
            .map(parse_file)
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("Client manifest has an invalid files list.".into()),
        None => vec![TorrentFile {
            path: safe_component(bytes(
                required(info, b"name", "torrent info")?,
                "torrent name",
            )?)?,
            length: length(required(info, b"length", "torrent info")?)?,
        }],
    };
    if files.is_empty() {
        return Err("Client manifest does not describe any files.".into());
    }
    Ok(files)
}

fn parse_file(value: &BValue) -> Result<TorrentFile, String> {
    let entry = dictionary(value, "torrent file entry")?;
    let components = match required(entry, b"path", "torrent file entry")? {
        BValue::List(parts) if !parts.is_empty() => parts
            .iter()
            .map(|part| safe_component(bytes(part, "torrent path component")?))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("Client manifest contains an invalid file path.".into()),
    };
    Ok(TorrentFile {
        path: components.join("/"),
        length: length(required(entry, b"length", "torrent file entry")?)?,
    })
}

fn parse_value(input: &[u8], offset: &mut usize, depth: usize) -> Result<BValue, String> {
    if depth > 64 {
        return Err("Client manifest exceeds the bencode nesting limit.".into());
    }
    let token = *input
        .get(*offset)
        .ok_or_else(|| "Unexpected end of client manifest.".to_string())?;
    match token {
        b'i' => {
            *offset += 1;
            let start = *offset;
            while input.get(*offset).is_some_and(|byte| *byte != b'e') {
                *offset += 1;
            }
            let text = std::str::from_utf8(
                input
                    .get(start..*offset)
                    .ok_or_else(|| "Unterminated bencode integer.".to_string())?,
            )
            .map_err(|_| "Client manifest contains a non-UTF8 integer.".to_string())?;
            *offset += 1;
            Ok(BValue::Integer(text.parse().map_err(|_| {
                "Client manifest contains an invalid integer.".to_string()
            })?))
        }
        b'l' => {
            *offset += 1;
            let mut values = Vec::new();
            while input.get(*offset).is_some_and(|byte| *byte != b'e') {
                values.push(parse_value(input, offset, depth + 1)?);
            }
            *offset += 1;
            Ok(BValue::List(values))
        }
        b'd' => {
            *offset += 1;
            let mut values = BTreeMap::new();
            while input.get(*offset).is_some_and(|byte| *byte != b'e') {
                let key = match parse_value(input, offset, depth + 1)? {
                    BValue::Bytes(key) => key,
                    _ => return Err("Client manifest has a non-string dictionary key.".into()),
                };
                let value = parse_value(input, offset, depth + 1)?;
                if values.insert(key, value).is_some() {
                    return Err("Client manifest contains a duplicate dictionary key.".into());
                }
            }
            *offset += 1;
            Ok(BValue::Dictionary(values))
        }
        b'0'..=b'9' => {
            let start = *offset;
            while input.get(*offset).is_some_and(|byte| byte.is_ascii_digit()) {
                *offset += 1;
            }
            if input.get(*offset) != Some(&b':') {
                return Err("Client manifest contains an invalid byte string length.".into());
            }
            let length: usize = std::str::from_utf8(&input[start..*offset])
                .ok()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| {
                    "Client manifest contains an invalid byte string length.".to_string()
                })?;
            *offset += 1;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| "Client manifest string length overflows.".to_string())?;
            let value = input
                .get(*offset..end)
                .ok_or_else(|| "Client manifest ends inside a byte string.".to_string())?
                .to_vec();
            *offset = end;
            Ok(BValue::Bytes(value))
        }
        _ => Err("Client manifest contains an unsupported bencode value.".into()),
    }
}

fn dictionary<'a>(
    value: &'a BValue,
    context: &str,
) -> Result<&'a BTreeMap<Vec<u8>, BValue>, String> {
    match value {
        BValue::Dictionary(values) => Ok(values),
        _ => Err(format!("Client manifest has an invalid {context}.")),
    }
}
fn required<'a>(
    dictionary: &'a BTreeMap<Vec<u8>, BValue>,
    key: &[u8],
    context: &str,
) -> Result<&'a BValue, String> {
    dictionary.get(key).ok_or_else(|| {
        format!(
            "Client manifest is missing {} in {context}.",
            String::from_utf8_lossy(key)
        )
    })
}
fn bytes<'a>(value: &'a BValue, context: &str) -> Result<&'a [u8], String> {
    match value {
        BValue::Bytes(value) => Ok(value),
        _ => Err(format!("Client manifest has an invalid {context}.")),
    }
}
fn length(value: &BValue) -> Result<u64, String> {
    match value {
        BValue::Integer(value) if *value >= 0 => Ok(*value as u64),
        _ => Err("Client manifest contains an invalid file length.".into()),
    }
}
fn safe_component(raw: &[u8]) -> Result<String, String> {
    let component = std::str::from_utf8(raw)
        .map_err(|_| "Client manifest contains a non-UTF8 path component.".to_string())?;
    if component.is_empty()
        || matches!(component, "." | "..")
        || component.contains(['/', '\\', '\0', ':'])
    {
        return Err("Client manifest contains an unsafe file path.".into());
    }
    Ok(component.into())
}
