// --- loading ---------------------------------------------------------------

async function refreshProjects() {
  try {
    state.projects = (await api("projects")).projects;
  } catch (error) {
    state.error = String(error.message || error);
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
      if (state.tab === "runs") {
        if (state.runId) state.run = await api("run?" + q({ path: state.path, id: state.runId }));
        else await refreshRuns();
        startPollingIfLive();
      }
    }
  } catch (error) {
    state.error = String(error.message || error);
  }
  render();
}

async function refreshRuns() {
  const answer = await api("runs?" + q({ path: state.path }));
  state.runs = answer.runs;
  state.launches = answer.launches;
}

// Two reasons to keep re-reading: a record with no result line, which is a run
// that is going or was interrupted and the page cannot tell which; and a child
// this studio started that has not exited yet.
function startPollingIfLive() {
  const live = state.runId
    ? state.run && state.run.state === "unfinished"
    : (state.runs || []).some((run) => run.state === "unfinished") ||
      (state.launches || []).some((launch) => launch.state === "running");
  if (!live) return;
  state.poll = setTimeout(async () => {
    if (document.hidden) return startPollingIfLive();
    try {
      if (state.runId) state.run = await api("run?" + q({ path: state.path, id: state.runId }));
      else await refreshRuns();
      render();
      startPollingIfLive();
    } catch (_) { /* the studio went away; stop quietly */ }
  }, 2000);
}

function stopPolling() {
  if (state.poll) clearTimeout(state.poll);
  state.poll = null;
}
