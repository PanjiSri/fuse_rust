use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::exit;
use std::env;

const SOCKET_PATH: &str = "/tmp/fuselog.sock";

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let command = if args.len() > 1 && args[1] == "--train" {
        b't'
    } else {
        b'g'
    };

    let mut stream = UnixStream::connect(SOCKET_PATH)
        .map_err(|e| format!("Failed to connect to socket: {}", e))?;

    stream.write_all(&[command])?;

    if command == b't' {
        println!("'train' command sent to fuselog_core.");
        return Ok(());
    }

    let mut size_buf = [0u8; 8];
    stream.read_exact(&mut size_buf)?;
    let size = u64::from_le_bytes(size_buf) as usize;

    if size == 0 {
        eprintln!("Info: There is no diff to display.");
        return Ok(());
    }

    let mut buffer = vec![0u8; size];
    stream.read_exact(&mut buffer)?;

    std::io::stdout().write_all(&buffer)?;

    Ok(())
}