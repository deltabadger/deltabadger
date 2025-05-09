import { Controller } from "@hotwired/stimulus";
import { createConsumer } from "@rails/actioncable";

// Connects to data-controller="broadcast-on-connect"
export default class extends Controller {
  static values = { method: String, methodArgs: Object };

  connect() {
    fetch(`/${this.methodValue}`, {
      method: "POST",
      headers: {
        "X-CSRF-Token": document.querySelector('[name="csrf-token"]').content,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(this.methodArgsValue),
    });
  }
}
