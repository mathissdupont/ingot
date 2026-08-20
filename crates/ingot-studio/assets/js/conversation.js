// --- the conversation -------------------------------------------------------
//
// A run, read as the exchange it was, with its own door.
//
// Two sources and one shape. A record on disk is the same event stream whether
// the run is finished or still going — it is flushed a line at a time — so this
// renders both, and the only thing a live run adds is that the question at the
// end is still answerable.
//
// What this cannot show is what a model said. `modelCall` carries the model, the
// shape of its answer and the tokens it cost, and deliberately not the text: an
// event stream that carried prompts would put every prompt and every reply in a
// file on disk. So a model's turns are the machinery *between* the human ones,
// which is also an honest picture of where a person's attention is worth
// spending.

// Which record this tab is showing.
//
// The pinned one when somebody opened a particular run, and otherwise the one
// they would have come here for: a run waiting to be answered, then a run still
// going, then the last one. An empty tab that only fills up sometimes teaches
// people not to look at it, so this tab is never empty while a project has
// history.
function conversationId() {
  if (state.chatId) return state.chatId;
  const launches = state.launches || [];
  const waiting = launches.find((launch) => launch.pending && launch.record);
  if (waiting) return waiting.record;
  const running = launches.find((launch) => launch.state === "running" && launch.record);
  if (running) return running.record;
  // Then simply the newest record. Deliberately *not* the newest unfinished
  // one: a record with no result line is a run that started and did not report
  // one, which is a run going in somebody's terminal and a run that was killed
  // equally. Preferring it would mean an interrupted run from last week
  // outranking the one that just finished.
  const runs = state.runs || [];
  return runs.length ? runs[0].id : null;
}

// What the tab itself says from the other tabs, because the reason to come here
// is usually that something needs you and you are somewhere else.
// Only this studio's own children count as live. A record with no result line
// could be a run going in a terminal or one that was killed, and this page
// cannot tell which — a dot that stays lit next to an interrupted run from last
// week is worse than no dot.
function conversationMark() {
  const launches = state.launches || [];
  if (launches.some((launch) => launch.pending)) return "waiting";
  return launches.some((launch) => launch.state === "running") ? "live" : null;
}

const MARK_WORDS = { waiting: "a run is waiting for you", live: "a run is going" };

function renderConversation(inner) {
  if (!state.runs) return inner.appendChild(el("div", { class: "empty", text: "Reading the run history…" }));

  const id = conversationId();
  if (!id) {
    inner.appendChild(card("Conversation", el("div", { class: "body" }, [
      el("p", { class: "muted", text: "Nothing has run here yet, so there is nothing anybody said." }),
      el("p", { class: "muted", text: "This is where a run's exchange goes: what it worked out, what it used, and every question it put to you — answered here, in place." }),
      el("div", { class: "add", style: "margin-top:12px" }, [
        el("button", { class: "action", text: "Start a run", onclick: () => show("project", { tab: "runs" }) }),
      ]),
    ])));
    return;
  }
  if (!state.chat || state.chatFor !== id) {
    return inner.appendChild(el("div", { class: "empty", text: "Reading the conversation…" }));
  }

  const chat = state.chat;
  const launch = (state.launches || []).find((candidate) => candidate.record === id) || null;
  const pending = launch && launch.pending;

  inner.appendChild(conversationHead(chat, launch));

  const moments = momentsOf(chat.events);

  // The pending ask is the last thing in the record and also the thing the
  // panel below is about. Shown twice it reads as two questions, so the static
  // one gives way to the one that can be answered.
  const last = moments[moments.length - 1];
  if (pending && last && last.node === pending.node &&
      ((last.shape === "asked" && pending.waitingFor === "question") ||
       (last.shape === "gate" && pending.waitingFor === "approval"))) {
    moments.pop();
  }

  const who = chat.agent || "the agent";  // qualified: the sprite's palette follows it
  const turns = moments.map((moment) => momentNode(moment, who));

  if (pending) {
    turns.push(el("div", { class: "live" }, [waitingBlock(launch)]));
  } else if (chat.state === "unfinished" && lastAsk(chat.events)) {
    // A run started from a terminal is answered in that terminal. Saying so is
    // the difference between a page that looks broken and a page that is clear
    // about which surface holds the pipe.
    turns.push(el("div", { class: "note" }, [
      el("span", { class: "kind", text: "waiting" }),
      el("span", { class: "what", text: "This run is waiting for an answer, and this studio did not start it — the terminal that did is where its question was put." }),
    ]));
  }

  // `unfinished` is the honest word for a record with no result line, and the
  // wrong word for a process this studio can see running. Where it knows, it
  // says the better thing.
  const chip = launch && launch.state === "running"
    ? { tone: "warn", word: pending ? "waiting for you" : "going" }
    : { tone: STATE_CHIP[chat.state], word: STATE_WORD[chat.state] };

  inner.appendChild(card("Conversation",
    turns.length
      ? el("div", { class: "body flush chat" }, turns)
      : el("div", { class: "body flush" }, [el("div", { class: "empty", text: "The record holds no events yet." })]),
    el("span", { class: "chip " + chip.tone, text: chip.word })));

  // What it actually produced, when this studio is the one holding the pipe.
  // A finished conversation whose result is on another tab is a conversation
  // with the last line cut off.
  if (launch && launch.output) {
    inner.appendChild(card("What it produced", el("pre", { class: "block", text: launch.output.trimEnd() })));
  }
}

