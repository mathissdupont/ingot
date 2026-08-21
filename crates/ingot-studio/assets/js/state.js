// --- state ----------------------------------------------------------------

const state = {
  view: "projects",     // projects | project | machine
  projects: [],
  path: null,           // the selected project
  tab: "overview",      // overview | canvas | conversation | runs | boundary
  canvas: null,         // the drawn flow, and the source it was drawn from
  proposed: null,       // an edit the person has been shown and not yet applied
  runId: null,
  detail: null,
  // Which readiness subjects are expanded, by subject key. Absent means the
  // default — open when something in it needs attention — so a click sticks
  // across the two-second poll without also freezing every group somebody did
  // not touch at whatever it happened to be.
  groups: {},
  runs: null,
  launches: null,
  run: null,
  // The conversation tab. `chatId` is a run somebody pinned by opening it; with
  // none pinned the tab picks the run a person came here for. `chatFor` is which
  // record `chat` actually holds, so a changed choice refetches and an unchanged
  // one does not.
  chatId: null,
  chat: null,
  chatFor: null,
  machine: null,
  error: null,
  poll: null,
  // The start panel is one live DOM node, reused across renders. Rebuilding it
  // every two seconds while a run is going would take the caret out of whatever
  // field somebody was typing in.
  form: null,
  formFor: null,
  // The one part of that panel that must follow the machine rather than the
  // person: whether a boundary can be raised right now.
  formBoundaries: null,
  // The panel that writes a new project, kept alive for the same reason.
  createForm: null,
  // What this machine can say about the reference image, and any build going.
  image: null,
  // The same rule for the box a question is answered in: as long as the run is
  // waiting at one question, that is one live node. A rebuilt one loses a
  // half-typed answer every poll.
  asked: null,
  askedKey: null,
};

function show(view, extra) {
  Object.assign(state, { view, error: null }, extra || {});
  render();
  load();
}
