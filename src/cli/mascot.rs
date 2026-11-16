//! The owl mascot above the help on a terminal. Pure: no I/O.
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

/// Text beside the owl, by art row.
const SIDE: [(usize, &str); 3] = [
    (5, "SpendBranch CLI"),
    (7, "Start with: spendbranch setup"),
    (9, "Or: spendbranch --help for all commands"),
];
const ART_WIDTH: usize = 36;

/// The owl with its side text; truecolor when `color`, plain characters otherwise.
pub fn mascot(color: bool) -> String {
    let mut out = String::new();
    for (row, (art, ink)) in OWL.iter().zip(OWL_INK).enumerate() {
        let mut width = 0;
        for (ch, key) in art.chars().zip(ink.chars()) {
            match PALETTE.iter().find(|(k, _)| *k == key) {
                Some((_, [r, g, b])) if color && ch != ' ' => {
                    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{ch}\x1b[0m"));
                }
                _ => out.push(ch),
            }
            width += 1;
        }
        if let Some((_, text)) = SIDE.iter().find(|(r, _)| *r == row) {
            out.push_str(&" ".repeat(ART_WIDTH + 4 - width));
            if color {
                out.push_str(&format!("\x1b[1m{text}\x1b[0m"));
            } else {
                out.push_str(text);
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn plain_mascot_has_no_escape_codes() {
        let plain = mascot(false);
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("( @ )") && plain.contains("SpendBranch CLI"));
        assert!(mascot(true).contains("\x1b[38;2;"));
    }
}
