namespace :seed do
  desc 'Generate seed data JSON files from CoinGecko + exchange APIs (requires COINGECKO_API_KEY)'
  task generate: :environment do
    require 'dotenv/load' if defined?(Dotenv)

    unless AppConfig.coingecko_api_key.present?
      puts 'ERROR: CoinGecko API key required. Set COINGECKO_API_KEY in your environment.'
      exit 1
    end

    temp_db_path = Rails.root.join('tmp/seed_generation.sqlite3')
    FileUtils.rm_f(temp_db_path)

    seed_dir = Rails.root.join('db/seed_data')
    cached_colors = load_cached_colors(seed_dir.join('assets.json'))

    puts '=' * 80
    puts 'Generating seed data: db/seed_data/'
    puts '=' * 80
    puts ''

    # 1. Connect to a fresh temporary database
    puts '[1/7] Creating fresh database and loading schema...'
    ActiveRecord::Base.connection_handler.clear_all_connections!
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: temp_db_path.to_s)
    ActiveRecord::Base.connection
    ActiveRecord::Schema.verbose = false
    load(Rails.root.join('db/schema.rb'))
    puts '       Done.'

    # 2. Create exchange records and configure CoinGecko
    puts ''
    puts '[2/7] Creating exchanges and configuring CoinGecko provider...'
    create_exchanges
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    puts "       Created #{Exchange.count} exchanges."

    # 3. Sync tickers and assets for each exchange
    exchanges = Exchange.available.to_a
    puts ''
    puts "[3/7] Syncing tickers and assets for #{exchanges.size} exchanges..."
    puts '       Each exchange requires 2 API calls (CoinGecko + exchange API).'
    puts '       Rate limit: 65s between exchanges.'
    puts ''

    dot = lambda do
      print '.'
      $stdout.flush
    end

    start_from = ENV['START_FROM']
    exchanges.each_with_index do |exchange, idx|
      if start_from.present? && !exchange.name.downcase.include?(start_from.downcase)
        puts "       [#{idx + 1}/#{exchanges.size}] #{exchange.name} (skipped)"
        next
      end
      start_from = nil # found it, sync all remaining

      print "       [#{idx + 1}/#{exchanges.size}] #{exchange.name} "
      $stdout.flush

      result = exchange.sync_tickers_and_assets_with_external_data(skip_async_jobs: true, on_progress: dot)

      if result.failure?
        puts "\n       FAILED (#{result.errors.first.to_s.truncate(60)})"
      else
        ticker_count = exchange.tickers.available.count
        asset_count = exchange.exchange_assets.count
        puts "\n       #{ticker_count} tickers, #{asset_count} assets"
      end

      wait_with_countdown(65, prefix: '       ') if idx < exchanges.size - 1
    end

    # 4. Fetch asset metadata + colors
    crypto_count = Asset.where(category: 'Cryptocurrency').count
    puts ''
    puts "[4/7] Fetching metadata and colors for #{crypto_count} crypto assets..."
    wait_with_countdown(65, prefix: '       ')

    coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)
    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact

    if asset_ids.any?
      print '       Fetching from CoinGecko... '
      $stdout.flush
      result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
      if result.failure?
        puts "FAILED: #{result.errors.to_sentence}"
      else
        puts 'OK'
        synced = 0
        Asset.where(category: 'Cryptocurrency').find_each do |asset|
          prefetched = result.data.find { |coin| coin['id'] == asset.external_id }
          asset.sync_data_with_coingecko(prefetched_data: prefetched)
          synced += 1
        end
        puts "       Updated metadata for #{synced} assets."

        # Apply cached colors from previous seed data
        if cached_colors.any?
          applied = 0
          Asset.where(category: 'Cryptocurrency', color: nil)
               .where(external_id: cached_colors.keys).find_each do |asset|
            asset.update_column(:color, cached_colors[asset.external_id])
            applied += 1
          end
          puts "       Restored #{applied} colors from previous seed data." if applied.positive?
        end

        # Apply color overrides (takes precedence over cached and inferred colors)
        override_applied = 0
        Asset::COLOR_OVERRIDES.each do |external_id, color|
          asset = Asset.find_by(external_id: external_id)
          next unless asset

          asset.update_column(:color, color)
          override_applied += 1
        end
        puts "       Applied #{override_applied} color overrides." if override_applied.positive?

        # Infer colors only for truly new assets
        colorless = Asset.where(category: 'Cryptocurrency', color: nil).where.not(image_url: nil)
        colorless_count = colorless.count
        if colorless_count.positive?
          colored = 0
          processed = 0
          colorless.find_each do |asset|
            asset.infer_color_from_image
            colored += 1 if asset.color.present?
            processed += 1
            pct = (processed * 100.0 / colorless_count).round(1)
            print "\r       Extracting colors... #{processed}/#{colorless_count} (#{pct}%)  "
            $stdout.flush
          end
          puts "\r       Extracted #{colored} colors from #{colorless_count} new assets.     "
        else
          puts '       All assets already have colors.'
        end
      end
    end

    # 5. Sync indices with top_coins_by_exchange
    puts ''
    puts '[5/7] Syncing indices from CoinGecko...'
    wait_with_countdown(65, prefix: '       ')
    sync_indices(coingecko)

    # 6. Export to JSON files
    puts ''
    puts '[6/7] Exporting to JSON...'
    FileUtils.mkdir_p(seed_dir.join('tickers'))

    # Export assets
    assets_data = Asset.order(:market_cap_rank).map { |a| export_asset(a) }
    assets_json = { metadata: { count: assets_data.size, generated_at: Time.current.iso8601 }, data: assets_data }
    File.write(seed_dir.join('assets.json'), JSON.pretty_generate(assets_json))
    puts "       assets.json: #{assets_data.size} assets"

    # Export indices
    indices_data = Index.order(weight: :desc, market_cap: :desc).map { |i| export_index(i) }
    indices_json = { metadata: { count: indices_data.size, generated_at: Time.current.iso8601 }, data: indices_data }
    File.write(seed_dir.join('indices.json'), JSON.pretty_generate(indices_json))
    puts "       indices.json: #{indices_data.size} indices"

    # Export tickers per exchange
    Exchange.available.each do |exchange|
      name_id = exchange.name_id
      tickers = exchange.tickers.available.includes(:base_asset, :quote_asset)
      tickers_data = tickers.map { |t| export_ticker(t) }
      tickers_json = {
        metadata: { exchange_name_id: name_id, count: tickers_data.size, generated_at: Time.current.iso8601 },
        data: tickers_data
      }
      File.write(seed_dir.join("tickers/#{name_id}.json"), JSON.pretty_generate(tickers_json))
      puts "       tickers/#{name_id}.json: #{tickers_data.size} tickers"
    end

    # 7. Print summary
    puts ''
    puts '[7/7] Done!'
    puts ''
    puts '=' * 80
    puts 'Summary:'
    crypto = Asset.where(category: 'Cryptocurrency').count
    fiat = Asset.where(category: 'Currency').count
    puts "  Assets:          #{Asset.count} (#{crypto} crypto, #{fiat} fiat)"
    puts "  Exchanges:       #{Exchange.count}"
    puts "  Tickers:         #{Ticker.count}"
    puts "  Exchange Assets: #{ExchangeAsset.count}"
    puts "  Indices:         #{Index.count}"
    puts ''
    colorless_assets = Asset.where(category: 'Cryptocurrency', color: nil).pluck(:external_id)
    colorless_assets.reject! { |id| Asset::COLOR_OVERRIDES.key?(id) }
    if colorless_assets.any?
      puts "  Missing colors (#{colorless_assets.size}):"
      colorless_assets.each { |id| puts "    #{id}" }
    end
    puts ''
    puts '  Output: db/seed_data/'
    puts '=' * 80

    # Clean up temp database and reconnect
    FileUtils.rm_f(temp_db_path)
    ActiveRecord::Base.connection_handler.clear_all_connections!
    ActiveRecord::Base.establish_connection(Rails.env.to_sym)
  end
