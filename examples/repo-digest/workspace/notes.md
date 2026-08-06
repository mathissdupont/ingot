# Notes

- The MCP server is started with `--root workspace`, so this directory is the
  entire filesystem as far as the agent is concerned.
- `../` is refused by the server, and `filesystem_read` is separately checked
  against the artifact's own policy before the call is even attempted.
