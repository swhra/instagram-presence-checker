#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! base64 = "0.22"
//! chrono = "0.4"
//! regex = "1.12"
//! serde_json = "1.0"
//! tungstenite = { version = "0.24", features = ["native-tls"] }
//! ureq = "3.3"
//! ```

use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::thread;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as Base64StandardEngine, Engine};
use chrono::{offset::LocalResult, Local, TimeZone};
use regex::Regex;
use serde_json::Value;
use tungstenite::{client::IntoClientRequest, connect, Error::Io as WSError, http::header::HeaderName, Message, stream::MaybeTlsStream};

macro_rules! verbose {
    ($($arg:tt)*) => {{
        if std::io::stderr().is_terminal() {
            eprintln!("\x1b[2m{}\x1b[0m", format_args!($($arg)*));
        } else { eprintln!($($arg)*); }
    }};
}

const CREDS_PATH: &str = "credentials.json";
const LOG_PATH: &str   = "/tmp/instagram.log";
const TARGET_ID: &str  = "101046441298018";

const MONITOR_ERR_WAIT_S: Duration = Duration::from_secs(2);

fn main() {
    loop {
        if let Err(e) = run_monitor() {
            eprintln!("{}", e);

            verbose!("waiting {}s", MONITOR_ERR_WAIT_S.as_secs());
            thread::sleep(MONITOR_ERR_WAIT_S);
            verbose!("waited, restarting");
        }
    }
}

fn run_monitor() -> Result<(), Box<dyn std::error::Error>> {
    let data = fs::read_to_string(CREDS_PATH)?;
    let creds: Value = serde_json::from_str(&data)?;
    let ua = creds["headers"]["user-agent"].as_str().unwrap_or_default();

    verbose!("refreshing WSS URL");
    let res = ureq::get("https://www.instagram.com/direct/inbox/")
        .header("Cookie", creds["headers"]["cookie"].as_str().unwrap_or_default())
        .header("User-Agent", ua)
        .call()?.body_mut().read_to_string()?;

    let d_id = Regex::new(r#""device_id":"(.*?)"#)?.captures(&res).ok_or("no `device_id` in Instagram HTML response")?[1].to_string();
    let u_id = Regex::new(r#""NON_FACEBOOK_USER_ID":"(.*?)"#)?.captures(&res).ok_or("no `NON_FACEBOOK_USER_ID` in Instagram HTML response")?[1].to_string();
    let wss_url = format!("wss://gateway.instagram.com/ws/streamcontroller?x-dgw-appid=936619743392459&x-dgw-appversion=0&x-dgw-authtype=6:0&x-dgw-version=5&x-dgw-uuid={u_id}&x-dgw-tier=prod&x-dgw-deviceid={d_id}&x-dgw-app-stream-group=group1");

    let mut request = wss_url.into_client_request()?;
    let headers = request.headers_mut();
    if let Some(h_map) = creds["headers"].as_object() {
        for (k, v) in h_map {
            let kl = k.to_lowercase();
            if kl.starts_with("sec-websocket") || kl == "host" || kl == "upgrade" || kl == "connection" { continue; }
            if let Some(vs) = v.as_str() {
                headers.insert(HeaderName::from_bytes(k.as_bytes())?, vs.parse()?);
            }
        }
    }
    headers.insert("Origin", "https://www.instagram.com".parse()?);

    let (mut socket, _) = connect(request)?;
    
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp_s)     => tcp_s          .set_read_timeout(Some(Duration::from_secs(1)))?,
        MaybeTlsStream::NativeTls(tls_s) => tls_s.get_mut().set_read_timeout(Some(Duration::from_secs(1)))?,
    
        &mut _ => unimplemented!("we thought this variable of type `MaybeTlsStream` might be one of the actually existing variants of the enum `MaybeTlsStream`, but it was in fact some other sort of thing, perhaps a manatee or an aeroplane, and I am required to handle this error path")
    }

    verbose!("ws connected");
    verbose!("replaying handshake");

    let mut additional_contacts_packet = None;
    let mut foreground_packet = None;

    if let Some(packets) = creds["packets"].as_array() {
        for p_b64 in packets {
            let p_raw = Base64StandardEngine.decode(p_b64.as_str().unwrap())?;
            if p_raw.contains(&b'{') {
                socket.send(Message::Binary(p_raw.clone().into()))?;
                
                let s = String::from_utf8_lossy(&p_raw);

                // isolate additionalContacts packet
                if s.contains("additionalContacts") {
                    additional_contacts_packet = Some(p_raw.clone());
                }
                // isolate foreground packet
                if s.contains("foreground") || s.contains("is_foreground") {
                    foreground_packet = Some(p_raw.clone());
                }
            }
        }
    }

    let additional_contacts_packet = additional_contacts_packet.expect("no additionalContacts packet received");
    let foreground_packet = foreground_packet.expect("no foreground packet received");

    let mut last_heartbeat = Instant::now();
    let mut log_file = OpenOptions::new().append(true).create(true).open(LOG_PATH)?;
    loop {
        use io::ErrorKind::*;

        match socket.read() {
            Ok(Message::Binary(bin)) => {
                verbose!("heartbeat: binary response received: {:?}", bin);

                if bin.len() >= 7 && bin[0] == 13 {
                    socket.send(Message::Binary(vec![12, bin[1], 0, 2, 0, bin[5], bin[6], 0].into()))?;
                }

                // brace-counting JSON extractor handles multiple JSON objects in one packet
                let raw = String::from_utf8_lossy(&bin);
                let mut depth = 0;
                let mut start = None;

                for (i, c) in raw.char_indices() {
                    if c == '{' {
                        if depth == 0 { start = Some(i); }
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            if let Some(s) = start {
                                if let Ok(json) = serde_json::from_str::<Value>(&raw[s..=i]) {
                                    process_message(&json, &mut log_file)?;
                                }
                                start = None;
                            }
                        }
                    }
                }
            },
            Ok(Message::Close(_)) => return Err("ws close".into()),
            Ok(Message::Ping(_)) => { verbose!("heartbeat: ws ping received"); },
            Ok(Message::Pong(_)) => { verbose!("heartbeat: ws pong received"); },
            Ok(Message::Text(text)) => { verbose!("heartbeat: ws: somewhat unexpected text message: {}", text); }
            Ok(_) => (),

            Err(WSError(ref e)) => match e.kind() {
                // no data
                ResourceBusy | TimedOut | UnexpectedEof | WouldBlock => { verbose!("heartbeat: no data, keep waiting"); },
    
                // transient, ignore
                Interrupted | ConnectionRefused | AddrNotAvailable | ConnectionReset | ConnectionAborted | BrokenPipe | NotConnected | HostUnreachable | NetworkUnreachable | NetworkDown => { eprintln!("heartbeat: transient error: {}", e); },

                // nasal demons, crash & probably crashloop because fuck knows what else to do
                //   internet
                NotFound | AddrInUse | AlreadyExists | InvalidInput | InvalidData | Unsupported |WriteZero | StaleNetworkFileHandle | Deadlock => return Err(format!("network error: {}", e).into()),
                //   filesystem (shouldn't happen for an IP socket)
                NotADirectory | IsADirectory | DirectoryNotEmpty | CrossesDevices | TooManyLinks | InvalidFilename | ReadOnlyFilesystem => return Err(format!("very weird fs error: {}",           e).into()),
                PermissionDenied =>   return Err(format!("very weird fs/privilege error: {}", e).into()),
                //   resource exhaustion
                OutOfMemory | StorageFull | QuotaExceeded |FileTooLarge => return Err(format!("resource exhausted error: {}", e).into()),
                //   incomprehensible, god knows
                ExecutableFileBusy | ArgumentListTooLong => return Err(format!("incomprehensible error: {}", e).into()),
                //   see earlier manatee comment
                _ => unimplemented!("oh no - something that was not, at the time of writing, a variant of enum `std::io::ErrorKind` became one, and whoever compiled this code did not notice because Rust's stupid `#[non-exhaustive]` macro made me add this wildcard match arm")
            },
            Err(e) => return Err(e.into()),
        }

        let elapsed = last_heartbeat.elapsed().as_secs_f32();
        if elapsed > 5f32 {
            verbose!("heartbeat: send IG (foreground, additionalContacts) and WS pings");

            // IG pings
            socket.send(Message::Binary(additional_contacts_packet.clone().into()))?;
            socket.send(Message::Binary(foreground_packet.clone().into()))?;
            
            // WS ping
            socket.send(Message::Binary(vec![9].into()))?;
            
            last_heartbeat = Instant::now();
        }
    }
}

