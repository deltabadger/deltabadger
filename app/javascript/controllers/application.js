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

// A repeat click on the link whose visit is already in flight would abort that request and start
// the render over — and the server still renders the abandoned one, ahead of the new one. Only the
// identical URL is ignored; a different link still cancels the visit and moves on. Both events are
// cancelled: a turbo:click left unfollowed falls through to the browser as a full navigation.
document.addEventListener("turbo:click", (event) => {
  const visit = Turbo.navigator.currentVisit;
  if (visit?.state !== "started" || visit.location.href !== event.detail.url) return;
  // Only a navigation duplicating a navigation: a Back/Forward restoration or a page refresh in
  // flight is not what the click asked for, and a frame-targeted link is its own navigation even
  // at the same URL.
  if (visit.action === "restore" || visit.isPageRefresh) return;
  if (event.target.closest("turbo-frame") || event.target.hasAttribute("data-turbo-frame")) return;

  event.preventDefault();
  event.detail.originalEvent.preventDefault();
});

// The menu controller stamps data-menu-visit on <html> for a menu-tile visit, which hides Turbo's
// progress bar for that visit alone. turbo:visit fires only when a visit STARTS, and a form
// submission emits none while its request runs — and starting one cancels the visit in flight
// without a load or an error of its own — so the stamp has to come off when the visit ends or is
// cut short. Here, rather than in the controller, because the page the visit lands on may not
// carry the menu at all (a session that expired mid-visit lands on the sign-in layout).
["turbo:load", "turbo:fetch-request-error", "turbo:submit-start"].forEach((name) =>
  document.addEventListener(name, () => document.documentElement.removeAttribute("data-menu-visit"))
);

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
