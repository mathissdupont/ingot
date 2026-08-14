# repo-digest

The example that runs end to end **with real tools**. Every tool it declares is
served by `ingot-mcp-fs`, the sandboxed MCP server this repository ships, so no
third-party server and no credential are involved.

It lists a directory, reads its README, asks a model for a digest, and files the
result — exercising three MCP tools, a `file` result, working memory, and a
filesystem policy the runtime re-checks against the artifact.

## Set up

The server has to be on `PATH`:

```bash
cargo install ingot-mcp          # or `--path crates/ingot-mcp` in a checkout
```

Or point `command` in [`ingot.toml`](ingot.toml) at the built binary directly.

## Check the wiring before running anything

```bash
ingot tools
```

```text
workspace  (ingot-mcp-fs 0.2.0, protocol 2025-06-18)
  fs.read_file            Read a UTF-8 text file from the server root.
  fs.list_dir             List the entries of a directory under the server root.
  fs.write_file           Write a UTF-8 text file under the server root, creating it if needed.

declared tools
  fs.list_dir             -> workspace:fs.list_dir
  fs.read_file            -> workspace:fs.read_file
  fs.write_file           -> workspace:fs.write_file
```

`ingot tools` exits non-zero if any declared tool has no server, so it works as
a deployment precondition rather than something you find out at the first call.

## Run it

```bash
export ANTHROPIC_API_KEY=...
ingot run --input directory=. --input out=out/digest.md
```

The digest is printed to stdout and written to `data/out/digest.md` by the
server. `data/` is the entire filesystem as far as this agent is concerned:
the manifest starts the server with `--root data`.

## Two sandboxes

`policy` in [`main.ing`](main.ing) says `filesystem_write allow ["data/out"]`,
and the manifest says `--root data`. Both hold, independently:

* the **compiler** refused to build the artifact until the policy granted the
  effects of the tools it holds;
* the **runtime** re-checks that grant before every call, because whoever runs
  an artifact is usually not whoever built it;
* the **server** refuses anything outside `data/`, whatever the artifact
  says. A path that is absolute, contains `..`, or resolves through a symlink
  out of the root is refused.

Widening the policy does not widen the server, and vice versa.

Note which frame each is written in. The **policy** says `data/out` because a
policy path is relative to the *workspace* — here, the project directory. The
**server root** says `data` because that is the operator's own choice, made in
the manifest. `ingot sandbox` prints the first of those:

```text
$ ingot sandbox
workspace  …/examples/repo-digest

server `workspace`  (for agent heptapus.examples.digest.RepoDigest)
  mount    /workspace/data      ro   filesystem_read allow ["data"]
  mount    /workspace/data/out  rw   filesystem_write allow ["data/out"]
  network  none
  env      (none)
  workdir  /workspace

every policy rule above is enforced by the boundary
```

## Offline, with the tools still live

`ingot test` does not host tools — a cassette records model exchanges only, so a
tool call during replay would have to reach a real server, and a test that
touches the filesystem is not the offline, repeatable thing `ingot test`
promises. This agent therefore has no cassette tests.

To get a deterministic model *and* real tools, replay through `ingot run`:

```bash
ingot run --input directory=. --input out=out/digest.md --record /tmp/digest.json
ingot run --input directory=. --input out=out/digest.md \
  --provider replay --cassette /tmp/digest.json
```

One caveat worth knowing, because it is the digest check working correctly: the
first run leaves `data/out/` behind, `fs.list_dir` then sees it, the prompt
changes, and the replay is refused. Remove the directory between runs.
