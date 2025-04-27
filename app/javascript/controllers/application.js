import { Application } from "@hotwired/stimulus";

const application = Application.start();

// Configure Stimulus development experience
application.debug = false;
window.Stimulus = application;

export { application };

// FIXME: temporary solution from https://github.com/hotwired/turbo-rails/pull/367#issuecomment-1934729149
Turbo.StreamActions.redirect = function () {
  Turbo.visit(this.target);
};
