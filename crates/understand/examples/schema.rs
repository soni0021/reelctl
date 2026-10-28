use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = env::args().nth(1).map_or_else(
        || PathBuf::from("crates/understand/schema/understanding.schema.json"),
        PathBuf::from,
    );
    let body = match serde_json::to_string_pretty(&understand::json_schema()) {
        Ok(body) => body,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = fs::write(&path, format!("{body}\n")) {
        eprintln!("error: {err}");
        return ExitCode::from(1);
    }
    println!("wrote {}", path.display());
    ExitCode::SUCCESS
}
