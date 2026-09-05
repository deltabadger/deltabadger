require 'test_helper'

# Index availability answers "can this index run on this venue". It has to answer the same
# question the quote picker then asks — enough TRADABLE index coins against ONE quote — or the
# exchange step offers a venue whose quote list comes back empty.
class Index::ExchangeAvailabilityTest < ActiveSupport::TestCase
  setup do
    @binance = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @eur = create(:asset, symbol: 'EUR', name: 'Euro', external_id: 'euro')
    @coins = %w[bitcoin ethereum solana].map do |id|
      create(:asset, symbol: id[0..2].upcase, name: id, external_id: id)
    end
  end

  def list(asset, quote, trading_enabled: true)
    create(:ticker, exchange: @binance, base_asset: asset, quote_asset: quote,
                    trading_enabled: trading_enabled)
  end

  def availability(coins = @coins.map(&:external_id))
    Index.calculate_available_exchanges(top_coins: coins)
  end

  test 'a venue whose index coins are all disabled is not available' do
    @coins.each { |c| list(c, @usd, trading_enabled: false) }

    assert_empty availability
  end

  test 'a venue whose index coins are split across quotes is not available' do
    list(@coins[0], @usd)
    list(@coins[1], @eur)
    list(@coins[2], @eur)

    assert_empty availability, 'two on EUR and one on USD is not a home for a three-coin index'
  end

  test 'a venue with enough coins on one quote is available' do
    @coins.each { |c| list(c, @usd) }

    assert_equal 3, availability['Exchanges::Binance']
  end

  test 'a venue with no matching tickers is absent rather than raising' do
    assert_nothing_raised { availability }
    assert_empty availability
  end
end
