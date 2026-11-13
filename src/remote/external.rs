//! `//RUN` programs, jailed to the EXTERNAL folder.
//!
//! Linux would execute a first line such as `#! /usr/lib/cgi-bin/php7.2`
//! by itself. macOS does that only when the file is marked executable, and
//! Windows never does. Every build reads that line here and starts the
//! Python, PHP, Perl, Ruby, Node, Lua, or Java that is installed on this
//! computer. The script's printed text is the reply. A line it reads is
//! the next text from the station. Java is started headless, so it cannot
//! open a window.
//!
//! The command is not a shell. Arguments cannot name another folder.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Stop a script that prints without end. A packet screen, not a disk dump.
const MAX_OUT: usize = 12_000;
/// A prompt with no newline (`input("Call? ")`) is sent after this pause.
const PROMPT_WAIT: Duration = Duration::from_millis(250);
/// Once the script has exited, collect the last printed bytes, then drop it.
const EXIT_DRAIN: Duration = Duration::from_millis(40);
/// No typed line and no new printed text for this long closes the process.
const QUIET: Duration = Duration::from_secs(300);
/// Told to a station that is still linked when that idle limit closes the run.
const TIMED_OUT: &str = "closed run program after timeout";
const TEXT_ONLY: &str = "python, php, perl, ruby, node, lua, or java only";

/// What a second `//RUN` on a channel that already has a program is told.
pub const BUSY: &str = "program is running";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Python,
    Php,
    Perl,
    Ruby,
    Node,
    Lua,
    Java,
}

impl Family {
    fn label(self) -> &'static str {
        match self {
            Family::Python => "python",
            Family::Php => "php",
            Family::Perl => "perl",
            Family::Ruby => "ruby",
            Family::Node => "node",
            Family::Lua => "lua",
            Family::Java => "java",
        }
    }

    fn all() -> [Family; 7] {
        [
            Family::Python,
            Family::Php,
            Family::Perl,
            Family::Ruby,
            Family::Node,
            Family::Lua,
            Family::Java,
        ]
    }
}

/// One interpreter binary this computer can start.
#[derive(Clone)]
struct Tool {
    path: PathBuf,
    /// Args that belong to the launcher, before the script. `py` uses `-3`.
    leading: Vec<String>,
}

struct HostTools {
    python: Option<Tool>,
    php: Option<Tool>,
    perl: Option<Tool>,
    ruby: Option<Tool>,
    node: Option<Tool>,
    lua: Option<Tool>,
    java: Option<Tool>,
}

/// What `#!` asked for. The path is used only when that file exists here.
struct Shebang {
    family: Family,
    absolute: Option<PathBuf>,
    /// Flags written after the program name, such as perl `-w` or java `--source`.
    flags: Vec<String>,
    asked: String,
}

enum Ask {
    List,
    Run { name: String, args: Vec<String> },
}

#[derive(Debug)]
struct Plan {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    name: String,
}

#[derive(Debug)]
enum Planned {
    List(String),
    Run(Plan),
}

/// `//RUN` with no name, or `//RUN ?`.
pub enum Begin {
    List(String),
    Started(Running),
}

/// Text ready for the link, and the run ending when the process has stopped.
pub struct Tick {
    pub text: String,
    pub finished: Option<Finish>,
}

pub struct Finish {
    pub code: Option<i32>,
    /// Extra line when the script printed nothing useful.
    pub air: String,
}

enum Ev {
    Out(String),
    Err(String),
}

/// One program on one channel. Drop kills it.
pub struct Running {
    child: Child,
    pid: u32,
    stdin: Option<ChildStdin>,
    events: Receiver<Ev>,
    pending: String,
    partial_at: Option<Instant>,
    produced: usize,
    err_tail: String,
    pipes_done: bool,
    exit_drained: bool,
    code: Option<i32>,
    code_known: bool,
    capped: bool,
    cap_noted: bool,
    /// Last input or new output. Spawn counts as the start.
    replied: Instant,
    quiet_for: Duration,
    silent: bool,
    silent_noted: bool,
    /// The station was still linked when the idle limit closed the run.
    report_timeout: bool,
    while_linked: bool,
    link_stopped: bool,
    name: String,
    /// Kills any process the interpreter itself started, when this run is dropped.
    #[cfg(windows)]
    job: Option<WinJob>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stdin.take();
        let _ = self.child.kill();
        self.release_children();
        let _ = self.child.wait();
    }
}