// Which run, whose it is, and what it spent — in one row, with a way back to
// the others. The picker rather than a list, because this tab is about one
// conversation at a time.
function conversationHead(chat, launch) {
  const runs = state.runs || [];
  const picker = el("select", {
    class: "text",
    "aria-label": "which run",
    onchange: (event) => show("project", { tab: "conversation", chatId: event.target.value }),
  }, runs.map((run) => el("option", {
    value: run.id,
    selected: run.id === chat.id,
    text: pickerLabel(run),
  })));

  const spend = [
    chat.steps === null || chat.steps === undefined ? null : chat.steps + " step(s)",
    chat.usage ? chat.usage.inputTokens + " in / " + chat.usage.outputTokens + " out" : null,
    chat.cost || null,
    chat.contained ? "behind the boundary" : null,
  ].filter(Boolean).join("  ·  ");

  return el("div", { class: "chat-head" }, [
    el("div", { class: "stand" }, [sprite(chat.agent || "agent", "card", moodOf(chat, launch))]),
    el("div", { class: "grow" }, [
      el("div", {}, [
        el("b", { text: splitName(chat.agent || "the agent").short }),
        el("span", { class: "sub", text: "  " + chat.provider }),
      ]),
      el("div", { class: "sub", text: spend || "nothing recorded yet" }),
    ]),
    runs.length > 1 ? picker : null,
  ]);
}

// One line per run in the picker. `unfinished` is only "going" when this studio
// can see the process; otherwise it is a run that reported no result, and
// calling that "going" would be a guess dressed as a fact.
function pickerLabel(run) {
  const live = (state.launches || [])
    .some((launch) => launch.record === run.id && launch.state === "running");
  const tail = live ? "going" : run.state === "finished" ? null : STATE_WORD[run.state];
  return splitName(run.agent).short + "  ·  " + when(run.startedUnix) +
    (tail ? "  ·  " + tail : "");
}

// The character's face follows the run: asking beats going, and a failure is
// not a shrug.
function moodOf(chat, launch) {
  if (launch && launch.pending) return "asking";
  if (chat.state === "failed") return "refused";
  if (chat.state === "finished") return "done";
  return "idle";
}

// Whether the record's last consultation is still unanswered.
function lastAsk(events) {
  let open = false;
  for (const event of events || []) {
    if (event.event === "consultationAsked" || event.event === "approvalRequested") open = true;
    if (event.event === "consultationAnswered" || event.event === "approvalDecided") open = false;
  }
  return open;
}

