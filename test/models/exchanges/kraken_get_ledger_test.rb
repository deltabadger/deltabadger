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

  test 'trade fee is reported separately and base_amount stays gross' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:get_ledgers).returns(
      Result::Success.new({
                            'error' => [],
                            'result' => {
                              'ledger' => {
                                'L-BTC' => {
                                  'refid' => 'R-GROSS',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'trade',
                                  'subtype' => '',
                                  'aclass' => 'currency',
                                  'asset' => 'XXBT',
                                  'amount' => '0.5',
                                  'fee' => '0',
                                  'balance' => '1.0'
                                },
                                'L-EUR' => {
                                  'refid' => 'R-GROSS',
                                  'time' => 1_710_936_000.0,
                                  'type' => 'trade',
                                  'subtype' => '',
                                  'aclass' => 'currency',
                                  'asset' => 'ZEUR',
                                  'amount' => '-10000',
                                  'fee' => '26',
                                  'balance' => '5000.0'
                                }
                              },
                              'count' => 2
                            }
                          })
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    btc_entry = result.data.find { |entry| entry[:base_currency] == 'BTC' }
    eur_entry = result.data.find { |entry| entry[:base_currency] == 'EUR' }
    assert_not_nil btc_entry
    assert_not_nil eur_entry
    # The tax engine capitalises acquisition fees and would double-count if base_amount ever became net.
    assert_equal :buy, btc_entry[:entry_type]
    assert_equal 0.5.to_d, btc_entry[:base_amount]
    assert_nil btc_entry[:fee_currency]
    assert_nil btc_entry[:fee_amount]
    assert_equal :sell, eur_entry[:entry_type]
    assert_equal 10_000.to_d, eur_entry[:base_amount]
    assert_equal 'EUR', eur_entry[:fee_currency]
    assert_equal 26.to_d, eur_entry[:fee_amount]
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

  # --- Kraken's decaying rate counter ---------------------------------------------------------
  # Each Ledgers page costs counter points that come back slowly, so a long history runs out part
  # way through. Giving up there threw every page away and the next sync started over from page
  # one — an account with a long history could never sync at all.

  def ledger_page(id, count:)
    Result::Success.new({ 'error' => [], 'result' => {
                          'ledger' => { id => { 'refid' => "R#{id}", 'time' => 1_710_936_000.0, 'type' => 'deposit',
                                                'subtype' => '', 'aclass' => 'currency', 'asset' => 'XXBT',
                                                'amount' => '0.1', 'fee' => '0', 'balance' => '1.0' } },
                          'count' => count
                        } })
  end

  def throttled
    Result::Success.new({ 'error' => ['EAPI:Rate limit exceeded'] })
  end

  test 'a throttled page is waited out and fetched again, not the whole history' do
    client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(client)
    client.expects(:get_ledgers).with(start: nil, ofs: 0).returns(ledger_page('L1', count: 2))
    client.expects(:get_ledgers).with(start: nil, ofs: 1).times(3)
          .returns(throttled).then.returns(throttled).then.returns(ledger_page('L2', count: 2))
    @exchange.expects(:sleep).with(Exchanges::Kraken::LEDGER_THROTTLE_WAIT).twice

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal(%w[L1 L2], result.data.map { |entry| entry[:tx_id] })
  end

  test 'a counter that never recovers still fails, after a bounded wait' do
    client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(client)
    client.stubs(:get_ledgers).returns(throttled)
    @exchange.expects(:sleep).times(Exchanges::Kraken::LEDGER_THROTTLE_RETRIES)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.failure?
    assert_equal ['EAPI:Rate limit exceeded'], result.errors
  end

  test 'other errors are not waited on' do
    client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(client)
    client.stubs(:get_ledgers).returns(Result::Success.new({ 'error' => ['EGeneral:Permission denied'] }))
    @exchange.expects(:sleep).never

    assert @exchange.get_ledger(api_key: @api_key).failure?
  end
end
