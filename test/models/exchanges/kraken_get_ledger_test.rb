require 'test_helper'

class Exchanges::KrakenGetLedgerTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:kraken_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'returns normalized trade entries from ledger' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({
                            'error' => [],
                            'result' => {
                              'ledger' => {
                                'L1' => {
                                  'refid' => 'R1',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'trade',
                                  'subtype' => '',
                                  'aclass' => 'currency',
                                  'asset' => 'XXBT',
                                  'amount' => '0.5',
                                  'fee' => '0.001',
                                  'balance' => '1.0'
                                }
                              },
                              'count' => 1
                            }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries = result.data
    assert_equal 1, entries.size

    trade = entries.first
    assert_equal :buy, trade[:entry_type]
    assert_equal 'BTC', trade[:base_currency]
    assert_equal 0.5, trade[:base_amount]
    assert_equal 'L1', trade[:tx_id]
    assert_equal 'R1', trade[:group_id]
  end

  test 'returns deposit entries' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({
                            'error' => [],
                            'result' => {
                              'ledger' => {
                                'L2' => {
                                  'refid' => 'R2',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'deposit',
                                  'asset' => 'ZUSD',
                                  'amount' => '5000.0',
                                  'fee' => '0.0',
                                  'balance' => '5000.0'
                                }
                              },
                              'count' => 1
                            }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.first
    assert_equal :deposit, deposit[:entry_type]
    assert_equal 'USD', deposit[:base_currency]
    assert_equal 5000.0, deposit[:base_amount]
  end

  test 'returns staking reward entries' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({
                            'error' => [],
                            'result' => {
                              'ledger' => {
                                'L3' => {
                                  'refid' => 'R3',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'staking',
                                  'asset' => 'ETH2',
                                  'amount' => '0.01',
                                  'fee' => '0.0',
                                  'balance' => '1.01'
                                }
                              },
                              'count' => 1
                            }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    reward = result.data.first
    assert_equal :staking_reward, reward[:entry_type]
    assert_equal 'ETH2', reward[:base_currency]
  end

  test 'skips internal transfer entries' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({
                            'error' => [],
                            'result' => {
                              'ledger' => {
                                'L4' => {
                                  'refid' => 'R4',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'transfer',
                                  'asset' => 'XXBT',
                                  'amount' => '0.1',
                                  'fee' => '0.0',
                                  'balance' => '0.1'
                                }
                              },
                              'count' => 1
                            }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_empty result.data
  end

  test 'returns failure on API error' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(Result::Failure.new('Rate limit exceeded'))

    result = @exchange.get_ledger(api_key: @api_key)
    assert result.failure?
  end

  test 'returns failure on Kraken error array' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({ 'error' => ['EAPI:Invalid key'], 'result' => {} })
    )

    result = @exchange.get_ledger(api_key: @api_key)
    assert result.failure?
  end
end
