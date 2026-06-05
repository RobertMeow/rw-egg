use std::os::unix::process::CommandExt;

fn main() {
    // Fix permissions if SFTP didn't preserve +x
    let _ = std::process::Command::new("chmod")
        .args(["+x", "remnanode-bin"])
        .status();

    let err = std::process::Command::new("./remnanode-bin").exec();
    eprintln!("exec failed: {err}");
    std::process::exit(1);
}
