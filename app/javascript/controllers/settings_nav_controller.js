import { Controller } from "@hotwired/stimulus";

// The settings sidebar. On a phone it is a row of tabs that scrolls sideways, and the section it
// opens on may be past the edge — so the active tab is brought into view each time the section
// renders. Desktop lays the items out in a column where nothing scrolls, and that part does nothing.
//
// A page reached with ?settings=<section> (a language change coming back) has done that param's one
// job once the dialog is up, so it leaves the address — or a reload would reopen a dialog the
// user has since closed.
//
// The pane beside it marks itself settings__pane--more while there is content below its fold, which
// shows an arrow at its bottom edge. It is checked on scroll, on a resize, and whenever a widget
// inside it is replaced or opens.
//
// Connects to data-controller="settings-nav"
export default class extends Controller {
  connect() {
    this.pane = this.element.closest(".settings__frame")?.querySelector(".settings__pane");
    if (this.pane) {
      this.check = () => {
        // The arrow is the pane's last child, so it can stick to the pane's bottom edge.
        if (this.pane.lastElementChild?.className !== "settings__more") {
          this.pane.querySelector(":scope > .settings__more")?.remove();
          this.pane.insertAdjacentHTML("beforeend", '<span class="settings__more" aria-hidden="true"></span>');
        }
        this.pane.classList.toggle("settings__pane--more",
          this.pane.scrollTop + this.pane.clientHeight < this.pane.scrollHeight - 1);
      };
      this.pane.addEventListener("scroll", this.check, { passive: true });
      this.resized = new ResizeObserver(this.check);
      this.resized.observe(this.pane);
      this.changed = new MutationObserver(this.check);
      this.changed.observe(this.pane, { childList: true, subtree: true, attributes: true });
      this.check();
    }

    this.element.querySelector("[aria-current=page]")?.scrollIntoView({ block: "nearest", inline: "nearest" });

    const url = new URL(window.location.href);
    if (url.searchParams.has("settings")) {
      url.searchParams.delete("settings");
      history.replaceState(history.state, "", url);
    }
  }

  disconnect() {
    this.pane?.removeEventListener("scroll", this.check);
    this.resized?.disconnect();
    this.changed?.disconnect();
  }
}
