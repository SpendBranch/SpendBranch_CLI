//! `repository_key_of`: a git remote URL to `host/owner/name`, without credentials or port.

/// Accepts SCP (`git@host:owner/name.git`), `ssh://`, `https://`, `http://` and `git://`.
/// `None` for local paths, `file://`, other schemes and anything that is not `host/owner/name`.
///
/// ponytail: hand-rolled URL split, not the full WHATWG algorithm. International host names,
/// percent-escapes and dot segments end in `None` instead of being normalized. Add a URL
/// crate if real remotes need them.
pub fn repository_key_of(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if url.contains("://") {
        standard(url)?
    } else {
        let (host, path) = scp(url)?;
        (host.to_owned(), path.to_owned())
    };
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let key = format!("{}/{}", host, parts.join("/")).to_lowercase();
    (parts.len() >= 2 && key.len() <= 200 && key_shape_ok(&key)).then_some(key)
}

/// `^[^@\s]+@([^:\s]+):(.+)$`
fn scp(url: &str) -> Option<(&str, &str)> {
    let (user, rest) = url.split_once('@')?;
    let (host, path) = rest.split_once(':')?;
    let blank = |c: char| c.is_whitespace();
    let ok = !user.is_empty()
        && !user.contains(blank)
        && !host.is_empty()
        && !host.contains(blank)
        && !path.is_empty()
        && !path.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    ok.then_some((host, path))
}

fn standard(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    if !web && !scheme.eq_ignore_ascii_case("ssh") && !scheme.eq_ignore_ascii_case("git") {
        return None;
    }
    // http(s) treats a backslash like a slash.
    let rest = if web {
        rest.replace('\\', "/")
    } else {
        rest.to_owned()
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(authority_end);
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host_port.split_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && port.len() <= 5 => host,
        Some(_) => return None,
        None => host_port,
    };
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    Some((host.to_owned(), after[..path_end].to_owned()))
}

/// `^[a-z0-9.-]+(/[a-z0-9._~-]+){2,}$`
fn key_shape_ok(key: &str) -> bool {
    let mut parts = key.split('/');
    let host = parts.next().unwrap_or("");
    let segment = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || extra.contains(&c))
    };
    segment(host, &['.', '-'])
        && parts.clone().count() >= 2
        && parts.all(|s| segment(s, &['.', '_', '~', '-']))
}
