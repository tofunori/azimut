//! `process` needs to be told where the base coordinates come from.

use std::process::{Command, Output};

fn process(args: &[&str]) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no-such-flight");
    Command::new(env!("CARGO_BIN_EXE_azimut"))
        .arg("process")
        .arg(&missing)
        .args(["--out", tmp.path().to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn process_refuses_without_a_base_source() {
    let o = process(&[]);
    assert!(!o.status.success());
    let err = stderr(&o);
    for flag in ["--base-llh", "--base-ppp", "--submit-ppp", "--approximate-base"] {
        assert!(err.contains(flag), "{flag} not named in: {err}");
    }
}

#[test]
fn approximate_base_is_accepted_explicitly() {
    // Arguments pass; the run then stops on the missing folder.
    let err = stderr(&process(&["--approximate-base"]));
    assert!(err.contains("is not a folder"), "{err}");
    let err = stderr(&process(&["--base-llh", "46.35,-72.55,31.2"]));
    assert!(err.contains("is not a folder"), "{err}");
}

#[test]
fn approximate_base_excludes_the_other_sources() {
    let o = process(&["--approximate-base", "--base-llh", "46.35,-72.55,31.2"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("cannot be used with"), "{}", stderr(&o));
}
