module Bots::Searchable
  extend ActiveSupport::Concern

  ASSET_PAGE_SIZE = 20

  private

  def asset_search_results(bot, query, asset_type)
    offset = params[:offset].to_i
    all_assets = filtered_asset_rows(bot, query, asset_type)
    page = all_assets[offset, ASSET_PAGE_SIZE] || []

    next_offset = offset + ASSET_PAGE_SIZE
    if all_assets.size > next_offset
      @next_page_frame_id = "assets-page-#{next_offset}"
      @next_page_url = "#{request.path}?#{request.query_parameters.merge('offset' => next_offset).to_query}"
    end
    @asset_page_offset = offset

    # Exchanges are display-only, so resolve them for the page rows only — never for the full
    # (now ~10k-asset) result set. See attach_exchanges.
    attach_exchanges(page)
  end

  def render_asset_page(bot:, asset_field:)
    return false unless @asset_page_offset.positive?

    render partial: 'bots/asset_page', locals: {
      page_frame_id: "assets-page-#{@asset_page_offset}",
      assets: @assets,
      bot: bot,
      asset_field: asset_field,
      next_page_url: @next_page_url,
      next_page_frame_id: @next_page_frame_id
    }
    true
  end

  # Ranked rows for the FULL result set, without exchange data. Sorting/search use only asset
  # fields, so we don't pay any cross-exchange cost here — exchanges are attached per page.
  def filtered_asset_rows(bot, query, asset_type)
    available_assets = bot.available_assets_for_current_settings(asset_type: asset_type)
    filter_assets_by_query(assets: available_assets, query: query)
      .pluck(:id, :symbol, :name, :color, :category)
      .reject { |_, symbol, _| symbol.blank? }
  end

  # Resolve the exchanges each row trades on, for the (≤ ASSET_PAGE_SIZE) page rows ONLY.
  # Same source as before — exchange_assets.available + Exchange.available — so output is
  # identical, but membership is a scoped `WHERE asset_id IN (…page ids)` instead of an
  # O(assets × Σ exchange_assets) Array#include? over every asset. binance_name is derived
  # independently of the page (Binance available + has assets), so the binance_us → binance
  # collapse still applies on pages that contain no Binance asset.
  def attach_exchanges(rows)
    return rows if rows.empty?

    ids = rows.map(&:first)
    available_exchanges = Exchange.available.index_by(&:id)
    exchanges_by_asset = Hash.new { |hash, key| hash[key] = [] }
    ExchangeAsset.available
                 .where(asset_id: ids, exchange_id: available_exchanges.keys)
                 .pluck(:asset_id, :exchange_id)
                 .each do |asset_id, exchange_id|
      exchange = available_exchanges[exchange_id]
      exchanges_by_asset[asset_id] << [exchange.name_id, exchange.name] if exchange
    end

    binance = available_exchanges.values.find { |exchange| exchange.name_id == 'binance' }
    binance_name = binance.name if binance && ExchangeAsset.available.where(exchange_id: binance.id).exists?

    rows.map do |id, symbol, name, color, category|
      [id, symbol, name, color, category, parse_exchanges(exchanges_by_asset[id], binance_name)]
    end
  end

  def parse_exchanges(exchanges, binance_name)
    exchanges.map { |name_id, name, _| [name_id, name] }
             .map { |name_id, name| name_id == 'binance_us' && binance_name ? ['binance', binance_name] : [name_id, name] }
             .uniq.sort
  end

  def exchange_search_results(bot, query)
    exchanges = bot.available_exchanges_for_current_settings
    filter_exchanges_by_query(exchanges: exchanges, query: query)
  end

  def filter_assets_by_query(assets:, query:)
    # Sort by market cap rank (lower = higher market cap), assets without market cap go last
    return assets.order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank, :symbol) if query.blank?

    q = query.downcase

    # Phase 1: exact symbol match (BTC → BTC)
    exact = assets.where('LOWER(symbol) = ?', q)

    # Phase 2: symbol starts with query (BT → BTC, BTT, …)
    prefix = assets.where('LOWER(symbol) LIKE ?', "#{q}%").where.not(id: exact.select(:id))

    # Phase 3: name contains query (bitcoin → Bitcoin)
    name_match = assets.where('LOWER(name) LIKE ?', "%#{q}%")
                       .where.not(id: exact.select(:id))
                       .where.not(id: prefix.select(:id))

    # Phase 4: fuzzy match via JaroWinkler on remaining assets (capped to avoid loading all 18k)
    excluded_ids = exact.pluck(:id) + prefix.pluck(:id) + name_match.pluck(:id)
    fuzzy_candidates = excluded_ids.any? ? assets.where.not(id: excluded_ids) : assets
    fuzzy = fuzzy_candidates.limit(2000)
                            .select { |asset| similarities_for_asset(asset, q).first >= 0.85 }

    # Each phase sorted by market cap rank
    rank_sort = ->(a) { a.market_cap_rank || Float::INFINITY }

    exact.order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank).to_a +
      prefix.order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank).to_a +
      name_match.order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank).to_a +
      fuzzy.sort_by(&rank_sort)
  end

  def similarities_for_asset(asset, query)
    [
      asset.symbol.present? ? JaroWinkler.similarity(asset.symbol.downcase.to_s, query) : 0,
      asset.name.present? ? JaroWinkler.similarity(asset.name.downcase.to_s, query) : 0
    ].sort.reverse
  end

  def filter_exchanges_by_query(exchanges:, query:)
    return exchanges.order(Arel.sql("type IN (#{Exchange::STABLE_TYPES.map { |t| "'#{t}'" }.join(',')}) DESC"), :name) if query.blank?

    exchanges
      .map { |exchange| [exchange, similarities_for_exchange(exchange, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |exchange, similarities| [exchange.beta? ? 1 : 0, similarities.map(&:-@)] }
      .map(&:first)
  end

  def similarities_for_exchange(exchange, query)
    [
      exchange.name.present? ? JaroWinkler.similarity(exchange.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
