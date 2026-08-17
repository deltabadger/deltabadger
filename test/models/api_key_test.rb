require 'test_helper'

class ApiKeyTest < ActiveSupport::TestCase
  test 'stores and encrypts the IBKR OAuth credential fields' do
    api_key = create(:api_key,
                     access_token: 'tok-123',
                     rsa_signature_key: "-----BEGIN PRIVATE KEY-----\nsig\n-----END PRIVATE KEY-----",
                     rsa_encryption_key: "-----BEGIN PRIVATE KEY-----\nenc\n-----END PRIVATE KEY-----",
                     dh_param: "-----BEGIN DH PARAMETERS-----\ndh\n-----END DH PARAMETERS-----",
                     ibkr_realm: 'limited_poa')

    api_key.reload
    assert_equal 'tok-123', api_key.access_token
    assert_includes api_key.rsa_signature_key, 'sig'
    assert_includes api_key.rsa_encryption_key, 'enc'
    assert_includes api_key.dh_param, 'DH PARAMETERS'
    assert_equal 'limited_poa', api_key.ibkr_realm

    # encrypted at rest: the raw column must not contain the plaintext
    raw = ApiKey.connection.select_value("SELECT access_token FROM api_keys WHERE id = #{api_key.id}")
    assert raw.present?
    refute_equal 'tok-123', raw
  end

  test 'pending_activation is a valid status appended without shifting existing ones' do
    assert_equal 0, ApiKey.statuses['pending_validation']
    assert_equal 1, ApiKey.statuses['correct']
    assert_equal 2, ApiKey.statuses['incorrect']
    assert_equal 3, ApiKey.statuses['pending_activation']

    api_key = create(:api_key)
    api_key.update!(status: :pending_activation)
    assert_predicate api_key.reload, :pending_activation?
  end

  test 'validate_credentials! maps a :pending_activation validity to pending_activation and persists the creds' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(:pending_activation))

    api_key.validate_credentials!(key: 'CONSUMER1', secret: 'sekret', access_token: 'tok', ibkr_realm: 'limited_poa')

    api_key.reload
    assert_predicate api_key, :pending_activation?
    assert_equal 'tok', api_key.access_token
    assert_equal 'limited_poa', api_key.ibkr_realm
  end

  test 'validate_credentials! still maps a truthy validity to correct (regression)' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(true))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_predicate api_key.reload, :correct?
  end

  # A sync failure writes `last_sync_error` and flips the key to :incorrect. Replacing the
  # credentials flips it back but used to leave the error behind, so `sync_issue` kept bannering the
  # tax report for a healthy exchange — and since a sync only runs when the user opens the tracker,
  # "until the next sync" can be months.
  test 'validate_credentials! clears a stale sync error once the new credentials check out' do
    api_key = create(:api_key, :pending, last_synced_at: 1.day.ago, last_sync_error: 'StandardError: API error')
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(true))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_predicate api_key.reload, :correct?
    assert_nil api_key.last_sync_error
    assert_nil api_key.sync_issue
  end

  test 'validate_credentials! keeps the sync error when the new credentials are rejected' do
    api_key = create(:api_key, :pending, last_sync_error: 'StandardError: API error')
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(false))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_equal 'StandardError: API error', api_key.reload.last_sync_error
    assert_equal :failed, api_key.sync_issue[:reason]
  end

  test 'validate_credentials! still maps a falsey validity to incorrect (regression)' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(false))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_predicate api_key, :incorrect?
  end

  test 'stop_dependent_bots! stops only the working bots on the key exchange' do
    api_key = create(:api_key) # binance, trading
    user = api_key.user
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    create(:dca_single_asset, :started, user: user, exchange: api_key.exchange, base_asset: btc, quote_asset: usd)
    create(:dca_single_asset, :started, user: user, exchange: create(:kraken_exchange), base_asset: eth, quote_asset: usd)

    # Exactly the one bot on the key's exchange must be stopped (stub stop to dodge the
    # Accountable save guard that fires on a freshly-loaded bot in tests).
    Bots::DcaSingleAsset.any_instance.expects(:stop).once

    api_key.stop_dependent_bots!
  end

  # --- IBKR activation staleness -------------------------------------------------------------
  # IBKR activates a self-service consumer key on its weekend server restart. A registration still
  # pending after ACTIVATION_DEADLINE has sat through several restarts and will never activate, so
  # the wizard has to stop promising an activation that is not coming. Time is frozen throughout:
  # updated_at is only second-precise, and a boundary assertion against an independently evaluated
  # Time.current is flaky.

  test 'activation_stalled? only applies to keys actually awaiting IBKR activation' do
    freeze_time do
      exchange = create(:binance_exchange) # one exchange, one key per user — both are unique-scoped
      %i[pending_validation correct incorrect].each do |status|
        api_key = create(:api_key, exchange: exchange, user: create(:user), status: status)
        api_key.update_column(:updated_at, 1.year.ago)

        refute_predicate api_key, :activation_stalled?, "#{status} is not an activation wait"
      end
    end
  end

  test 'activation_stalled? flips once the registration waits past the deadline' do
    freeze_time do
      api_key = create(:api_key, status: :pending_activation)

      api_key.update_column(:updated_at, ApiKey::ACTIVATION_DEADLINE.ago + 1.day)
      refute_predicate api_key, :activation_stalled?, 'still inside the activation window'

      api_key.update_column(:updated_at, ApiKey::ACTIVATION_DEADLINE.ago)
      assert_predicate api_key, :activation_stalled?, 'exactly at the deadline counts as stalled'

      api_key.update_column(:updated_at, ApiKey::ACTIVATION_DEADLINE.ago - 1.day)
      assert_predicate api_key, :activation_stalled?
    end
  end

  test 'resubmitting identical credentials restarts the IBKR activation clock' do
    freeze_time do
      api_key = create(:api_key, exchange: create(:ibkr_exchange), status: :pending_activation,
                                 raw_key: 'CONSUMERAB', raw_secret: 'SECRET789')
      api_key.update!(access_token: 'TOKEN456')
      api_key.update_column(:updated_at, 30.days.ago)
      assert_predicate api_key, :activation_stalled?
      api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(:pending_activation))

      # Exactly the values the row already holds — the real fix path is re-registering the SAME
      # consumer key on a working portal host and resubmitting. Nothing is dirty, so without an
      # explicit timestamp bump Active Record issues no UPDATE and the expired clock survives the
      # retry, leaving the wizard shouting "failed" at a user who just did the right thing.
      api_key.validate_credentials!(key: 'CONSUMERAB', secret: 'SECRET789', access_token: 'TOKEN456')

      assert_predicate api_key.reload, :pending_activation?
      assert_in_delta Time.current, api_key.updated_at, 1.second
      refute_predicate api_key, :activation_stalled?
    end
  end

  test 'update_status! keeps a pending IBKR key pending' do
    key = create(:api_key, status: :pending_activation)

    key.update_status!(Result::Success.new(:pending_activation))

    assert key.reload.pending_activation?, 'a pending key must not be marked correct'
  end

  test 'update_status! does not restart the IBKR activation deadline on a passive re-poll' do
    key = create(:api_key, status: :pending_activation)
    key.update_column(:updated_at, 30.days.ago)

    key.update_status!(Result::Success.new(:pending_activation))

    key.reload
    assert key.pending_activation?
    assert key.activation_stalled?, 'a passive re-poll must not restart the IBKR activation deadline'
  end

  test 'update_status! still marks a genuinely valid key correct' do
    key = create(:api_key, status: :pending_validation)

    key.update_status!(Result::Success.new(true))

    assert key.reload.correct?
  end

  test 'update_status! marks an invalid key incorrect' do
    key = create(:api_key, status: :pending_validation)

    key.update_status!(Result::Success.new(false))

    assert key.reload.incorrect?
  end

  # --- assign_credentials must not wipe fields the submitter never sent -------------------------

  test 'submitting only key and secret does not wipe stored IBKR material' do
    key = create(:api_key)
    key.update_columns(rsa_signature_key: 'SIGKEY', dh_param: 'DHPARAM')

    key.send(:assign_credentials, { key: 'new-key', secret: 'new-secret' })

    assert_equal 'new-key', key.key
    assert_equal 'new-secret', key.secret
    assert_equal 'SIGKEY', key.rsa_signature_key, 'omitted fields must be untouched'
    assert_equal 'DHPARAM', key.dh_param
  end

  test 'an explicitly nil field is still cleared' do
    key = create(:api_key)
    key.update_columns(passphrase: 'OLD')

    key.send(:assign_credentials, { key: 'k', passphrase: nil })

    assert_nil key.passphrase
  end

  test 'a string-keyed hash still assigns submitted fields and preserves omitted ones' do
    key = create(:api_key)
    key.update_columns(rsa_signature_key: 'SIGKEY')

    key.send(:assign_credentials, { 'key' => 'new-key', 'secret' => 'new-secret' })

    assert_equal 'new-key', key.key, 'a submitted string-keyed field must be assigned'
    assert_equal 'new-secret', key.secret
    assert_equal 'SIGKEY', key.rsa_signature_key, 'omitted fields must be untouched'
  end

  test 'a recorded sync error keeps credentials, emails and account ids out of the column' do
    key = create(:api_key)

    key.record_sync_error!(
      StandardError.new('rejected key AKIA1234567890ABCDEFG for jan@example.com account 123456789012 ' \
                        'at https://api.exchange.com/v3/ledger?apiKey=abc123def456ghi789jkl&sig=zz')
    )

    stored = key.reload.last_sync_error
    assert_includes stored, 'StandardError'
    assert_not_includes stored, 'AKIA1234567890ABCDEFG'
    assert_not_includes stored, 'jan@example.com'
    assert_not_includes stored, '123456789012'
    assert_not_includes stored, 'abc123def456ghi789jkl'
  end

  test 'a recorded sync error is truncated so a huge exchange body cannot land in the column' do
    key = create(:api_key)

    key.record_sync_error!(StandardError.new('a' * 5000))

    assert_operator key.reload.last_sync_error.length, :<=, ApiKey::SYNC_ERROR_LIMIT
  end

  test 'a recorded sync error accepts a plain exchange error string' do
    key = create(:api_key)

    key.record_sync_error!('Invalid API-key')

    assert_equal 'Invalid API-key', key.reload.last_sync_error
  end
end
