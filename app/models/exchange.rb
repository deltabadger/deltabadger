class Exchange < ApplicationRecord
  def symbols
    case name.downcase
    when 'binance' then binance_symbols
    when 'bitbay' then bitbay_symbols
    when 'kraken' then kraken_symbols
    else default_symbols
    end
  end

  private

  def binance_symbols
    market = ExchangeApi::Markets::Binance::Market.new
    market.all_symbols
  end

  def bitbay_symbols
    market = ExchangeApi::Markets::Bitbay::Market.new
    market.all_symbols
  end

  def kraken_symbols
    market = ExchangeApi::Markets::Kraken::Market.new
    market.all_symbols
  end

  def default_symbols
    %w[BTCUSD]
  end
end