fn process_message(json: &Value, log_file: &mut fs::File) -> Result<(), Box<dyn std::error::Error>> {
    // 1. real-time updates
    if let Some(updates) = json["presenceUpdates"].as_array() {
        for u in updates {
            log_presence(u, log_file)?;
        }
    }
    
    // 2. initial sync
    if let Some(contacts) = json["additionalContacts"]["additionalContacts"].as_array() {
        for c in contacts {
            log_presence(c, log_file)?;
        }
    }

    // 3. unified presence format
    if let Some(updates) = json["PresenceUnifiedJSON"]["presence_updates"].as_array() {
        for u in updates {
            log_presence(u, log_file)?;
        }
    }

    Ok(())
}

fn log_presence(u: &Value, log_file: &mut fs::File) -> Result<(), Box<dyn std::error::Error>> {
    let uid = u["userId"].to_string().replace('"', "");
    if uid == TARGET_ID {
        let status = &u["presenceStatus"];
        let ts = u["lastActiveTimeSeconds"].as_i64().unwrap_or(0);
        let now = Local::now();
        let seen = match Local.timestamp_opt(ts, 0) {
            LocalResult::Single(t) => t,
            LocalResult::Ambiguous(t1, t2) => {
                eprintln!("ambiguous timestamp {} ({}, {}), taking earlier time", ts, t1, t2);
                t1
            },
            LocalResult::None => return Err(format!("invalid timestamp {}", ts).into())
        };
        
        let now_s = now.format("%Y-%m-%d %H:%M:%S");
        let seen_s = if now.date_naive() == seen.date_naive() {
            seen.format("%H:%M:%S")
        } else {
            seen.format("%Y-%m-%d %H:%M:%S")
        };
        
        let log_line = format!("{}: {}: {}: seen at {}\n", now_s, uid, status, seen_s);
        let log_line = log_line.as_bytes();
        
        io::stdout().lock().write(log_line)?;
        log_file.write_all(log_line)?;
    }
    Ok(())
}