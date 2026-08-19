#!/usr/bin/env python3
"""Regenerate the conformance fixtures from each case's source.

A case is authored as three things a human wrote — `main.ing`, `case.toml` and
`bless.toml` — and three things this script derives: the artifact, the cassette,
and the expected result. Deriving them is what keeps a case honest: the
expectation is what the reference interpreter *did*, not what somebody typed
out, and `case.toml` records which specification clause makes that the right
answer.

Nothing here touches a network. Completions come from a local stub that speaks
the OpenAI-compatible shape, so a cassette is recorded with real request digests
and a replay of it is a real replay.

    python crates/ingot-conformance/tools/bless.py            # every case
    python crates/ingot-conformance/tools/bless.py prose      # just one

Run it from the repository root, with `ingot` already built
(`cargo build -p ingot-cli`).
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
# Relative to this file rather than to the repository root, so the two cannot
# drift again: the suite moved out of `specs/conformance` and into this crate so
# that `cargo package` would carry it, and this path was left behind pointing at
# a directory that no longer exists. `sys.exit` below is what makes that loud.
CASES = Path(__file__).resolve().parents[1] / "cases"
if not CASES.is_dir():
    sys.exit(f"no cases directory at {CASES}")


def ingot() -> str:
    # `CARGO_TARGET_DIR` first, because this repository has to set one: a
    # `target/` directory inside the OneDrive-synchronised working tree and cargo
    # do not get along, so the default path is frequently not where the binary is.
    roots = []
    override = os.environ.get("CARGO_TARGET_DIR")
    if override:
        roots.append(Path(override))
    roots.append(ROOT / "target")
    for root in roots:
        for candidate in ("ingot.exe", "ingot"):
            path = root / "debug" / candidate
            if path.is_file():
                return str(path)
    sys.exit(
        "build it first: cargo build -p ingot-cli\n"
        "looked in: %s" % ", ".join(str(root / "debug") for root in roots)
    )


class Stub(BaseHTTPRequestHandler):
    """Answers in order, in the Chat Completions shape."""

    answers: list = []
    served = 0

    def do_POST(self):  # noqa: N802 - the name the base class dispatches on
        length = int(self.headers.get("content-length", 0))
        request = json.loads(self.rfile.read(length) or b"{}")

        index = Stub.served
        Stub.served += 1
        if index >= len(Stub.answers):
            self.send_error(500, "the stub has no further answer")
            return

        answer = Stub.answers[index]
        content = answer if isinstance(answer, str) else json.dumps(answer)
        usage = {"prompt_tokens": 100, "completion_tokens": 50}

        # Both providers stream, so the stub has to. The same answer is
        # re-framed as the event stream that would have produced it, rather
        # than kept as a second fixture that could drift from the first.
        if request.get("stream"):
            frames = [
                {"model": "stub-1", "choices": [{"delta": {"content": content}}]},
                {"choices": [{"delta": {}, "finish_reason": "stop"}]},
                {"choices": [], "usage": usage},
            ]
            payload = "".join("data: %s\n\n" % json.dumps(f) for f in frames)
            payload += "data: [DONE]\n\n"
            body = payload.encode("utf-8")
            content_type = "text/event-stream"
        else:
            body = json.dumps(
                {
                    "model": "stub-1",
                    "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
                    "usage": usage,
                }
            ).encode("utf-8")
            content_type = "application/json"

        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def serve(answers):
    Stub.answers = answers
    Stub.served = 0
    server = HTTPServer(("127.0.0.1", 0), Stub)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, "http://127.0.0.1:%d/v1/chat/completions" % server.server_port


def run(args, env=None, check=True):
    merged = dict(os.environ)
    for name in (
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "INGOT_OPENAI_BASE_URL",
    ):
        merged.pop(name, None)
    merged.update(env or {})
    merged["PYTHONIOENCODING"] = "utf-8"
    done = subprocess.run(args, env=merged, capture_output=True, text=True, encoding="utf-8")
    if check and done.returncode != 0:
        sys.exit("failed: %s\n%s" % (" ".join(args), done.stderr))
    return done


def write(path, text):
    """Write with LF endings, whatever platform this is."""
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)


def inputs_as_args(inputs):
    out = []
    for name, value in inputs.items():
        rendered = value if isinstance(value, str) else json.dumps(value)
        out += ["--input", "%s=%s" % (name, rendered)]
    return out


def bless(case_dir: Path):
    name = case_dir.name
    recipe = tomllib.loads((case_dir / "bless.toml").read_text(encoding="utf-8"))
    declared = tomllib.loads((case_dir / "case.toml").read_text(encoding="utf-8"))
    inputs = recipe.get("inputs", {})
    # A case may replay with inputs the recording never saw. That is the point
    # of the mismatch case, and it is the only reason these can differ.
    replay_inputs = recipe.get("replay-inputs", inputs)

    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        source = str(case_dir / "main.ing")

        # 1. The artifact.
        built = run([ingot(), "build", source, "--out-dir", str(work / "ir")])
        artifacts = sorted((work / "ir").glob("*.ir.json"))
        if len(artifacts) != 1:
            sys.exit("%s: expected one artifact, got %s" % (name, artifacts))
        shutil.copyfile(artifacts[0], case_dir / "agent.ir.json")

        # 2. The cassette, recorded against a local stub so the digests are real.
        # Not checked for success. A case that exists to show a run failing
        # fails here too, and the cassette is written either way — which is
        # exactly the "a partial cassette beats none" rule `--record` follows.
        server, url = serve(recipe.get("answers", []))
        try:
            run(
                [ingot(), "run", source, "--provider", "openai", "--model",
                 "openai/stub-1", "--record", str(work / "cassette.json"),
                 "--out-dir", str(work / "rec"), "--no-history", "--events", "quiet",
                 # This run exists only to record a cassette. The expectation
                 # comes from the adapter, which a conformance request never
                 # hands a store, so opening one here would write a file into
                 # the case directory that nothing reads.
                 "--no-memory"]
                + inputs_as_args(inputs),
                env={"OPENAI_API_KEY": "stub", "INGOT_OPENAI_BASE_URL": url},
                check=False,
            )
        finally:
            server.shutdown()
        if not (work / "cassette.json").is_file():
            sys.exit("%s: the recording produced no cassette" % name)
        shutil.copyfile(work / "cassette.json", case_dir / "cassette.json")

        write(case_dir / "inputs.json",
              json.dumps(replay_inputs, indent=2, sort_keys=True) + "\n")

        # 3. The expected result, taken from the adapter — the same door a
        #    third-party backend comes through, so the fixture is the contract's
        #    output rather than the CLI's.
        out_dir = work / "out"
        out_dir.mkdir()
        request = work / "request.json"
        request.write_text(
            json.dumps(
                {
                    "conformance": "0.1",
                    "artifact": str(case_dir / "agent.ir.json"),
                    "cassette": str(case_dir / "cassette.json"),
                    "inputs": replay_inputs,
                    "outDir": str(out_dir),
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        done = run([ingot(), "conform", "--adapter", str(request)], check=False)

        expected_outcome = declared["outcome"]
        actual_outcome = "finished" if done.returncode == 0 else "failed"
        if expected_outcome != actual_outcome:
            sys.exit(
                "%s: case.toml expects the run to have %s and it %s:\n%s"
                % (name, expected_outcome, actual_outcome, done.stderr)
            )

        events = [
            line for line in done.stderr.splitlines()
            if line.strip().startswith("{") and '"event"' in line
        ]
        expected = case_dir / "expected"
        if expected.exists():
            shutil.rmtree(expected)
        (expected / "outputs").mkdir(parents=True)
        write(expected / "events.jsonl", "\n".join(events) + "\n")
        for produced in sorted(out_dir.iterdir()):
            shutil.copyfile(produced, expected / "outputs" / produced.name)

        print("blessed %-18s %d event(s), %d artifact(s), outcome %s"
              % (name, len(events), len(list((expected / "outputs").iterdir())), actual_outcome))
        _ = built


def main():
    wanted = sys.argv[1:]
    found = sorted(path for path in CASES.iterdir() if (path / "bless.toml").is_file())
    if wanted:
        found = [path for path in found if path.name in wanted]
        if not found:
            sys.exit("no such case: %s" % ", ".join(wanted))
    for case_dir in found:
        bless(case_dir)


if __name__ == "__main__":
    main()
