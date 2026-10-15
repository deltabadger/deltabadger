import { Controller } from "@hotwired/stimulus";

// The settings sidebar. On a phone it is a row of tabs that scrolls sideways, and the section it
// opens on may be past the edge — so the active tab is brought into view each time the section
// renders. Desktop lays the items out in a column where nothing scrolls, and that part does nothing.
//
// A page reached with ?settings=<section> (a language change coming back) has done that param's one
// job once the dialog is up, so it leaves the address — or a reload would reopen a dialog the
// user has since closed.
//
// Connects to data-controller="settings-nav"
export default class extends Controller {
  connect() {
    this.element.querySelector("[aria-current=page]")?.scrollIntoView({ block: "nearest", inline: "nearest" });

    const url = new URL(window.location.href);
    if (url.searchParams.has("settings")) {
      url.searchParams.delete("settings");
      history.replaceState(history.state, "", url);
    }
  }
}
