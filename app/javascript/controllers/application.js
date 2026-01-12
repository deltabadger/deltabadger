import { Application } from "@hotwired/stimulus";

const application = Application.start();

// Configure Stimulus development experience
application.debug = false;
window.Stimulus = application;

export { application };

// Custom Turbo Stream Actions
Turbo.StreamActions.redirect = function () {
  Turbo.visit(this.target);
};

Turbo.StreamActions.add_class = function () {
  const className = this.getAttribute("class-name");
  this.targetElements.forEach((element) => element.classList.add(className));
};

Turbo.StreamActions.remove_class = function () {
  const className = this.getAttribute("class-name");
  this.targetElements.forEach((element) => element.classList.remove(className));
};
