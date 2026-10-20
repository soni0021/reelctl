//! Word timing with whisper.cpp, through `whisper-rs`.

#[cfg(test)]
mod tests {
    #[test]
    fn links_the_documented_whisper_cpp_version() {
        // docs/specs/whisper-asr.md and asr_note name this version.
        assert_eq!(whisper_rs::get_whisper_version(), "1.8.3");
    }
}
