//! Small terminal UI helpers for modern human-readable output.

use crate::args::AccentArg;
use unicode_width::UnicodeWidthChar;

const CONTROL_REPLACEMENT: char = '�';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Primary,
    Secondary,
    Success,
    Warning,
    Danger,
}

pub(crate) fn paint_tone(enabled: bool, accent: AccentArg, tone: Tone, text: &str) -> String {
    let text = terminal_safe_text(text);
    if enabled {
        format!("\x1b[{}m{text}\x1b[0m", tone_code(accent, tone))
    } else {
        text
    }
}

pub(crate) fn tone_code(accent: AccentArg, tone: Tone) -> &'static str {
    match (accent, tone) {
        (AccentArg::Squallz, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Squallz, Tone::Secondary) => "38;2;14;165;233",
        (AccentArg::Brand, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Brand, Tone::Secondary) => "38;2;14;165;233",
        (AccentArg::Icon, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Icon, Tone::Secondary) => "38;2;14;165;233",
        (AccentArg::Cascade, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Cascade, Tone::Secondary) => "38;2;125;211;252",
        (AccentArg::Daylight, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Daylight, Tone::Secondary) => "38;2;103;232;249",
        (AccentArg::Foam, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Foam, Tone::Secondary) => "38;2;224;242;254",
        (AccentArg::Skyline, Tone::Primary) => "1;38;2;14;165;233",
        (AccentArg::Skyline, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Aero, Tone::Primary) => "1;38;2;125;211;252",
        (AccentArg::Aero, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Crest, Tone::Primary) => "1;38;2;56;189;248",
        (AccentArg::Crest, Tone::Secondary) => "38;2;94;234;212",
        (AccentArg::Halo, Tone::Primary) => "1;38;2;94;234;212",
        (AccentArg::Halo, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Tropic, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Tropic, Tone::Secondary) => "38;2;34;211;238",
        (AccentArg::Kinetic, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Kinetic, Tone::Secondary) => "38;2;96;165;250",
        (AccentArg::Radiant, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Radiant, Tone::Secondary) => "38;2;186;230;253",
        (AccentArg::Surge, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Surge, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Glass, Tone::Primary) => "1;38;2;103;232;249",
        (AccentArg::Glass, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Nova, Tone::Primary) => "1;38;2;34;211;238",
        (AccentArg::Nova, Tone::Secondary) => "38;2;250;204;21",
        (AccentArg::Crystal, Tone::Primary) => "1;38;2;94;234;212",
        (AccentArg::Crystal, Tone::Secondary) => "38;2;125;211;252",
        (AccentArg::Lumina, Tone::Primary) => "1;38;2;103;232;249",
        (AccentArg::Lumina, Tone::Secondary) => "38;2;251;113;133",
        (AccentArg::Azure, Tone::Primary) => "1;38;2;56;189;248",
        (AccentArg::Azure, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Surf, Tone::Primary) => "1;38;2;34;211;238",
        (AccentArg::Surf, Tone::Secondary) => "38;2;14;165;233",
        (AccentArg::Signal, Tone::Primary) => "1;38;2;94;234;212",
        (AccentArg::Signal, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Tide, Tone::Primary) => "1;38;2;103;232;249",
        (AccentArg::Tide, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Breeze, Tone::Primary) => "1;38;2;20;184;166",
        (AccentArg::Breeze, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Neon, Tone::Primary) => "1;38;2;34;211;238",
        (AccentArg::Neon, Tone::Secondary) => "38;2;244;114;182",
        (AccentArg::Electric, Tone::Primary) => "1;38;2;34;211;238",
        (AccentArg::Electric, Tone::Secondary) => "38;2;167;139;250",
        (AccentArg::Vapor, Tone::Primary) => "1;38;2;125;211;252",
        (AccentArg::Vapor, Tone::Secondary) => "38;2;192;132;252",
        (AccentArg::Ocean, Tone::Primary) => "1;38;2;14;165;233",
        (AccentArg::Ocean, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Jade, Tone::Primary) => "1;38;2;52;211;153",
        (AccentArg::Jade, Tone::Secondary) => "38;2;45;212;191",
        (AccentArg::Teal, Tone::Primary) => "1;38;5;45",
        (AccentArg::Teal, Tone::Secondary) => "38;5;44",
        (AccentArg::Aqua, Tone::Primary) => "1;38;5;51",
        (AccentArg::Aqua, Tone::Secondary) => "38;5;43",
        (AccentArg::Glacier, Tone::Primary) => "1;38;5;87",
        (AccentArg::Glacier, Tone::Secondary) => "38;5;117",
        (AccentArg::Aurora, Tone::Primary) => "1;38;5;86",
        (AccentArg::Aurora, Tone::Secondary) => "38;5;81",
        (AccentArg::Prism, Tone::Primary) => "1;38;5;51",
        (AccentArg::Prism, Tone::Secondary) => "38;5;213",
        (AccentArg::Lagoon, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Lagoon, Tone::Secondary) => "38;2;56;189;248",
        (AccentArg::Mint, Tone::Primary) => "1;38;2;45;212;191",
        (AccentArg::Mint, Tone::Secondary) => "38;2;125;211;252",
        (AccentArg::Sunset, Tone::Primary) => "1;38;2;251;146;60",
        (AccentArg::Sunset, Tone::Secondary) => "38;2;244;114;182",
        (AccentArg::Citrus, Tone::Primary) => "1;38;2;163;230;53",
        (AccentArg::Citrus, Tone::Secondary) => "38;2;34;211;238",
        (AccentArg::Blue, Tone::Primary) => "1;38;5;39",
        (AccentArg::Blue, Tone::Secondary) => "38;5;33",
        (AccentArg::Violet, Tone::Primary) => "1;38;5;141",
        (AccentArg::Violet, Tone::Secondary) => "38;5;99",
        (AccentArg::Amber, Tone::Primary) => "1;38;5;214",
        (AccentArg::Amber, Tone::Secondary) => "38;5;178",
        (AccentArg::Rose, Tone::Primary) => "1;38;5;205",
        (AccentArg::Rose, Tone::Secondary) => "38;5;204",
        (AccentArg::Mono, Tone::Primary) => "1;37",
        (AccentArg::Mono, Tone::Secondary) => "37",
        (_, Tone::Success) => "1;32",
        (_, Tone::Warning) => "1;33",
        (_, Tone::Danger) => "1;31",
    }
}

