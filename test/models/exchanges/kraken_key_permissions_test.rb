require 'test_helper'

# Kraken reports a key's own permission flags (GetApiKeyInfo, which needs no permission itself), so
# the key check compares them with what that key type's setup steps ask for — every line of the
# steps is proven, instead of one order probe standing in for all of them.
class KrakenKeyPermissionsTest < ActiveSupport::TestCase
  TRADING = %w[query-funds query-open-trades query-closed-trades modify-trades close-trades query-ledger].freeze

  setup do
    # Tests run in dry run, where Dryable answers the trading and withdrawal checks without asking.
    Rails.configuration.stubs(:dry_run).returns(false)
    @exchange = create(:kraken_exchange)
  end

  def key(type)
    create(:api_key, exchange: @exchange, key_type: type, key: 'test_key', secret: 'dGVzdF9zZWNyZXQ=')
  end

  def answer(permissions: nil, errors: [])
    body = { 'error' => errors }
    body['result'] = { 'apiKeyName' => 'k', 'permissions' => permissions } if permissions
    Honeymaker::Clients::Kraken.any_instance.stubs(:get_api_key_info).returns(Result::Success.new(body))
  end

  def validity(type)
    api_key = key(type)
    type == :read_only ? @exchange.get_read_api_key_validity(api_key:) : @exchange.get_api_key_validity(api_key:)
  end

  # Every other test here stubs the client, so this is what stops the check shipping against a
  # honeymaker that cannot make the call.
  test 'the installed honeymaker can ask Kraken for a key\'s permissions' do
    assert Honeymaker::Clients::Kraken.method_defined?(:get_api_key_info),
           'honeymaker needs Kraken#get_api_key_info (GetApiKeyInfo) — bump the gem'
  end

  test 'a trading key with exactly the listed permissions is valid' do
    answer(permissions: TRADING)

    assert_equal true, validity(:trading).data
  end

  test 'permissions the steps do not mention are ignored' do
    answer(permissions: TRADING + %w[export-data create-ws-token earn-funds])

    assert_equal true, validity(:trading).data
  end

  test 'each missing permission is named' do
    answer(permissions: TRADING - %w[query-ledger close-trades])

    assert_equal({ missing_permissions: %w[close-trades query-ledger], forbidden_permissions: [] },
                 validity(:trading).data)
  end

  test 'withdraw on a trading key is named as forbidden' do
    answer(permissions: TRADING + %w[withdraw-funds])

    assert_equal({ missing_permissions: [], forbidden_permissions: %w[withdraw-funds] }, validity(:trading).data)
  end

  test 'missing and forbidden are reported together' do
    answer(permissions: %w[query-funds withdraw-funds])

    data = validity(:trading).data
    assert_equal TRADING - %w[query-funds], data[:missing_permissions]
    assert_equal %w[withdraw-funds], data[:forbidden_permissions]
  end

  # The tracker only reads: balances and the ledger. Order permissions are not asked for, and not
  # held against the key either.
  test 'a reading key needs funds query and the ledger' do
    answer(permissions: %w[query-funds query-ledger])
    assert_equal true, validity(:read_only).data

    answer(permissions: %w[query-funds])
    assert_equal({ missing_permissions: %w[query-ledger], forbidden_permissions: [] }, validity(:read_only).data)

    answer(permissions: TRADING)
    assert_equal true, validity(:read_only).data
  end

  test 'a reading key may not withdraw' do
    answer(permissions: %w[query-funds query-ledger withdraw-funds])

    assert_equal %w[withdraw-funds], validity(:read_only).data[:forbidden_permissions]
  end

  test 'a withdrawal key needs query and withdraw, and no order permission' do
    answer(permissions: %w[query-funds withdraw-funds])
    assert_equal true, validity(:withdrawal).data

    answer(permissions: %w[query-funds])
    assert_equal %w[withdraw-funds], validity(:withdrawal).data[:missing_permissions]

    answer(permissions: %w[query-funds withdraw-funds modify-trades query-open-trades])
    assert_equal %w[query-open-trades modify-trades], validity(:withdrawal).data[:forbidden_permissions]
  end

  test 'a key Kraken does not recognise is incorrect' do
    answer(errors: ['EAPI:Invalid key'])
    assert_equal false, validity(:trading).data

    answer(errors: ['EAPI:Invalid signature'])
    assert_equal false, validity(:read_only).data
  end

  # Nothing is inferred from an answer that is not a permission list: the key stays unverified.
  test 'throttles, lockouts, outages and malformed answers are inconclusive' do
    [['EAPI:Rate limit exceeded'], ['EGeneral:Temporary lockout'], ['EService:Unavailable'],
     ['EGeneral:Permission denied']].each do |errors|
      answer(errors:)
      assert_predicate validity(:trading), :failure?, errors.inspect
    end

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_api_key_info).returns(Result::Failure.new('execution expired'))
    assert_predicate validity(:trading), :failure?

    Honeymaker::Clients::Kraken.any_instance.stubs(:get_api_key_info)
                               .returns(Result::Success.new({ 'error' => [], 'result' => {} }))
    assert_predicate validity(:trading), :failure?
  end

  test 'the check makes one request and places no order' do
    answer(permissions: TRADING)
    Honeymaker::Clients::Kraken.any_instance.expects(:add_order).never
    Honeymaker::Clients::Kraken.any_instance.expects(:get_extended_balance).never

    validity(:trading)
  end

  test 'a reading key is valid without asking in dry run' do
    Rails.configuration.stubs(:dry_run).returns(true)
    Honeymaker::Clients::Kraken.any_instance.expects(:get_api_key_info).never

    assert_equal true, validity(:read_only).data
  end
end
