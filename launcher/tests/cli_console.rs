use std::process::Command;

fn launcher() -> Command {
    Command::new(env!("CARGO_BIN_EXE_krasnokamen-launcher"))
}

#[test]
fn help_survives_redirected_stdout() {
    let output = launcher().arg("--help").output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--headless-bootstrap"), "{help}");
    assert!(help.contains("--singleplayer-runtime"), "{help}");
}

#[test]
fn argument_errors_survive_redirected_stderr() {
    for (args, message) in [
        (vec!["--unknown"], "unknown argument --unknown"),
        (vec!["--data-dir"], "--data-dir needs a value"),
        (
            vec!["--headless-bootstrap", "--manifest"],
            "--manifest needs a value",
        ),
    ] {
        let output = launcher().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(message), "{error}");
    }
}
