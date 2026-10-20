use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let path = args.next().map_or_else(
        || PathBuf::from("crates/edit-dna/schema/edit_dna.schema.json"),
        PathBuf::from,
    );
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!("error: {err}");
                return ExitCode::from(1);
            }
        }
    }
    let schema = edit_dna::json_schema();
    let body = match serde_json::to_string_pretty(&schema) {
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
