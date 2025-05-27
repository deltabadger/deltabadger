import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="turbo-frame-trigger"
export default class extends Controller {
  static values = {
    url: String,
    frame: String,
  };

  connect() {
  }

  loadFrame(event) {
    event.preventDefault();
    Turbo.visit(this.urlValue, { frame: this.frameValue });
  }
}
