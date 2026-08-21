const LAUNCH_CHIP = { running: "warn", exited: "pass", failed: "fail" };

// Which agent a launch is running.
//
// A launch knows only what it was told to start, and it is allowed to be told
// nothing: a project declaring one agent runs that one without naming it. The
// record it is writing knows the answer, because the run resolved it — so that is
// asked first, and the phrase is what is left when there is no record yet.
//
// This mattered more than it looks. The question panel greeted somebody as *your
// agent* while the heading two inches above it said `Framing`, which reads as the
// page not knowing what it is showing.
function launchAgent(launch) {
  if (launch.agent) return launch.agent;
  const record = (state.runs || []).find((run) => run.id === launch.record);
  return (record && record.agent) || null;
}

// The one thing the run is waiting for, whichever kind it is.
//
// A gate and a question are answered through the same channel and are not the
// same act: one is a decision about an effect, the other is a value the flow
// goes on to read. So they are drawn differently on purpose — there is nothing
// in a question to dismiss, and no safe side to guess.
function waitingBlock(launch) {
  const waiting = launch.pending;
  if (!waiting) return null;
  return waiting.waitingFor === "question"
    ? questionBlock(launch, waiting)
    : gateBlock(launch, waiting);
}

// The node is always sent back, so an answer from a tab showing an older gate
// or question is refused rather than applied to whichever one is waiting now.
async function answerRun(launch, body) {
  try {
    const reply = await api("answer?" + q({ path: state.path, pid: launch.pid }), {
      method: "POST",
      headers: { "X-Ingot-Token": TOKEN, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    state.runs = reply.runs;
    state.launches = reply.launches;
    state.error = null;
    state.asked = null;
    state.askedKey = null;
    // The answer is now a line in the record, and the record is what the
    // transcript is. Waiting for the next poll would leave the answer somebody
    // just gave missing from the conversation they gave it in.
    state.chatFor = null;
    if (state.tab === "conversation") await refreshChat();
  } catch (error) { failed("Your answer did not reach the run", error); }
  render();
}

// One gate, at the moment the run reaches it, with the effect and the reason in
// front of the person answering. Deliberately not a blanket "approve this run":
// that is `--yes`, which this studio does not pass and has no field for. The
// next gate asks again.
function gateBlock(launch, gate) {
  return el("div", { class: "gate" }, [
    el("b", { text: "This run is waiting for you" }),
    el("div", { class: "fix", text: gate.reason || "an effect this artifact's policy gates" }),
    el("div", { class: "what", text: "it " + (gate.effects || []).map(effectWords).join(", ") }),
    el("div", { class: "answer" }, [
      el("button", { class: "action", text: "Allow this", onclick: () => answerRun(launch, { node: gate.node, allowed: true }) }),
      el("button", { class: "action quiet", text: "Refuse", onclick: () => answerRun(launch, { node: gate.node, allowed: false }) }),
    ]),
  ]);
}

// One question the agent put to a person, at the moment it reached it.
//
// Kept as a live node while the same question is outstanding, for the reason
// the start panel is: a poll two seconds later must not take the caret out of a
// half-typed answer.
function questionBlock(launch, question) {
  const key = launch.pid + " " + question.node + " " + question.index;
  if (state.asked && state.askedKey === key) return state.asked;

  const choices = question.choices || [];
  const body = (answer) => ({ node: question.node, answer: answer });

  let field = null;
  if (!choices.length) {
    const box = el("input", {
      class: "text",
      placeholder: "your answer",
      spellcheck: "false",
      onkeydown: (event) => {
        if (event.key === "Enter" && event.target.value.trim()) answerRun(launch, body(event.target.value));
      },
    });
    field = el("div", { class: "answer" }, [
      box,
      el("button", {
        class: "action",
        text: "Answer",
        onclick: () => { if (box.value.trim()) answerRun(launch, body(box.value)); },
      }),
    ]);
  } else {
    // Every offered answer is its own button. A picker with a submit beside it
    // would make one of them the default, and a question whose answer the
    // program cares about has no default.
    field = el("div", { class: "answer wrap" }, choices.map((choice) =>
      el("button", { class: "action", text: choice, onclick: () => answerRun(launch, body(choice)) })));
  }

  // The one place on this page a character appears at full height, because it
  // is the one place the agent is addressing you rather than being reported on.
  // A gate gets no face: that is the policy stopping the run, not the agent
  // asking for something.
  // The character is drawn from the qualified name so that two agents with the
  // same short name in different packages are different characters; what is
  // written next to it is the short name, because that is what somebody called
  // this agent.
  const who = launchAgent(launch) || "your agent";
  const name = splitName(who).short;
  const node = el("div", { class: "asked" }, [
    el("div", { class: "stand" }, [
      sprite(who, "tall", "asking"),
      el("div", { class: "who", text: name }),
    ]),
    el("div", { class: "grow" }, [
      el("b", { text: name + " is asking you something" }),
      el("div", { class: "question", text: question.question }),
      field,
      // Not `node n0 · consultation 0`. The node id is in the run record for
      // whoever needs it; what belongs under a question is which question it is.
      el("div", { class: "what", text: "the " + ordinalWords(question.index) + " question this run has asked" }),
    ]),
  ]);

  state.asked = node;
  state.askedKey = key;
  return node;
}
