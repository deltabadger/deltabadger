# frozen_string_literal: true

module Oauth
  class BaseController < ActionController::Base
    # Inherits from ActionController::Base (not ApplicationController) to avoid
    # Devise's authenticate_user!, setup redirect, and locale switching that
    # would break OAuth JSON endpoints.
    protect_from_forgery with: :exception
  end
end
