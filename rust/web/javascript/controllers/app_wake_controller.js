import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="app-wake"
// Detects when the app becomes visible after being in background/sleep
// and triggers the wake_dispatcher to release any overdue scheduled jobs
export default class extends Controller {
  connect() {
    this.boundHandleVisibilityChange = this.#handleVisibilityChange.bind(this);
    document.addEventListener("visibilitychange", this.boundHandleVisibilityChange);
  }

  disconnect() {
    document.removeEventListener("visibilitychange", this.boundHandleVisibilityChange);
  }

  #handleVisibilityChange() {
    if (document.visibilityState === "visible") {
      this.#wakeDispatcher();
    }
  }

  #wakeDispatcher() {
    const csrfToken = document.querySelector('[name="csrf-token"]');
    if (!csrfToken) return;

    fetch(`/${this.#getLocaleFromUrl()}/broadcasts/wake_dispatcher`, {
      method: "POST",
      headers: {
        "X-CSRF-Token": csrfToken.content,
        "Content-Type": "application/json",
      },
    });
  }

  #getLocaleFromUrl(defaultLocale = "en") {
    const path = window.location.pathname;
    const segments = path.split("/").filter((segment) => segment);

    const firstSegment = segments[0];
    if (firstSegment && /^[a-z]{2}$/i.test(firstSegment)) {
      return firstSegment;
    }

    return defaultLocale;
  }
}
