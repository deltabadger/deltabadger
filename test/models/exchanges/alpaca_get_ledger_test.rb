require 'test_helper'

class Exchanges::AlpacaGetLedgerTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange, passphrase: 'live')
  end

  def stub_activities(activities)
    client = mock('alpaca_client')
    client.stubs(:get_account_activities).returns(Result::Success.new(activities))
    @exchange.stubs(:client).returns(client)
    @exchange.instance_variable_set(:@client, client)
    client
  end

  test 'returns normalized buy entry from FILL activity' do
    stub_activities([
                      {
                        'id' => 'fill-1', 'activity_type' => 'FILL',
                        'symbol' => 'AAPL', 'side' => 'buy',
                        'qty' => '10', 'price' => '150.00',
                        'transaction_time' => '2026-03-20T14:30:00Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    buy = result.data.find { |e| e[:entry_type] == :buy }
    assert_not_nil buy
    assert_equal 'AAPL', buy[:base_currency]
    assert_equal 10.to_d, buy[:base_amount]
    assert_equal 'USD', buy[:quote_currency]
    assert_equal 1500.to_d, buy[:quote_amount]
    assert_equal 'fill-1', buy[:tx_id]
  end

  test 'returns normalized sell entry from FILL activity' do
    stub_activities([
                      {
                        'id' => 'fill-2', 'activity_type' => 'FILL',
                        'symbol' => 'AAPL', 'side' => 'sell',
                        'qty' => '5', 'price' => '155.00',
                        'transaction_time' => '2026-03-20T15:00:00Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    sell = result.data.find { |e| e[:entry_type] == :sell }
    assert_not_nil sell
    assert_equal 'AAPL', sell[:base_currency]
    assert_equal 5.to_d, sell[:base_amount]
    assert_equal 'USD', sell[:quote_currency]
    assert_equal 775.to_d, sell[:quote_amount]
  end

  test 'returns normalized deposit entry from CSD activity' do
    stub_activities([
                      {
                        'id' => 'csd-1', 'activity_type' => 'CSD',
                        'net_amount' => '5000.00', 'date' => '2026-03-19'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.find { |e| e[:entry_type] == :deposit }
    assert_not_nil deposit
    assert_equal 'USD', deposit[:base_currency]
    assert_equal 5000.to_d, deposit[:base_amount]
    assert_equal 'csd-1', deposit[:tx_id]
  end

  test 'returns normalized withdrawal entry from CSW activity' do
    stub_activities([
                      {
                        'id' => 'csw-1', 'activity_type' => 'CSW',
                        'net_amount' => '-2000.00', 'date' => '2026-03-19'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    withdrawal = result.data.find { |e| e[:entry_type] == :withdrawal }
    assert_not_nil withdrawal
    assert_equal 'USD', withdrawal[:base_currency]
    assert_equal 2000.to_d, withdrawal[:base_amount]
    assert_equal 'csw-1', withdrawal[:tx_id]
  end

  test 'returns normalized dividend entry from DIV activity' do
    stub_activities([
                      {
                        'id' => 'div-1', 'activity_type' => 'DIV',
                        'symbol' => 'AAPL', 'net_amount' => '12.50',
                        'qty' => '50', 'per_share_amount' => '0.25',
                        'date' => '2026-03-15'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    div = result.data.find { |e| e[:entry_type] == :other_income }
    assert_not_nil div
    assert_equal 'USD', div[:base_currency]
    assert_equal 12.5.to_d, div[:base_amount]
    assert_equal 'div-1', div[:tx_id]
    assert_match(/dividend/i, div[:description])
  end

  test 'returns normalized fee entry from FEE activity' do
    stub_activities([
                      {
                        'id' => 'fee-1', 'activity_type' => 'FEE',
                        'net_amount' => '-1.50', 'date' => '2026-03-18'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fee = result.data.find { |e| e[:entry_type] == :fee }
    assert_not_nil fee
    assert_equal 'USD', fee[:base_currency]
    assert_equal 1.5.to_d, fee[:base_amount]
    assert_equal 'fee-1', fee[:tx_id]
  end

  test 'returns normalized interest entry from INT activity' do
    stub_activities([
                      {
                        'id' => 'int-1', 'activity_type' => 'INT',
                        'net_amount' => '3.25', 'date' => '2026-03-18'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    interest = result.data.find { |e| e[:entry_type] == :other_income }
    assert_not_nil interest
    assert_equal 'USD', interest[:base_currency]
    assert_equal 3.25.to_d, interest[:base_amount]
  end

  test 'skips unknown activity types' do
    stub_activities([
                      {
                        'id' => 'unknown-1', 'activity_type' => 'JNLC',
                        'net_amount' => '100.00', 'date' => '2026-03-18'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_empty result.data
  end

  test 'passes start_time as after parameter' do
    start_time = Time.utc(2026, 3, 20)
    client = mock('alpaca_client')
    client.expects(:get_account_activities).with(has_entry(after: start_time.iso8601)).returns(Result::Success.new([]))
    @exchange.stubs(:client).returns(client)
    @exchange.instance_variable_set(:@client, client)

    @exchange.get_ledger(api_key: @api_key, start_time: start_time)
  end

  test 'returns failure when client returns failure' do
    client = mock('alpaca_client')
    client.stubs(:get_account_activities).returns(Result::Failure.new('API error'))
    @exchange.stubs(:client).returns(client)
    @exchange.instance_variable_set(:@client, client)

    result = @exchange.get_ledger(api_key: @api_key)
    assert result.failure?
  end

  test 'handles multiple activity types in one response' do
    stub_activities([
                      {
                        'id' => 'fill-1', 'activity_type' => 'FILL',
                        'symbol' => 'AAPL', 'side' => 'buy',
                        'qty' => '10', 'price' => '150.00',
                        'transaction_time' => '2026-03-20T14:30:00Z'
                      },
                      {
                        'id' => 'div-1', 'activity_type' => 'DIV',
                        'symbol' => 'AAPL', 'net_amount' => '12.50',
                        'qty' => '50', 'per_share_amount' => '0.25',
                        'date' => '2026-03-15'
                      },
                      {
                        'id' => 'csd-1', 'activity_type' => 'CSD',
                        'net_amount' => '5000.00', 'date' => '2026-03-10'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 3, result.data.size
    assert_equal(%i[buy other_income deposit], result.data.map { |e| e[:entry_type] })
  end
end
