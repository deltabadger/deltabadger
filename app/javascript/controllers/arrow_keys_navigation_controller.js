import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="arrow-keys-navigation"
export default class extends Controller {
  connect() {
    this.index = 0
    this.navigationInitialized = false
  }

  navigate(event) {
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
        this.buttons[this.index].click();
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
    const new_items = document.querySelectorAll('[data-arrow-keys-navigation-target="item"]')
    if (this.items && new_items.length !== this.items.length) {
      this.index = 0
      this.navigationInitialized = false
    }
    this.items = new_items
    this.buttons = document.querySelectorAll('[data-arrow-keys-navigation-target="button"]')
  }
}
