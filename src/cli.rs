//! Command-line parsing: `jdctl [--connection NAME] [--json] <command> [args]`.

use lexopt::{Arg, Parser, ValueExt};

use crate::error::Error;

#[derive(Debug, PartialEq)]
pub enum Target {
    Links(Vec<i64>),
    Packages(Vec<i64>),
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    ConnectLocal {
        name: String,
        url: String,
        default: bool,
    },
    ConnectCloud {
        name: String,
        email: String,
        password_env: Option<String>,
        device: Option<String>,
        default: bool,
    },
    Forget {
        name: String,
    },
    Connections,
    Devices,
    State,
    Start,
    Stop,
    Pause,
    Resume,
    Add {
        links: Vec<String>,
        package: Option<String>,
        folder: Option<String>,
        start: bool,
    },
    /// Link grabber: send to downloads, remove, check online.
    Confirm(Target),
    Discard(Target),
    Check(Target),
    /// Downloads list.
    Enable(Target),
    Disable(Target),
    Remove(Target),
    Force(Target),
    Reset(Target),
    Priority {
        level: String,
        target: Target,
    },
    Rename {
        target: Target,
        name: String,
    },
    Folder {
        packages: Vec<i64>,
        folder: String,
    },
    /// Any API call, for diagnostics: `call /path '{json}' …`.
    Call {
        path: String,
        params: Vec<serde_json::Value>,
    },
}

#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub connection: Option<String>,
    pub json: bool,
    pub command: Command,
}

pub const PRIORITIES: [&str; 6] = ["highest", "higher", "high", "default", "lower", "lowest"];

/// `1,2, 3` → ids (JDownloader's numeric uuids).
pub fn parse_ids(value: &str) -> Result<Vec<i64>, Error> {
    let ids: Result<Vec<i64>, _> = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect();
    match ids {
        Ok(ids) if !ids.is_empty() => Ok(ids),
        _ => Err(Error::Usage(format!(
            "\"{value}\" is not a list of ids (like 12,34)"
        ))),
    }
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(message.into())
}

fn text(value: std::ffi::OsString) -> Result<String, Error> {
    value
        .string()
        .map_err(|_| usage("arguments must be valid text"))
}

/// `--link IDS` or `--package IDS` (one of them).
fn target(parser: &mut Parser) -> Result<Target, Error> {
    let mut found = None;
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Arg::Long("link") | Arg::Long("links") => {
                found = Some(Target::Links(parse_ids(&text(
                    parser.value().map_err(|e| usage(e.to_string()))?,
                )?)?))
            }
            Arg::Long("package") | Arg::Long("packages") => {
                found = Some(Target::Packages(parse_ids(&text(
                    parser.value().map_err(|e| usage(e.to_string()))?,
                )?)?))
            }
            other => return Err(usage(format!("unexpected {}", describe(&other)))),
        }
    }
    found.ok_or_else(|| usage("say which with --link IDS or --package IDS"))
}

fn describe(arg: &Arg) -> String {
    match arg {
        Arg::Short(c) => format!("-{c}"),
        Arg::Long(name) => format!("--{name}"),
        Arg::Value(value) => format!("\"{}\"", value.to_string_lossy()),
    }
}

pub fn parse(args: Vec<std::ffi::OsString>) -> Result<Invocation, Error> {
    let mut parser = Parser::from_args(args);
    let mut connection = None;
    let mut json = false;
    let mut command_name = None;
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Arg::Long("connection") | Arg::Short('c') => {
                connection = Some(text(parser.value().map_err(|e| usage(e.to_string()))?)?)
            }
            Arg::Long("json") => json = true,
            Arg::Long("help") | Arg::Short('h') => {
                return Ok(Invocation {
                    connection,
                    json,
                    command: Command::Help,
                });
            }
            Arg::Long("version") | Arg::Short('V') => {
                return Ok(Invocation {
                    connection,
                    json,
                    command: Command::Version,
                });
            }
            Arg::Value(value) => {
                command_name = Some(text(value)?);
                break;
            }
            other => return Err(usage(format!("unexpected {}", describe(&other)))),
        }
    }
    let Some(name) = command_name else {
        return Ok(Invocation {
            connection,
            json,
            command: Command::Help,
        });
    };
    let command = match name.as_str() {
        "help" => Command::Help,
        "connect" => connect(&mut parser)?,
        "forget" => Command::Forget {
            name: single(&mut parser, "forget NAME")?,
        },
        "connections" => Command::Connections,
        "devices" => Command::Devices,
        "state" | "status" => Command::State,
        "start" => Command::Start,
        "stop" => Command::Stop,
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        "add" => add(&mut parser)?,
        "confirm" => Command::Confirm(target(&mut parser)?),
        "discard" => Command::Discard(target(&mut parser)?),
        "check" => Command::Check(target(&mut parser)?),
        "enable" => Command::Enable(target(&mut parser)?),
        "disable" => Command::Disable(target(&mut parser)?),
        "remove" => Command::Remove(target(&mut parser)?),
        "force" => Command::Force(target(&mut parser)?),
        "reset" => Command::Reset(target(&mut parser)?),
        "priority" => priority(&mut parser)?,
        "rename" => rename(&mut parser)?,
        "folder" => folder(&mut parser)?,
        "call" => call(&mut parser)?,
        other => {
            return Err(usage(format!(
                "unknown command \"{other}\" (see jdctl --help)"
            )));
        }
    };
    Ok(Invocation {
        connection,
        json,
        command,
    })
}