impl Running {
    pub fn name(&self) -> &str {
        &self.name
    }

    fn release_children(&mut self) {
        #[cfg(windows)]
        {
            self.job.take();
        }
    }

    /// The process is still in the OS, including while it waits at a question.
    pub fn still_open(&mut self) -> bool {
        self.note_exit();
        !self.code_known
    }


    /// The peer was linked when this started. A dropped link stops the script.
    pub fn set_while_linked(&mut self, yes: bool) {
        self.while_linked = yes;
    }

    /// One line of input, the way a terminal would deliver it.
    pub fn write_line(&mut self, line: &str) -> Result<(), String> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.chars().count() > 200 {
            return Err("line too long".into());
        }
        if line.chars().any(|c| c != '\t' && c.is_control()) {
            return Err("that line cannot be sent to the program".into());
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err("program is not waiting".into());
        };
        writeln!(stdin, "{line}").map_err(|_| "program is not waiting".to_string())?;
        stdin
            .flush()
            .map_err(|_| "program is not waiting".to_string())?;
        self.replied = Instant::now();
        Ok(())
    }

    pub fn tick(&mut self, link_up: bool) -> Tick {
        if self.while_linked && !link_up {
            self.link_stopped = true;
            let _ = self.stdin.take();
            let _ = self.child.kill();
        }
        self.drain();
        let mut text = self.take_text(false);
        self.note_cap_line(&mut text);
        self.note_exit();
        if !self.code_known && !self.link_stopped && self.replied.elapsed() >= self.quiet_for {
            self.silent = true;
            self.report_timeout = link_up;
            let _ = self.stdin.take();
            let _ = self.child.kill();
            self.note_exit();
        }
        // Still at a question: keep this same process. Do not finish the run.
        if !self.code_known {
            return Tick {
                text,
                finished: None,
            };
        }
        // The script has left. Read whatever it printed and close it now.
        self.drain_exit();
        let rest = self.take_text(true);
        if !rest.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&rest);
        }
        self.note_cap_line(&mut text);
        self.note_silent_line(&mut text);
        let air = self.finish_air();
        Tick {
            text,
            finished: Some(Finish {
                code: self.code,
                air,
            }),
        }
    }

    fn note_cap_line(&mut self, text: &mut String) {
        if self.capped && !self.cap_noted {
            self.cap_noted = true;
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("(output stopped)");
        }
    }

    fn note_silent_line(&mut self, text: &mut String) {
        if self.report_timeout && !self.silent_noted {
            self.silent_noted = true;
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(TIMED_OUT);
        }
    }

    fn finish_air(&self) -> String {
        if self.link_stopped || self.capped || self.silent || self.produced > 0 {
            return String::new();
        }
        if self.code == Some(0) {
            return "(no output)".into();
        }
        let err = self.err_tail.trim();
        if err.is_empty() {
            return "program failed".into();
        }
        let line = err.lines().next().unwrap_or(err);
        format!("program failed: {}", clip(line, 120))
    }

    fn drain(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(Ev::Out(chunk)) => self.push_out(&chunk),
                Ok(Ev::Err(chunk)) => self.err_tail = chunk,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pipes_done = true;
                    break;
                }
            }
        }
    }

    fn push_out(&mut self, chunk: &str) {
        let chunk = crate::tnc::normalize_newlines(&clean_text(chunk));
        if chunk.is_empty() {
            return;
        }
        self.replied = Instant::now();
        self.pending.push_str(&chunk);
    }

    fn take_text(&mut self, flush: bool) -> String {
        let mut lines = Vec::new();
        loop {
            let Some(pos) = self.pending.find('\n') else {
                break;
            };
            let line = self.pending[..pos].trim_end_matches('\r').to_string();
            self.pending.drain(..=pos);
            lines.push(line);
            self.partial_at = None;
        }
        let partial_due = flush || self.partial_at.is_some_and(|t| t.elapsed() >= PROMPT_WAIT);
        if partial_due && !self.pending.is_empty() {
            lines.push(std::mem::take(&mut self.pending));
            self.partial_at = None;
        } else if !self.pending.is_empty() && self.partial_at.is_none() {
            self.partial_at = Some(Instant::now());
        }
        if lines.is_empty() {
            return String::new();
        }
        let mut out = lines.join("\n");
        let room = MAX_OUT.saturating_sub(self.produced);
        if out.len() > room {
            out = clip(&out, room).to_string();
            self.capped = true;
            let _ = self.child.kill();
        }
        self.produced += out.len();
        out
    }

    fn note_exit(&mut self) {
        if self.code_known {
            return;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.code = status.code();
                self.code_known = true;
                let _ = self.stdin.take();
            }
            Ok(None) => {}
            Err(_) => {
                self.code_known = true;
                let _ = self.stdin.take();
            }
        }
    }

    /// Last bytes after the process has gone, then the caller drops the handle.
    fn drain_exit(&mut self) {
        if self.exit_drained {
            return;
        }
        self.exit_drained = true;
        let _ = self.stdin.take();
        let deadline = Instant::now() + EXIT_DRAIN;
        loop {
            self.drain();
            if self.pipes_done {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self.events.recv_timeout(deadline - now) {
                Ok(Ev::Out(chunk)) => self.push_out(&chunk),
                Ok(Ev::Err(chunk)) => self.err_tail = chunk,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    self.pipes_done = true;
                    break;
                }
            }
        }
    }

    fn spawn(plan: Plan) -> Result<Self, String> {
        let mut child =
            spawn_child(&plan).map_err(|err| format!("could not start {}: {err}", plan.name))?;
        let pid = child.id();
        #[cfg(windows)]
        let job = WinJob::contain(&child);
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err(format!("could not read {}", plan.name));
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            return Err(format!("could not read {}", plan.name));
        };
        let (tx, rx) = mpsc::sync_channel(32);
        let tx_err = tx.clone();
        if let Err(err) = thread::Builder::new()
            .name("gpng-external-out".into())
            .spawn(move || read_stdout(stdout, tx))
        {
            let _ = child.kill();
            return Err(format!("could not start {}: {err}", plan.name));
        }
        if let Err(err) = thread::Builder::new()
            .name("gpng-external-err".into())
            .spawn(move || read_stderr(stderr, tx_err))
        {
            let _ = child.kill();
            return Err(format!("could not start {}: {err}", plan.name));
        }
        Ok(Self {
            child,
            pid,
            stdin,
            events: rx,
            pending: String::new(),
            partial_at: None,
            produced: 0,
            err_tail: String::new(),
            pipes_done: false,
            exit_drained: false,
            code: None,
            code_known: false,
            capped: false,
            cap_noted: false,
            replied: Instant::now(),
            quiet_for: QUIET,
            silent: false,
            silent_noted: false,
            report_timeout: false,
            while_linked: false,
            link_stopped: false,
            name: plan.name,
            #[cfg(windows)]
            job,
        })
    }
}

