# Session-authenticated JSON for the browser UI — Devise only, CSRF intact, no
# bearer token ever reaches it. Per-client tool permissions live on the OAuth
# surface (Api::V1::BaseController); there is no client here to scope to.
module Api
  class BaseController < ApplicationController
    before_action :authenticate_user!
  end
end
