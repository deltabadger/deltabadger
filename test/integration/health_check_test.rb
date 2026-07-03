require 'test_helper'

class HealthCheckTest < ActionDispatch::IntegrationTest
  # /up is served by Rails' built-in health controller, which inherits from
  # ActionController::Base rather than the app's ApplicationController, so it
  # must NOT be caught by the locale / setup / auth before_actions that redirect
  # ordinary requests. Guards the endpoint the Docker HEALTHCHECK relies on.
  test 'GET /up returns 200 without app redirects' do
    get '/up'

    assert_response :success
  end
end
