import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="broadcast--on-connect"
export default class extends Controller {
  // retryWhile: a selector that is present ONLY while the answer has not arrived (the global
  // PnL's spinner). Opt-in, because this controller also drives broadcasts that must fire once.
  static values = {
    method: String,
    methodArgs: Object,
    retryWhile: String,
    retryAfter: { type: Number, default: 8000 },
    retryFor: { type: Number, default: 90000 },
  };

  connect() {
    this.deadline = Date.now() + this.retryForValue;
    this.checkConnectionInterval = setInterval(() => {
      if (this.#isConnectedToTurboStreamsChannel()) {
        this.#triggerBroadcast();
        clearInterval(this.checkConnectionInterval);
      }
    }, 100);
  }

  disconnect() {
    clearInterval(this.checkConnectionInterval);
    clearTimeout(this.retryTimeout);
  }

  // The request can fail, the job can error, and a broadcast can land during a disconnect —
  // none of which this page would otherwise notice. Until this existed, the only thing clearing
  // a stuck spinner was one of the N per-bot jobs broadcasting the total as a side effect, which
  // is precisely the work that made a large dashboard quadratic.
  //
  // Bounded by a DEADLINE, not by a count of attempts: on a cold eighty-bot account the first
  // pass can outlast several checks, and those checks are discarded server-side by the job's
  // per-user concurrency lock — counting them would spend the whole budget waiting for a pass
  // that is still running, leaving nothing for the failure the retry exists to cover. A discarded
  // request is cheap; a stuck spinner is not.
  #scheduleRetry() {
    if (!this.hasRetryWhileValue || Date.now() >= this.deadline) return;

    this.retryTimeout = setTimeout(() => {
      if (!document.querySelector(this.retryWhileValue)) return; // the answer arrived

      this.#triggerBroadcast();
    }, this.retryAfterValue);
  }

  #triggerBroadcast() {
    this.#scheduleRetry();
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
