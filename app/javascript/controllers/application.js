import { Application } from "@hotwired/stimulus";

const application = Application.start();

// Configure Stimulus development experience
application.debug = false;
window.Stimulus = application;

export { application };

// Custom Turbo Stream Actions
Turbo.StreamActions.redirect = function () {
  Turbo.visit(this.target);
};

Turbo.StreamActions.add_class = function () {
  const className = this.getAttribute("class-name");
  this.targetElements.forEach((element) => element.classList.add(className));
};

Turbo.StreamActions.remove_class = function () {
  const className = this.getAttribute("class-name");
  this.targetElements.forEach((element) => element.classList.remove(className));
};

// Turbo keeps a snapshot of every page visited and restores it, without a request, when the user
// goes Back. Turning "Hide balances" on therefore leaves the balances one Back press away: the
// snapshot was taken while they were still on screen, and a broadcast refresh reaches the live
// document but not the cache behind it.
//
// Per tab, and keyed on what the tab has actually RENDERED rather than on the click, because the
// preference is account-wide: a tab that was never the one toggled still holds stale snapshots and
// still has to drop them. The class on <body> is the rendered state, so a change in it between two
// loads is exactly the moment every snapshot in this tab became wrong.
//
// The limit: a tab asleep or with its cable reconnecting misses the refresh broadcast entirely, so
// it goes on showing what it last rendered until it is navigated. Closing that would cost a server
// round-trip every time any tab regains focus, which is a poor trade for a window in which nobody
// is looking at the tab anyway.
document.addEventListener("turbo:load", () => {
  const hidden = document.body.classList.contains("hide-balances") ? "1" : "0";
  try {
    if (sessionStorage.getItem("hide-balances") === hidden) return;

    sessionStorage.setItem("hide-balances", hidden);
  } catch {
    // Storage can be denied outright (private mode, embedded webview). Clearing a cache we
    // cannot reason about is the safe way to be wrong.
  }
  Turbo.cache.clear();
});
