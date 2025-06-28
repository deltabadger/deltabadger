module Bots::Searchable
  extend ActiveSupport::Concern

  private

  def asset_search_results(bot, query, asset_type)
    available_assets = bot.available_assets_for_current_settings(asset_type: asset_type)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: query)
                      .pluck(:id, :symbol, :name)
                      .reject { |_, symbol, _| symbol.blank? }
    exchanges_data = Exchange.available_for_new_bots.each_with_object([]) do |exchange, list|
      assets = exchange.exchange_assets.available.pluck(:asset_id)
      list << [exchange.name_id, exchange.name, assets] if assets.any?
    end
    filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, parse_exchanges(exchanges)]
    end
  end

  def parse_exchanges(exchanges)
    # exchanges sorted by name_id
    # exchanges.map { |name_id, name, _| [name_id, name] }.sort

    # flattening binance and binance_us to binance
    binance_name_id = 'binance'
    binance_us_name_id = 'binance_us'
    binance_name = Exchange.find_by(name_id: binance_name_id).name

    exchanges.map { |name_id, name, _| [name_id, name] }
             .map { |name_id, name| name_id == binance_us_name_id ? [binance_name_id, binance_name] : [name_id, name] }
             .uniq.sort
  end

  def exchange_search_results(bot, query)
    exchanges = bot.available_exchanges_for_current_settings
    filter_exchanges_by_query(exchanges: exchanges, query: query)
  end

  def filter_assets_by_query(assets:, query:)
    return assets.order(:market_cap_rank, :symbol) if query.blank?

    assets
      .map { |asset| [asset, similarities_for_asset(asset, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |asset, similarities| [similarities.map(&:-@), asset.market_cap_rank || Float::INFINITY] }
      .map(&:first)
  end

  def similarities_for_asset(asset, query)
    [
      asset.symbol.present? ? JaroWinkler.similarity(asset.symbol.downcase.to_s, query) : 0,
      asset.name.present? ? JaroWinkler.similarity(asset.name.downcase.to_s, query) : 0
    ].sort.reverse
  end

  def filter_exchanges_by_query(exchanges:, query:)
    return exchanges.order(:name) if query.blank?

    exchanges
      .map { |exchange| [exchange, similarities_for_exchange(exchange, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |_, similarities| similarities.map(&:-@) }
      .map(&:first)
  end

  def similarities_for_exchange(exchange, query)
    [
      exchange.name.present? ? JaroWinkler.similarity(exchange.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