/// The value of the option just read.
fn next_value(parser: &mut Parser) -> Result<String, Error> {
    text(parser.value().map_err(|e| usage(e.to_string()))?)
}

fn single(parser: &mut Parser, shape: &str) -> Result<String, Error> {
    let value = parser
        .value()
        .map_err(|_| usage(format!("usage: jdctl {shape}")))?;
    text(value)
}

fn connect(parser: &mut Parser) -> Result<Command, Error> {
    let kind = single(parser, "connect local|cloud NAME …")?;
    let name = single(parser, "connect local|cloud NAME …")?;
    let (mut url, mut email, mut password_env, mut device, mut default) =
        (None, None, None, None, false);
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Arg::Long("url") => url = Some(next_value(parser)?),
            Arg::Long("email") => email = Some(next_value(parser)?),
            Arg::Long("password-env") => password_env = Some(next_value(parser)?),
            Arg::Long("device") => device = Some(next_value(parser)?),
            Arg::Long("default") => default = true,
            other => return Err(usage(format!("unexpected {}", describe(&other)))),
        }
    }
    match kind.as_str() {
        "local" => {
            let url = url.ok_or_else(|| usage("connect local needs --url http://host:3128"))?;
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(usage("--url must start with http:// or https://"));
            }
            Ok(Command::ConnectLocal { name, url, default })
        }
        "cloud" => Ok(Command::ConnectCloud {
            name,
            email: email.ok_or_else(|| usage("connect cloud needs --email"))?,
            password_env,
            device,
            default,
        }),
        _ => Err(usage("connect takes local or cloud")),
    }
}

fn add(parser: &mut Parser) -> Result<Command, Error> {
    let (mut links, mut package, mut folder, mut start) = (Vec::new(), None, None, false);
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Arg::Long("package") => {
                package = Some(text(parser.value().map_err(|e| usage(e.to_string()))?)?)
            }
            Arg::Long("folder") => {
                folder = Some(text(parser.value().map_err(|e| usage(e.to_string()))?)?)
            }
            Arg::Long("start") => start = true,
            Arg::Value(value) => links.extend(text(value)?.split_whitespace().map(str::to_owned)),
            other => return Err(usage(format!("unexpected {}", describe(&other)))),
        }
    }
    if links.is_empty() {
        return Err(usage("add needs at least one link"));
    }
    Ok(Command::Add {
        links,
        package,
        folder,
        start,
    })
}

fn priority(parser: &mut Parser) -> Result<Command, Error> {
    let level = single(parser, "priority LEVEL --link IDS")?.to_lowercase();
    if !PRIORITIES.contains(&level.as_str()) {
        return Err(usage(format!(
            "priority is one of {}",
            PRIORITIES.join(", ")
        )));
    }
    Ok(Command::Priority {
        level,
        target: target(parser)?,
    })
}

fn rename(parser: &mut Parser) -> Result<Command, Error> {
    let name = single(parser, "rename NEW_NAME --link ID")?;
    let target = target(parser)?;
    let single_id = match &target {
        Target::Links(ids) | Target::Packages(ids) => ids.len() == 1,
    };
    if !single_id || name.trim().is_empty() {
        return Err(usage(
            "rename takes a new name and exactly one --link or --package id",
        ));
    }
    Ok(Command::Rename { target, name })
}

fn folder(parser: &mut Parser) -> Result<Command, Error> {
    let folder = single(parser, "folder PATH --package IDS")?;
    match target(parser)? {
        Target::Packages(packages) => Ok(Command::Folder { packages, folder }),
        Target::Links(_) => Err(usage("folder applies to packages: --package IDS")),
    }
}

