mod api;
mod cli;
mod crypto;
mod error;
mod jd;
mod store;

use serde_json::{Value, json};

use api::{Api, CloudAccount, CloudApi, LocalApi};
use cli::{Command, Invocation, Target};
use error::Error;
use store::{Connection, Store};

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let code = match cli::parse(args).and_then(run) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("jdctl: {error}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}

fn run(invocation: Invocation) -> Result<(), Error> {
    let Invocation {
        connection,
        json,
        command,
    } = invocation;
    match command {
        Command::Help => {
            print!("{}", cli::HELP);
            Ok(())
        }
        Command::Version => {
            println!("jdctl {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::ConnectLocal { name, url, default } => connect_local(&name, &url, default),
        Command::ConnectCloud {
            name,
            email,
            password_env,
            device,
            default,
        } => connect_cloud(
            &name,
            &email,
            password_env.as_deref(),
            device.as_deref(),
            default,
        ),
        Command::Forget { name } => forget(&name),
        Command::Connections => list_connections(json),
        Command::Devices => devices(connection.as_deref(), json),
        other => with_api(connection.as_deref(), |api| act(api, other, json)),
    }
}

/// Runs `work` on the chosen connection, then saves a renewed MyJDownloader session.
fn with_api(
    name: Option<&str>,
    work: impl FnOnce(&mut dyn Api) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut store = Store::load()?;
    let (chosen, connection) = store.pick(name)?;
    match connection {
        Connection::Local { url } => work(&mut LocalApi::new(&url)),
        Connection::Cloud(account) => {
            let mut api = CloudApi::new(account);
            let result = work(&mut api);
            if api.session_changed {
                store
                    .connections
                    .insert(chosen, Connection::Cloud(api.account));
                store.save()?;
            }
            result
        }
    }
}

fn ids(target: &Target) -> (Value, Value) {
    match target {
        Target::Links(ids) => (json!(ids), json!([])),
        Target::Packages(ids) => (json!([]), json!(ids)),
    }
}

fn done(json: bool, what: &str) {
    if json {
        println!("{}", json!({ "ok": true }));
    } else {
        println!("{what}");
    }
}

/// One call with the target's link and package ids (most list actions take both lists).
fn on_target(
    api: &mut dyn Api,
    path: &str,
    before: &[Value],
    target: &Target,
) -> Result<(), Error> {
    let (links, packages) = ids(target);
    let mut params = before.to_vec();
    params.extend([links, packages]);
    api.call(path, &params).map(|_| ())
}

fn act(api: &mut dyn Api, command: Command, json: bool) -> Result<(), Error> {
    let message = match command {
        Command::State => {
            let state = jd::fetch_state(api)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&state).expect("state serializes")
                );
            } else {
                print_state(&state);
            }
            return Ok(());
        }
        Command::Start => {
            api.call("/downloadcontroller/start", &[])?;
            "Downloads started.".to_owned()
        }
        Command::Stop => {
            api.call("/downloadcontroller/stop", &[])?;
            "Downloads stopped.".to_owned()
        }
        Command::Pause => {
            api.call("/downloadcontroller/pause", &[json!(true)])?;
            "Downloads paused.".to_owned()
        }
        Command::Resume => {
            let state = api.call("/downloadcontroller/getCurrentState", &[])?;
            // A paused controller resumes; a stopped or idle one has to start.
            if state
                .as_str()
                .is_some_and(|state| state.eq_ignore_ascii_case("PAUSE"))
            {
                api.call("/downloadcontroller/pause", &[json!(false)])?;
            } else {
                api.call("/downloadcontroller/start", &[])?;
            }
            "Downloads resumed.".to_owned()
        }
        Command::Add {
            links,
            package,
            folder,
            start,
        } => {
            let mut query = json!({ "links": links.join("\n"), "autostart": start });
            if let Some(package) = package {
                query["packageName"] = json!(package);
            }
            if let Some(folder) = folder {
                query["destinationFolder"] = json!(folder);
            }
            api.call("/linkgrabberv2/addLinks", &[query])?;
            format!("{} link(s) sent to the link grabber.", links.len())
        }
        Command::Confirm(target) => {
            on_target(api, "/linkgrabberv2/moveToDownloadlist", &[], &target)?;
            "Sent to the downloads.".to_owned()
        }
        Command::Discard(target) => {
            on_target(api, "/linkgrabberv2/removeLinks", &[], &target)?;
            "Removed from the link grabber.".to_owned()
        }
        Command::Check(target) => {
            on_target(api, "/linkgrabberv2/startOnlineStatusCheck", &[], &target)?;
            "Online check started.".to_owned()
        }
        Command::Enable(target) => {
            on_target(api, "/downloadsV2/setEnabled", &[json!(true)], &target)?;
            "Enabled.".to_owned()
        }
        Command::Disable(target) => {
            on_target(api, "/downloadsV2/setEnabled", &[json!(false)], &target)?;
            "Disabled.".to_owned()
        }
        Command::Remove(target) => {
            on_target(api, "/downloadsV2/removeLinks", &[], &target)?;
            "Removed from the downloads (files stay on disk).".to_owned()
        }
        Command::Force(target) => {
            on_target(api, "/downloadsV2/forceDownload", &[], &target)?;
            "Forced.".to_owned()
        }
        Command::Reset(target) => {
            on_target(api, "/downloadsV2/resetLinks", &[], &target)?;
            "Reset.".to_owned()
        }
        Command::Priority { level, target } => {
            on_target(
                api,
                "/downloadsV2/setPriority",
                &[json!(level.to_uppercase())],
                &target,
            )?;
            format!("Priority set to {level}.")
        }
        Command::Rename { target, name } => {
            match target {
                Target::Links(ids) => {
                    api.call("/downloadsV2/renameLink", &[json!(ids[0]), json!(name)])?
                }
                Target::Packages(ids) => {
                    api.call("/downloadsV2/renamePackage", &[json!(ids[0]), json!(name)])?
                }
            };
            "Renamed.".to_owned()
        }
        Command::Folder { packages, folder } => {
            api.call(
                "/downloadsV2/setDownloadDirectory",
                &[json!(folder), json!(packages)],
            )?;
            "Download folder changed.".to_owned()
        }
        Command::Call { path, params } => {
            let answer = api.call(&path, &params)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).expect("answer serializes")
            );
            return Ok(());
        }
        other => return Err(Error::Usage(format!("{other:?} needs no connection"))),
    };
    done(json, &message);
    Ok(())
}

