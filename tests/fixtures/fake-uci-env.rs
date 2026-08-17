use std::io::{self, BufRead, Write};

fn main() {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let worker = std::thread::Builder::new()
        .name("fake-uci-worker".into())
        .spawn(move || {
            let _ = shutdown_rx.recv();
        })
        .expect("spawn fake UCI worker thread");
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        match line.expect("fake UCI input").as_str() {
            "uci" => {
                let mut environment: Vec<_> = std::env::vars_os().collect();
                environment.sort_by(|left, right| left.0.cmp(&right.0));
                write!(stdout, "id name ENV:").unwrap();
                for (name, value) in environment {
                    write!(
                        stdout,
                        "{}={}\u{1c}",
                        name.to_string_lossy(),
                        value.to_string_lossy()
                    )
                    .unwrap();
                }
                writeln!(stdout, "\nuciok").unwrap();
            }
            "isready" => writeln!(stdout, "readyok").unwrap(),
            "quit" => break,
            _ => {}
        }
        stdout.flush().unwrap();
    }
    drop(shutdown_tx);
    worker.join().expect("join fake UCI worker thread");
}
