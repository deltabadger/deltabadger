import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="broadcast--on-connect"
export default class extends Controller {
  static values = { method: String, methodArgs: Object };

  connect() {
    // console.log("broadcast--on-connect controller connected");
    this.checkConnectionInterval = setInterval(() => {
      if (this.#isConnectedToTurboStreamsChannel()) {
        this.#triggerBroadcast();
        clearInterval(this.checkConnectionInterval);
      } else {
        // console.log("Client is not connected to Turbo::StreamsChannel");
      }
    }, 100);
  }

  #triggerBroadcast() {
    fetch(`/broadcasts/${this.methodValue}`, {
      method: "POST",
      headers: {
        "X-CSRF-Token": document.querySelector('[name="csrf-token"]').content,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(this.methodArgsValue),
    });
  }

  #isConnectedToTurboStreamsChannel() {
    if (!window.Turbo) {
      return false;
    }

    // Check for Turbo::StreamsChannel subscriptions
    const turboStreamElements = document.querySelectorAll(
      'turbo-cable-stream-source[channel="Turbo::StreamsChannel"][connected]'
    );

    return turboStreamElements.length > 0;
  }
}
