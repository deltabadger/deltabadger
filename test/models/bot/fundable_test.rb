require 'test_helper'

class Bot::FundableTest < ActiveSupport::TestCase
  # quote_amount 100 on a daily interval => 100 / 1.day * 3.days == 300
  BUFFER = 300

  # Cash-only venues (everything except Alpaca) keep comparing settled balance, unchanged.

  test 'funds_are_low? is true when a cash venue is under the buffer' do
    bot = create(:dca_single_asset, :started)
    bot.stubs(:get_balance).returns(Result::Success.new({ free: (BUFFER - 1).to_d, locked: 0 }))

    assert_predicate bot, :funds_are_low?
  end

  test 'funds_are_low? is false when a cash venue clears the buffer' do
    bot = create(:dca_single_asset, :started)
    bot.stubs(:get_balance).returns(Result::Success.new({ free: (BUFFER + 1).to_d, locked: 0 }))

    refute_predicate bot, :funds_are_low?
  end

  # Alpaca margin accounts run settled cash at or below zero by design — buying power is what
  # the venue actually checks when an order lands. Comparing cash false-alarmed every margin user.

  test 'funds_are_low? is false for a stock bot when buying power clears the buffer despite zero cash' do
    bot = alpaca_bot(base_asset: create(:asset, symbol: 'AAPL', category: 'Stock'))
    bot.stubs(:get_balance).returns(Result::Success.new(alpaca_balance(cash: 0, buying_power: 5000)))

    refute_predicate bot, :funds_are_low?
  end

  # Crypto is non-marginable at Alpaca: it spends settled cash, so stock-backed leverage must not
  # count toward the buffer.
  test 'funds_are_low? is true for a crypto bot when only marginable buying power is available' do
    bot = alpaca_bot(base_asset: create(:asset, :bitcoin))
    bot.stubs(:get_balance)
       .returns(Result::Success.new(alpaca_balance(cash: 0, buying_power: 5000, non_marginable: 0)))

    assert_predicate bot, :funds_are_low?
  end

  test 'funds_are_low? is still true when an Alpaca account is genuinely empty' do
    bot = alpaca_bot(base_asset: create(:asset, symbol: 'AAPL', category: 'Stock'))
    bot.stubs(:get_balance).returns(Result::Success.new(alpaca_balance(cash: 0, buying_power: 0)))

    assert_predicate bot, :funds_are_low?
  end

  # A balance hash missing a spend figure must fall back to settled cash, never compare nil.

  test 'funds_are_low? falls back to cash for a crypto bot when non-marginable buying power is absent' do
    bot = alpaca_bot(base_asset: create(:asset, :bitcoin))
    balance = { free: (BUFFER + 1).to_d, locked: 0, buying_power: 5000.to_d }
    bot.stubs(:get_balance).returns(Result::Success.new(balance))

    refute_predicate bot, :funds_are_low?
  end

  test 'funds_are_low? falls back to cash for a stock bot when buying power is absent' do
    bot = alpaca_bot(base_asset: create(:asset, symbol: 'AAPL', category: 'Stock'))
    balance = { free: (BUFFER + 1).to_d, locked: 0, non_marginable_buying_power: 0.to_d }
    bot.stubs(:get_balance).returns(Result::Success.new(balance))

    refute_predicate bot, :funds_are_low?
  end

  test 'funds_are_low? is false when the balance call fails' do
    bot = alpaca_bot(base_asset: create(:asset, :bitcoin))
    bot.stubs(:get_balance).returns(Result::Failure.new('connection error'))

    refute_predicate bot, :funds_are_low?
  end

  private

  def alpaca_bot(base_asset:)
    create(:dca_single_asset, :started, exchange: create(:alpaca_exchange), base_asset: base_asset)
  end

  def alpaca_balance(cash:, buying_power:, non_marginable: nil)
    {
      free: cash.to_d,
      locked: 0,
      buying_power: buying_power.to_d,
      non_marginable_buying_power: non_marginable&.to_d
    }
  end
end
