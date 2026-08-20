// --- start -----------------------------------------------------------------

for (const link of document.querySelectorAll(".rail-link")) {
  link.addEventListener("click", () => show(link.dataset.view, { path: link.dataset.view === "projects" ? null : state.path }));
}
document.addEventListener("visibilitychange", () => { if (!document.hidden && state.view === "project" && state.tab === "runs") load(); });

refreshProjects().then(() => api("machine").then((machine) => {
  state.machine = machine;
  document.getElementById("version").textContent = machine.version;
}).catch(() => {}));
render();
