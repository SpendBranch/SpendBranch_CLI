//! The rule that keeps the last message of a growing file from being sent too early.
use spendbranch::readers::hold::OpenMessage;

/// Feeds `(line, key)` pairs and returns what was sent, in order, and where a held read resumes.
fn run(lines: &[(&str, Option<&str>)], hold: bool) -> (Vec<String>, Option<u64>) {
    let mut sent = Vec::new();
    let mut feed = |bytes: &[u8], _: bool| sent.push(String::from_utf8_lossy(bytes).into_owned());
    let mut open = OpenMessage::default();
    let mut offset = 0u64;
    for (line, key) in lines {
        open.push(line.as_bytes(), false, offset, *key, &mut feed);
        offset += line.len() as u64 + 1;
    }
    let resume = open.finish(hold, &mut feed);
    (sent, resume)
}

#[test]
fn lines_before_any_keyed_line_are_sent_at_once() {
    let (sent, resume) = run(&[("a", None), ("b", None)], true);
    assert_eq!(sent, ["a", "b"]);
    assert_eq!(resume, None);
}

#[test]
fn the_last_message_waits_while_the_file_is_fresh() {
    let (sent, resume) = run(
        &[
            ("user", None),
            ("m1-a", Some("r1")),
            ("m1-b", Some("r1")),
            ("m2-a", Some("r2")),
            ("m2-b", Some("r2")),
        ],
        true,
    );
    assert_eq!(
        sent,
        ["user", "m1-a", "m1-b"],
        "r1 is settled because r2 followed it; r2 may still grow"
    );
    // "user\n" (5) + "m1-a\n" (5) + "m1-b\n" (5) = 15: the next read starts at m2-a
    assert_eq!(resume, Some(15));
}

#[test]
fn a_settled_file_sends_everything_in_order() {
    let lines = [
        ("m1", Some("r1")),
        ("x", None),
        ("m1b", Some("r1")),
        ("m2", Some("r2")),
    ];
    let (sent, resume) = run(&lines, false);
    assert_eq!(sent, ["m1", "x", "m1b", "m2"]);
    assert_eq!(resume, None);
}

#[test]
fn a_line_without_a_key_stays_with_the_open_message() {
    let (sent, resume) = run(&[("m1", Some("r1")), ("note", None)], true);
    assert!(
        sent.is_empty(),
        "the note belongs to the message still open"
    );
    assert_eq!(resume, Some(0));
}

#[test]
fn a_new_message_releases_the_previous_one_including_its_keyless_lines() {
    let (sent, resume) = run(
        &[("m1", Some("r1")), ("note", None), ("m2", Some("r2"))],
        true,
    );
    assert_eq!(sent, ["m1", "note"]);
    assert_eq!(resume, Some(8)); // "m1\n" + "note\n"
}

#[test]
fn an_empty_file_holds_nothing() {
    assert_eq!(run(&[], true), (vec![], None));
}

#[test]
fn a_message_that_comes_back_after_another_is_a_new_message() {
    // r1, r2, r1 again: the middle one closed the first, so nothing is merged here
    let (sent, _) = run(
        &[("a", Some("r1")), ("b", Some("r2")), ("c", Some("r1"))],
        true,
    );
    assert_eq!(sent, ["a", "b"]);
}
