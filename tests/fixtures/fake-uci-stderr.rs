//! Emits repeated mapping failures, including invalid UTF-8, on stderr.
use std::io::{self, BufRead, Write};
fn main() {
    let mut stdout = io::stdout().lock();
    let mut oversized = false;
    for line in io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "uci" => {
                writeln!(stdout, "id name StderrProbe").unwrap();
                writeln!(stdout, "uciok").unwrap();
            }
            "isready" => writeln!(stdout, "readyok").unwrap(),
            "setoption name Oversized value true" => oversized = true,
            line if line.starts_with("go") => {
                let mut stderr = io::stderr().lock();
                if oversized {
                    stderr.write_all(&vec![b'x'; 70 * 1024]).unwrap();
                    stderr.write_all(b"\n").unwrap();
                } else {
                    stderr
                        .write_all(b"ordinary message with invalid byte \xff\n")
                        .unwrap();
                    for _ in 0..206 {
                        stderr.write_all(b"Could not map KQvKR\xff.rtbz into memory; continuing without it.\n").unwrap();
                    }
                    stderr.write_all(b"mmap: Cannot allocate memory\n").unwrap();
                }
                stderr.flush().unwrap();
                // The parent must drain stderr before this barrier's readyok.
                writeln!(stdout, "bestmove e2e4").unwrap();
            }
            "quit" => break,
            _ => {}
        }
        stdout.flush().unwrap();
    }
}
