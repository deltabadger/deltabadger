import { Controller } from "@hotwired/stimulus";

// The sheet's side column: the settings menu, or the wizard's track. On a phone it is a row that
// scrolls sideways, and the current tab or step may be past the edge — so it is brought into view
// each time the frame renders. Desktop lays it out in a column where nothing scrolls, and that part
// does nothing.
//
// A page reached with ?settings=<section> (a language change coming back) has done that param's one
// job once the dialog is up, so it leaves the address — or a reload would reopen a dialog the
// user has since closed.
//
// The pane beside it marks itself sheet__pane--more while there is content below its fold, which
// shows an arrow at its bottom edge, and pushes a stuck heading out as the next one reaches it.
// It is checked on scroll, on a resize, and whenever a widget inside it is replaced or opens.
//
// Connects to data-controller="sheet"
export default class extends Controller {
  connect() {
    this.pane = this.element.closest(".sheet__frame")?.querySelector(".sheet__pane");
    if (this.pane) {
      this.check = () => {
        // The arrow is the pane's last child, so it can stick to the pane's bottom edge.
        if (this.pane.lastElementChild?.className !== "sheet__more") {
          this.pane.querySelector(":scope > .sheet__more")?.remove();
          this.pane.insertAdjacentHTML("beforeend", '<span class="sheet__more" aria-hidden="true"></span>');
        }
        // Something has scrolled under the pane's top edge (the wizard's sticky search draws its line then).
        this.pane.classList.toggle("sheet__pane--scrolled", this.pane.scrollTop > 0);
        this.pane.classList.toggle("sheet__pane--more",
          this.pane.scrollTop + this.pane.clientHeight < this.pane.scrollHeight - 1);
        // The headings stick to the pane's top edge; the next one coming up pushes the stuck one out
        // instead of sliding over it. Written only when it changes, or the observer below would loop.
        const top = this.pane.getBoundingClientRect().top;
        const headings = [...this.pane.querySelectorAll(".sheet__heading")];
        headings.forEach((heading, i) => {
          const next = headings[i + 1];
          const overlap = next ? heading.offsetHeight - (next.getBoundingClientRect().top - top) : 0;
          const shift = overlap > 0 ? `0 ${-Math.min(overlap, heading.offsetHeight)}px` : "";
          if (heading.style.translate !== shift) heading.style.translate = shift;
        });
      };
      this.pane.addEventListener("scroll", this.check, { passive: true });
      this.resized = new ResizeObserver(this.check);
      this.resized.observe(this.pane);
      this.changed = new MutationObserver(this.check);
      this.changed.observe(this.pane, { childList: true, subtree: true, attributes: true });
      this.check();
      // The DCA key step: Connect is the primary button once every key field holds something, secondary until then.
      this.ready = () => {
        const form = this.pane.querySelector(".set-api__form");
        const button = form?.querySelector(".set-api__actions [type=submit]");
        if (!button) return;
        const fields = [...form.querySelectorAll("input.rinput--text")];
        const filled = fields.length > 0 && fields.every((field) => field.value.trim() !== "");
        button.classList.toggle("rbutton--primary", filled);
        button.classList.toggle("rbutton--secondary", !filled);
      };
      this.pane.addEventListener("input", this.ready);
      this.ready();
    }

    this.element.querySelector("[aria-current]")?.scrollIntoView({ block: "nearest", inline: "nearest" });

    const url = new URL(window.location.href);
    if (url.searchParams.has("settings")) {
      url.searchParams.delete("settings");
      history.replaceState(history.state, "", url);
    }
  }

  disconnect() {
    this.pane?.removeEventListener("scroll", this.check);
    this.pane?.removeEventListener("input", this.ready);
    this.resized?.disconnect();
    this.changed?.disconnect();
  }
}
