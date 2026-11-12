//! The validated text types: each one checks the contract's pattern in its constructor.
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

macro_rules! text_type {
    ($(#[$doc:meta])* $name:ident, $check:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                // The message never repeats the text: it may come from a tampered file.
                Self::new(&text).ok_or_else(|| D::Error::custom(concat!("invalid ", stringify!($name))))
            }
        }
        impl $name {
            /// `None` when the text breaks the contract's pattern or length.
            pub fn new(text: &str) -> Option<Self> {
                let check: fn(&str) -> bool = $check;
                check(text).then(|| Self(text.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

fn all(text: &str, max: usize, ok: impl Fn(char) -> bool) -> bool {
    (1..=max).contains(&text.len()) && text.chars().all(ok)
}

text_type!(
    /// `^[A-Za-z0-9_:-]{1,128}$`
    ExternalId,
    |t| all(t, 128, |c| c.is_ascii_alphanumeric()
        || matches!(c, '_' | ':' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9<>._-]{1,64}$`
    Model,
    |t| all(t, 64, |c| c.is_ascii_alphanumeric()
        || matches!(c, '<' | '>' | '.' | '_' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9._+-]{1,32}$`: the agent's own version.
    SourceVersion,
    |t| all(t, 32, |c| c.is_ascii_alphanumeric()
        || matches!(c, '.' | '_' | '+' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9._:-]{1,200}$`: the provider account that pays.
    AccountRef,
    |t| all(t, 200, |c| c.is_ascii_alphanumeric()
        || matches!(c, '.' | '_' | ':' | '-'))
);
text_type!(
    /// `^[a-z0-9-]{1,48}/v[0-9]{1,4}$`, e.g. `claude-code/v2`.
    ParserVersion,
    |t| {
        let Some((name, v)) = t.split_once("/v") else {
            return false;
        };
        all(name, 48, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        }) && all(v, 4, |c| c.is_ascii_digit())
    }
);
text_type!(
    /// `host[:port]/owner/name[/...]`, at most 200 characters.
    Repository,
    |t| {
        let mut parts = t.split('/');
        let host_ok = parts.next().is_some_and(|h| {
            let (host, port) = h.split_once(':').map_or((h, None), |(a, b)| (a, Some(b)));
            all(host, 200, |c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-')
            }) && port.is_none_or(|p| all(p, 5, |c| c.is_ascii_digit()))
        });
        let segments: Vec<&str> = parts.collect();
        t.len() <= 200
            && host_ok
            && segments.len() >= 2
            && segments.iter().all(|s| {
                all(s, 200, |c| {
                    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-')
                })
            })
    }
);
text_type!(
    /// 1 to 255 code points, none of them whitespace, control or invisible format characters.
    Branch,
    |t| { (1..=255).contains(&t.chars().count()) && t.chars().all(is_branch_char) }
);

/// No whitespace, no control (Cc) and no format (Cf: bidi marks, zero-width, BOM...) characters.
fn is_branch_char(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || is_format_char(c))
}

/// Unicode general category Cf. Std has no such table; this is the Unicode 16 list.
fn is_format_char(c: char) -> bool {
    matches!(u32::from(c),
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}
