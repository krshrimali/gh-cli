use anyhow::Context;
use std::process::Command;

/// Opens `$VISUAL`, `$EDITOR`, or `vim` with a temp file and returns the saved contents.
pub fn edit_string(initial: &str) -> anyhow::Result<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".into());

    let dir = tempfile::tempdir().context("temp dir for editor")?;
    let path = dir.path().join("gh-pr-cli.md");
    std::fs::write(&path, initial).context("write temp file")?;

    let mut words = editor.split_whitespace();
    let bin = words.next().context("empty EDITOR")?;
    let mut cmd = Command::new(bin);
    for w in words {
        cmd.arg(w);
    }
    cmd.arg(&path);

    let st = cmd.status().context("spawn editor")?;
    anyhow::ensure!(st.success(), "editor exited with {:?}", st.code());

    std::fs::read_to_string(&path).context("read edited file")
}

/// Opens `$VISUAL` / `$EDITOR` with a throwaway buffer (diff, reviews, etc.). Exit code ignored.
pub fn view_text(content: &str, file_suffix: &str) -> anyhow::Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".into());

    let dir = tempfile::tempdir().context("temp dir for editor")?;
    let path = dir.path().join(format!("gh-pr-cli-view{file_suffix}"));
    std::fs::write(&path, content).context("write temp file")?;

    let mut words = editor.split_whitespace();
    let bin = words.next().context("empty EDITOR")?;
    let mut cmd = std::process::Command::new(bin);
    for w in words {
        cmd.arg(w);
    }
    cmd.arg(&path);

    let _ = cmd.status();
    Ok(())
}
