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
    // console.log("triggerBroadcast", this.methodValue);
    fetch(`/${this.#getLocaleFromUrl()}/broadcasts/${this.methodValue}`, {
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

  #getLocaleFromUrl(defaultLocale = 'en') {
    const path = window.location.pathname;
    const segments = path.split('/').filter(segment => segment);

    const firstSegment = segments[0];
    if (firstSegment && /^[a-z]{2}$/i.test(firstSegment)) {
      return firstSegment;
    }

    return defaultLocale;
  }
}
