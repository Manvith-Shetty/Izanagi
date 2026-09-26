//! Small shared helpers.

use std::str::FromStr;

/// Read an environment variable and parse it into `F`.
///
/// Named `_unsafe` because it is the *caller's* job to decide whether a missing or
/// malformed value is fatal: it returns a `Result` rather than panicking, and every
/// failure is logged with the variable name so a misconfigured deployment says which
/// key it choked on.
///
/// ```no_run
/// # use common::utils::get_from_env_unsafe;
/// let port: u16 = get_from_env_unsafe("PORT")?;
/// let debug: bool = get_from_env_unsafe("DEBUG").unwrap_or(false);
/// # Ok::<(), String>(())
/// ```
pub fn get_from_env_unsafe<F>(name: &str) -> Result<F, String>
where
    F: FromStr,
    <F as FromStr>::Err: std::fmt::Debug,
{
    let var = std::env::var(name).map_err(|e| {
        tracing::info!("{name} env not found with error: {e}");
        format!("{name} env not found with error: {e}")
    })?;

    var.parse().map_err(|e| {
        tracing::info!("{name} failed to parse env value to expected type: {e:?}");
        format!("{name} failed to parse env value to expected type: {e:?}")
    })
}

/// Load a crate's `.env` into the process environment -- all of it, or not at all.
///
/// `dotenvy` alone fails quietly in a way that cost real debugging time: a quote or apostrophe
/// inside an inline comment (`KEY=1  # the tab's limit`) opens a string that swallows the
/// rest of the file, and depending on what follows it either errors or silently drops keys.
/// Services used to ignore that error, so they ran on defaults -- the wrong chain, no control
/// token -- while the file looked correct.
///
/// This checks every key the file declares against what was actually loaded, and refuses to
/// start if one went missing, naming the key and line. It never prints a value: the raw parser
/// error echoes the rest of the file, secrets included.
///
/// A missing file is fine (returns 0). Values already exported in the environment win, as
/// with `dotenvy`.
pub fn load_env_file(path: &std::path::Path) -> Result<usize, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let declared = declared_keys(&text);

    let iter = dotenvy::from_path_iter(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut loaded = std::collections::HashSet::new();
    for item in iter {
        let (key, value) = item.map_err(|e| {
            // name the line, never echo it: the parser's message carries the rest of the file
            let at = match &e {
                dotenvy::Error::LineParse(rest, _) => rest
                    .lines()
                    .next()
                    .and_then(|l| l.split('=').next())
                    .map(|k| format!(" at {}", k.trim()))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            format!(
                "{} could not be parsed{at}. Most likely a quote or apostrophe in an inline \
                 comment on or above that line: remove it (write `tabs limit`, not `tab's limit`)",
                path.display()
            )
        })?;
        if std::env::var_os(&key).is_none() {
            std::env::set_var(&key, value);
        }
        loaded.insert(key);
    }

    if let Some((line, key)) = declared.iter().find(|(_, k)| !loaded.contains(k)) {
        return Err(format!(
            "{}: {key} on line {line} was silently dropped by the .env parser - a quote or \
             apostrophe in an inline comment above it opened a string. Remove it.",
            path.display()
        ));
    }
    Ok(loaded.len())
}

/// `(line number, KEY)` for every assignment a person reading the file would see.
fn declared_keys(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let t = line.trim_start();
            let t = t.strip_prefix("export ").unwrap_or(t);
            let (key, _) = t.split_once('=')?;
            let valid = !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !key.starts_with(|c: char| c.is_ascii_digit());
            valid.then(|| (i + 1, key.to_string()))
        })
        .collect()
}

