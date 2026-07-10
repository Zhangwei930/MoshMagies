//! Drop-in `mosh-client` CLI for Netcatty (and standalone use).
//!
//! ```text
//! MOSH_KEY=<key> mosh-client <host> <port>
//! ```
//!
//! Cross-platform I/O:
//! - Unix: raw + non-blocking stdin (node-pty compatible)
//! - Windows: dedicated stdin thread so UDP poll/keepalive never block
//!   (fixes ConPTY/node-pty stall — multi-agent audit CRITICAL)

use std::env;
use std::io::{self, Read, Write};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use moshcatty::Client;

fn main() {
    if let Err(e) = run() {
        eprintln!("mosh-client: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-#" || args[i] == "--help" {
            print_usage();
            process::exit(0);
        }
        if args[i].starts_with('-') {
            if args[i] == "-p" || args[i] == "-s" || args[i] == "-c" {
                args.remove(i);
                if i < args.len() {
                    args.remove(i);
                }
                continue;
            }
            args.remove(i);
            continue;
        }
        i += 1;
    }

    if args.len() < 2 {
        print_usage();
        process::exit(2);
    }

    let host = args[0].clone();
    let port: u16 = args[1]
        .parse()
        .map_err(|_| format!("invalid port: {}", args[1]))?;
    let key = env::var("MOSH_KEY").map_err(|_| "MOSH_KEY environment variable is required")?;

    let cols = env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(term_cols)
        .unwrap_or(80u16);
    let rows = env::var("LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(term_rows)
        .unwrap_or(24u16);

    let mut client = Client::dial(&host, port, &key)?;
    client.resize(cols, rows);

    let running = Arc::new(AtomicBool::new(true));
    install_signal_flag(running.clone());

    let _raw_guard = enter_raw_mode_if_tty();

    // Always use a stdin thread so the UDP loop never blocks (Unix+Windows).
    let stdin_rx = spawn_stdin_reader();

    let mut stdout = io::stdout();
    let mut last_resize_check = Instant::now();
    let mut cur_cols = cols;
    let mut cur_rows = rows;

    while running.load(Ordering::SeqCst) {
        if client.is_dead() {
            if let Some(r) = client.dead_reason() {
                eprintln!("mosh-client: {r}");
            }
            break;
        }

        let paint = client.poll()?;
        if !paint.is_empty() {
            stdout.write_all(&paint)?;
            stdout.flush()?;
        }

        match stdin_rx.try_recv() {
            Ok(Some(buf)) if !buf.is_empty() => client.send_keys(&buf),
            Ok(None) => {
                // EOF: drain remaining paint briefly then exit (PTY closed).
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline && !client.is_dead() {
                    let paint = client.poll()?;
                    if paint.is_empty() {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    stdout.write_all(&paint)?;
                    stdout.flush()?;
                }
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
            Ok(Some(_)) => {}
        }

        if last_resize_check.elapsed() > Duration::from_millis(250) {
            if let (Some(c), Some(r)) = (term_cols(), term_rows()) {
                if c != cur_cols || r != cur_rows {
                    cur_cols = c;
                    cur_rows = r;
                    client.resize(c, r);
                }
            }
            last_resize_check = Instant::now();
        }

        thread::sleep(Duration::from_millis(2));
    }

    Ok(())
}

/// Background stdin reader. Sends `Some(bytes)` on data, `None` on EOF.
fn spawn_stdin_reader() -> Receiver<Option<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(None);
                    break;
                }
                Ok(n) => {
                    if tx.send(Some(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => {
                    let _ = tx.send(None);
                    break;
                }
            }
        }
    });
    rx
}

fn print_usage() {
    eprintln!("Usage: MOSH_KEY=<key> mosh-client <host> <port>");
    eprintln!("Pure Rust Mosh client (Netcatty). No Cygwin / terminfo required.");
}

#[cfg(unix)]
fn term_cols() -> Option<u16> {
    winsize()
        .map(|w| w.ws_col)
        .or_else(|| env::var("COLUMNS").ok().and_then(|s| s.parse().ok()))
}

#[cfg(unix)]
fn term_rows() -> Option<u16> {
    winsize()
        .map(|w| w.ws_row)
        .or_else(|| env::var("LINES").ok().and_then(|s| s.parse().ok()))
}

#[cfg(not(unix))]
fn term_cols() -> Option<u16> {
    winsize_windows()
        .map(|(c, _)| c)
        .or_else(|| env::var("COLUMNS").ok().and_then(|s| s.parse().ok()))
}

#[cfg(not(unix))]
fn term_rows() -> Option<u16> {
    winsize_windows()
        .map(|(_, r)| r)
        .or_else(|| env::var("LINES").ok().and_then(|s| s.parse().ok()))
}

/// Live console size on Windows (ConPTY / node-pty). Prefer this over env so
/// Netcatty resizeSession updates reach mosh-server as UserInstruction::resize.
#[cfg(windows)]
fn winsize_windows() -> Option<(u16, u16)> {
    use std::mem::MaybeUninit;
    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor_position: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window_size: Coord,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleScreenBufferInfo(
            console_output: *mut std::ffi::c_void,
            info: *mut ConsoleScreenBufferInfo,
        ) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5; // (u32)-11
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || handle == (-1isize as *mut _) {
            return None;
        }
        let mut info = MaybeUninit::<ConsoleScreenBufferInfo>::uninit();
        if GetConsoleScreenBufferInfo(handle, info.as_mut_ptr()) == 0 {
            return None;
        }
        let info = info.assume_init();
        let cols = (info.window.right - info.window.left + 1) as u16;
        let rows = (info.window.bottom - info.window.top + 1) as u16;
        if cols > 0 && rows > 0 {
            Some((cols, rows))
        } else {
            None
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
fn winsize_windows() -> Option<(u16, u16)> {
    None
}

#[cfg(unix)]
fn winsize() -> Option<libc::winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if rc == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        Some(ws)
    } else {
        None
    }
}

#[cfg(unix)]
struct RawMode {
    fd: i32,
    original: libc::termios,
}

#[cfg(unix)]
impl RawMode {
    fn enter() -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}

#[cfg(unix)]
fn enter_raw_mode_if_tty() -> Option<RawMode> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::isatty(io::stdin().as_raw_fd()) } == 1 {
        RawMode::enter().ok()
    } else {
        None
    }
}

#[cfg(not(unix))]
fn enter_raw_mode_if_tty() -> Option<()> {
    None
}

#[cfg(unix)]
fn install_signal_flag(running: Arc<AtomicBool>) {
    unsafe {
        static mut FLAG: *const AtomicBool = std::ptr::null();
        FLAG = Arc::into_raw(running);
        extern "C" fn handler(_: i32) {
            unsafe {
                if !FLAG.is_null() {
                    (*FLAG).store(false, Ordering::SeqCst);
                }
            }
        }
        libc::signal(libc::SIGINT, handler as *const () as usize);
        libc::signal(libc::SIGTERM, handler as *const () as usize);
    }
}

#[cfg(not(unix))]
fn install_signal_flag(running: Arc<AtomicBool>) {
    std::mem::forget(running);
}
