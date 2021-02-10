class Exchange < ApplicationRecord
  include ExchangeApi::BinanceEnum
  def symbols
    market = case name.downcase
             when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
             when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
             when 'bitbay' then ExchangeApi::Markets::Bitbay::Market.new
             when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
             when 'coinbase pro' then ExchangeApi::Markets::CoinbasePro::Market.new
             when 'gemini' then ExchangeApi::Markets::Gemini::Market.new
             when 'ftx' then ExchangeApi::Markets::Ftx::Market.new
             else
               Result::Failure.new("Unsupported exchange #{name}")
             end
    all_symbols = market.all_symbols
    return all_symbols unless all_symbols.success?

    Result::Success.new(all_symbols.data)
  end

  def non_hodler_symbols
    all_symbols = symbols
    return all_symbols unless all_symbols.success?

    Result::Success.new(filter_non_hodler_symbols(all_symbols.data))
  end

  private

  def filter_non_hodler_symbols(symbols)
    is_kraken = name.downcase == 'kraken'
    btc_eth = is_kraken ? %w[XBT ETH LTC XMR] : %w[BTC ETH LTC XMR]
    symbols.select { |s| btc_eth.include?(s.base) }
  end
end
