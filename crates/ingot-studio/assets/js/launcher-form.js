// --- runs ------------------------------------------------------------------

const STATE_CHIP = { finished: "pass", failed: "fail", unfinished: "warn" };
const STATE_WORD = { finished: "finished", failed: "failed", unfinished: "no result recorded" };

function when(unix) {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
}

function lasted(run) {
  if (!run.finishedUnix || !run.startedUnix) return "";
  const seconds = run.finishedUnix - run.startedUnix;
  return seconds < 60 ? seconds + "s" : Math.floor(seconds / 60) + "m " + (seconds % 60) + "s";
}

// The panel that starts a run.
//
// Built once per project and kept as a live node, so a poll two seconds later
// does not take the caret out of the field being typed in.
function startPanel() {
  if (state.form && state.formFor === state.path) {
    // The panel is kept alive to protect what somebody typed. Readiness is not
    // something they typed: a container runtime started a minute ago must not
    // leave the switch disabled until the page is reloaded.
    if (state.formBoundaries) state.formBoundaries();
    return state.form;
  }

  const agents = (state.detail && state.detail.agents) || [];
  const chosen = {
    agent: agents.length ? agents[0].name : null,
    provider: "auto",
    cassette: "",
    inputs: {},
    contained: false,
    sandbox: false,
  };

  const inputsBox = el("div", { style: "display:grid;gap:8px;margin-top:12px" });
  function drawInputs() {
    inputsBox.textContent = "";
    const agent = agents.find((candidate) => candidate.name === chosen.agent);
    if (!agent || !agent.inputs.length) {
      inputsBox.appendChild(el("div", { class: "sub", text: agent ? "This agent takes no input." : "" }));
      return;
    }
    for (const field of agent.inputs) {
      const box = el("input", {
        class: "text",
        placeholder: field.type,
        spellcheck: "false",
        value: chosen.inputs[field.name] || "",
        oninput: (event) => { chosen.inputs[field.name] = event.target.value; },
      });
      inputsBox.appendChild(el("label", { style: "display:flex;gap:10px;align-items:center" }, [
        el("span", { class: "sub", style: "flex:0 0 132px", text: field.name }),
        box,
      ]));
    }
  }

  const agentPicker = el("select", { class: "text", onchange: (event) => { chosen.agent = event.target.value; drawInputs(); } },
    agents.map((agent) => el("option", { value: agent.name, text: agent.name })));
  const providerPicker = el("select", { class: "text", onchange: (event) => { chosen.provider = event.target.value; cassetteRow.style.display = chosen.provider === "replay" ? "flex" : "none"; } },
    ["auto", "anthropic", "google", "openai", "replay"].map((name) => el("option", { value: name, text: name })));
  const cassetteBox = el("input", {
    class: "text",
    placeholder: "tests/cassettes/example.json",
    spellcheck: "false",
    oninput: (event) => { chosen.cassette = event.target.value; },
  });
  const cassetteRow = el("label", { style: "display:none;gap:10px;align-items:center;margin-top:12px" }, [
    el("span", { class: "sub", style: "flex:0 0 132px", text: "cassette" }),
    cassetteBox,
  ]);

  // The two boundaries, offered where the run is started rather than only
  // described on the tab that draws them.
  //
  // Redrawn when what the machine can do changes, and only then — rebuilding it
  // on every poll would clear a box somebody had just ticked.
  const boundaries = el("div", { class: "switches" });
  function drawBoundaries() {
    const contained = containedReadiness(state.detail);
    const sandbox = sandboxReadiness(state.detail);
    const signature = JSON.stringify([contained, sandbox]);
    if (boundaries.dataset.signature === signature) return;
    boundaries.dataset.signature = signature;
    boundaries.textContent = "";

    // A switch that has just become unavailable must not leave a flag behind
    // it: the box is gone from the screen, so the run must not carry it.
    if (!contained.ready) chosen.contained = false;
    if (!sandbox.ready) chosen.sandbox = false;

    boundaries.appendChild(boundarySwitch(
      "Put the agent in a container",
      "It runs behind a boundary built from its own policy. The model call and any question it asks you cross it; nothing else does.",
      contained,
      (on) => { chosen.contained = on; }));
    // A switch for something this project does not have would be a puzzle
    // rather than an option, so a project with no tool server simply has one
    // switch.
    if (!sandbox.none) {
      boundaries.appendChild(boundarySwitch(
        "Put each tool server in its own container",
        "Every declared server runs behind a boundary derived from the policy of the agent that calls it.",
        sandbox,
        (on) => { chosen.sandbox = on; }));
    }
  }

  const button = el("button", { class: "action", text: "Start run" });
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      const body = { provider: chosen.provider, inputs: chosen.inputs };
      if (chosen.agent) body.agent = chosen.agent;
      if (chosen.contained) body.contained = true;
      if (chosen.sandbox) body.sandbox = true;
      if (chosen.provider === "replay" && chosen.cassette) body.cassette = chosen.cassette;
      const answer = await api("run?" + q({ path: state.path }), {
        method: "POST",
        headers: { "X-Ingot-Token": TOKEN, "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      state.runs = answer.runs;
      state.launches = answer.launches;
      state.error = null;
    } catch (error) {
      failed("The run did not start", error);
    }
    button.disabled = false;
    render();
    startPollingIfLive();
  });

  const node = el("div", { class: "card" }, [
    el("h3", {}, [el("span", { text: "Start a run" })]),
    el("div", { class: "body" }, [
      el("div", { style: "display:flex;gap:10px;align-items:center" }, [
        el("span", { class: "sub", style: "flex:0 0 132px", text: "agent" }),
        agentPicker,
        el("span", { class: "sub", text: "provider" }),
        providerPicker,
      ]),
      cassetteRow,
      inputsBox,
      boundaries,
      el("div", { style: "margin-top:14px;display:flex;gap:10px;align-items:center" }, [
        button,
        // What this button is, said once. It used to say an effect needing a
        // person is denied here, which stopped being true when the studio
        // learned to answer: this run's questions arrive on the conversation
        // tab, and until they are answered the run does not move.
        el("span", { class: "sub", text: "The same command as the terminal, from here. Anything that needs a person stops the run and asks you on the Conversation tab." }),
      ]),
    ]),
  ]);
  drawInputs();
  drawBoundaries();
  state.form = node;
  state.formFor = state.path;
  state.formBoundaries = drawBoundaries;
  return node;
}
