require 'test_helper'

# The price a row gets is only as right as the coin its symbol is taken to mean.
class Tax::PriceServiceIdentityTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
  end

  def row(symbol, at)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :other_income, base_currency: symbol,
                                 base_amount: 1, quote_currency: nil, quote_amount: nil, transacted_at: at)
  end

  def enrich
    Tax::PriceService.new.enrich(AccountTransaction.for_user(@user).order(:transacted_at).to_a, currency: 'USD')
  end

  test 'a symbol that changed coin is fetched as each coin over its own days' do
    create(:asset, symbol: 'LUNA', name: 'Terra', external_id: 'terra-luna-2', category: 'Cryptocurrency')
    row('LUNA', Time.utc(2021, 9, 10, 12))
    row('LUNA', Time.utc(2025, 6, 22, 12))
    prices = { 'terra-luna' => [[Time.utc(2021, 9, 10).to_i * 1000, 40]],
               'terra-luna-2' => [[Time.utc(2025, 6, 22).to_i * 1000, 0.2]] }
    fetched = []
    MarketData.stubs(:get_historical_price_range).with do |args|
      fetched << args[:coin_id]
      true
    end.returns(Result::Success.new('prices' => prices['terra-luna']))
    MarketData.stubs(:get_historical_price_range).with { |args| args[:coin_id] == 'terra-luna-2' }
              .returns(Result::Success.new('prices' => prices['terra-luna-2']))

    rows = enrich

    assert_equal %w[terra-luna terra-luna-2], fetched.sort, 'the 2021 row is Terra Classic, the 2025 row the relaunched chain'
    assert_equal([40.to_d, 0.2.to_d], rows.map { |r| r[:fiat_value] })
  end

  test 'a coin is never priced off a stock that shares its ticker' do
    create(:asset, symbol: 'XYZ', name: 'XYZ Inc', external_id: 'alpaca_xyz', category: 'Stock')
    row('XYZ', Time.utc(2024, 1, 5, 12))
    MarketData.expects(:get_historical_price_range).never

    rows = enrich

    assert rows.sole[:price_missing], 'unpriced, honestly — not priced as a share of XYZ Inc'
  end
end