/// Load `<crate>/.env`, then `./.env` if it is a different file. For binaries' `main`.
pub fn load_env(manifest_dir: &str) -> Result<(), String> {
    let own = std::path::Path::new(manifest_dir).join(".env");
    load_env_file(&own)?;
    let cwd = std::path::Path::new(".env");
    let same = std::fs::canonicalize(cwd).ok() == std::fs::canonicalize(&own).ok();
    if !same {
        load_env_file(cwd)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cs-env-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(".env");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn an_apostrophe_in_an_inline_comment_is_caught_not_swallowed() {
        let p = write(
            "apostrophe",
            "CS_T1_A=20000      # a person raises a tab's limit in 0.02 steps\nCS_T1_B=2\nCS_T1_SECRET=super-secret-value\n",
        );
        let e = load_env_file(&p).unwrap_err();
        assert!(e.contains("inline"), "{e}");
        assert!(!e.contains("super-secret-value"), "the error must not echo the file: {e}");
    }

    #[test]
    fn a_silently_dropped_key_is_named() {
        // two apostrophes: the parser does not error, it quietly eats CS_T2_B
        let p = write("dropped", "CS_T2_A=1  # it's\nCS_T2_B=2\nCS_T2_C=3  # that's all\nCS_T2_D=4\n");
        match load_env_file(&p) {
            Err(e) => assert!(e.contains("CS_T2_") && !e.contains("=2"), "{e}"),
            Ok(_) => panic!("a dropped key must not load silently"),
        }
    }

    #[test]
    fn a_clean_file_loads_every_key_and_exports_win() {
        std::env::set_var("CS_T3_B", "from-the-shell");
        let p = write("clean", "# comment with it's apostrophe on its own line is fine\nCS_T3_A=1  # plain comment\nCS_T3_B=from-the-file\nCS_T3_C=\"USD Coin\"\n");
        assert_eq!(load_env_file(&p).unwrap(), 3);
        assert_eq!(std::env::var("CS_T3_A").unwrap(), "1");
        assert_eq!(std::env::var("CS_T3_B").unwrap(), "from-the-shell", "an export wins, as with dotenvy");
        assert_eq!(std::env::var("CS_T3_C").unwrap(), "USD Coin");
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        assert_eq!(load_env_file(std::path::Path::new("/definitely/not/here/.env")).unwrap(), 0);
    }

    /// Every committed example must load completely, so copying one can never break a service.
    #[test]
    fn every_env_example_in_the_repo_loads_completely() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut checked = 0;
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let example = entry.path().join(".env.example");
            if !example.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&example).unwrap();
            let declared = declared_keys(&text).len();
            let parsed: Vec<_> = dotenvy::from_path_iter(&example)
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| panic!("{} does not parse: {e}", example.display()));
            assert_eq!(parsed.len(), declared, "{} drops keys (quote in an inline comment?)", example.display());
            checked += 1;
        }
        assert!(checked >= 4, "expected the crates' .env.example files, found {checked}");
    }

    #[test]
    fn parses_typed_values() {
        std::env::set_var("CS_TEST_U64", "42");
        std::env::set_var("CS_TEST_BOOL", "true");
        std::env::set_var("CS_TEST_STR", "hello");
        assert_eq!(get_from_env_unsafe::<u64>("CS_TEST_U64"), Ok(42));
        assert_eq!(get_from_env_unsafe::<bool>("CS_TEST_BOOL"), Ok(true));
        assert_eq!(get_from_env_unsafe::<String>("CS_TEST_STR"), Ok("hello".into()));
    }

    #[test]
    fn missing_var_names_itself_in_the_error() {
        let e = get_from_env_unsafe::<u64>("CS_TEST_DEFINITELY_MISSING").unwrap_err();
        assert!(e.contains("CS_TEST_DEFINITELY_MISSING"), "error must name the variable");
    }

    #[test]
    fn bad_value_names_itself_in_the_error() {
        std::env::set_var("CS_TEST_BAD", "not-a-number");
        let e = get_from_env_unsafe::<u64>("CS_TEST_BAD").unwrap_err();
        assert!(e.contains("CS_TEST_BAD") && e.contains("failed to parse"));
    }

    #[test]
    fn defaults_compose_with_unwrap_or() {
        std::env::remove_var("CS_TEST_ABSENT");
        assert_eq!(get_from_env_unsafe::<u32>("CS_TEST_ABSENT").unwrap_or(300), 300);
    }
}