/// One channel. At most one OS process, kept while it waits for a line.
#[derive(Default)]
pub struct Gate {
    run: Option<Running>,
}

/// What [`Gate::open`] did. A started program is stored in the gate.
#[derive(Debug)]
pub enum Opened {
    List(String),
    Started,
}

impl Gate {
    pub fn new() -> Self {
        Self { run: None }
    }

    /// A process handle is still stored, finished or not.
    pub fn occupied(&self) -> bool {
        self.run.is_some()
    }

    /// Alive in the OS, including paused at a question.
    pub fn is_open(&mut self) -> bool {
        self.run.as_mut().is_some_and(|run| run.still_open())
    }

    pub fn id(&self) -> Option<u32> {
        self.run.as_ref().map(|run| run.pid)
    }

    pub fn name(&self) -> Option<&str> {
        self.run.as_ref().map(|run| run.name())
    }

    pub fn configure(&mut self, while_linked: bool) {
        if let Some(run) = self.run.as_mut() {
            run.set_while_linked(while_linked);
        }
    }

    pub fn close(&mut self) {
        self.run = None;
    }

    /// Start a program, or refuse when this channel already has one open.
    pub fn open(&mut self, dir: &Path, request: &str) -> Result<Opened, String> {
        if self.is_open() {
            return Err(BUSY.into());
        }
        self.run = None;
        match begin(dir, request)? {
            Begin::List(text) => Ok(Opened::List(text)),
            Begin::Started(run) => {
                self.run = Some(run);
                Ok(Opened::Started)
            }
        }
    }

