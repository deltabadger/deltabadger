# frozen_string_literal: true

require 'test_helper'
require 'middleware/empty_forwarded_host'

# With ALLOWED_HOSTS, an X-Forwarded-Host that names no host (`,`) made HostAuthorization raise
# (500). It is refused as a blocked host is: 403, empty body, as the Rust port answers.
class EmptyForwardedHostTest < ActiveSupport::TestCase
  setup do
    authorized = ActionDispatch::HostAuthorization.new(->(_env) { [200, {}, ['ok']] }, ['example.com'])
    @app = Rack::MockRequest.new(EmptyForwardedHost.new(authorized))
  end

  test 'a forwarded host naming nothing is refused with 403' do
    response = @app.get('/', 'HTTP_HOST' => 'example.com', 'HTTP_X_FORWARDED_HOST' => ', ')

    assert_equal 403, response.status
    assert_equal '', response.body
    assert_equal 'text/html; charset=UTF-8', response.content_type
  end

  test 'an XHR gets plain text' do
    response = @app.get('/', 'HTTP_HOST' => 'example.com', 'HTTP_X_FORWARDED_HOST' => ',',
                             'HTTP_X_REQUESTED_WITH' => 'XMLHttpRequest')

    assert_equal 403, response.status
    assert_equal 'text/plain; charset=UTF-8', response.content_type
  end

  test 'an allowed or absent forwarded host passes' do
    assert_equal 200, @app.get('/', 'HTTP_HOST' => 'example.com', 'HTTP_X_FORWARDED_HOST' => 'a, example.com').status
    assert_equal 200, @app.get('/', 'HTTP_HOST' => 'example.com').status
  end
end
