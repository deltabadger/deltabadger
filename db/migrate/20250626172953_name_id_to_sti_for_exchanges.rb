class NameIdToStiForExchanges < ActiveRecord::Migration[6.0]
  def change
    remove_column :exchanges, :name_id, :string
    remove_column :exchanges, :url, :string
    remove_column :exchanges, :color, :string
    add_column :exchanges, :type, :string
    add_column :exchanges, :available, :boolean, default: true
    add_index :exchanges, :type, unique: true

    Exchange.find_each do |exchange|
      case exchange.name.downcase
      when 'binance' then exchange.update!(type: 'Exchanges::Binance')
      when 'binance.us' then exchange.update!(type: 'Exchanges::BinanceUs')
      when 'zonda' then exchange.update!(type: 'Exchanges::Zonda')
      when 'kraken' then exchange.update!(type: 'Exchanges::Kraken')
      when 'coinbase pro' then exchange.update!(type: 'Exchanges::CoinbasePro')
      when 'coinbase' then exchange.update!(type: 'Exchanges::Coinbase')
      when 'gemini' then exchange.update!(type: 'Exchanges::Gemini')
      when 'ftx' then exchange.update!(type: 'Exchanges::Ftx')
      when 'ftx.us' then exchange.update!(type: 'Exchanges::FtxUs')
      when 'bitso' then exchange.update!(type: 'Exchanges::Bitso')
      when 'kucoin' then exchange.update!(type: 'Exchanges::Kucoin')
      when 'bitfinex' then exchange.update!(type: 'Exchanges::Bitfinex')
      when 'bitstamp' then exchange.update!(type: 'Exchanges::Bitstamp')
      when 'probit global' then exchange.update!(type: 'Exchanges::ProbitGlobal')
      end
    end
  end
end
