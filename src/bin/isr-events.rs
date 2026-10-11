//! Replay or follow a retained run ledger without starting a guest process.
use cranelift_isr::live::LiveFeed;
fn main() {
    if let Err(e) = run() {
        eprintln!("isr-events: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut bind = "127.0.0.1:0".parse()?;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" => {
                println!("isr-events EVENTS.jsonl [--bind 127.0.0.1:0]\nGET /events?after=N (or Last-Event-ID: N) follows and replays grouped ledger events; GET /groups describes operations.");
                return Ok(());
            }
            "--bind" => bind = args.next().ok_or("missing bind address")?.parse()?,
            _ if !arg.starts_with('-') && path.is_none() => {
                path = Some(std::path::PathBuf::from(arg))
            }
            _ => return Err(format!("unknown or duplicate input: {arg}").into()),
        }
    }
    let feed = LiveFeed::from_file(&path.ok_or("expected events.jsonl; use --help")?, bind)?;
    println!("{}", feed.endpoint());
    loop {
        std::thread::park();
    }
}