end

def wait_with_countdown(seconds, prefix: '')
  seconds.downto(1) do |remaining|
    print "\r#{prefix}Waiting #{remaining}s for rate limit...  "
    $stdout.flush
    sleep(1)
  end
  print "\r#{prefix}#{' ' * 40}\r"
  $stdout.flush
end

def create_exchanges
  [
    { type: 'Exchanges::Binance', name: 'Binance', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::BinanceUs', name: 'Binance.US', maker_fee: '0.0', taker_fee: '0.01' },
    { type: 'Exchanges::Kraken', name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4' },
    { type: 'Exchanges::Coinbase', name: 'Coinbase', maker_fee: '0.6', taker_fee: '1.2' },
    { type: 'Exchanges::Bitget', name: 'Bitget', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::Kucoin', name: 'KuCoin', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::Bybit', name: 'Bybit', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::Mexc', name: 'MEXC', maker_fee: '0.0', taker_fee: '0.05' },
    { type: 'Exchanges::Gemini', name: 'Gemini', maker_fee: '0.2', taker_fee: '0.4' },
    { type: 'Exchanges::Bitvavo', name: 'Bitvavo', maker_fee: '0.15', taker_fee: '0.25' },
    { type: 'Exchanges::Hyperliquid', name: 'Hyperliquid', maker_fee: '0.01', taker_fee: '0.035' },
    { type: 'Exchanges::Bingx', name: 'BingX', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::Bitrue', name: 'Bitrue', maker_fee: '0.1', taker_fee: '0.1' },
    { type: 'Exchanges::BitMart', name: 'BitMart', maker_fee: '0.1', taker_fee: '0.1' }
  ].each do |attrs|
    klass = attrs[:type].constantize
    exchange = klass.find_or_create_by!(name: attrs[:name])
    exchange.update!(attrs.except(:type, :name))
  end
end

def sync_indices(coingecko)
  available_asset_ids = Asset.where.not(external_id: nil).pluck(:external_id).to_set

  # Sync Top Coins index first
  print '       Top Coins... '
  $stdout.flush
  sync_top_coins_index(coingecko, available_asset_ids)

  # Sync category indices
  print '       Fetching categories... '
  $stdout.flush
  result = coingecko.get_categories_with_market_data
  if result.failure?
    puts 'FAILED'
    return
  end

  categories = result.data
  puts "#{categories.size} categories"

  synced = 0
  skipped = 0

  categories.each_with_index do |category, idx|
    next if category['id'].blank?
    next if Index::EXCLUDED_CATEGORY_IDS.include?(category['id'])
    next if category['content'].blank?

    print "       [#{idx + 1}/#{categories.size}] #{category['name']&.truncate(40)}... "
    $stdout.flush

    coins_result = coingecko.get_coins_list_with_market_data(category: category['id'], limit: 250)
    sleep(3)

    if coins_result.failure?
      puts 'SKIP (API error)'
      skipped += 1
      next
    end

    all_coins = coins_result.data.map { |coin| coin['id'] }
    valid_coins = all_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

    if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS
      puts "SKIP (#{valid_coins.size} coins in DB)"
      skipped += 1
      next
    end

    top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
    available_exchanges = Index.calculate_available_exchanges(top_coins: valid_coins)

    if available_exchanges.empty?
      puts 'SKIP (no exchanges)'
      skipped += 1
      next
    end

    top_coins_by_exchange = calculate_top_coins_by_exchange(valid_coins)

    index = Index.find_or_initialize_by(
      external_id: category['id'],
      source: Index::SOURCE_COINGECKO
    )

    name = category['name']&.gsub(/\s*\([^)]+\)/, '')&.gsub(/\s+/, ' ')&.strip

    index.update!(
      name: name,
      description: category['content'],
      top_coins: top_coins_for_display,
      top_coins_by_exchange: top_coins_by_exchange,
      market_cap: category['market_cap'],
      available_exchanges: available_exchanges,
      weight: Index::WEIGHTED_CATEGORIES[category['id']] || 0
    )

    synced += 1
    exchanges_summary = available_exchanges.map { |k, v| "#{k.demodulize[0..2]}:#{v}" }.join(' ')
    puts "OK (#{exchanges_summary})"
  end

  puts "       Synced #{synced} indices (#{skipped} skipped)"
end

def sync_top_coins_index(coingecko, available_asset_ids)
  coins_result = coingecko.get_coins_list_with_market_data(limit: 250)
  sleep(3)

  if coins_result.failure?
    puts 'FAILED (API error)'
    return
  end

  all_coins = coins_result.data.map { |coin| coin['id'] }
  valid_coins = all_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

  if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS
    puts "SKIP (#{valid_coins.size} coins)"
    return
  end

  top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
  available_exchanges = Index.calculate_available_exchanges(top_coins: valid_coins)
  top_coins_by_exchange = calculate_top_coins_by_exchange(valid_coins)

  Index.find_or_initialize_by(
    external_id: Index::TOP_COINS_EXTERNAL_ID,
    source: Index::SOURCE_INTERNAL
  ).update!(
    name: I18n.t('bot.dca_index.setup.pick_index.top_coins', locale: :en),
    description: I18n.t('bot.dca_index.setup.pick_index.top_coins_description', locale: :en),
    top_coins: top_coins_for_display,
    top_coins_by_exchange: top_coins_by_exchange,
    market_cap: nil,
    available_exchanges: available_exchanges,
    weight: 0
  )

  puts "OK (#{available_exchanges.size} exchanges)"
end

def calculate_top_coins_by_exchange(valid_coins)
  result = {}

  Exchange.available.each do |exchange|
    exchange_coin_ids = exchange.tickers.available
                                .joins(:base_asset)
                                .where(assets: { external_id: valid_coins })
                                .pluck('assets.external_id')
                                .uniq

    # Preserve market cap order from valid_coins
    ordered = valid_coins.select { |coin_id| exchange_coin_ids.include?(coin_id) }
    result[exchange.type] = ordered.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
  end

  result
end

def load_cached_colors(assets_json_path)
  return {} unless File.exist?(assets_json_path)

  data = JSON.parse(File.read(assets_json_path))
  (data['data'] || []).each_with_object({}) do |asset, hash|
    hash[asset['external_id']] = asset['color'] if asset['color'].present?
  end
rescue StandardError => e
  puts "       Warning: could not read colors from previous seed data: #{e.message}"
  {}
end

# Export helpers — produce JSON matching the format consumed by MarketData.import_*

def export_asset(asset)
  {
    'external_id' => asset.external_id,
    'symbol' => asset.symbol,
    'name' => asset.name,
    'category' => asset.category,
    'image_url' => asset.image_url,
    'color' => asset.color,
    'market_cap_rank' => asset.market_cap_rank,
    'market_cap' => asset.market_cap,
    'circulating_supply' => asset.circulating_supply&.to_s,
    'url' => asset.url
  }
end

def export_index(index)
  {
    'external_id' => index.external_id,
    'source' => index.source,
    'name' => index.name,
    'description' => index.description,
    'top_coins' => index.top_coins,
    'top_coins_by_exchange' => index.top_coins_by_exchange,
    'market_cap' => index.market_cap,
    'available_exchanges' => index.available_exchanges,
    'weight' => index.weight
  }
end

def export_ticker(ticker)
  {
    'ticker' => ticker.ticker,
    'base' => ticker.base,
    'quote' => ticker.quote,
    'base_external_id' => ticker.base_asset.external_id,
    'quote_external_id' => ticker.quote_asset.external_id,
    'minimum_base_size' => ticker.minimum_base_size.to_s,
    'minimum_quote_size' => ticker.minimum_quote_size.to_s,
    'maximum_base_size' => ticker.maximum_base_size&.to_s,
    'maximum_quote_size' => ticker.maximum_quote_size&.to_s,
    'base_decimals' => ticker.base_decimals,
    'quote_decimals' => ticker.quote_decimals,
    'price_decimals' => ticker.price_decimals
  }
end
