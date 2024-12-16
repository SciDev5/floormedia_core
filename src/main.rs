use std::{
    env, fs,
    io::{stdout, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::{self, sleep},
    time::Duration,
};

use crossterm::{
    execute, queue,
    style::{Print, PrintStyledContent, Stylize},
    terminal,
};

const SUBSERVER_NAMES: [&str; 2] = ["floormedia_frontend", "floormedia_backend"];
const SUBSERVER_BACKEND_I: usize = 1;
const SUBSERVER_DIR: &str = "./sub/";

fn main() {
    let args: ParsedArgs = env::args().into();

    if subservers_present() {
        subservers_sync();
    } else {
        subservers_initialize();
    }
    subservers_run(args);
}

#[derive(Debug, Clone, Copy)]
struct ParsedArgs {
    distinguish_child_stdouts: bool,
    server_alternate_port: Option<u16>,
}
impl<T: Iterator<Item = String>> From<T> for ParsedArgs {
    fn from(value: T) -> Self {
        let mut value = value.skip(1);
        let mut out = Self {
            distinguish_child_stdouts: true,
            server_alternate_port: None,
        };
        loop {
            let Some(arg) = value.next() else {
                break;
            };
            match arg.as_str() {
                "inherit_stdouts" | "-m" => {
                    out.distinguish_child_stdouts = false;
                }
                "backend_port" | "-bp" => {
                    let Some(port) = value.next().and_then(|v| v.parse::<u16>().ok()) else {
                        execute!(
                            stdout(),
                            PrintStyledContent(" ".on_red()),
                            Print("  "),
                            PrintStyledContent(format!("invalid server port, ignoring.").red()),
                        )
                        .unwrap();
                        continue;
                    };
                    out.server_alternate_port = Some(port);
                }
                _ => {
                    execute!(
                        stdout(),
                        PrintStyledContent(" ".on_red()),
                        Print("  "),
                        PrintStyledContent(format!("invalid argument '{}', ignoring.", arg).red()),
                    )
                    .unwrap();
                    continue;
                }
            }
        }
        out
    }
}

fn get_base_url() -> String {
    String::from_utf8(
        Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim_end_matches(|c| c != '/')
    .to_string()
}
fn get_subserver_root_cwd() -> PathBuf {
    Path::new(SUBSERVER_DIR).canonicalize().unwrap()
}
fn get_subserver_cwd(name: &str) -> PathBuf {
    (Path::new(SUBSERVER_DIR).join(name))
        .canonicalize()
        .unwrap()
}

fn subservers_present() -> bool {
    fs::read_dir(SUBSERVER_DIR).is_ok_and(|entries| {
        let mut subserver_is_ok = SUBSERVER_NAMES.map(|_| false);
        for entry in entries {
            match entry {
                Ok(entry) => {
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        continue;
                    };
                    let Some(i) = SUBSERVER_NAMES
                        .into_iter()
                        .enumerate()
                        .find_map(|(i, name_test)| if name == name_test { Some(i) } else { None })
                    else {
                        continue;
                    };
                    subserver_is_ok[i] = true;
                }
                Err(_) => {
                    continue;
                }
            }
        }
        subserver_is_ok.into_iter().all(|v| v)
    })
}
fn subservers_initialize() {
    Style::Header.println(format!("initializing servers"));
    if fs::read_dir(SUBSERVER_DIR).is_err() {
        fs::create_dir(SUBSERVER_DIR).unwrap()
    }
    git_clone();
    for name in SUBSERVER_NAMES {
        node_build(name);
    }
}
fn subservers_sync() {
    Style::Header.println(format!("updating servers"));
    for updated in git_pull() {
        node_build(updated);
    }
}
fn subservers_run(args: ParsedArgs) {
    Style::Header.println(format!("launching servers"));
    Style::SubHeader.println(format!("press `ctrl+C` to exit"));

    let child_processes = SUBSERVER_NAMES.map(|name| node_run(name, args));

    for mut child in child_processes {
        child.wait().unwrap();
    }
}

fn node_build(name: &str) {
    Style::StatusInfo.println(format!("[{}] update dependencies", name));
    if !Command::new("npm")
        .arg("install")
        .current_dir(get_subserver_cwd(name))
        .status()
        .unwrap()
        .success()
    {
        panic!();
    }
    Style::StatusInfo.println(format!("[{}] build", name));
    if !Command::new("npm")
        .args(["run", "build"])
        .current_dir(get_subserver_cwd(name))
        .status()
        .unwrap()
        .success()
    {
        panic!();
    }
}
fn node_run(name: &'static str, args: ParsedArgs) -> Child {
    let mut child = Command::new("npm")
        .arg("start")
        .args(if let Some(port) = args.server_alternate_port {
            if name == SUBSERVER_NAMES[SUBSERVER_BACKEND_I] {
                vec!["--".to_string(), format!("-p={}", port)]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        })
        .current_dir(get_subserver_cwd(name))
        .stdout(if args.distinguish_child_stdouts {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .unwrap();

    if let Some(child_stdout) = child.stdout.take() {
        thread::spawn(move || {
            let n_spaces = SUBSERVER_NAMES.map(str::len).into_iter().max().unwrap() - name.len();
            let header = match name {
                name if name == SUBSERVER_NAMES[0] => [
                    format!(" {}{} ", name, " ".repeat(n_spaces)).blue(),
                    "  ".to_string().on_blue(),
                ],
                _ => [
                    format!(" {}{} ", name, " ".repeat(n_spaces)).dark_magenta(),
                    "  ".to_string().on_dark_magenta(),
                ],
            };
            execute!(
                stdout(),
                PrintStyledContent(
                    format!(" [{}] :: start of stdout ", name)
                        .white()
                        .on_dark_yellow()
                ),
                Print("\r\n"),
            )
            .unwrap();
            let mut line = Vec::new();
            for byte in child_stdout.bytes() {
                match byte {
                    Err(err) => {
                        dbg!(err);
                        break;
                    }
                    Ok(b) => {
                        let mut stdout = stdout();
                        line.push(b);
                        if b == '\n' as u8 {
                            queue!(
                                stdout,
                                PrintStyledContent(header[0].clone()),
                                PrintStyledContent(header[1].clone()),
                                Print(" ")
                            )
                            .unwrap();
                            stdout.write_all(&line).unwrap();
                            line.clear();
                            stdout.flush().unwrap();
                        }
                    }
                }
            }
            execute!(
                stdout(),
                PrintStyledContent(
                    format!("\n [{}] :: end of stdout ", name)
                        .white()
                        .on_dark_yellow()
                ),
                Print("\r\n"),
            )
            .unwrap();
        });
    }

    sleep(Duration::from_millis(10));

    child
}

fn git_clone() {
    let base_url = get_base_url();
    for name in SUBSERVER_NAMES {
        Style::StatusInfo.println(format!("[{}] download", name));
        let mut url = base_url.clone();
        url += name;
        url += ".git";
        if !Command::new("git")
            .arg("clone")
            .arg(url)
            .current_dir(get_subserver_root_cwd().to_str().unwrap())
            .status()
            .unwrap()
            .success()
        {
            panic!();
        }
    }
}
fn git_pull() -> Vec<&'static str> {
    SUBSERVER_NAMES
        .into_iter()
        .filter_map(|name| {
            Style::StatusInfo.println(format!("[{}] download updates", name));
            let output = Command::new("git")
                .arg("pull")
                .current_dir(get_subserver_cwd(name).to_str().unwrap())
                .output()
                .unwrap();
            if !output.status.success() {
                panic!();
            }
            stdout().write_all(&output.stdout).unwrap();
            stdout().flush().unwrap();
            if String::from_utf8(output.stdout).unwrap().trim() == "Already up to date." {
                None
            } else {
                Some(name)
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum Style {
    StatusInfo,
    SubHeader,
    Header,
}
impl Style {
    fn println(self, line: String) {
        let width = terminal::size().unwrap().0 as usize;
        let line_len = line.len();
        match self {
            Self::Header => execute!(
                stdout(),
                Print("\r\n\r\n"),
                PrintStyledContent("    ".on_cyan()),
                PrintStyledContent(" :::: ".cyan()),
                PrintStyledContent(line.cyan()),
                PrintStyledContent(" :::: ".cyan()),
                PrintStyledContent(" ".repeat(width - line_len - 20).on_cyan()),
                Print("\r\n"),
            )
            .unwrap(),
            Self::StatusInfo => execute!(
                stdout(),
                Print("\r\n"),
                PrintStyledContent("    ".on_dark_grey()),
                PrintStyledContent(" :: ".dark_grey()),
                PrintStyledContent(line.dark_grey()),
                PrintStyledContent(" :: ".dark_grey()),
                PrintStyledContent(" ".repeat(width - line_len - 16).on_dark_grey()),
                Print("\r\n\r\n"),
            )
            .unwrap(),
            Self::SubHeader => execute!(
                stdout(),
                PrintStyledContent(" ".on_dark_cyan()),
                PrintStyledContent("  ".stylize()),
                PrintStyledContent(line.dark_cyan()),
                Print("\r\n\r\n"),
            )
            .unwrap(),
        };
    }
}
