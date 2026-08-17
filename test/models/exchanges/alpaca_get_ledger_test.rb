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

  def stub_paginated_activities(first_page, second_result)
    client = mock('alpaca_client')
    pages = sequence('activity pagination')
    client.expects(:get_account_activities)
          .with(direction: 'asc', page_size: 100)
          .returns(Result::Success.new(first_page))
          .in_sequence(pages)
    client.expects(:get_account_activities)
          .with(direction: 'asc', page_size: 100, page_token: first_page.last.fetch('id'))
          .returns(second_result)
          .in_sequence(pages)
    @exchange.stubs(:client).returns(client)
    @exchange.instance_variable_set(:@client, client)
    client
  end

  test 'get_ledger follows exclusive id page_token until a short page' do
    page1 = Array.new(100) do |index|
      {
        'id' => "page-#{index + 1}", 'activity_type' => 'INT',
        'net_amount' => (index + 1).to_s, 'date' => '2026-04-01'
      }
    end
    page2 = [
      {
        'id' => 'page-101', 'activity_type' => 'INT',
        'net_amount' => '101.25', 'date' => '2026-04-02'
      }
    ]
    stub_paginated_activities(page1, Result::Success.new(page2))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 101, result.data.size
    assert_equal(
      (1..101).map { |index| "page-#{index}" },
      result.data.map { |entry| entry[:tx_id] }
    )
  end

  test 'get_ledger returns a second page failure instead of partial success' do
    page1 = Array.new(100) do |index|
      {
        'id' => "failure-page-#{index + 1}", 'activity_type' => 'INT',
        'net_amount' => (index + 1).to_s, 'date' => '2026-04-03'
      }
    end
    failure = Result::Failure.new('invented second page error')
    stub_paginated_activities(page1, failure)

    result = @exchange.get_ledger(api_key: @api_key)

    assert_same failure, result
  end

  test 'get_ledger stops pagination after an empty page' do
    page1 = Array.new(100) do |index|
      {
        'id' => "empty-page-#{index + 1}", 'activity_type' => 'INT',
        'net_amount' => (index + 1).to_s, 'date' => '2026-04-04'
      }
    end
    stub_paginated_activities(page1, Result::Success.new([]))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 100, result.data.size
  end

  test 'get_ledger stops when the cursor fails to advance' do
    page = Array.new(100) do |index|
      {
        'id' => "stuck-page-#{index + 1}", 'activity_type' => 'INT',
        'net_amount' => (index + 1).to_s, 'date' => '2026-04-04'
      }
    end
    client = mock('alpaca_client')
    # An API that ignored page_token would hand back the same page forever.
    client.expects(:get_account_activities).twice.returns(Result::Success.new(page))
    @exchange.stubs(:client).returns(client)
    @exchange.instance_variable_set(:@client, client)
    Rails.logger.expects(:warn).with { |message| message.include?('stuck-page-100') }

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 100, result.data.size
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

  test 'trade fee is reported separately and base_amount stays gross' do
    stub_activities([
                      {
                        'id' => 'fill-gross', 'activity_type' => 'FILL',
                        'symbol' => 'AAPL', 'side' => 'buy',
                        'qty' => '10', 'price' => '150.00',
                        'transaction_time' => '2026-03-20T14:30:00Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fill = result.data.find { |entry| entry[:entry_type] == :buy }
    assert_not_nil fill
    # The tax engine capitalises acquisition fees and would double-count if base_amount ever became net.
    assert_equal 10.to_d, fill[:base_amount]
    assert_nil fill[:fee_currency]
    assert_nil fill[:fee_amount]
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

  test 'normalizes every dividend income activity type with its symbol' do
    amounts = {
      'DIV' => '12.50',
      'DIVCGL' => '4.75',
      'DIVCGS' => '3.60',
      'CGD' => '2.45',
      'DIVTXEX' => '1.30'
    }
    activities = amounts.map do |type, amount|
      {
        'id' => "income-#{type.downcase}", 'activity_type' => type,
        'symbol' => 'ZZTOP', 'net_amount' => amount,
        'date' => '2026-04-05', 'status' => 'executed'
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    amounts.each do |type, amount|
      entry = entries_by_id.fetch("income-#{type.downcase}")
      assert_equal :other_income, entry[:entry_type]
      assert_equal 'USD', entry[:base_currency]
      assert_equal amount.to_d.abs, entry[:base_amount]
      assert_equal 'ZZTOP', entry[:quote_currency]
      assert_match(/ZZTOP/, entry[:description])
    end
  end

  test 'normalizes dividend withholding activities as positive USD amounts' do
    amounts = {
      'DIVNRA' => '-2.15',
      'DIVFT' => '-1.40',
      'DIVTW' => '-0.65'
    }
    activities = amounts.map do |type, amount|
      {
        'id' => "withholding-#{type.downcase}", 'activity_type' => type,
        'symbol' => 'QQTEST', 'net_amount' => amount,
        'date' => '2026-04-06', 'status' => 'correct'
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    amounts.each do |type, amount|
      entry = entries_by_id.fetch("withholding-#{type.downcase}")
      assert_equal :withholding_tax, entry[:entry_type]
      assert_equal 'USD', entry[:base_currency]
      assert_equal amount.to_d.abs, entry[:base_amount]
      assert_equal 'QQTEST', entry[:quote_currency]
      assert_equal 'Withholding (QQTEST)', entry[:description]
    end
  end

  test 'normalizes interest withholding activities without a symbol' do
    amounts = {
      'INTNRA' => '-3.25',
      'INTTW' => '-1.15'
    }
    activities = amounts.map do |type, amount|
      {
        'id' => "interest-#{type.downcase}", 'activity_type' => type,
        'net_amount' => amount, 'date' => '2026-04-07', 'status' => 'executed'
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    amounts.each do |type, amount|
      entry = entries_by_id.fetch("interest-#{type.downcase}")
      assert_equal :withholding_tax, entry[:entry_type]
      assert_equal 'USD', entry[:base_currency]
      assert_equal amount.to_d.abs, entry[:base_amount]
      assert_nil entry[:quote_currency]
      assert_equal 'Withholding (interest)', entry[:description]
    end
  end

  test 'normalizes return of capital and preserves per share raw data' do
    stub_activities([
                      {
                        'id' => 'roc-with-qty', 'activity_type' => 'DIVROC',
                        'symbol' => 'ZZTOP', 'qty' => '4.25', 'net_amount' => '8.75',
                        'per_share_amount' => '2.05', 'date' => '2026-04-08',
                        'status' => 'executed'
                      },
                      {
                        'id' => 'roc-without-qty', 'activity_type' => 'DIVROC',
                        'symbol' => 'QQTEST', 'net_amount' => '6.35',
                        'per_share_amount' => '0.55', 'date' => '2026-04-08',
                        'status' => 'correct'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    with_qty = entries_by_id.fetch('roc-with-qty')
    assert_equal :return_of_capital, with_qty[:entry_type]
    assert_equal 'ZZTOP', with_qty[:base_currency]
    assert_equal 4.25.to_d, with_qty[:base_amount]
    assert_equal 'USD', with_qty[:quote_currency]
    assert_equal 8.75.to_d, with_qty[:quote_amount]
    assert_equal '2.05', with_qty[:raw_data]['per_share_amount']
    assert_match(/ZZTOP/, with_qty[:description])

    without_qty = entries_by_id.fetch('roc-without-qty')
    assert_equal :return_of_capital, without_qty[:entry_type]
    assert_equal 'QQTEST', without_qty[:base_currency]
    assert_equal 0.to_d, without_qty[:base_amount]
    assert_equal 'USD', without_qty[:quote_currency]
    assert_equal 6.35.to_d, without_qty[:quote_amount]
    assert_equal '0.55', without_qty[:raw_data]['per_share_amount']
    assert_match(/QQTEST/, without_qty[:description])
  end

  test 'preserves the Alpaca group_id for dividend and withholding siblings' do
    stub_activities([
                      {
                        'id' => 'group-div', 'activity_type' => 'DIV',
                        'symbol' => 'ZZTOP', 'net_amount' => '9.45',
                        'date' => '2026-04-09', 'status' => 'executed',
                        'group_id' => 'invented-group-7'
                      },
                      {
                        'id' => 'group-divnra', 'activity_type' => 'DIVNRA',
                        'symbol' => 'ZZTOP', 'net_amount' => '-1.85',
                        'date' => '2026-04-09', 'status' => 'executed',
                        'group_id' => 'invented-group-7'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    assert_equal 'invented-group-7', entries_by_id.fetch('group-div')[:group_id]
    assert_equal 'invented-group-7', entries_by_id.fetch('group-divnra')[:group_id]
  end

  test 'uses date for NTA timestamps and transaction_time for FILL timestamps' do
    stub_activities([
                      {
                        'id' => 'timestamp-div', 'activity_type' => 'DIV',
                        'symbol' => 'ZZTOP', 'net_amount' => '5.25',
                        'date' => '2026-04-10',
                        'transaction_time' => '2026-04-20T08:00:00Z',
                        'status' => 'executed'
                      },
                      {
                        'id' => 'timestamp-fill', 'activity_type' => 'FILL',
                        'symbol' => 'QQTEST', 'side' => 'buy',
                        'qty' => '2.50', 'price' => '7.20',
                        'date' => '2026-04-01',
                        'transaction_time' => '2026-04-11T12:34:56Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    assert_equal Time.utc(2026, 4, 10), entries_by_id.fetch('timestamp-div')[:transacted_at]
    assert_equal Time.utc(2026, 4, 11, 12, 34, 56), entries_by_id.fetch('timestamp-fill')[:transacted_at]
  end

  test 'skips canceled NTAs while keeping executed and correct NTAs' do
    activities = %w[canceled executed correct].map.with_index do |status, index|
      {
        'id' => "status-#{status}", 'activity_type' => 'DIV',
        'symbol' => 'ZZTOP', 'net_amount' => (index + 1).to_s,
        'date' => '2026-04-12', 'status' => status
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal(
      %w[status-executed status-correct],
      result.data.map { |entry| entry[:tx_id] }
    )
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

  test 'returns normalized crypto fee entry from CFEE activity, denominated in the crypto asset' do
    create(:ticker, :eth_usd, exchange: @exchange)

    stub_activities([
                      {
                        'id' => 'cfee-1', 'activity_type' => 'CFEE',
                        'date' => '2026-03-18', 'net_amount' => '0',
                        'symbol' => 'ETHUSD', 'qty' => '-0.000195', 'price' => '1884.5'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fee = result.data.find { |e| e[:entry_type] == :fee }
    assert_not_nil fee, 'CFEE (Alpaca crypto fee) must not be silently dropped'
    assert_equal 'ETH', fee[:base_currency]
    assert_equal 0.000195.to_d, fee[:base_amount]
    assert_equal 'cfee-1', fee[:tx_id]
  end

  test 'CFEE falls back to the raw compact symbol when the crypto asset cannot be resolved locally' do
    stub_activities([
                      {
                        'id' => 'cfee-2', 'activity_type' => 'CFEE',
                        'date' => '2026-03-18', 'net_amount' => '0',
                        'symbol' => 'UNKNOWNUSD', 'qty' => '-0.5', 'price' => '10.0'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fee = result.data.find { |e| e[:entry_type] == :fee }
    assert_not_nil fee
    assert_equal 'UNKNOWNUSD', fee[:base_currency]
    assert_equal 0.5.to_d, fee[:base_amount]
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

  test 'normalizes positive JNLC as a cash deposit' do
    stub_activities([
                      {
                        'id' => 'jnlc-deposit', 'activity_type' => 'JNLC',
                        'net_amount' => '123.45', 'date' => '2026-04-13',
                        'status' => 'executed'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.sole
    assert_equal :deposit, deposit[:entry_type]
    assert_equal 'USD', deposit[:base_currency]
    assert_equal 123.45.to_d, deposit[:base_amount]
    assert_equal 'jnlc-deposit', deposit[:tx_id]
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

  test 'merges a forward SPLIT pair and preserves every source activity id' do
    stub_activities([
                      {
                        'id' => 'split-forward-remove', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZZTOP', 'qty' => '-10', 'net_amount' => '0',
                        'date' => '2026-05-01', 'status' => 'executed'
                      },
                      {
                        'id' => 'split-forward-add', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZZTOP', 'qty' => '30', 'net_amount' => '0',
                        'date' => '2026-05-01', 'status' => 'executed'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    adjustment = result.data.sole
    assert_equal :adjustment, adjustment[:entry_type]
    assert_equal 'ZZTOP', adjustment[:base_currency]
    assert_equal 20.to_d, adjustment[:base_amount]
    assert_equal 'split-forward-remove', adjustment[:tx_id]
    assert_equal 'split-forward-remove', adjustment[:raw_data]['id']
    assert_equal %w[split-forward-remove split-forward-add], adjustment[:raw_data]['merged_activity_ids']
  end

  test 'merges a reverse SPLIT pair into one negative adjustment' do
    stub_activities([
                      {
                        'id' => 'split-reverse-remove', 'activity_type' => 'SPLIT',
                        'symbol' => 'QQTEST', 'qty' => '-30', 'net_amount' => '0',
                        'date' => '2026-05-02', 'status' => 'executed'
                      },
                      {
                        'id' => 'split-reverse-add', 'activity_type' => 'SPLIT',
                        'symbol' => 'QQTEST', 'qty' => '10', 'net_amount' => '0',
                        'date' => '2026-05-02', 'status' => 'executed'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    adjustment = result.data.sole
    assert_equal :adjustment, adjustment[:entry_type]
    assert_equal 'QQTEST', adjustment[:base_currency]
    assert_equal(-20.to_d, adjustment[:base_amount])
  end

  test 'does not merge SPLIT entries across symbols or dates' do
    stub_activities([
                      {
                        'id' => 'split-symbol-one', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZZTOP', 'qty' => '-9', 'net_amount' => '0',
                        'date' => '2026-05-03', 'status' => 'executed'
                      },
                      {
                        'id' => 'split-symbol-two', 'activity_type' => 'SPLIT',
                        'symbol' => 'QQTEST', 'qty' => '27', 'net_amount' => '0',
                        'date' => '2026-05-03', 'status' => 'executed'
                      },
                      {
                        'id' => 'split-date-one', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZDATE', 'qty' => '-12', 'net_amount' => '0',
                        'date' => '2026-05-04', 'status' => 'executed'
                      },
                      {
                        'id' => 'split-date-two', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZDATE', 'qty' => '36', 'net_amount' => '0',
                        'date' => '2026-05-05', 'status' => 'executed'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 4, result.data.size
    assert_equal(
      %w[split-symbol-one split-symbol-two split-date-one split-date-two],
      result.data.map { |entry| entry[:tx_id] }
    )
    assert_equal(
      [-9.to_d, 27.to_d, -12.to_d, 36.to_d],
      result.data.map { |entry| entry[:base_amount] }
    )
  end

  test 'keeps a lone SPLIT as a signed adjustment with its NTA date' do
    stub_activities([
                      {
                        'id' => 'split-lone', 'activity_type' => 'SPLIT',
                        'symbol' => 'ZLONE', 'qty' => '-7.25', 'net_amount' => '0',
                        'date' => '2026-05-06', 'status' => 'correct'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    adjustment = result.data.sole
    assert_equal :adjustment, adjustment[:entry_type]
    assert_equal 'ZLONE', adjustment[:base_currency]
    assert_equal(-7.25.to_d, adjustment[:base_amount])
    assert_equal Time.utc(2026, 5, 6), adjustment[:transacted_at]
  end

  test 'handles forward and reverse SSP split pairs identically to SPLIT pairs' do
    stub_activities([
                      {
                        'id' => 'ssp-forward-remove', 'activity_type' => 'SSP',
                        'symbol' => 'ZSSP', 'qty' => '-8', 'net_amount' => '0',
                        'date' => '2026-05-07', 'status' => 'executed'
                      },
                      {
                        'id' => 'ssp-forward-add', 'activity_type' => 'SSP',
                        'symbol' => 'ZSSP', 'qty' => '24', 'net_amount' => '0',
                        'date' => '2026-05-07', 'status' => 'executed'
                      },
                      {
                        'id' => 'ssp-reverse-remove', 'activity_type' => 'SSP',
                        'symbol' => 'ZSSP', 'qty' => '-24', 'net_amount' => '0',
                        'date' => '2026-05-08', 'status' => 'executed'
                      },
                      {
                        'id' => 'ssp-reverse-add', 'activity_type' => 'SSP',
                        'symbol' => 'ZSSP', 'qty' => '8', 'net_amount' => '0',
                        'date' => '2026-05-08', 'status' => 'executed'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    assert_equal 2, result.data.size
    forward, reverse = result.data
    assert_equal :adjustment, forward[:entry_type]
    assert_equal 'ZSSP', forward[:base_currency]
    assert_equal 16.to_d, forward[:base_amount]
    assert_equal 'ssp-forward-remove', forward[:tx_id]
    assert_equal %w[ssp-forward-remove ssp-forward-add], forward[:raw_data]['merged_activity_ids']
    assert_equal :adjustment, reverse[:entry_type]
    assert_equal 'ZSSP', reverse[:base_currency]
    assert_equal(-16.to_d, reverse[:base_amount])
    assert_equal 'ssp-reverse-remove', reverse[:tx_id]
    assert_equal %w[ssp-reverse-remove ssp-reverse-add], reverse[:raw_data]['merged_activity_ids']
  end

  test 'normalizes cash journal codes by the sign of net_amount' do
    activities = %w[JNLC OCT ACATC].flat_map do |type|
      [
        {
          'id' => "#{type.downcase}-positive", 'activity_type' => type,
          'net_amount' => '21.35', 'date' => '2026-05-08', 'status' => 'executed'
        },
        {
          'id' => "#{type.downcase}-negative", 'activity_type' => type,
          'net_amount' => '-13.45', 'date' => '2026-05-08', 'status' => 'correct'
        }
      ]
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    %w[JNLC OCT ACATC].each do |type|
      deposit = entries_by_id.fetch("#{type.downcase}-positive")
      assert_equal :deposit, deposit[:entry_type]
      assert_equal 'USD', deposit[:base_currency]
      assert_equal 21.35.to_d, deposit[:base_amount]

      withdrawal = entries_by_id.fetch("#{type.downcase}-negative")
      assert_equal :withdrawal, withdrawal[:entry_type]
      assert_equal 'USD', withdrawal[:base_currency]
      assert_equal 13.45.to_d, withdrawal[:base_amount]
    end
  end

  test 'normalizes cash fee and rebate activity codes' do
    expected = {
      'FEE' => [:fee, '-1.25'],
      'DIVFEE' => [:fee, '-2.35'],
      'PTC' => [:fee, '-3.45'],
      'PTR' => [:other_income, '4.55']
    }
    activities = expected.map do |type, (_entry_type, amount)|
      {
        'id' => "cash-#{type.downcase}", 'activity_type' => type,
        'net_amount' => amount, 'date' => '2026-05-09', 'status' => 'executed'
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    expected.each do |type, (entry_type, amount)|
      entry = entries_by_id.fetch("cash-#{type.downcase}")
      assert_equal entry_type, entry[:entry_type]
      assert_equal 'USD', entry[:base_currency]
      assert_equal amount.to_d.abs, entry[:base_amount]
    end
  end

  test 'normalizes every known opaque activity code as unsupported' do
    types = %w[
      JNLS ACATS SSO SPIN MA REORG NC SC CIL FOPT REO
      OPASN OPEXP OPEXC OPXRC OPCA OPTRD OPCSH
    ]
    activities = types.map.with_index do |type, index|
      qty = index.even? ? "-#{index + 1}.25" : "#{index + 1}.25"
      net_amount = index.even? ? "#{index + 2}.35" : "-#{index + 2}.35"
      {
        'id' => "unsupported-#{type.downcase}", 'activity_type' => type,
        'symbol' => 'ZOPAQUE', 'qty' => qty, 'net_amount' => net_amount,
        'date' => '2026-05-10', 'status' => 'executed',
        'group_id' => "opaque-group-#{index}", 'invented_note' => "raw-#{type.downcase}"
      }
    end
    stub_activities(activities)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries_by_id = result.data.index_by { |entry| entry[:tx_id] }
    activities.each do |activity|
      entry = entries_by_id.fetch(activity.fetch('id'))
      assert_equal :unsupported_activity, entry[:entry_type]
      assert_equal 'ZOPAQUE', entry[:base_currency]
      assert_equal activity.fetch('qty').to_d, entry[:base_amount]
      assert_equal activity.fetch('net_amount').to_d, entry[:quote_amount]
      assert_equal activity.fetch('activity_type'), entry[:description]
      assert_equal activity.fetch('group_id'), entry[:group_id]
      assert_equal activity, entry[:raw_data]
    end
  end

  test 'normalizes an invented future activity code as unsupported' do
    activity = {
      'id' => 'unsupported-zqxx', 'activity_type' => 'ZQXX',
      'symbol' => 'ZFUTURE', 'qty' => '-6.75', 'net_amount' => '14.85',
      'date' => '2026-05-11', 'status' => 'executed',
      'group_id' => 'future-group-1', 'invented_note' => 'future raw payload'
    }
    stub_activities([activity])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entry = result.data.sole
    assert_equal :unsupported_activity, entry[:entry_type]
    assert_equal 'ZFUTURE', entry[:base_currency]
    assert_equal(-6.75.to_d, entry[:base_amount])
    assert_equal 14.85.to_d, entry[:quote_amount]
    assert_equal 'ZQXX', entry[:description]
    assert_equal 'future-group-1', entry[:group_id]
    assert_equal activity, entry[:raw_data]
  end

  test 'falls back to USD and zero quantity for unsupported activity without a symbol' do
    activity = {
      'id' => 'unsupported-no-symbol', 'activity_type' => 'REO',
      'net_amount' => '-5.95', 'date' => '2026-05-12',
      'status' => 'correct', 'group_id' => 'no-symbol-group'
    }
    stub_activities([activity])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entry = result.data.sole
    assert_equal :unsupported_activity, entry[:entry_type]
    assert_equal 'USD', entry[:base_currency]
    assert_equal 0.to_d, entry[:base_amount]
    assert_equal(-5.95.to_d, entry[:quote_amount])
    assert_equal 'REO', entry[:description]
    assert_equal 'no-symbol-group', entry[:group_id]
    assert_equal activity, entry[:raw_data]
  end

  test 'resolves slash-pair crypto FILL symbols into base and quote currencies' do
    stub_activities([
                      {
                        'id' => 'fill-slash-pair', 'activity_type' => 'FILL',
                        'symbol' => 'ETH/USD', 'side' => 'buy',
                        'qty' => '1.75', 'price' => '2345.65',
                        'transaction_time' => '2026-05-13T10:20:30Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fill = result.data.sole
    assert_equal 'ETH', fill[:base_currency]
    assert_equal 1.75.to_d, fill[:base_amount]
    assert_equal 'USD', fill[:quote_currency]
    assert_equal 1.75.to_d * 2345.65.to_d, fill[:quote_amount]
  end

  test 'resolves compact crypto FILL symbols through the crypto ticker index' do
    create(:ticker, :eth_usd, exchange: @exchange)
    stub_activities([
                      {
                        'id' => 'fill-compact-pair', 'activity_type' => 'FILL',
                        'symbol' => 'ETHUSD', 'side' => 'sell',
                        'qty' => '2.25', 'price' => '1987.35',
                        'transaction_time' => '2026-05-14T11:21:31Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fill = result.data.sole
    assert_equal 'ETH', fill[:base_currency]
    assert_equal 2.25.to_d, fill[:base_amount]
    assert_equal 'USD', fill[:quote_currency]
    assert_equal 2.25.to_d * 1987.35.to_d, fill[:quote_amount]
  end

  test 'keeps an unresolvable compact FILL symbol verbatim' do
    stub_activities([
                      {
                        'id' => 'fill-unresolved-pair', 'activity_type' => 'FILL',
                        'symbol' => 'ZZCOINUSD', 'side' => 'buy',
                        'qty' => '3.50', 'price' => '17.45',
                        'transaction_time' => '2026-05-15T12:22:32Z'
                      }
                    ])

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    fill = result.data.sole
    assert_equal 'ZZCOINUSD', fill[:base_currency]
    assert_equal 3.50.to_d, fill[:base_amount]
    assert_equal 'USD', fill[:quote_currency]
    assert_equal 3.50.to_d * 17.45.to_d, fill[:quote_amount]
  end
end
