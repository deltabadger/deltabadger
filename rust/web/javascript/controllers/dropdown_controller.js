import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["wrapper", "trigger"];
  static classes = ["active"];

  connect() {
    this.handleOutsideClick = this.handleOutsideClick.bind(this);
  }

  // Every click inside the wrapper bubbles to here, which is what closes the menu after picking an
  // item — right for a link, wrong for a control the menu OWNS: flipping a switch is not choosing
  // where to go, and the menu closing under the finger makes it unusable for a second flip.
  // Marked elements opt out; the menu still closes on an outside click or on any item that leads
  // somewhere.
  toggle(event) {
    if (event?.target?.closest?.("[data-dropdown-keep-open]")) return;

    this.wrapperTarget.classList.remove("dropdown-wrapper--closing");
    this.wrapperTarget.classList.toggle("dropdown-wrapper--on");
    const isOpen = this.wrapperTarget.classList.contains("dropdown-wrapper--on");

    if (this.hasTriggerTarget && this.hasActiveClass) {
      this.triggerTarget.classList.toggle(this.activeClass, isOpen);
    }

    if (isOpen) {
      this.#offsetFromButton();
      this.#hideTooltip();
      document.addEventListener("click", this.handleOutsideClick);
    } else {
      this.#animateClose();
      this.#showTooltip();
      document.removeEventListener("click", this.handleOutsideClick);
      if (event?.target?.closest?.('[data-turbo-frame="modal"]')) this.#holdTintForModal();
    }
  }

  // The menu stays shown (--closing) while its zoom out runs, or 200 ms if no animation ends (reduced motion, say).
  #animateClose() {
    const wrapper = this.wrapperTarget;
    const menu = wrapper.querySelector(".dropdown");
    if (!menu || wrapper.classList.contains("dropdown-wrapper--fullscreen")) return;
    wrapper.classList.add("dropdown-wrapper--closing");
    const done = () => {
      clearTimeout(timer);
      menu.removeEventListener("animationend", done);
      wrapper.classList.remove("dropdown-wrapper--closing");
    };
    menu.addEventListener("animationend", done);
    const timer = setTimeout(done, 200);
  }

  // The button's offset from the menu's centre, for the menu's zoom to travel along (as a modal's does). The last
  // opening's offset is cleared first, so the menu is only scaled about its centre while it is measured.
  #offsetFromButton() {
    const menu = this.wrapperTarget.querySelector(".dropdown");
    if (!menu) return;
    menu.style.removeProperty("--from-x");
    menu.style.removeProperty("--from-y");
    const b = (this.hasTriggerTarget ? this.triggerTarget : this.wrapperTarget).getBoundingClientRect();
    const m = menu.getBoundingClientRect();
    menu.style.setProperty("--from-x", `${b.left + b.width / 2 - (m.left + m.width / 2)}px`);
    menu.style.setProperty("--from-y", `${b.top + b.height / 2 - (m.top + m.height / 2)}px`);
  }

  // The item opens a modal: the tint stays until the modal says it is open (modal--base's modalIsOpen), or 3 s if it
  // never comes, then fades under the modal's own backdrop.
  #holdTintForModal() {
    const wrapper = this.wrapperTarget;
    wrapper.classList.add("dropdown-wrapper--handoff");
    const release = () => {
      wrapper.classList.remove("dropdown-wrapper--handoff");
      window.removeEventListener("modalIsOpen", release);
      clearTimeout(timer);
    };
    window.addEventListener("modalIsOpen", release);
    const timer = setTimeout(release, 3000);
  }

  handleOutsideClick(event) {
    if (!this.wrapperTarget.contains(event.target)) {
      this.wrapperTarget.classList.remove("dropdown-wrapper--on");
      this.#animateClose();
      if (this.hasTriggerTarget && this.hasActiveClass) {
        this.triggerTarget.classList.remove(this.activeClass);
      }
      this.#showTooltip();
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }

  #hideTooltip() {
    const tooltip = this.wrapperTarget.querySelector(".tooltip");
    if (tooltip) tooltip.style.display = "none";
  }

  #showTooltip() {
    const tooltip = this.wrapperTarget.querySelector(".tooltip");
    if (tooltip) tooltip.style.display = "";
  }
}