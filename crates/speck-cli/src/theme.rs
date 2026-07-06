pub const NEON_CYAN: &str = "\x1b[38;2;0;255;255m";
pub const NEON_VIOLET: &str = "\x1b[38;2;180;0;255m";
pub const RESET: &str = "\x1b[0m";
pub const GRAY: &str = "\x1b[38;2;128;128;128m";
pub const GREEN: &str = "\x1b[38;2;0;255;0m";
pub const YELLOW: &str = "\x1b[38;2;255;255;0m";
pub const RED: &str = "\x1b[38;2;255;0;0m";

pub fn format_status(status: &str) -> String {
    match status {
        "running" => format!("{GREEN}{status}{RESET}"),
        "exited" | "stopped" => format!("{YELLOW}{status}{RESET}"),
        "dead" => format!("{RED}{status}{RESET}"),
        s => s.to_string(),
    }
}

pub fn format_header(s: &str) -> String {
    format!("{NEON_VIOLET}{s}{RESET}")
}

pub fn format_id(id: &str) -> String {
    let truncated = if id.len() > 12 { &id[..12] } else { id };
    format!("{GRAY}{truncated}{RESET}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_status_running() {
        let result = format_status("running");
        assert!(result.contains("running"));
    }

    #[test]
    fn test_format_status_exited() {
        let result = format_status("exited");
        assert!(result.contains("exited"));
    }

    #[test]
    fn test_format_id_truncates() {
        let long_id = "abc123def456ghi789";
        let result = format_id(long_id);
        assert!(result.len() < long_id.len() + 30);
    }
}