// --- events, as moments -----------------------------------------------------
//
// One pass over the record. Nothing here computes a new fact: every moment is
// one event's own fields, said in words. The turns a person took and the ones
// the agent took are drawn as an exchange; everything else is a quiet line
// between them, because a transcript where the machinery shouts as loudly as
// the question is a transcript nobody reads twice.
function momentsOf(events) {
  const moments = [];
  const note = (kind, text) => moments.push({ shape: "note", kind: kind, text: text });
  const tokens = (usage) =>
    usage ? usage.inputTokens + " in / " + usage.outputTokens + " out" : "no tokens counted";

  for (const event of events || []) {
    switch (event.event) {
      // The header already says which agent and which provider, and a node
      // starting is the same fact as what it then did.
      case "runStarted":
      case "nodeStarted":
        break;
      case "modelCall":
        note("thought", "worked something out with " + event.model + " — " + tokens(event.usage) +
          ", giving back " + typeWords(event.responseType));
        break;
      case "toolCall":
        note("used a tool", event.tool +
          ((event.effects || []).length ? " — it " + event.effects.map(effectWords).join(", ") : ""));
        break;
      case "agentCall":
        note("handed over", "gave the work to " + splitName(event.agent).short);
        break;
      case "approvalRequested":
        moments.push({ shape: "gate", node: event.node, reason: event.reason, effects: event.effects || [] });
        break;
      case "approvalDecided":
        moments.push({ shape: "decided", allowed: event.allowed });
        break;
      case "consultationAsked":
        moments.push({
          shape: "asked",
          node: event.node,
          index: event.index,
          question: event.question,
          choices: event.choices || [],
        });
        break;
      case "consultationAnswered":
        moments.push({ shape: "answered", answer: event.answer });
        break;
      case "fallbackTaken":
        note("fell back", "the " + event.because + " failed, so it used the fallback the program states");
        break;
      case "stateWritten":
        note("remembered", event.field);
        break;
      case "verified":
        note("checked", event.verifier + " — " + verifyWords(event.outcome));
        break;
      case "checkpoint":
        note("checkpoint", event.label);
        break;
      case "branchTaken":
        note("chose", "took the " + event.arm + " path");
        break;
      case "loopIteration":
        note("round", String(event.iteration));
        break;
      case "mapIteration":
        note("element", (event.index + 1) + " of " + event.total);
        break;
      case "emitted":
        note("produced", event.output);
        break;
      case "runFinished":
        moments.push({
          shape: "end",
          tone: "pass",
          text: "Finished — " + event.steps + " step(s), " + tokens(event.usage) + ".",
        });
        break;
      case "runFailed":
        moments.push({ shape: "end", tone: "fail", text: event.reason });
        break;
      case "runStopped":
        moments.push({
          shape: "end",
          tone: "warn",
          text: "Stopped at the checkpoint “" + event.label + "”. Resuming carries on from there.",
        });
        break;
      default:
        // An event this page has never heard of is shown as itself. A new kind
        // should look unfamiliar rather than be described wrongly, and dropping
        // it would leave a transcript that is quietly incomplete.
        note(event.event, describe(event));
    }
  }
  return moments;
}

function momentNode(moment, who) {
  if (moment.shape === "note") {
    return el("div", { class: "note" }, [
      el("span", { class: "kind", text: moment.kind }),
      el("span", { class: "what", text: moment.text }),
    ]);
  }
  if (moment.shape === "asked") {
    return el("div", { class: "turn from" }, [
      sprite(who, "bust", "asking"),
      el("div", { class: "said" }, [
        el("div", { class: "line", text: moment.question }),
        // The answers it offered, as they were offered. A recording of a
        // question whose menu is missing is a recording of a different question.
        moment.choices.length
          ? el("div", { class: "tags" }, moment.choices.map((choice) =>
              el("span", { class: "tag", text: choice })))
          : null,
        el("div", { class: "meta", text: "the " + ordinalWords(moment.index) + " question this run asked" }),
      ]),
    ]);
  }
  if (moment.shape === "answered") {
    return el("div", { class: "turn mine" }, [
      el("div", { class: "said" }, [
        el("div", { class: "line", text: moment.answer }),
        el("div", { class: "meta", text: "what a person answered" }),
      ]),
    ]);
  }
  if (moment.shape === "gate") {
    return el("div", { class: "turn stop" }, [
      el("div", { class: "said" }, [
        el("div", { class: "line", text: "It stopped for permission: " + moment.reason }),
        moment.effects.length
          ? el("div", { class: "meta", text: "it " + moment.effects.map(effectWords).join(", ") })
          : null,
      ]),
    ]);
  }
  if (moment.shape === "decided") {
    return el("div", { class: "turn mine" }, [
      el("div", { class: "said" }, [
        el("div", { class: "line", text: moment.allowed ? "A person allowed it." : "A person refused." }),
      ]),
    ]);
  }
  return el("div", { class: "closed " + moment.tone }, [el("span", { text: moment.text })]);
}