pub(crate) fn table_rule(left: &str, join: &str, right: &str, widths: &[usize]) -> String {
    let body = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>()
        .join(join);
    format!("{left}{body}{right}")
}

pub(crate) fn table_title_rule(title: &str, widths: &[usize]) -> String {
    let total_width = table_width(widths);
    let title_budget = total_width.saturating_sub(6);
    let title = truncate_end(title, title_budget);
    let prefix = format!("╭─ {title} ");
    let used = display_width(&prefix) + display_width("╮");
    let fill = "─".repeat(total_width.saturating_sub(used));
    format!("{prefix}{fill}╮")
}

pub(crate) fn table_width(widths: &[usize]) -> usize {
    if widths.is_empty() {
        return 2;
    }
    widths.iter().map(|width| width + 2).sum::<usize>() + widths.len() + 1
}

pub(crate) fn panel_title_rule(title: &str, inner_width: usize) -> String {
    let total_width = inner_width + 4;
    let title_budget = total_width.saturating_sub(6);
    let title = truncate_end(title, title_budget);
    let prefix = format!("╭─ {title} ");
    let used = display_width(&prefix) + display_width("╮");
    let fill = "─".repeat(total_width.saturating_sub(used));
    format!("{prefix}{fill}╮")
}

pub(crate) fn panel_separator(inner_width: usize) -> String {
    format!("├{}┤", "─".repeat(inner_width + 2))
}

pub(crate) fn panel_bottom_rule(inner_width: usize) -> String {
    format!("╰{}╯", "─".repeat(inner_width + 2))
}

