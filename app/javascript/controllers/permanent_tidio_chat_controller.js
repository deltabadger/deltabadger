import { Controller } from "@hotwired/stimulus"

// A hacky controller that moves the Tidio Chat out of the body to prevent Turbo from
// removing it on page load

// Connects to data-controller="permanent-tidio-chat"
export default class extends Controller {
  connect() {
    this.listener = this.#moveElements.bind(this);
    document.addEventListener("tidioChat-ready", this.listener);
  }

  disconnect() {
    document.removeEventListener("tidioChat-ready", this.listener);
  }

  #moveElements() {
    ["tidio-chat-code", "tidio-chat"].forEach((id) => {
      const element = document.getElementById(id);

      if (!element) {
        console.warn(`Element with ID "${id}" not found.`);
        return;
      }

      const htmlElement = document.documentElement; // Target the <html> element
      htmlElement.appendChild(element); // Move the element into <html>
      console.log(`Element with ID "${id}" moved to <html>.`);
    });
  }
}