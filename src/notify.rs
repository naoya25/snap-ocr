/// Show a macOS notification via `osascript`. Never pass secrets here.
pub fn notify(title: &str, message: &str) {
    let script = format!(
        "display notification {} with title {}",
        applescript_quote(message),
        applescript_quote(title)
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output();
}

fn applescript_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
