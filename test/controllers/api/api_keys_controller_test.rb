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

  HL_KEY = "0x#{'a' * 40}".freeze
  HL_SECRET = "0x#{'b' * 64}".freeze

  def hyperliquid_key_with_history
    api_key = create(:api_key, user: @user, exchange: create(:hyperliquid_exchange), raw_key: HL_KEY, raw_secret: HL_SECRET)
    [api_key, create(:account_transaction, api_key: api_key)]
  end

  test 'a rejected replacement keeps the stored key and its ledger links' do
    api_key, ledger_row = hyperliquid_key_with_history

    post '/api/api_keys', params: { api_key: { key: 'not-a-wallet', secret: 'bad', exchange_id: api_key.exchange_id, key_type: 'trading' } }

    assert_response :unprocessable_entity
    assert_equal({ 'data' => false }, response.parsed_body)
    api_key.reload
    assert_equal [HL_KEY, HL_SECRET, 'correct'], [api_key.key, api_key.secret, api_key.status]
    assert_equal api_key.id, ledger_row.reload.api_key_id
  end

  test 'an accepted replacement updates the key in place and keeps its ledger links' do
    api_key, ledger_row = hyperliquid_key_with_history
    new_secret = "0x#{'c' * 64}"

    assert_no_difference -> { ApiKey.count } do
      post '/api/api_keys', params: { api_key: { key: HL_KEY, secret: new_secret, exchange_id: api_key.exchange_id, key_type: 'trading' } }
    end

    assert_response :created
    assert_equal({ 'data' => true }, response.parsed_body)
    replaced = @user.api_keys.sole
    assert_equal api_key.id, replaced.id
    assert_equal [new_secret, 'pending_validation'], [replaced.secret, replaced.status]
    assert_equal api_key.id, ledger_row.reload.api_key_id
  end
end
