# --- ingot runtime, python target ---------------------------------------------
#
# Written from specs/runtime/v0.1.md and specs/ir/v0.1.md, deliberately NOT from
# reading crates/ingot-runtime. A transliteration of the reference interpreter
# would agree with it by construction and demonstrate nothing; the point of a
# second implementation is that it can disagree. See RFC-0006.
#
# Where this and the reference interpreter differ, the specification decides. If
# the specification is silent, that is the finding, and it gets amended.
#
# Standard library only. No pip install, no framework, one file.

import base64
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request


# --- failures -----------------------------------------------------------------
#
# Every failure names the node it happened at. "the agent failed" is not
# actionable; "node n7 needed `network`, which the policy denies" is.


class RunFailed(Exception):
    """The run stopped. `operator` marks the ones the caller can fix."""

    def __init__(self, message, operator=False):
        super().__init__(message)
        self.operator = operator


def _fail(message, operator=False):
    raise RunFailed(message, operator)


# --- events -------------------------------------------------------------------
#
# Runtime 0.1 §9. No timestamps and no durations: replaying the same recorded
# exchange must produce the same event sequence byte for byte.


class Events:
    def __init__(self, form):
        self.form = form
        self.collected = []

    def emit(self, **event):
        self.collected.append(event)
        if self.form == "json":
            sys.stderr.write(json.dumps(event, separators=(",", ":")) + "\n")
        elif self.form == "text":
            line = self.line(event)
            if line is not None:
                sys.stderr.write(line + "\n")

    @staticmethod
    def line(event):
        kind = event["event"]
        if kind == "runStarted":
            return "run %s (provider: %s)" % (event["agent"], event["provider"])
        if kind == "nodeStarted":
            return "  %s  %s" % (event["node"], event["kind"])
        if kind == "modelCall":
            usage = event["usage"]
            return "        model %s -> %s (%d in, %d out)" % (
                event["model"],
                event["responseType"],
                usage.get("inputTokens", 0),
                usage.get("outputTokens", 0),
            )
        if kind == "toolCall":
            return "        tool %s [%s]" % (event["tool"], ", ".join(event["effects"]))
        if kind == "agentCall":
            return "        agent %s" % event["agent"]
        if kind == "approvalRequested":
            return "        approval needed for [%s]: %s" % (
                ", ".join(event["effects"]),
                event["reason"],
            )
        if kind == "approvalDecided":
            return "        approval %s" % ("granted" if event["allowed"] else "denied")
        if kind == "stateWritten":
            return "        state.%s written" % event["field"]
        if kind == "verified":
            # Three outcomes, not a boolean: a check that never ran has neither
            # passed nor failed. Runtime 0.2 section 1.
            return "        verify %s: %s" % (
                event["verifier"],
                {
                    "notPerformed": "not performed",
                    "passed": "passed",
                    "failed": "FAILED",
                }[event["outcome"]],
            )
        if kind == "checkpoint":
            return '        checkpoint "%s"' % event["label"]
        if kind == "branchTaken":
            return "        branch: %s" % event["arm"]
        if kind == "loopIteration":
            return "        iteration %d" % event["iteration"]
        if kind == "mapIteration":
            return "        element %d/%d" % (event["index"] + 1, event["total"])
        if kind == "emitted":
            return "        emit %s" % event["output"]
        if kind == "runFinished":
            usage = event["usage"]
            return "done: %d step(s), %d token(s)" % (
                event["steps"],
                usage.get("inputTokens", 0) + usage.get("outputTokens", 0),
            )
        if kind == "runFailed":
            return "failed: %s" % event["reason"]
        return None


def _usage(input_tokens=0, output_tokens=0, cache_read_tokens=0):
    usage = {"inputTokens": input_tokens, "outputTokens": output_tokens}
    if cache_read_tokens:
        usage["cacheReadTokens"] = cache_read_tokens
    return usage


