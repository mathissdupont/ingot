#!/usr/bin/env python3
"""A conformance adapter for the Ingot Python target.

This is the whole of what an adapter is, and it is deliberately short: read the
request, run the artifact, exit non-zero if the run failed. It is also a
worked example — a backend in another language writes the same forty lines
against its own runner.

    ingot conform --backend "python crates/ingot-conformance/tools/python-adapter.py"

The Python target compiles an artifact to a standalone program, so this adapter
does that first and then runs it. A backend that interprets Agent IR directly
would skip the compile and call its interpreter instead; nothing in the contract
cares which.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

CONTRACT = "0.1"
ROOT = Path(__file__).resolve().parents[3]


def ingot() -> str:
    for candidate in ("ingot.exe", "ingot"):
        path = ROOT / "target" / "debug" / candidate
        if path.is_file():
            return str(path)
    return "ingot"


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit("usage: python-adapter.py <request.json>")

    request = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    if request["conformance"] != CONTRACT:
        sys.exit(
            "this request declares conformance contract %r and this adapter "
            "implements %r; refusing rather than guessing which fields changed"
            % (request["conformance"], CONTRACT)
        )

    with tempfile.TemporaryDirectory() as work:
        work = Path(work)

        # The Python target compiles; an interpreting backend would not.
        built = subprocess.run(
            [ingot(), "build", "--target", "python", "--from-ir",
             request["artifact"], "--out-dir", str(work)],
            capture_output=True, text=True, encoding="utf-8",
        )
        if built.returncode != 0:
            sys.stderr.write(built.stderr)
            return 2

        programs = sorted(work.glob("*.py"))
        if len(programs) != 1:
            sys.stderr.write("expected one generated program, got %s\n" % programs)
            return 2

        args = [sys.executable, str(programs[0]),
                "--cassette", request["cassette"],
                "--out-dir", request["outDir"],
                "--events", "json"]
        for name, value in request["inputs"].items():
            rendered = value if isinstance(value, str) else json.dumps(value)
            args += ["--input", "%s=%s" % (name, rendered)]

        env = dict(os.environ)
        for key in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY",
                    "GOOGLE_API_KEY", "INGOT_OPENAI_BASE_URL"):
            env.pop(key, None)
        env["PYTHONIOENCODING"] = "utf-8"

        done = subprocess.run(args, env=env, capture_output=True, text=True,
                              encoding="utf-8")
        # The event stream goes to standard error, unchanged. Standard output
        # is left alone: it carries the run's own writing.
        sys.stderr.write(done.stderr)
        sys.stdout.write(done.stdout)
        return done.returncode


if __name__ == "__main__":
    sys.exit(main())


_ = shutil  # kept for adapters that copy artifacts rather than write in place