fn size(bytes: i64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

fn percent(progress: Option<f64>) -> String {
    progress
        .map(|p| format!("{:>3.0}%", p * 100.0))
        .unwrap_or_else(|| "   ?".into())
}

fn print_state(state: &jd::State) {
    let summary = &state.summary;
    println!(
        "JDownloader {} · {}/s · {} of {}{}",
        summary.controller,
        size(summary.speed),
        size(summary.bytes_loaded),
        size(summary.bytes_total),
        summary
            .eta
            .map(|eta| format!(" · {} min left", eta / 60))
            .unwrap_or_default()
    );
    println!("\nDownloads ({} links):", summary.links);
    for package in &state.downloads {
        println!(
            "  [{}] {} {:?} {}",
            package.id,
            percent(package.progress),
            package.state,
            package.name
        );
        for link in &package.items {
            println!(
                "      [{}] {} {:?} {} ({})",
                link.id,
                percent(link.progress),
                link.state,
                link.name,
                size(link.bytes_total)
            );
        }
    }
    println!(
        "\nLink grabber ({} links{}):",
        summary.grabber_links,
        if summary.collecting {
            ", still collecting"
        } else {
            ""
        }
    );
    for package in &state.grabber {
        println!("  [{}] {:?} {}", package.id, package.state, package.name);
        for link in &package.items {
            println!(
                "      [{}] {:?} {} ({})",
                link.id,
                link.state,
                link.name,
                size(link.bytes_total)
            );
        }
    }
}

fn save_connection(name: &str, connection: Connection, default: bool) -> Result<(), Error> {
    let mut store = Store::load()?;
    store.connections.insert(name.to_owned(), connection);
    if default || store.default.is_none() {
        store.default = Some(name.to_owned());
    }
    store.save()
}

fn connect_local(name: &str, url: &str, default: bool) -> Result<(), Error> {
    let mut api = LocalApi::new(url);
    // Proves the address answers before saving it.
    api.call("/downloadcontroller/getCurrentState", &[])?;
    save_connection(
        name,
        Connection::Local {
            url: url.trim_end_matches('/').to_owned(),
        },
        default,
    )?;
    println!("Connection \"{name}\" saved: JDownloader answers at {url}.");
    Ok(())
}

fn read_password(password_env: Option<&str>) -> Result<String, Error> {
    if let Some(variable) = password_env {
        return std::env::var(variable)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Usage(format!("environment variable {variable} is empty")));
    }
    rpassword::prompt_password("MyJDownloader password: ")
        .map_err(|_| Error::Usage("no terminal to ask the password: use --password-env VAR".into()))
}

