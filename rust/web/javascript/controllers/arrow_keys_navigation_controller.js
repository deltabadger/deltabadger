import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="arrow-keys-navigation"
export default class extends Controller {
  static targets = ["item", "button"]

  connect() {
    this.index = 0
    this.navigationInitialized = false
  }

  navigate(event) {
    // The keys are the list's from anywhere on the page (the search box above it, nothing focused), but a control of its
    // own outside the list (a step's Next, the track, a menu) keeps them: its Enter is its own.
    const control = event.target.closest?.("button, a[href], select, textarea, [contenteditable]");
    if (control && !this.element.contains(control)) return;

    const handleNavigation = (direction) => {
      if (!this.navigationInitialized) {
        this.index = 0;
      } else {
        this.index = Math.max(0, Math.min(this.index + direction, this.items.length - 1));
      }
      this.#highlightCurrentItem();
      this.navigationInitialized = true;
    };

    switch (event.key) {
      case "ArrowUp":
        event.preventDefault();
        this.#updateItems();
        handleNavigation(-1);
        break;
      case "ArrowDown":
        event.preventDefault();
        this.#updateItems();
        handleNavigation(1);
        break;
      case "Enter":
        event.preventDefault();
        this.#updateItems();
        if (this.buttons[this.index]) this.buttons[this.index].click();
        break;
    }
  }

  #highlightCurrentItem() {
    this.items.forEach((item, i) => {
      item.classList.toggle("modal--search__assets__item__highlight", i === this.index)
    })
    // if (this.buttons.length > 0) {
    //   this.buttons[this.index].focus()
    // }
  }

  #updateItems() {
    const new_items = this.itemTargets
    if (this.items && new_items.length !== this.items.length) {
      this.index = 0
      this.navigationInitialized = false
    }
    this.items = new_items
    this.buttons = this.buttonTargets
  }
}
