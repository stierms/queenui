//! Fake UCI engine that maps an 80 MiB sparse file at startup.
//!
//! QueenUI's Hash budget for the matching test is 32 MiB. Mapping this file
//! only succeeds if `RLIMIT_AS` includes tablebase address-space headroom.

use std::{
    fs::OpenOptions,
    io::{self, BufRead, Write},
    os::unix::io::AsRawFd,
};

const MAP_BYTES: usize = 80 * 1024 * 1024;
const PROT_READ: i32 = 1;
const MAP_SHARED: i32 = 1;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
}

fn main() {
    let path = std::env::temp_dir().join(format!("queenui-mmap-{}.tb", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("create sparse tablebase stand-in");
    file.set_len(MAP_BYTES as u64)
        .expect("truncate sparse tablebase stand-in");
    let mapped = unsafe {
        mmap(
            std::ptr::null_mut(),
            MAP_BYTES,
            PROT_READ,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    let mmap_ok = mapped != (!0usize as *mut u8);
    std::mem::forget(file);

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        match line.expect("fake UCI input").as_str() {
            "uci" => {
                writeln!(stdout, "id name MmapProbe").unwrap();
                writeln!(stdout, "uciok").unwrap();
            }
            "isready" => writeln!(stdout, "readyok").unwrap(),
            line if line.starts_with("go") => {
                if !mmap_ok {
                    std::process::exit(2);
                }
                writeln!(stdout, "info depth 1 score cp 0 pv e2e4").unwrap();
                writeln!(stdout, "bestmove e2e4").unwrap();
            }
            "quit" => break,
            _ => {}
        }
        stdout.flush().unwrap();
    }
    let _ = std::fs::remove_file(path);
}
