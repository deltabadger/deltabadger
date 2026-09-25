class Index < ApplicationRecord
  include Index::ExchangeAvailability

  SOURCE_COINGECKO = 'coingecko'.freeze
  SOURCE_DELTABADGER = 'deltabadger'.freeze
  SOURCE_INTERNAL = 'internal'.freeze
  TOP_COINS_EXTERNAL_ID = 'top-coins'.freeze

  # Categories to exclude from fixture generation and sync
  # These are CoinGecko category IDs (e.g., stablecoin-only indices)
  EXCLUDED_CATEGORY_IDS = %w[
    stablecoins
    fiat-backed-stablecoin
    eur-stablecoin
    usd-stablecoin
  ].freeze

  # Weighted categories shown first in picker (higher weight = shown earlier)
  # Maps CoinGecko category ID to weight (1-12, higher = more prominent)
  WEIGHTED_CATEGORIES = {
    'layer-1' => 12,
    'layer-2' => 11,
    'meme-token' => 10,
    'privacy-coins' => 9,
    'yield-farming' => 8,
    'runes' => 7,
    'decentralized-finance-defi' => 6,
    'artificial-intelligence' => 5,
    'gaming' => 4,
    'real-world-assets-rwa' => 3,
    'ai-agents' => 2,
    'zero-knowledge-zk' => 1
  }.freeze

  # The product's name for a count-named index comes from the container's own map, never from the
  # feed — see Bots::DcaIndex::COUNT_NAMED_INDICES. Everything else is called what its source calls it.
  def display_name
    Bots::DcaIndex::COUNT_NAMED_INDICES.dig(external_id, :name) || name
  end

  validates :external_id, presence: true
  validates :source, presence: true
  validates :name, presence: true

  scope :coingecko, -> { where(source: SOURCE_COINGECKO) }
  scope :deltabadger, -> { where(source: SOURCE_DELTABADGER) }
  scope :with_description, -> { where.not(description: [nil, '']) }

  # Filter to indices available on a specific exchange
  scope :available_on_exchange, lambda { |exchange|
    exchange_type = exchange.is_a?(Exchange) ? exchange.type : exchange.to_s
    where('json_extract(available_exchanges, ?) IS NOT NULL', "$.\"#{exchange_type}\"")
  }

  # Filter to indices available on at least one exchange
  scope :available_on_any_exchange, lambda {
    where("json_extract(available_exchanges, '$') != '{}' AND available_exchanges IS NOT NULL")
  }

  # Returns assets for the top_coins array (for displaying tickers with colors)
  def top_assets
    return [] if top_coins.blank?

    Asset.where(external_id: top_coins)
  end

  # The picker's list: stock (deltabadger-sourced) indices first, then internal Top Coins, then
  # coingecko categories; within each group by weight desc, then market cap. Deltabadger-sourced
  # indices only make sense when the Data API is the provider — the container can't serve them on
  # CoinGecko-direct — so they are hidden otherwise, which also covers a self-hosted provider switch.
  def self.for_picker
    scope = MarketDataSettings.deltabadger? ? all : where.not(source: SOURCE_DELTABADGER)
    scope.order(
      Arel.sql(
        'CASE source ' \
        "WHEN '#{SOURCE_DELTABADGER}' THEN 0 " \
        "WHEN '#{SOURCE_INTERNAL}' THEN 1 " \
        'ELSE 2 END'
      ),
      weight: :desc,
      market_cap: :desc
    )
  end

  # The picker's indices an existing bot could follow without changing venue or spending currency:
  # at least MINIMUM_SUPPORTED_COINS of the members exported for the venue trade there at that quote —
  # the same bar the wizard's quote step sets (Bots::DcaIndex#available_assets_for_current_settings).
  def self.followable_on(exchange, quote_asset_id)
    traded = Asset.where(id: exchange.tickers.available.trading_enabled.where(quote_asset_id:).select(:base_asset_id))
                  .where.not(external_id: nil).pluck(:external_id).to_set
    for_picker.select do |index|
      index.top_coins_for_exchange(exchange.type).count { |coin_id| traded.include?(coin_id) } >= MINIMUM_SUPPORTED_COINS
    end
  end

  # What a DCA index bot stores about the index it follows.
  def bot_settings
    if external_id == TOP_COINS_EXTERNAL_ID
      { 'index_type' => Bots::DcaIndex::INDEX_TYPE_TOP, 'index_category_id' => nil,
        'index_name' => nil, 'index_name_prefix' => nil }
    else
      { 'index_type' => Bots::DcaIndex::INDEX_TYPE_CATEGORY, 'index_category_id' => external_id,
        'index_name' => name,
        'index_name_prefix' => Bots::DcaIndex::COUNT_NAMED_INDICES.dig(external_id, :prefix) }
    end
  end

  # Returns top coins for a specific exchange, falling back to global top_coins
  # @param exchange_type [String] Exchange class name, e.g. "Exchanges::Binance"
  # @return [Array<String>] Array of CoinGecko coin IDs (external_ids)
  def top_coins_for_exchange(exchange_type)
    top_coins_by_exchange&.dig(exchange_type.to_s) || top_coins || []
  end
end