fn call(parser: &mut Parser) -> Result<Command, Error> {
    let path = single(parser, "call /namespace/method [JSON…]")?;
    if !path.starts_with('/') {
        return Err(usage(
            "the path starts with / (like /downloadsV2/queryLinks)",
        ));
    }
    let mut params = Vec::new();
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Arg::Value(value) => {
                let value = text(value)?;
                params.push(
                    serde_json::from_str(&value)
                        .map_err(|_| usage(format!("not JSON: {value}")))?,
                );
            }
            other => return Err(usage(format!("unexpected {}", describe(&other)))),
        }
    }
    Ok(Command::Call { path, params })
}

pub const HELP: &str = "jdctl — control JDownloader 2, locally or through MyJDownloader

Usage: jdctl [--connection NAME] [--json] <command> [args]

Connections (saved, encrypted when CANOPY_PLUGIN_KEY or JDCTL_KEY is set):
  connect local NAME --url http://HOST:3128 [--default]   local API (enable \"Deprecated API\")
  connect cloud NAME --email EMAIL [--password-env VAR] [--device NAME] [--default]
                                                          MyJDownloader (asks the password)
  connections                 list them        forget NAME      remove one
  devices                     JDownloader instances of a MyJDownloader account

Downloads:
  state                       packages, links, progress, speed (--json for scripts)
  start | stop | pause | resume
  enable | disable | remove | force | reset   --link IDS | --package IDS
  priority LEVEL --link IDS   highest, higher, high, default, lower, lowest
  rename NEW_NAME --link ID | --package ID
  folder PATH --package IDS   download folder (JDownloader's own paths)

Link grabber:
  add LINK… [--package NAME] [--folder PATH] [--start]
  check | confirm | discard   --link IDS | --package IDS   (confirm sends to downloads)

Diagnostics:
  call /NAMESPACE/METHOD [JSON…]   any API call, prints the answer
";

#[cfg(test)]
mod tests {
    use super::*;

    fn run(line: &str) -> Result<Invocation, Error> {
        parse(line.split_whitespace().map(Into::into).collect())
    }

    #[test]
    fn parses_ids() {
        assert_eq!(parse_ids("1, 2,3").unwrap(), vec![1, 2, 3]);
        assert!(parse_ids("").is_err());
        assert!(parse_ids("a").is_err());
    }

    #[test]
    fn parses_global_options_and_commands() {
        let invocation = run("--connection home --json state").unwrap();
        assert_eq!(invocation.connection.as_deref(), Some("home"));
        assert!(invocation.json);
        assert_eq!(invocation.command, Command::State);
        assert_eq!(run("").unwrap().command, Command::Help);
        assert!(run("nope").is_err());
    }

    #[test]
    fn parses_connections() {
        assert_eq!(
            run("connect local home --url http://jd:3128 --default")
                .unwrap()
                .command,
            Command::ConnectLocal {
                name: "home".into(),
                url: "http://jd:3128".into(),
                default: true
            }
        );
        assert!(run("connect local home --url jd:3128").is_err());
        assert_eq!(
            run("connect cloud my --email a@b.c --password-env PW")
                .unwrap()
                .command,
            Command::ConnectCloud {
                name: "my".into(),
                email: "a@b.c".into(),
                password_env: Some("PW".into()),
                device: None,
                default: false
            }
        );
        assert!(run("connect cloud my").is_err());
        assert!(run("connect smb x").is_err());
    }

    #[test]
    fn parses_actions() {
        assert_eq!(
            run("enable --link 1,2").unwrap().command,
            Command::Enable(Target::Links(vec![1, 2]))
        );
        assert_eq!(
            run("confirm --package 9").unwrap().command,
            Command::Confirm(Target::Packages(vec![9]))
        );
        assert!(run("remove").is_err());
        assert_eq!(
            run("priority HIGH --link 4").unwrap().command,
            Command::Priority {
                level: "high".into(),
                target: Target::Links(vec![4])
            }
        );
        assert!(run("priority urgent --link 4").is_err());
        assert!(run("rename new --link 1,2").is_err());
        assert!(run("folder /x --link 1").is_err());
        assert_eq!(
            run("add https://a https://b --package Show --start")
                .unwrap()
                .command,
            Command::Add {
                links: vec!["https://a".into(), "https://b".into()],
                package: Some("Show".into()),
                folder: None,
                start: true
            }
        );
        assert!(run("add").is_err());
        assert_eq!(
            run(r#"call /a/b {"x":1} true"#).unwrap().command,
            Command::Call {
                path: "/a/b".into(),
                params: vec![serde_json::json!({"x": 1}), serde_json::json!(true)]
            }
        );
        assert!(run("call a/b").is_err());
        assert!(run("call /a/b nope").is_err());
    }
}
