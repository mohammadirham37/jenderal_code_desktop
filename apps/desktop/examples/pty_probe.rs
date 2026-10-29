//! PTY behavior probe: spawn a shell exactly like TerminalManager does,
//! send text + Backspace (0x7f), and dump the raw response bytes.
//! Run: cargo run -p jenderalcode-desktop --example pty_probe [-- <shell> <term>]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let shell = args.get(1).map(|s| s.as_str()).unwrap_or("/bin/zsh");
    let term = args.get(2).map(|s| s.as_str()).to_owned();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-i");
    if let Some(t) = &term {
        cmd.env("TERM", t);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let collect = |ms: u64, label: &str| {
        let mut acc = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        while let Ok(d) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            acc.extend_from_slice(&d);
        }
        println!("[{label}] {} bytes: {:?}", acc.len(), String::from_utf8_lossy(&acc));
        println!("      hex: {}", acc.iter().map(|b| format!("{b:02x}")).collect::<String>());
        acc
    };

    let inherited_term = std::env::var("TERM").ok();
    println!("shell={shell} TERM={:?} (inherited={:?})", term.clone(), inherited_term);
    collect(1200, "startup/prompt");

    writer.write_all(b"abc").unwrap();
    writer.flush().unwrap();
    collect(600, "after 'abc'");

    writer.write_all(&[0x7f]).unwrap();
    writer.flush().unwrap();
    let after_bs = collect(600, "after BACKSPACE (0x7f)");

    // Healthy ZLE/readline response moves the cursor left ("\x08 \x08" or a
    // line redraw). Unhealthy: literal echo like "^?" (0x5e 0x3f) or nothing.
    let echo_caret = after_bs.windows(2).any(|w| w == b"^?");
    let moved_left =
        after_bs.contains(&0x08) || after_bs.contains(&0x15) || after_bs.windows(2).any(|w| w == b"\x1b[");
    println!("RESULT: caret-echo={}, cursor-left-redraw={}", echo_caret, moved_left);

    let _ = child.kill();
    let _ = child.wait();
}