    pub fn write_line(&mut self, line: &str) -> Result<(), String> {
        let Some(run) = self.run.as_mut() else {
            return Err("program is not waiting".into());
        };
        run.write_line(line)
    }

    /// `None` when this channel has no program. A finished script is dropped here.
    pub fn tick(&mut self, link_up: bool) -> Option<Tick> {
        let tick = self.run.as_mut()?.tick(link_up);
        if tick.finished.is_some() {
            self.run = None;
        }
        Some(tick)
    }
}

/// `//FREE` when that file is a script in this folder. `//RUN` is separate.
pub fn has_program(dir: &Path, name: &str) -> bool {
    find_script(dir, name).is_ok()
}

/// A `//name args` line that is not a built-in command. Empty when it is not one.
pub fn slash_request(line: &str) -> Option<&str> {
    if line.bytes().next().is_some_and(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let rest = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("//")?
        .trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

pub fn begin(dir: &Path, request: &str) -> Result<Begin, String> {
    crate::paths::ensure_dir(dir).map_err(|err| format!("EXTERNAL: {err}"))?;
    match plan(dir, request, &current_host())? {
        Planned::List(text) => Ok(Begin::List(text)),
        Planned::Run(plan) => {
            tracing::info!("EXTERNAL start {}", plan.name);
            Running::spawn(plan).map(Begin::Started)
        }
    }
}

fn plan(dir: &Path, request: &str, host: &HostTools) -> Result<Planned, String> {
    match parse_request(request)? {
        Ask::List => Ok(Planned::List(catalog(dir, host))),
        Ask::Run { name, args } => {
            let script = find_script(dir, &name)?;
            let hint = kind_of(&script).ok_or_else(|| TEXT_ONLY.to_string())?;
            let tool = pick_tool(&hint, host)?;
            let cwd = dir
                .canonicalize()
                .map_err(|err| format!("EXTERNAL: {err}"))?;
            let display = script
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&name)
                .to_string();
            Ok(Planned::Run(Plan {
                args: command_args(&hint, &tool, &script, &args),
                program: tool.path,
                cwd,
                name: display,
            }))
        }
    }
}

fn command_args(hint: &Shebang, tool: &Tool, script: &Path, user: &[String]) -> Vec<String> {
    let mut args = tool.leading.clone();
    let file = script.to_string_lossy().into_owned();
    match hint.family {
        Family::Python => {
            args.push("-I".into());
            args.push("-u".into());
            args.push(file);
            args.extend(user.iter().cloned());
        }
        Family::Php => {
            args.push("-d".into());
            args.push("output_buffering=0".into());
            args.push("-d".into());
            args.push("implicit_flush=1".into());
            args.push(file);
            args.push("--".into());
            args.extend(user.iter().cloned());
        }
        Family::Perl | Family::Ruby | Family::Node | Family::Lua => {
            args.extend(hint.flags.iter().cloned());
            args.push(file);
            args.extend(user.iter().cloned());
        }
        Family::Java => {
            // Headless: a jar must not open a window on the sysop computer.
            args.push("-Djava.awt.headless=true".into());
            let jar = script
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
                || hint.flags.iter().any(|flag| flag == "-jar");
            for flag in &hint.flags {
                if flag != "-jar" {
                    args.push(flag.clone());
                }
            }
            if jar {
                args.push("-jar".into());
            }
            args.push(file);
            args.extend(user.iter().cloned());
        }
    }
    args
}

fn pick_tool(hint: &Shebang, host: &HostTools) -> Result<Tool, String> {
    if let Some(path) = &hint.absolute {
        if path.is_file() {
            return Ok(Tool {
                path: path.clone(),
                leading: Vec::new(),
            });
        }
    }
    let found = match hint.family {
        Family::Python => host.python.clone(),
        Family::Php => host.php.clone(),
        Family::Perl => host.perl.clone(),
        Family::Ruby => host.ruby.clone(),
        Family::Node => host.node.clone(),
        Family::Lua => host.lua.clone(),
        Family::Java => host.java.clone(),
    };
    found.ok_or_else(|| {
        format!(
            "{} is not installed (script asks for {})",
            hint.family.label(),
            hint.asked
        )
    })
}

fn current_host() -> HostTools {
    HostTools {
        python: detect_python(),
        php: detect_php(),
        perl: detect_plain(&["perl"]),
        ruby: detect_plain(&["ruby"]),
        node: detect_plain(&["node", "nodejs"]),
        lua: detect_plain(&["lua", "luajit"]),
        java: detect_plain(&["java"]),
    }
}

fn detect_python() -> Option<Tool> {
    // A direct `python` is one process. The Windows `py` launcher is only the
    // fallback: it starts a second process, and that is the one that would
    // leave a window behind. WindowsApps stubs are skipped in the search.
    let names: &[&str] = if cfg!(windows) {
        &["python", "python3", "py"]
    } else {
        &["python3", "python"]
    };
    let path = find_tool(names)?;
    let leading = if is_py_launcher(&path) {
        vec!["-3".into()]
    } else {
        Vec::new()
    };
    Some(Tool { path, leading })
}

fn is_py_launcher(path: &Path) -> bool {
    #[cfg(windows)]
    {
        path.file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("py"))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

fn detect_php() -> Option<Tool> {
    detect_plain(&["php"])
}

fn detect_plain(names: &[&str]) -> Option<Tool> {
    find_tool(names).map(|path| Tool {
        path,
        leading: Vec::new(),
    })
}

fn find_tool(names: &[&str]) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    push_known_dirs(&mut dirs);
    for dir in dirs {
        for name in names {
            for candidate in candidate_names(&dir, name) {
                if candidate.is_file() && !store_stub(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Install locations a double-clicked app may not have on `PATH`.
#[cfg(unix)]
fn push_known_dirs(dirs: &mut Vec<PathBuf>) {
    dirs.push(PathBuf::from("/usr/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    #[cfg(target_os = "macos")]
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
}

#[cfg(windows)]
fn push_known_dirs(_dirs: &mut Vec<PathBuf>) {}

#[cfg(windows)]
fn candidate_names(dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![dir.join(format!("{name}.exe")), dir.join(name)]
}

#[cfg(not(windows))]
fn candidate_names(dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![dir.join(name)]
}

#[cfg(windows)]
fn store_stub(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("WindowsApps")
    })
}

#[cfg(not(windows))]
fn store_stub(_path: &Path) -> bool {
    false
}

fn spawn_child(plan: &Plan) -> std::io::Result<Child> {
    let mut cmd = Command::new(&plan.program);
    cmd.args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    hide_console(&mut cmd);
    cmd.spawn()
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

/// One job for the interpreter and anything it starts. Closing it kills them.
#[cfg(windows)]
struct WinJob(*mut std::ffi::c_void);

#[cfg(windows)]
impl WinJob {
    fn contain(child: &Child) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JobExtendedLimit = std::mem::zeroed();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const JobExtendedLimit as *const std::ffi::c_void,
                std::mem::size_of::<JobExtendedLimit>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Self(job))
        }
    }
}

#[cfg(windows)]
impl Drop for WinJob {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
#[cfg(windows)]
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

#[cfg(windows)]
#[repr(C)]
struct JobBasicLimit {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[cfg(windows)]
#[repr(C)]
struct JobIoCounters {
    read_operation: u64,
    write_operation: u64,
    other_operation: u64,
    read_transfer: u64,
    write_transfer: u64,
    other_transfer: u64,
}

#[cfg(windows)]
#[repr(C)]
struct JobExtendedLimit {
    basic: JobBasicLimit,
    io: JobIoCounters,
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(
        job: *mut std::ffi::c_void,
        class: i32,
        info: *const std::ffi::c_void,
        len: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

fn read_stdout(mut pipe: ChildStdout, tx: SyncSender<Ev>) {
    let mut buf = [0u8; 1024];
    let mut sent = 0usize;
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if sent < MAX_OUT.saturating_mul(2) {
                    let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                    sent += chunk.len();
                    if tx.send(Ev::Out(chunk)).is_err() {
                        continue;
                    }
                }
            }
        }
    }
}

fn read_stderr(mut pipe: ChildStderr, tx: SyncSender<Ev>) {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.len() < 400 {
                    let take = n.min(400 - out.len());
                    out.extend_from_slice(&buf[..take]);
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&out).into_owned();
    let _ = tx.send(Ev::Err(text));
}

fn catalog(dir: &Path, host: &HostTools) -> String {
    let rows = programs(dir);
    let mut out = String::from("EXTERNAL");
    for (name, kind) in &rows {
        out.push('\n');
        out.push_str(name);
        out.push_str("  ");
        out.push_str(kind);
    }
    out.push('\n');
    out.push_str(&format!("{} program(s)", rows.len()));
    for family in Family::all() {
        let ready = match family {
            Family::Python => host.python.is_some(),
            Family::Php => host.php.is_some(),
            Family::Perl => host.perl.is_some(),
            Family::Ruby => host.ruby.is_some(),
            Family::Node => host.node.is_some(),
            Family::Lua => host.lua.is_some(),
            Family::Java => host.java.is_some(),
        };
        out.push('\n');
        out.push_str(family.label());
        out.push_str(if ready { ": ready" } else { ": not installed" });
    }
    out
}

fn programs(dir: &Path) -> Vec<(String, &'static str)> {
    let mut rows = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return rows;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = ent.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Some(kind) = kind_of(&path) else {
            continue;
        };
        if !inside(dir, &path) {
            continue;
        }
        rows.push((name, kind.family.label()));
    }
    rows.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    rows
}

fn find_script(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !safe_name(name) {
        return Err("use a program name in EXTERNAL".into());
    }
    // The command word is uppercased and must be the whole program name.
    // `//D` does not start `DOS`; `//DOS` does.
    let want = name.to_ascii_uppercase();
    let dir = dir
        .canonicalize()
        .map_err(|_| "EXTERNAL is not a folder".to_string())?;
    let mut exact = None;
    let mut stems = Vec::new();
    for ent in fs::read_dir(&dir).map_err(|err| format!("EXTERNAL: {err}"))? {
        let ent = ent.map_err(|err| format!("EXTERNAL: {err}"))?;
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let fname = ent.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        if fname.starts_with('.') {
            continue;
        }
        if fname.to_ascii_uppercase() == want {
            exact = Some(path);
            break;
        }
        let stem = Path::new(fname)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if stem.to_ascii_uppercase() == want && kind_of(&path).is_some() {
            stems.push(path);
        }
    }
    let path = if let Some(path) = exact {
        path
    } else if stems.len() == 1 {
        stems.remove(0)
    } else if stems.len() > 1 {
        return Err("use the full file name".into());
    } else {
        return Err(format!("not in EXTERNAL: {name}"));
    };
    let path = path
        .canonicalize()
        .map_err(|err| format!("EXTERNAL: {err}"))?;
    if path.parent() != Some(dir.as_path()) {
        return Err("EXTERNAL only".into());
    }
    if kind_of(&path).is_none() {
        return Err(TEXT_ONLY.into());
    }
    Ok(path)
}

fn inside(dir: &Path, path: &Path) -> bool {
    crate::paths::is_direct_child(dir, path)
}

fn kind_of(path: &Path) -> Option<Shebang> {
    match first_line(path) {
        Some(line) if line.starts_with("#!") => parse_shebang(&line),
        _ => extension_kind(path),
    }
}

fn extension_kind(path: &Path) -> Option<Shebang> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("py") | Some("pyw") => Some(plain_kind(Family::Python, "python3")),
        Some("php") => Some(plain_kind(Family::Php, "php")),
        Some("pl") => Some(plain_kind(Family::Perl, "perl")),
        Some("rb") => Some(plain_kind(Family::Ruby, "ruby")),
        Some("js") | Some("mjs") | Some("cjs") => Some(plain_kind(Family::Node, "node")),
        Some("lua") => Some(plain_kind(Family::Lua, "lua")),
        Some("java") | Some("jar") => Some(plain_kind(Family::Java, "java")),
        _ => None,
    }
}

fn plain_kind(family: Family, asked: &str) -> Shebang {
    Shebang {
        family,
        absolute: None,
        flags: Vec::new(),
        asked: asked.into(),
    }
}

fn first_line(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut buf = [0u8; 512];
    let n = file.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let line = text.lines().next()?.trim_end_matches('\r').to_string();
    Some(line)
}

/// `#! /usr/lib/cgi-bin/php7.2`, `#!/usr/bin/env python3`, `#!/usr/bin/perl -w`.
fn parse_shebang(line: &str) -> Option<Shebang> {
    let rest = line.trim().strip_prefix("#!")?.trim();
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.split_whitespace();
    let first = parts.next()?;
    let (token, flags) = if file_base(first).eq_ignore_ascii_case("env") {
        let mut token = None;
        let mut flags = Vec::new();
        for part in parts {
            if token.is_none() {
                if part.starts_with('-') {
                    continue;
                }
                token = Some(part);
            } else {
                flags.push(part);
            }
        }
        (token?, flags)
    } else {
        (first, parts.collect())
    };
    let flags = shebang_flags(&flags)?;
    let asked = file_base(token).to_string();
    let family = family_of(&asked)?;
    let absolute = if token.contains('/') || token.contains('\\') {
        Some(PathBuf::from(token))
    } else {
        None
    };
    Some(Shebang {
        family,
        absolute,
        flags,
        asked,
    })
}

/// Flags the sysop wrote after the program name. Not a path.
fn shebang_flags(parts: &[&str]) -> Option<Vec<String>> {
    if parts.len() > 8 {
        return None;
    }
    let mut flags = Vec::new();
    for part in parts {
        if part.len() > 40
            || part.contains('/')
            || part.contains('\\')
            || part.contains("..")
            || part.contains(':')
            || part.chars().any(|c| c.is_control())
        {
            return None;
        }
        flags.push((*part).to_string());
    }
    Some(flags)
}

fn file_base(token: &str) -> &str {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
}

fn family_of(base: &str) -> Option<Family> {
    let lower = base.to_ascii_lowercase();
    let lower = lower.strip_suffix(".exe").unwrap_or(lower.as_str());
    if is_python(lower) {
        Some(Family::Python)
    } else if is_php(lower) {
        Some(Family::Php)
    } else if is_named(lower, "perl") {
        Some(Family::Perl)
    } else if is_named(lower, "ruby") {
        Some(Family::Ruby)
    } else if lower == "node" || lower == "nodejs" || is_named(lower, "node") {
        Some(Family::Node)
    } else if lower == "luajit" || is_named(lower, "lua") {
        Some(Family::Lua)
    } else if lower == "java" {
        Some(Family::Java)
    } else {
        None
    }
}

fn is_named(lower: &str, name: &str) -> bool {
    if lower == name {
        return true;
    }
    match lower.strip_prefix(name) {
        Some(rest) => version_tail(rest),
        None => false,
    }
}

fn is_python(lower: &str) -> bool {
    if lower == "python" || lower == "python3" || lower == "py" {
        return true;
    }
    match lower.strip_prefix("python") {
        Some(rest) => version_tail(rest),
        None => false,
    }
}

fn is_php(lower: &str) -> bool {
    if lower == "php" {
        return true;
    }
    match lower.strip_prefix("php") {
        Some(rest) => version_tail(rest),
        None => false,
    }
}

fn version_tail(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => chars.all(|c| c.is_ascii_digit() || c == '.'),
        _ => false,
    }
}

fn parse_request(request: &str) -> Result<Ask, String> {
    let request = request.trim();
    if request.is_empty() || request == "?" {
        return Ok(Ask::List);
    }
    let mut words = request.split_whitespace();
    let name = words.next().unwrap_or("");
    if name == "?" {
        return Ok(Ask::List);
    }
    let mut args = Vec::new();
    for word in words {
        if args.len() >= 8 {
            return Err("too many arguments".into());
        }
        if word.len() > 80 {
            return Err("argument too long".into());
        }
        if word.contains("..")
            || word.contains('/')
            || word.contains('\\')
            || word.contains(':')
            || word.chars().any(|c| c.is_control())
        {
            return Err("arguments stay words, not paths".into());
        }
        args.push(word.to_string());
    }
    Ok(Ask::Run {
        name: name.to_string(),
        args,
    })
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name != "."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.contains(':')
        && !name.contains('*')
        && !name.contains('?')
        && !name.contains('"')
        && !name.chars().any(|c| c.is_control())
}

fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || *c == '\r' || *c == '\t' || !c.is_control())
        .collect()
}

fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
