require 'test_helper'

class Api::ApiKeysControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    sign_in @user
  end

  test 'stores a secret containing a double quote intact' do
    secret = 'abc"def'
    post '/api/api_keys', params: { api_key: { key: 'k', secret: secret,
                                               exchange_id: create(:exchange).id } }

    assert_response :created
    assert_equal secret, ApiKey.last.secret
  end

  test 'unescapes a pasted PEM secret into real newlines' do
    pem = '-----BEGIN EC PRIVATE KEY-----\nabc123\n-----END EC PRIVATE KEY-----\n'
    post '/api/api_keys', params: { api_key: { key: 'k', secret: pem,
                                               exchange_id: create(:exchange).id } }

    assert_response :created
    assert_equal "-----BEGIN EC PRIVATE KEY-----\nabc123\n-----END EC PRIVATE KEY-----\n",
                 ApiKey.last.secret
  end
end