# --- types --------------------------------------------------------------------
#
# Runtime 0.1 §6. Prose is not constrained; a non-object schema is wrapped under
# `value` and unwrapped on the way back.

_PROSE = ("text", "markdown")
_SCALARS = {
    "string": {"type": "string"},
    "text": {"type": "string"},
    "markdown": {"type": "string"},
    "int": {"type": "integer"},
    "float": {"type": "number"},
    "bool": {"type": "boolean"},
}


def _type_schema(ty, types):
    if ty.endswith("[]"):
        return {"type": "array", "items": _type_schema(ty[:-2], types)}
    if ty in ("bytes", "file"):
        # §6: not requestable, at any depth. A model cannot produce binary
        # content; a tool writes the file and returns its handle.
        _fail(
            "`%s` cannot be requested from a model; use a tool that writes the "
            "file and return its handle" % ty
        )
    if ty in _SCALARS:
        return dict(_SCALARS[ty])
    record = types.get(ty)
    if record is None:
        _fail("no type named `%s` is declared in this artifact" % ty)
    properties = {}
    required = []
    for field in record["fields"]:
        properties[field["name"]] = _type_schema(field["type"], types)
        required.append(field["name"])
    return {
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": False,
    }


def response_shape(ty, types):
    """(mode, schema, wrapped). `mode` is prose, freeJson or schema."""
    if ty in _PROSE:
        return ("prose", None, False)
    if ty == "json":
        return ("freeJson", None, False)
    schema = _type_schema(ty, types)
    if schema.get("type") == "object":
        return ("schema", schema, False)
    # §6: providers generally require an object at the schema root.
    return (
        "schema",
        {
            "type": "object",
            "properties": {"value": schema},
            "required": ["value"],
            "additionalProperties": False,
        },
        True,
    )


def validate(value, ty, types, where):
    """A value against a declared type. §6: a mismatch is an error."""
    if ty.endswith("[]"):
        if not isinstance(value, list):
            _fail("%s: expected a list of `%s`, got %s" % (where, ty[:-2], _shape(value)))
        for index, item in enumerate(value):
            validate(item, ty[:-2], types, "%s[%d]" % (where, index))
        return value
    if ty in ("string", "text", "markdown"):
        if not isinstance(value, str):
            _fail("%s: expected `%s`, got %s" % (where, ty, _shape(value)))
        return value
    if ty == "int":
        # A JSON bool is an int in Python, and is not an Ingot int.
        if isinstance(value, bool) or not isinstance(value, int):
            _fail("%s: expected `int`, got %s" % (where, _shape(value)))
        return value
    if ty == "float":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            _fail("%s: expected `float`, got %s" % (where, _shape(value)))
        return value
    if ty == "bool":
        if not isinstance(value, bool):
            _fail("%s: expected `bool`, got %s" % (where, _shape(value)))
        return value
    if ty == "json":
        return value
    if ty == "bytes":
        # §6.1: base64, because the IR, events and cassettes are all JSON.
        if not isinstance(value, str):
            _fail("%s: expected `bytes` as a base64 string, got %s" % (where, _shape(value)))
        try:
            base64.b64decode(value, validate=True)
        except Exception:
            _fail("%s: `bytes` is not valid base64" % where)
        return value
    if ty == "file":
        # §6.1: a handle. `path` is required; further fields are ignored, so a
        # producer may add a media type without breaking a consumer.
        if not isinstance(value, dict):
            _fail("%s: expected a `file` handle, got %s" % (where, _shape(value)))
        if not isinstance(value.get("path"), str):
            _fail("%s: a `file` handle needs a string `path`" % where)
        return value

    record = types.get(ty)
    if record is None:
        _fail("%s: no type named `%s` is declared in this artifact" % (where, ty))
    if not isinstance(value, dict):
        _fail("%s: expected `%s`, got %s" % (where, ty, _shape(value)))
    for field in record["fields"]:
        if field["name"] not in value:
            _fail("%s: `%s` is missing the field `%s`" % (where, ty, field["name"]))
        validate(value[field["name"]], field["type"], types, "%s.%s" % (where, field["name"]))
    return value


