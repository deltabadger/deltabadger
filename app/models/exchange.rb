class Exchange < ApplicationRecord
  def symbols
    case name.downcase
    when 'binance' then binance_symbols
    when 'bitbay' then bitbay_symbols
    when 'kraken' then kraken_symbols
    else default_symbols
    end
  end

  def non_hodler_symbols
    filter_non_hodler_symbols(symbols)
  end

  private

  def binance_symbols
    market = ExchangeApi::Markets::Binance::Market.new
    market.all_symbols
  end

  def bitbay_symbols
    market = ExchangeApi::Markets::Bitbay::Market.new
    all_symbols = market.all_symbols
    return all_symbols unless all_symbols.success?

    all_symbols.data
  end

  def kraken_symbols
    market = ExchangeApi::Markets::Kraken::Market.new
    market.all_symbols
  end

  def default_symbols
    { base: 'BTC', quote: 'USD' }
  end

  def filter_non_hodler_symbols(symbols)
    is_kraken = name.downcase == 'kraken'
    btc_eth = is_kraken ? ['XBT', 'ETH'] : ['BTC', 'ETH']
    symbols.select { |s| btc_eth.include?(s.base) }
  end
end