fn connect_cloud(
    name: &str,
    email: &str,
    password_env: Option<&str>,
    device: Option<&str>,
    default: bool,
) -> Result<(), Error> {
    let password = read_password(password_env)?;
    let account = CloudAccount {
        email: email.to_lowercase(),
        login_secret: hex::encode(crypto::account_secret(email, &password, "server")),
        device_secret: hex::encode(crypto::account_secret(email, &password, "device")),
        device_id: None,
        device_name: None,
        session: None,
    };
    drop(password);
    let mut api = CloudApi::new(account);
    api.connect()?;
    let devices = api.list_devices()?;
    let chosen = match device {
        Some(wanted) => devices
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(wanted) || d.id == wanted),
        None if devices.len() == 1 => devices.first(),
        None => None,
    };
    let Some(chosen) = chosen else {
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        return Err(Error::Usage(if names.is_empty() {
            "no JDownloader is connected to this account".into()
        } else {
            format!("pick the JDownloader with --device: {}", names.join(", "))
        }));
    };
    let device_name = chosen.name.clone();
    api.account.device_id = Some(chosen.id.clone());
    api.account.device_name = Some(device_name.clone());
    // Proves the device answers before saving.
    api.call("/downloadcontroller/getCurrentState", &[])?;
    save_connection(name, Connection::Cloud(api.account), default)?;
    println!("Connection \"{name}\" saved: {device_name} through MyJDownloader.");
    Ok(())
}

fn forget(name: &str) -> Result<(), Error> {
    let mut store = Store::load()?;
    if store.connections.remove(name).is_none() {
        return Err(Error::Config(format!("no connection named \"{name}\"")));
    }
    if store.default.as_deref() == Some(name) {
        store.default = store.connections.keys().next().cloned();
    }
    store.save()?;
    println!("Connection \"{name}\" forgotten.");
    Ok(())
}

fn list_connections(json: bool) -> Result<(), Error> {
    let store = Store::load()?;
    let rows: Vec<Value> = store
        .connections
        .iter()
        .map(|(name, connection)| {
            let default = store.default.as_deref() == Some(name.as_str());
            match connection {
                Connection::Local { url } => json!({ "name": name, "kind": "local", "target": url, "default": default }),
                Connection::Cloud(account) => json!({
                    "name": name, "kind": "cloud", "default": default,
                    "target": format!("{} ({})", account.device_name.as_deref().unwrap_or("?"), account.email),
                }),
            }
        })
        .collect();
    if json {
        println!("{}", Value::Array(rows));
    } else if rows.is_empty() {
        println!("No connection yet: add one with `jdctl connect`.");
    } else {
        for row in rows {
            println!(
                "{}{}\t{}\t{}",
                row["name"].as_str().unwrap_or_default(),
                if row["default"].as_bool().unwrap_or(false) {
                    " (default)"
                } else {
                    ""
                },
                row["kind"].as_str().unwrap_or_default(),
                row["target"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn devices(name: Option<&str>, json: bool) -> Result<(), Error> {
    let store = Store::load()?;
    let (_, connection) = store.pick(name)?;
    let Connection::Cloud(account) = connection else {
        return Err(Error::Usage(
            "devices lists the JDownloaders of a MyJDownloader connection".into(),
        ));
    };
    let mut api = CloudApi::new(account);
    let devices = api.list_devices()?;
    if json {
        let rows: Vec<Value> = devices
            .iter()
            .map(|d| json!({ "id": d.id, "name": d.name, "type": d.kind }))
            .collect();
        println!("{}", Value::Array(rows));
    } else {
        for device in devices {
            println!("{}\t{}", device.name, device.id);
        }
    }
    Ok(())
}
