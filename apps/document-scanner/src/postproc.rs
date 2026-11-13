//! Simple Scan compatible post-processing of saved files.
//!
//! GSettings keys (`org.gnome.SimpleScan`): `postproc-enabled` (b, false),
//! `postproc-script` (s, ''), `postproc-arguments` (s, ''),
//! `postproc-keep-original` (b, false).
//!
//! After a file is saved the script is executed directly (no shell) as
//!
//! ```text
//! SCRIPT MIME-TYPE KEEP-ORIGINAL FILE [ARGUMENTS…]
//! ```
//!
//! where `KEEP-ORIGINAL` is `true` or `false` and `ARGUMENTS` are the words
//! of `postproc-arguments`, split with shell-style quoting rules (quotes and
//! backslashes group words; nothing is expanded or executed). When
//! keep-original is on, the unprocessed file is first copied to
//! `NAME_orig.EXT` (the schema documents the `_orig` suffix). Multi-page
//! image saves run the script once per file.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PostProc {
    pub enabled: bool,
    pub script: String,
    pub arguments: String,
    pub keep_original: bool,
}

/// A program and its arguments, ready to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Expands a leading `~/` to the home directory.
pub fn expand_script(script: &str, home: &Path) -> PathBuf {
    match script.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if script == "~" => home.to_path_buf(),
        None => PathBuf::from(script),
    }
}

/// Splits `postproc-arguments` into words (shell quoting rules, no
/// expansion).
pub fn split_arguments(args: &str) -> Result<Vec<String>, String> {
    if args.trim().is_empty() {
        return Ok(Vec::new());
    }
    gtk::glib::shell_parse_argv(args)
        .map(|v| v.into_iter().map(|s| s.to_string_lossy().into_owned()).collect())
        .map_err(|e| format!("Invalid post-processing arguments: {e}"))
}

impl PostProc {
    /// Whether a script should run after saving.
    pub fn active(&self) -> bool {
        self.enabled && !self.script.trim().is_empty()
    }

    /// Builds the command for one saved file.
    pub fn invocation(&self, mime: &str, file: &Path, home: &Path) -> Result<Invocation, String> {
        let program = expand_script(self.script.trim(), home);
        let mut args: Vec<OsString> = vec![
            mime.into(),
            (if self.keep_original { "true" } else { "false" }).into(),
            file.as_os_str().to_owned(),
        ];
        args.extend(split_arguments(&self.arguments)?.into_iter().map(OsString::from));
        Ok(Invocation { program, args })
    }
}

/// `dir/name_orig.ext` for `dir/name.ext`.
pub fn original_path(file: &Path) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match file.extension() {
        Some(ext) => file.with_file_name(format!("{stem}_orig.{}", ext.to_string_lossy())),
        None => file.with_file_name(format!("{stem}_orig")),
    }
}

/// Runs the script for every file (blocking; call from a worker thread).
/// Returns an error describing the first failure.
pub fn run(pp: &PostProc, mime: &str, files: &[PathBuf], home: &Path) -> Result<(), String> {
    for file in files {
        let inv = pp.invocation(mime, file, home)?;
        if pp.keep_original {
            std::fs::copy(file, original_path(file))
                .map_err(|e| format!("Could not keep the original of {}: {e}", file.display()))?;
        }
        let status = Command::new(&inv.program)
            .args(&inv.args)
            .stdin(Stdio::null())
            .status()
            .map_err(|e| format!("Could not run {}: {e}", inv.program.display()))?;
        if !status.success() {
            return Err(format!("The post-processing script failed ({status})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(script: &str, args: &str, keep: bool) -> PostProc {
        PostProc { enabled: true, script: script.into(), arguments: args.into(), keep_original: keep }
    }

    fn strs(v: &[OsString]) -> Vec<&str> {
        v.iter().map(|s| s.to_str().unwrap()).collect()
    }

    #[test]
    fn argument_order() {
        let inv = pp("/usr/bin/ocr", "", false)
            .invocation("application/pdf", Path::new("/d/Scanned Document.pdf"), Path::new("/h"))
            .unwrap();
        assert_eq!(inv.program, PathBuf::from("/usr/bin/ocr"));
        assert_eq!(strs(&inv.args), ["application/pdf", "false", "/d/Scanned Document.pdf"]);

        let inv = pp("~/bin/post.sh", "-l eng 'two words' \"$HOME\" a\\ b ; rm", true)
            .invocation("image/jpeg", Path::new("/d/x.jpg"), Path::new("/h"))
            .unwrap();
        assert_eq!(inv.program, PathBuf::from("/h/bin/post.sh"));
        // No expansion, no command separation: `$HOME` and `;` stay literal.
        assert_eq!(
            strs(&inv.args),
            ["image/jpeg", "true", "/d/x.jpg", "-l", "eng", "two words", "$HOME", "a b", ";", "rm"]
        );
    }

    #[test]
    fn bad_arguments_and_activity() {
        assert!(pp("s", "'unterminated", false).invocation("a", Path::new("f"), Path::new("/h")).is_err());
        assert!(split_arguments("   ").unwrap().is_empty());
        assert!(!PostProc { enabled: true, ..Default::default() }.active());
        assert!(!PostProc { script: "x".into(), ..Default::default() }.active());
        assert!(pp("x", "", false).active());
    }

    #[test]
    fn original_names() {
        assert_eq!(original_path(Path::new("/d/Scan.pdf")), PathBuf::from("/d/Scan_orig.pdf"));
        assert_eq!(original_path(Path::new("/d/a.b.jpg")), PathBuf::from("/d/a.b_orig.jpg"));
        assert_eq!(original_path(Path::new("/d/noext")), PathBuf::from("/d/noext_orig"));
    }

    #[test]
    fn runs_script() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, b"%PDF-original").unwrap();
        let log = dir.path().join("log");
        let script = dir.path().join("post.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho processed > \"$3\"\n", log.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = pp(script.to_str().unwrap(), "--x 'y z'", true);
        run(&p, "application/pdf", std::slice::from_ref(&file), dir.path()).unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(logged, format!("application/pdf\ntrue\n{}\n--x\ny z\n", file.display()));
        assert_eq!(std::fs::read(&file).unwrap(), b"processed\n");
        assert_eq!(std::fs::read(dir.path().join("doc_orig.pdf")).unwrap(), b"%PDF-original");

        let failing = pp("/bin/false", "", false);
        assert!(run(&failing, "application/pdf", &[file], dir.path()).is_err());
    }
}
