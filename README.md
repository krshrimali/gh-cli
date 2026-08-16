# gh-pr-cli

A keyboard-driven terminal UI for browsing and reviewing GitHub pull requests.

Run it inside a GitHub repository, or pass `--owner OWNER --repo REPO`. Authentication
uses the usual GitHub token/`gh` CLI credentials.

## Themes

Press `t` anywhere outside an overlay to cycle through `dark`, `light`,
`high-contrast`, and `terminal`. Select one directly with `:theme NAME`,
`--theme NAME`, or `GH_PR_CLI_THEME=NAME`. The `terminal` theme preserves your
terminal's own foreground and background colors.
