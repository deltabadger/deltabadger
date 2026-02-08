# 1. Create exchanges (always needed)
[
  { type: 'Exchanges::Binance', name: 'Binance', maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0002' },
  { type: 'Exchanges::BinanceUs', name: 'Binance.US', maker_fee: '0.0', taker_fee: '0.01', withdrawal_fee: '0.0002' },
  { type: 'Exchanges::Kraken', name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4', withdrawal_fee: '0.00005' },
  { type: 'Exchanges::Coinbase', name: 'Coinbase', maker_fee: '0.6', taker_fee: '1.2', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Bitget', name: 'Bitget', maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Kucoin', name: 'KuCoin', maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Bybit', name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Mexc', name: 'MEXC', maker_fee: '0.0', taker_fee: '0.05', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Gemini', name: 'Gemini', maker_fee: '0.2', taker_fee: '0.4', withdrawal_fee: '0.0' },
  { type: 'Exchanges::Bitvavo', name: 'Bitvavo', maker_fee: '0.15', taker_fee: '0.25', withdrawal_fee: '0.0' }
].each do |attrs|
  klass = attrs[:type].constantize
  klass.find_or_create_by!(name: attrs[:name]).update!(attrs.except(:type, :name))
end

# 2. Import seed data from JSON files
seed_dir = Rails.root.join('db/seed_data')
if seed_dir.exist?
  # Assets
  assets_file = seed_dir.join('assets.json')
  if assets_file.exist?
    data = JSON.parse(assets_file.read)
    MarketData.import_assets!(data['data'])
  end

  # Indices
  indices_file = seed_dir.join('indices.json')
  if indices_file.exist?
    data = JSON.parse(indices_file.read)
    MarketData.import_indices!(data['data'])
  end

  # Tickers per exchange
  tickers_dir = seed_dir.join('tickers')
  if tickers_dir.exist?
    Dir.glob(tickers_dir.join('*.json')).each do |file|
      data = JSON.parse(File.read(file))
      exchange_name_id = File.basename(file, '.json')
      exchange = Exchange.available.find { |e| e.name_id == exchange_name_id }
      MarketData.import_tickers!(exchange, data['data']) if exchange
    end
  end
end

# 3. Configure defaults
AppConfig.smtp_provider = 'env_smtp' if AppConfig.smtp_env_available? && AppConfig.smtp_provider.blank?

if AppConfig.market_data_env_available? && AppConfig.market_data_provider.blank?
  AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
end
