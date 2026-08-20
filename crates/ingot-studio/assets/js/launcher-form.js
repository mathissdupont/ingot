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
  if (state.form && state.formFor === state.path) return state.form;

  const agents = (state.detail && state.detail.agents) || [];
  const chosen = { agent: agents.length ? agents[0].name : null, provider: "auto", cassette: "", inputs: {} };

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

  const button = el("button", { class: "action", text: "Start run" });
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      const body = { provider: chosen.provider, inputs: chosen.inputs };
      if (chosen.agent) body.agent = chosen.agent;
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
      state.error = String(error.message || error);
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
      el("div", { style: "margin-top:14px;display:flex;gap:10px;align-items:center" }, [
        button,
        el("span", { class: "sub", text: "Runs here are the same command as the terminal. An effect that needs a person is denied, not assumed — there is nobody at this one's keyboard." }),
      ]),
    ]),
  ]);
  drawInputs();
  state.form = node;
  state.formFor = state.path;
  return node;
}
