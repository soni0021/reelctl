use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let dir = env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("crates/dataset/schema"), PathBuf::from);
    for (name, schema) in [
        ("record.schema.json", dataset::record_schema()),
        ("manifest.schema.json", dataset::manifest_schema()),
    ] {
        let body = match serde_json::to_string_pretty(&schema) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::from(1);
            }
        };
        let path = dir.join(name);
        if let Err(err) = fs::write(&path, format!("{body}\n")) {
            eprintln!("error: {}: {err}", path.display());
            return ExitCode::from(1);
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
