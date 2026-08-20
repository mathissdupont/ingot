// --- loading ---------------------------------------------------------------

async function refreshProjects() {
  try {
    state.projects = (await api("projects")).projects;
  } catch (error) {
    failed("The project list could not be read", error);
  }
  render();
}

async function load() {
  stopPolling();
  try {
    if (state.view === "machine" && !state.machine) {
      state.machine = await api("machine");
    }
    if (state.view === "project") {
      // Every tab wants the project read. The runs tab wants it too: the start
      // panel offers the agents the artifact declares and a field per input it
      // takes, which is the artifact's own signature and not a guess at one.
      if (!state.detail) state.detail = await api("project?" + q({ path: state.path }));
      if (state.tab === "canvas" && !state.canvas) {
        state.canvas = await api("canvas?" + q({ path: state.path }));
      }
      // Runs and launches are read on every tab, not only on Runs. The mark on
      // the conversation tab is how this page says somebody is being waited for,
      // and a mark you can only see from the tab it points at is not a mark.
      await refreshRuns();
      if (state.tab === "runs" && state.runId) {
        state.run = await api("run?" + q({ path: state.path, id: state.runId }));
      }
      if (state.tab === "conversation") await refreshChat();
      startPollingIfLive();
    }
  } catch (error) {
    failed("This project could not be read", error);
  }
  render();
}

async function refreshRuns() {
  const answer = await api("runs?" + q({ path: state.path }));
  state.runs = answer.runs;
  state.launches = answer.launches;
}

// The record the conversation tab is showing, fetched when the choice changes
// and re-fetched while the run is still writing it. A finished record does not
// change, so it is read once.
async function refreshChat() {
  const id = conversationId();
  if (!id) {
    state.chat = null;
    state.chatFor = null;
    return;
  }
  if (state.chat && state.chatFor === id && state.chat.state !== "unfinished") return;
  state.chat = await api("run?" + q({ path: state.path, id: id }));
  state.chatFor = id;
}

// Three reasons to keep re-reading, and one thing that deliberately is not one.
//
// A child this studio started that has not exited. The record being read having
// no result line. And the *newest* record having none, which is how a run
// started in a terminal streams onto this page at all.
//
// What is not a reason is an older record without a result line. That is an
// interrupted run — the page cannot tell one from a run still going, which is
// exactly why it must not poll for it: the polling would never stop, on every
// tab, for a run that ended weeks ago.
function startPollingIfLive() {
  const runs = state.runs || [];
  const reading = state.tab === "conversation" ? state.chat : state.runId ? state.run : null;
  const live = (state.launches || []).some((launch) => launch.state === "running") ||
    (reading && reading.state === "unfinished") ||
    (runs.length > 0 && runs[0].state === "unfinished");
  if (!live) return;
  state.poll = setTimeout(async () => {
    if (document.hidden) return startPollingIfLive();
    try {
      await refreshRuns();
      if (state.tab === "runs" && state.runId) {
        state.run = await api("run?" + q({ path: state.path, id: state.runId }));
      }
      if (state.tab === "conversation") await refreshChat();
      render();
      startPollingIfLive();
    } catch (_) { /* the studio went away; stop quietly */ }
  }, 2000);
}

function stopPolling() {
  if (state.poll) clearTimeout(state.poll);
  state.poll = null;
}
