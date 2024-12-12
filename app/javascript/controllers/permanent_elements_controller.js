import { Controller } from "@hotwired/stimulus"

// Waits for specific events and adds data-turbo-permanent to elements
// This will prevent Turbo from removing them when navigating
// Use the event 'turbo:load' to make elements permanent after a generic page load

// Connects to data-controller="permanent-elements"
export default class extends Controller {
  static values = { events: Object }; // Expecting a map of event IDs and element IDs

  connect() {
    this.eventListeners = [];

    if (this.hasEventsValue) {
      for (const [eventId, elementIds] of Object.entries(this.eventsValue)) {
        const listener = () => this.makeElementsPermanent(elementIds);
        this.eventListeners.push({ eventId, listener });
        document.addEventListener(eventId, listener);
      }
    }
  }

  disconnect() {
    this.eventListeners.forEach(({ eventId, listener }) => {
      document.removeEventListener(eventId, listener);
    });
  }

  makeElementsPermanent(ids) {
    ids.forEach((id) => {
      const element = document.getElementById(id);
      if (element) {
        element.setAttribute("data-turbo-permanent", "");
      }
    });
  }
}