pub(crate) fn panel_content_line(content: &str, inner_width: usize) -> String {
    format!("│ {} │", pad_end(content, inner_width))
}

pub(crate) fn display_width(value: &str) -> usize {
    value.chars().map(char_display_width).sum()
}

pub(crate) fn pad_end(value: &str, width: usize) -> String {
    let value = truncate_end(value, width);
    let padding = width.saturating_sub(display_width(&value));
    format!("{value}{}", " ".repeat(padding))
}

pub(crate) fn pad_start(value: &str, width: usize) -> String {
    let value = truncate_end(value, width);
    let padding = width.saturating_sub(display_width(&value));
    format!("{}{value}", " ".repeat(padding))
}

pub(crate) fn truncate_end(value: &str, max_width: usize) -> String {
    if display_width(value) <= max_width {
        return terminal_safe_text(value);
    }
    if max_width == 0 {
        return String::new();
    }
    let ellipsis = "…";
    let ellipsis_width = display_width(ellipsis);
    if max_width <= ellipsis_width {
        return ellipsis.to_owned();
    }

    let content_width = max_width - ellipsis_width;
    let mut out = String::new();
    let mut used = 0;
    for raw_ch in value.chars() {
        let ch = terminal_safe_char(raw_ch);
        let width = char_display_width(ch);
        if used + width > content_width {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push_str(ellipsis);
    out
}

pub(crate) fn wrap_words(value: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let word = truncate_end(word, max_width);
        let word_len = display_width(&word);
        let line_len = display_width(&line);
        if line.is_empty() {
            line.push_str(&word);
        } else if line_len + 1 + word_len <= max_width {
            line.push(' ');
            line.push_str(&word);
        } else {
            lines.push(line);
            line = word;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn terminal_safe_text(value: &str) -> String {
    value.chars().map(terminal_safe_char).collect()
}

fn terminal_safe_char(ch: char) -> char {
    if ch.is_control() {
        CONTROL_REPLACEMENT
    } else {
        ch
    }
}

fn char_display_width(ch: char) -> usize {
    let Some(width) = terminal_safe_char(ch).width() else {
        return 0;
    };
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_without_splitting_short_tokens() {
        assert_eq!(
            wrap_words("zip, tar, 7z, wim, sqz", 12),
            vec!["zip, tar,", "7z, wim, sqz"]
        );
    }

    #[test]
    fn wraps_long_tokens_by_truncating_to_cell_width() {
        assert_eq!(wrap_words("archive-with-a-long-name", 8), vec!["archive…"]);
    }

    #[test]
    fn pads_and_truncates_by_display_width() {
        assert_eq!(display_width("中文"), 4);
        assert_eq!(pad_end("中文", 6), "中文  ");
        assert_eq!(pad_start("中文", 6), "  中文");
        assert_eq!(truncate_end("压缩格式支持", 5), "压缩…");
        assert!(display_width(&truncate_end("压缩格式支持", 5)) <= 5);
    }

    #[test]
    fn wraps_cjk_words_by_display_width() {
        assert_eq!(
            wrap_words("压缩 格式 支持", 6),
            vec!["压缩", "格式", "支持"]
        );
        assert_eq!(wrap_words("压缩格式支持", 5), vec!["压缩…"]);
    }

    #[test]
    fn panel_title_matches_panel_width_with_cjk() {
        let title = panel_title_rule("支持格式", 12);
        let content = panel_content_line("中文", 12);
        let separator = panel_separator(12);
        assert_eq!(display_width(&title), display_width(&content));
        assert_eq!(display_width(&separator), display_width(&content));
    }

    #[test]
    fn control_characters_render_as_visible_cells() {
        assert_eq!(display_width("a\u{1b}b"), 3);
        assert_eq!(truncate_end("a\u{1b}bc", 3), "a�…");
        assert_eq!(pad_end("a\u{1b}", 4), "a�  ");
        assert_eq!(
            paint_tone(false, AccentArg::Mono, Tone::Primary, "ok\u{1b}[31m"),
            "ok�[31m"
        );
    }
}