def _shape(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "a bool"
    if isinstance(value, int):
        return "an int"
    if isinstance(value, float):
        return "a float"
    if isinstance(value, str):
        return "a string"
    if isinstance(value, list):
        return "a list"
    return "an object"


# --- policy -------------------------------------------------------------------


class Policy:
    """Runtime 0.1 §7, re-checked here rather than trusted from the compiler.

    The person running an artifact is frequently not the person who built it, and
    an artifact that arrived over a registry is exactly the case where "the
    compiler checked" is not an argument.
    """

    # `secret_access` is governed by `secrets`; every other effect by the subject
    # of the same name.
    _SUBJECTS = {"secret_access": "secrets"}

    def __init__(self, rules):
        self.rules = rules

    def check(self, node, effects):
        for effect in effects:
            # §7: model_access is implicitly granted and needs no rule.
            if effect == "model_access":
                continue
            subject = self._SUBJECTS.get(effect, effect)
            rule = self.rules.get(subject)
            if rule is None:
                # Default-deny. An absent rule is a denial, with its own message.
                _fail(
                    "node `%s` needs the `%s` effect, and the artifact's policy "
                    "grants no rule for it (an absent rule is a denial)" % (node, effect)
                )
            decision = rule.get("decision")
            if decision == "deny":
                _fail(
                    "node `%s` needs the `%s` effect, which the artifact's policy "
                    "denies" % (node, effect)
                )
            # allow, or requireApproval — the compiler already inserted the gate.


# --- providers ----------------------------------------------------------------


class Replay:
    """A recorded exchange, served in order.

    The digest is recomputed here from the same fields the recording used, so an
    edited prompt produces a loud mismatch instead of a stale answer.
    """

    name = "replay"

    def __init__(self, cassette):
        self.cassette = cassette
        self.position = 0

    def complete(self, request):
        interactions = self.cassette.get("interactions", [])
        if self.position >= len(interactions):
            _fail(
                "cassette replay failed: the recording has %d interaction(s) and "
                "the run asked for one more, at node `%s`"
                % (len(interactions), request["node"])
            )
        interaction = interactions[self.position]
        self.position += 1

        if interaction.get("node") != request["node"]:
            _fail(
                "cassette replay failed: interaction %d was recorded at node `%s` "
                "and the run reached `%s`"
                % (self.position - 1, interaction.get("node"), request["node"])
            )
        recorded = interaction.get("requestDigest")
        actual = digest(request)
        if recorded != actual:
            _fail(
                "cassette replay failed: interaction %d was recorded for a "
                "different request at node `%s`. The prompt or its context "
                "changed since recording — re-record the cassette and review the "
                "diff." % (self.position - 1, request["node"])
            )
        return {
            "value": interaction["value"],
            "usage": interaction.get("usage") or _usage(),
            "model": interaction.get("model") or "replay",
        }

    def remaining(self):
        return max(0, len(self.cassette.get("interactions", [])) - self.position)


def digest(request):
    """A stable digest of everything that determines the answer.

    Byte-compatible with the recorder: node, system, prompt, response type, then
    each context entry as name and canonical JSON. Object keys are sorted and
    there is no whitespace, so the same request digests the same in both
    implementations.
    """
    hasher = hashlib.sha256()
    hasher.update(request["node"].encode("utf-8"))
    hasher.update(b"\x00")
    hasher.update((request.get("system") or "").encode("utf-8"))
    hasher.update(b"\x00")
    hasher.update(request["prompt"].encode("utf-8"))
    hasher.update(b"\x00")
    hasher.update(request["responseType"].encode("utf-8"))
    for name, value in request.get("context", []):
        hasher.update(b"\x00")
        hasher.update(name.encode("utf-8"))
        hasher.update(b"\x00")
        hasher.update(canonical(value).encode("utf-8"))
    return hasher.hexdigest()


def canonical(value):
    """Canonical JSON: sorted keys, no whitespace, non-ASCII left as itself."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


_DEFAULT_TIMEOUT = 180
_MAX_RETRIES = 3


def _post(url, headers, body, timeout=_DEFAULT_TIMEOUT):
    payload = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(url, data=payload, method="POST")
    request.add_header("content-type", "application/json")
    for name, value in headers.items():
        request.add_header(name, value)

    last = None
    for attempt in range(_MAX_RETRIES):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            text = error.read().decode("utf-8", "replace")
            # 408, 429 and 5xx are worth another try; everything else is the
            # request's own fault and retrying would only be slower.
            if error.code in (408, 429) or error.code >= 500:
                last = "provider rejected the request (%d): %s" % (error.code, text[:400])
                if attempt + 1 < _MAX_RETRIES:
                    continue
                _fail(last)
            _fail("provider rejected the request (%d): %s" % (error.code, text[:400]))
        except urllib.error.URLError as error:
            last = "provider transport failed: %s" % error.reason
            if attempt + 1 < _MAX_RETRIES:
                continue
            _fail(last)
    _fail(last or "provider transport failed")


class Anthropic:
    """The Messages API."""

    name = "anthropic"

    def __init__(self, key, base_url, model):
        self.key = key
        self.base_url = base_url or "https://api.anthropic.com/v1/messages"
        self.model = model

    def complete(self, request):
        model = self.model or _pinned_model(request) or "claude-opus-4-5"
        body = {
            "model": model,
            "max_tokens": request["maxTokens"],
            "messages": [{"role": "user", "content": request["prompt"]}],
        }
        if request.get("system"):
            body["system"] = request["system"]
        if request["shape"][0] == "schema":
            # Structured output through a forced single-tool call, which is how
            # the Messages API constrains a response.
            body["tools"] = [
                {
                    "name": "respond",
                    "description": "Return the response in the required shape.",
                    "input_schema": request["shape"][1],
                }
            ]
            body["tool_choice"] = {"type": "tool", "name": "respond"}

        reply = _post(
            self.base_url,
            {"x-api-key": self.key, "anthropic-version": "2023-06-01"},
            body,
        )
        if isinstance(reply, dict) and reply.get("type") == "error":
            _fail("provider rejected the request: %s" % canonical(reply.get("error")))

        stop = reply.get("stop_reason")
        if stop == "max_tokens":
            _fail("the response was cut off at the %d token limit" % request["maxTokens"])

        text = None
        structured = None
        for block in reply.get("content") or []:
            if block.get("type") == "text" and text is None:
                text = block.get("text")
            if block.get("type") == "tool_use" and structured is None:
                structured = block.get("input")

        usage = reply.get("usage") or {}
        return {
            "value": _unwrap(request, structured, text),
            "usage": _usage(
                usage.get("input_tokens", 0),
                usage.get("output_tokens", 0),
                usage.get("cache_read_input_tokens", 0),
            ),
            "model": reply.get("model") or model,
        }


class OpenAiCompatible:
    """Chat Completions, spoken by more services than OpenAI's own."""

    name = "openai"

    def __init__(self, key, base_url, model):
        self.key = key
        self.base_url = base_url or "https://api.openai.com/v1/chat/completions"
        self.model = model

    def complete(self, request):
        model = self.model or _pinned_model(request)
        if not model:
            _fail(
                "no model was named for the OpenAI-compatible provider; pin one "
                "with `model exact \"<vendor>/<model>\"` or pass --model",
                operator=True,
            )
        messages = []
        if request.get("system"):
            messages.append({"role": "system", "content": request["system"]})
        messages.append({"role": "user", "content": request["prompt"]})

        body = {
            "model": model,
            "messages": messages,
            "max_completion_tokens": request["maxTokens"],
        }
        if request["shape"][0] == "schema":
            body["response_format"] = {
                "type": "json_schema",
                "json_schema": {
                    "name": "response",
                    "strict": True,
                    "schema": request["shape"][1],
                },
            }

        headers = {}
        if self.key:
            headers["authorization"] = "Bearer " + self.key
        reply = _post(self.base_url, headers, body)

        if isinstance(reply, dict) and reply.get("error"):
            _fail("provider rejected the request: %s" % canonical(reply["error"]))

        choices = reply.get("choices") or []
        if not choices:
            _fail("the provider returned no choices")
        choice = choices[0]
        message = choice.get("message") or {}
        if message.get("refusal"):
            _fail("the provider declined to answer: %s" % message["refusal"])
        finish = choice.get("finish_reason")
        if finish == "length":
            _fail("the response was cut off at the %d token limit" % request["maxTokens"])
        if finish == "content_filter":
            _fail("the provider declined to answer (content_filter)")

        text = message.get("content")
        structured = None
        if request["shape"][0] == "schema" and isinstance(text, str):
            try:
                structured = json.loads(text)
            except ValueError as error:
                _fail("the response did not match the declared type: %s" % error)

        usage = reply.get("usage") or {}
        return {
            "value": _unwrap(request, structured, text),
            "usage": _usage(
                usage.get("prompt_tokens", 0),
                usage.get("completion_tokens", 0),
            ),
            "model": reply.get("model") or model,
        }


def _pinned_model(request):
    """The model half of `vendor/model`, when the artifact pinned one."""
    reference = request.get("modelReference")
    if not reference:
        return None
    return reference.split("/", 1)[1] if "/" in reference else reference


def _unwrap(request, structured, text):
    mode, _schema, wrapped = request["shape"]
    if mode == "prose":
        if text is None:
            _fail("the provider returned no text")
        return text
    if mode == "freeJson":
        if text is None:
            _fail("the provider returned no text")
        try:
            return json.loads(text)
        except ValueError as error:
            _fail("the response did not match the declared type: %s" % error)
    if structured is None:
        _fail("the provider returned no structured response")
    if wrapped:
        if not isinstance(structured, dict) or "value" not in structured:
            _fail("the wrapped response has no `value` field")
        return structured["value"]
    return structured


# --- artifacts ----------------------------------------------------------------

_EXTENSIONS = {"markdown": "md", "text": "txt"}


def artifact_bytes(content_type, value):
    """What to write to disk.

    Prose as-is; anything else as canonical pretty JSON, because writing a
    JSON-quoted markdown document to a `.md` file would be useless.
    """
    if content_type in _PROSE and isinstance(value, str):
        return value.encode("utf-8")
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def artifact_extension(content_type):
    return _EXTENSIONS.get(content_type, "json")


# --- the runtime the generated flow calls into --------------------------------


class Runtime:
    def __init__(self, agent, types, policy, budget, model_reference, provider, events, approval):
        self.agent = agent
        self.types = types
        self.policy = policy
        self.provider = provider
        self.events = events
        self.approval = approval
        self.model_reference = model_reference

        self.inputs = {}
        self.state = {}
        self.outputs = {}
        self.steps = 0
        self.usage = _usage()
        self.max_steps = budget.get("maxSteps")
        self.token_limit = budget.get("tokens")

    # --- bookkeeping ---------------------------------------------------------

    def node(self, node_id, kind):
        self.events.emit(event="nodeStarted", node=node_id, kind=kind)

    def _charge_step(self, node_id):
        self.steps += 1
        if self.max_steps is not None and self.steps > self.max_steps:
            _fail(
                "the `steps` budget of %d was exhausted at node `%s`"
                % (self.max_steps, node_id)
            )

    def max_output_tokens(self, ceiling):
        """The cap on one call: what is left of the token budget, or the ceiling.

        Bounded from below by 1, because asking a provider for zero tokens is a
        request that cannot succeed, and from above by the ceiling this
        backend's transport earns: it asks once and reads one whole answer, so
        it keeps the whole-body ceiling of Runtime 0.3 section 4 (GAP-032).
        """
        if self.token_limit is None:
            remaining = ceiling
        else:
            spent = self.usage.get("inputTokens", 0) + self.usage.get("outputTokens", 0)
            remaining = max(0, self.token_limit - spent)
        return max(1, min(remaining, ceiling))

    def _charge_tokens(self, node_id, usage):
        for key in ("inputTokens", "outputTokens", "cacheReadTokens"):
            if key in usage:
                self.usage[key] = self.usage.get(key, 0) + usage[key]
        total = self.usage.get("inputTokens", 0) + self.usage.get("outputTokens", 0)
        if self.token_limit is not None and total > self.token_limit:
            _fail(
                "the `tokens` budget of %d was exhausted at node `%s`"
                % (self.token_limit, node_id)
            )

    # --- nodes ---------------------------------------------------------------

    def ask(self, node, effects, prompt, response_type, max_tokens, context=None, system=None):
        self.policy.check(node, effects)
        self._charge_step(node)

        shape = response_shape(response_type, self.types)
        request = {
            "node": node,
            "system": system,
            "prompt": prompt,
            "context": context or [],
            "responseType": response_type,
            "shape": shape,
            "maxTokens": max_tokens,
            "modelReference": self.model_reference,
        }
        reply = self.provider.complete(request)
        value = reply["value"]
        if shape[0] != "freeJson":
            validate(value, response_type, self.types, "node `%s`" % node)

        self.events.emit(
            event="modelCall",
            node=node,
            model=reply["model"],
            responseType=response_type,
            usage=reply["usage"],
        )
        self._charge_tokens(node, reply["usage"])
        return value

    def approve(self, node, effects, reason):
        self.events.emit(
            event="approvalRequested", node=node, effects=list(effects), reason=reason
        )
        allowed = self.approval(node, effects, reason)
        self.events.emit(event="approvalDecided", node=node, allowed=allowed)
        if not allowed:
            _fail("approval was refused at node `%s`: %s" % (node, reason), operator=True)

    def state_read(self, node, field):
        if field not in self.state:
            _fail("node `%s` read `state.%s` before it was written" % (node, field))
        return self.state[field]

    def state_write(self, node, field, value):
        self.state[field] = value
        self.events.emit(event="stateWritten", node=node, field=field)

    def emit(self, node, output, content_type, value):
        validate(value, content_type, self.types, "output `%s`" % output)
        self.outputs[output] = {
            "name": output,
            "contentType": content_type,
            "value": value,
        }
        self.events.emit(event="emitted", node=node, output=output)

    def verify(self, node, verifier, held):
        """Report a check, and stop the run when it did not hold.

        `held` is already decided: the check is a pure expression over values
        this run bound, inlined into the artifact as the node's `condition`,
        so the emitter renders it in place and hands the answer here. There is
        nothing to call out to, which is what makes the outcome reproducible
        from the run record alone.

        The event is emitted before the failure so the record says what the
        check found and then says the run ended.
        """
        if not isinstance(held, bool):
            _fail("node `%s` condition did not evaluate to a boolean" % node)
        self.events.emit(
            event="verified",
            node=node,
            verifier=verifier,
            outcome="passed" if held else "failed",
        )
        if not held:
            _fail(
                "the check `%s` did not hold at node `%s`, so the run stopped there"
                % (verifier, node)
            )

    def not_verified(self, node, verifier):
        """A verifier declared without a body: there is no check to carry out.

        Reported rather than skipped, and never as `passed`. Runtime 0.2 §1
        requires a consumer to read `notPerformed` as *unknown*.
        """
        self.events.emit(
            event="verified", node=node, verifier=verifier, outcome="notPerformed"
        )

    def checkpoint(self, node, label):
        self.events.emit(event="checkpoint", node=node, label=label)

    def branch(self, node, arm):
        self.events.emit(event="branchTaken", node=node, arm=arm)

    def iteration(self, node, index):
        self.events.emit(event="loopIteration", node=node, iteration=index)

    def element(self, node, index, total):
        self.events.emit(event="mapIteration", node=node, index=index, total=total)


# --- prompt rendering ---------------------------------------------------------


def render(parts):
    """A template's parts, joined.

    A substitution renders by its declared type: prose and strings as
    themselves, everything else as canonical JSON. §6.1 forbids `file` and
    `bytes` in a prompt, and the compiler already rejected it — checked here too,
    because this program may be run against an artifact this build did not make.
    """
    out = []
    for part in parts:
        if part[0] == "text":
            out.append(part[1])
            continue
        _, value, ty = part
        if ty in ("bytes", "file"):
            _fail("a `%s` value cannot be interpolated into a prompt" % ty)
        if isinstance(value, str) and ty in ("string", "text", "markdown"):
            out.append(value)
        else:
            out.append(canonical(value))
    return "".join(out)


# --- entry point --------------------------------------------------------------


def parse_inputs(argv, declared):
    """`--input name=value`, with `@path` reading a file.

    A value is JSON when it parses as JSON and a plain string otherwise, so
    `--input topic=compilers` does the obvious thing.
    """
    supplied = {}
    index = 0
    options = {
        "cassette": None,
        "provider": "auto",
        "model": None,
        "events": "text",
        "out_dir": None,
        "yes": False,
    }
    while index < len(argv):
        argument = argv[index]
        index += 1
        if argument in ("-i", "--input"):
            entry = argv[index]
            index += 1
            if "=" not in entry:
                _fail("`--input %s` is not name=value" % entry, operator=True)
            name, raw = entry.split("=", 1)
            name = name.strip()
            raw = raw.strip()
            if raw.startswith("@"):
                with open(raw[1:], "r", encoding="utf-8") as handle:
                    supplied[name] = handle.read()
            else:
                try:
                    supplied[name] = json.loads(raw)
                except ValueError:
                    supplied[name] = raw
        elif argument == "--cassette":
            options["cassette"] = argv[index]
            index += 1
        elif argument == "--provider":
            options["provider"] = argv[index]
            index += 1
        elif argument == "--model":
            options["model"] = argv[index]
            index += 1
        elif argument == "--events":
            options["events"] = argv[index]
            index += 1
        elif argument == "--out-dir":
            options["out_dir"] = argv[index]
            index += 1
        elif argument == "--yes":
            options["yes"] = True
        elif argument in ("-h", "--help"):
            _usage_text()
            raise SystemExit(0)
        else:
            _fail("unknown option `%s`" % argument, operator=True)

    for name in sorted(supplied):
        if name not in declared:
            _fail(
                "this agent has no input named `%s`; it declares: %s"
                % (name, ", ".join(sorted(declared)) or "none"),
                operator=True,
            )
    for name in sorted(declared):
        if name not in supplied:
            _fail("missing input `%s` (expected `%s`)" % (name, declared[name]), operator=True)
    return supplied, options


def _usage_text():
    sys.stderr.write(
        "Generated by ingot. Runs one agent.\n\n"
        "  --input NAME=VALUE   an agent input; `@path` reads a file\n"
        "  --cassette FILE      replay a recording instead of calling a provider\n"
        "  --provider NAME      auto, anthropic, openai or replay\n"
        "  --model MODEL        override the model the artifact asks for\n"
        "  --events FORM        text, json or quiet\n"
        "  --out-dir DIR        write artifacts here instead of to stdout\n"
        "  --yes                approve every gate without asking\n"
    )


def build_provider(options):
    choice = options["provider"]
    if options["cassette"] and choice in ("auto", "replay"):
        with open(options["cassette"], "r", encoding="utf-8") as handle:
            return Replay(json.load(handle))
    if choice == "replay":
        _fail("`--provider replay` needs `--cassette FILE`", operator=True)

    anthropic_key = os.environ.get("ANTHROPIC_API_KEY")
    openai_key = os.environ.get("OPENAI_API_KEY")
    openai_base = os.environ.get("INGOT_OPENAI_BASE_URL")

    if choice == "anthropic" or (choice == "auto" and anthropic_key and not openai_key):
        if not anthropic_key:
            _fail("ANTHROPIC_API_KEY is not set", operator=True)
        return Anthropic(
            anthropic_key,
            os.environ.get("INGOT_ANTHROPIC_BASE_URL"),
            options["model"],
        )
    if choice == "openai" or (choice == "auto" and (openai_key or openai_base)):
        # A local server needs no key, which is why an absent one is not fatal
        # when a base URL says where to go.
        if not openai_key and not openai_base:
            _fail("OPENAI_API_KEY is not set", operator=True)
        return OpenAiCompatible(openai_key, openai_base, options["model"])

    _fail(
        "no model provider is available\n  export ANTHROPIC_API_KEY or "
        "OPENAI_API_KEY, or pass --cassette FILE",
        operator=True,
    )


def terminal_approval(node, effects, reason):
    """Ask, and deny when there is nobody to ask.

    Runtime 0.1 §2: an unattended run must not approve by default. The artifact
    asked for a human and does not get one silently.
    """
    if not sys.stdin.isatty():
        return False
    sys.stderr.write("\n  APPROVAL REQUIRED at node %s\n" % node)
    sys.stderr.write("  %s\n" % reason)
    sys.stderr.write("  effects: %s\n" % ", ".join(effects))
    sys.stderr.write("  allow? [y/N] ")
    sys.stderr.flush()
    answer = sys.stdin.readline().strip().lower()
    return answer in ("y", "yes")


def main(agent, ir_version, declared_inputs, declared_outputs, types, policy, budget,
         model_reference, flow):
    if ir_version.split(".", 1)[0] != "0":
        sys.stderr.write(
            "error: this program was generated from IR version %s, which it does "
            "not implement\n" % ir_version
        )
        return 2

    try:
        supplied, options = parse_inputs(sys.argv[1:], declared_inputs)
        events = Events(options["events"])
        provider = build_provider(options)

        for name, ty in sorted(declared_inputs.items()):
            validate(supplied[name], ty, types, "input `%s`" % name)

        runtime = Runtime(
            agent=agent,
            types=types,
            policy=Policy(policy),
            budget=budget,
            model_reference=model_reference,
            provider=provider,
            events=events,
            approval=(lambda *_: True) if options["yes"] else terminal_approval,
        )
        runtime.inputs = supplied

        events.emit(event="runStarted", agent=agent, provider=provider.name)
        flow(runtime)

        for name in sorted(declared_outputs):
            if name not in runtime.outputs:
                _fail("the run finished without producing the declared output `%s`" % name)

        events.emit(event="runFinished", steps=runtime.steps, usage=runtime.usage)
        write_outputs(runtime.outputs, options["out_dir"])
        return 0
    except RunFailed as failure:
        message = str(failure)
        try:
            events.emit(event="runFailed", reason=message)
        except NameError:
            pass
        sys.stderr.write("error: %s\n" % message)
        if failure.operator:
            sys.stderr.write(
                "hint: this is a problem with how the run was invoked, not with "
                "the agent itself\n"
            )
        return 1


def write_outputs(outputs, out_dir):
    if out_dir is None:
        stream = sys.stdout.buffer
        for name in sorted(outputs):
            artifact = outputs[name]
            payload = artifact_bytes(artifact["contentType"], artifact["value"])
            stream.write(payload)
            if not payload.endswith(b"\n"):
                stream.write(b"\n")
        stream.flush()
        return

    os.makedirs(out_dir, exist_ok=True)
    for name in sorted(outputs):
        artifact = outputs[name]
        path = os.path.join(
            out_dir, "%s.%s" % (name, artifact_extension(artifact["contentType"]))
        )
        with open(path, "wb") as handle:
            handle.write(artifact_bytes(artifact["contentType"], artifact["value"]))
        sys.stdout.write("%s -> %s\n" % (name, path))
