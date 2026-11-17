//! The owl on the start screen, on a terminal. Pure: no I/O.
//!
//! Each art line has an ink line of the same length: one palette letter per character
//! (a space means no color). Drawn with halftone dots and ASCII details.

const OWL: [&str; 18] = [
    "        ::                 ::",
    "         :::             :::",
    "           .**o••••••**:*.",
    "         •••••••••o••••••••",
    "        •••.-~-.•••••.-~-.••",
    "       *•●•( @ )•●●●•( @ )••:",
    "       +•••`-.-'•••••`-.-'••*",
    "      .:.•••••••••\\/••••••o+:+",
    "      :.:.*•••••●•''•••oo++:+::",
    "     :.:.:.•••●●·●••·●••*+:+:+:",
    "     .:.:.:•●●·●●●·●●●·●•:+:+:+",
    "     :.:.:.•·●●●·●●●·●●●•+:+:+:",
    "      :.:.*•●●·●●●·●●●·●•++:+::",
    "      .:.:*o••●●·●●●·●••:+:+:+",
    "        . ++*••••••••••:+. .:",
    "             ::vVvo•::vVv",
    " ===================================",
    "   ~*                           *~",
];
const OWL_INK: [&str; 18] = [
    "        ab                 ba",
    "         acb             bca",
    "           acdddddddddebca",
    "         ffffffggddggffffff",
    "        fffbbbbbfffffbbbbbff",
    "       dfffhhhhhfffffhhhhhffb",
    "       ifffbbbbbfffffbbbbbffc",
    "      jjjkgffffglmnngffffopaaa",
    "     jjjjjiddmqqqqnnqqreesaaaaa",
    "     jjjjjjdqqqqqqqqqqqqeaaaaaa",
    "     jjjjjjqqqqqqqqqqqqqqaaaaaa",
    "     jjjjjjqqqqqqqqqqqqqqaaaaaa",
    "     jjjjjtqqqqqqqqqqqqqqcaaaaa",
    "      jjjjeeuuuuuuuuuuuucaaaaa",
    "       jj bbbvwwwwwwwwxaaa aa",
    "             bbyyyxxaayyy",
    " zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    "   {{                           {{",
];
const PALETTE: [(char, [u8; 3]); 27] = [
    ('a', [90, 60, 32]),
    ('b', [122, 84, 48]),
    ('c', [106, 72, 40]),
    ('d', [184, 140, 92]),
    ('e', [156, 112, 68]),
    ('f', [244, 230, 200]),
    ('g', [214, 185, 146]),
    ('h', [240, 176, 32]),
    ('i', [121, 89, 56]),
    ('j', [58, 38, 20]),
    ('k', [151, 134, 110]),
    ('l', [242, 225, 190]),
    ('m', [212, 180, 136]),
    ('n', [240, 138, 32]),
    ('o', [200, 171, 134]),
    ('p', [167, 145, 116]),
    ('q', [240, 220, 180]),
    ('r', [198, 166, 124]),
    ('s', [123, 86, 50]),
    ('t', [107, 75, 44]),
    ('u', [232, 210, 166]),
    ('v', [173, 142, 100]),
    ('w', [224, 200, 152]),
    ('x', [157, 130, 92]),
    ('y', [240, 160, 48]),
    ('z', [106, 70, 38]),
    ('{', [76, 170, 72]),
];

/// The owl, one string per row; truecolor when `color`, plain characters otherwise. A color
/// code goes out only where the color changes, and one reset ends each colored run.
pub fn owl_lines(color: bool) -> Vec<String> {
    use std::fmt::Write;
    let mut lines = Vec::new();
    for (art, ink) in OWL.iter().zip(OWL_INK) {
        let mut out = String::new();
        let mut current: Option<&[u8; 3]> = None;
        for (ch, key) in art.chars().zip(ink.chars()) {
            let want = match PALETTE.iter().find(|(k, _)| *k == key) {
                Some((_, rgb)) if color && ch != ' ' => Some(rgb),
                // A space shows no color, so it never breaks a run.
                _ if ch == ' ' => current,
                _ => None,
            };
            if want != current {
                match want {
                    Some([r, g, b]) => _ = write!(out, "\x1b[38;2;{r};{g};{b}m"),
                    None => out.push_str("\x1b[0m"),
                }
                current = want;
            }
            out.push(ch);
        }
        if current.is_some() {
            out.push_str("\x1b[0m");
        }
        lines.push(out);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owl's pane on the start screen is 40 columns, with a 2-column margin each side.
    const ART_WIDTH: usize = 36;

    #[test]
    fn every_art_line_has_a_matching_ink_line_with_known_colors() {
        for (art, ink) in OWL.iter().zip(OWL_INK) {
            assert_eq!(art.chars().count(), ink.chars().count(), "{art}");
            assert!(
                ink.chars()
                    .all(|k| k == ' ' || PALETTE.iter().any(|(p, _)| *p == k))
            );
            assert!(art.chars().count() <= ART_WIDTH);
        }
    }

    #[test]
    fn plain_owl_has_no_escape_codes() {
        let plain = owl_lines(false).concat();
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("( @ )"));
        assert_eq!(owl_lines(false), OWL.map(str::to_owned));
    }

    #[test]
    fn the_colored_owl_sends_a_color_only_when_it_changes() {
        let colored = owl_lines(true).concat();
        let codes = colored.matches("\x1b[38;2;").count();
        let inked: usize = OWL
            .iter()
            .map(|l| l.chars().filter(|c| *c != ' ').count())
            .sum();
        assert!(
            codes * 2 < inked,
            "{codes} color codes for {inked} characters"
        );
    }
}
