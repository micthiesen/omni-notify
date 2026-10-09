//! Node `path.posix` semantics used by the path-confinement guards.

/// `path.posix.isAbsolute`.
pub fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// `path.posix.normalize`.
pub fn normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let absolute = is_absolute(p);
    let trailing = p.ends_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in p.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    let mut out = segments.join("/");
    if out.is_empty() && !absolute {
        out.push('.');
    }
    if !out.is_empty() && trailing {
        out.push('/');
    }
    if absolute { format!("/{out}") } else { out }
}

/// `path.posix.resolve(p)` for an absolute `p` (normalized, no trailing slash
/// except the root). A relative `p` is resolved against `/`, never the
/// process working directory.
pub fn resolve(p: &str) -> String {
    let normalized = normalize(&format!("/{p}"));
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `path.posix.relative(from, to)` for absolute paths.
pub fn relative(from: &str, to: &str) -> String {
    let from = resolve(from);
    let to = resolve(to);
    if from == to {
        return String::new();
    }
    let from_parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<&str> = vec![".."; from_parts.len() - common];
    out.extend(&to_parts[common..]);
    out.join("/")
}

/// `path.posix.basename(p)`.
pub fn basename(p: &str) -> &str {
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) => &trimmed[index + 1..],
        None => trimmed,
    }
}

/// `path.posix.dirname(p)`.
pub fn dirname(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let absolute = is_absolute(p);
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(index) => {
            let parent = trimmed[..index].trim_end_matches('/');
            if parent.is_empty() && absolute {
                "/".to_owned()
            } else {
                parent.to_owned()
            }
        }
        None => ".".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_node_posix() {
        assert_eq!(normalize("/tmp/inter/../../library"), "/library");
        assert_eq!(normalize("/a//b/./c/"), "/a/b/c/");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("a/../.."), "..");
        assert_eq!(normalize("./"), "./");
        assert_eq!(resolve("/a/b/"), "/a/b");
        assert_eq!(relative("/a/b", "/a/b"), "");
        assert_eq!(relative("/a/b", "/a/b/c/d"), "c/d");
        assert_eq!(relative("/a/b", "/a/x"), "../x");
        assert_eq!(relative("/", "/a"), "a");
        assert_eq!(basename("/a/b.mkv"), "b.mkv");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("name"), "name");
        assert_eq!(dirname("/tmp/inter/job"), "/tmp/inter");
        assert_eq!(dirname("/job"), "/");
        assert_eq!(dirname("job"), ".");
    }
}
