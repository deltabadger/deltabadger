module Tax
  # Which coin a symbol means — for pricing, per venue and per day.
  #
  # `assets.symbol` is not unique, a venue's ticker is not a coin id, and a symbol can change coin
  # over time: Terra's LUNA before May 2022 is today's LUNC, and MATIC's history lives under the
  # coin that became POL. The catalogue knows none of this, and `Asset.find_by(symbol:)` answered
  # with whichever row came first — a stock for DAR, the wrong LIT, the new Terra for the old.
  #
  # So, in order: an alias, dated where the symbol changed hands and scoped where a venue's listing
  # differs from the catalogue's; what the VENUE lists under that symbol, which is its own fact; the
  # coin of that symbol by market rank. Never a security: which of the two a broker's name is, is
  # `identity`'s question.
  #
  # ponytail: prices are still stored by SYMBOL (`historical_prices.asset`), so two coins sharing a
  # ticker on two venues at the same time would share one price row. A dated alias never overlaps
  # itself. Keying the table by coin id is the upgrade path.
  module AssetIdentity
    Alias = Data.define(:symbol, :coin, :exchange, :until)

    # Verified against the provider's own coin records. `until` is the last day the alias holds;
    # `exchange` a venue's `name_id` where only that venue's listing means this coin.
    ALIASES = [
      # Terra collapsed in May 2022: the chain relaunched as `terra-luna-2`, the original became LUNC.
      Alias.new(symbol: 'LUNA', coin: 'terra-luna', exchange: nil, until: Date.new(2022, 5, 27)),
      # QuickSwap redenominated QUICK 1 old = 1000 new: `quick` is the old token, `quickswap` the new.
      # No venue carried the new one before 2023-07-21, when Binance reopened in new units. Coinbase
      # kept trading the old token afterwards, but a venue-scoped alias past this date would give
      # QUICK two coins on one day, and prices are stored by symbol and day (see above).
      Alias.new(symbol: 'QUICK', coin: 'quick', exchange: nil, until: Date.new(2023, 7, 20)),
      # MATIC migrated to POL; its whole history stays under the original id.
      Alias.new(symbol: 'MATIC', coin: 'matic-network', exchange: nil, until: nil),
      # Listed on Binance under a symbol the catalogue no longer carries, or carries as another coin.
      Alias.new(symbol: 'LIT', coin: 'litentry', exchange: 'binance', until: nil),
      Alias.new(symbol: 'GAL', coin: 'project-galaxy', exchange: 'binance', until: nil),
      Alias.new(symbol: 'DAR', coin: 'mines-of-dalarnia', exchange: 'binance', until: nil),
      Alias.new(symbol: 'PNT', coin: 'pnetwork', exchange: nil, until: nil),
      Alias.new(symbol: 'BURGER', coin: 'burger-swap', exchange: nil, until: nil)
    ].freeze

    STOCK = 'Stock'.freeze
    CRYPTO = 'Cryptocurrency'.freeze
    # The categories a broker's securities are filed under.
    STOCK_CATEGORIES = ['Stock', 'Common Stock', 'ETF', 'Fund'].freeze

    class << self
      def coin_id(symbol, exchange: nil, at: nil)
        alias_coin(symbol, exchange: exchange, at: at) || catalogue_coin(symbol, exchange: exchange)
      end

      # The dated, venue-scoped answer; nil where no alias speaks. Cheap — no query — so a caller
      # may ask per row and cache only `catalogue_coin`.
      def alias_coin(symbol, exchange: nil, at: nil)
        return if symbol.blank?

        ALIASES.find { |entry| entry.symbol == symbol && venue_fits?(entry, exchange) && holds_on?(entry, at) }&.coin
      end

      # What the venue lists, then the catalogue by rank. Always a coin, never a security: a broker can
      # list a security under the coin's ticker (Alpaca's BTC beside BTC/USD).
      def catalogue_coin(symbol, exchange: nil)
        return if symbol.blank?

        (listed(symbol, exchange) || catalogued(symbol, exchange))&.external_id
      end

      # The coin ids a symbol means over a span of days, as [range, coin id] pairs: one pair unless
      # a dated alias splits the span.
      def coin_ids_over(symbol, exchange:, from:, to:)
        cuts = ALIASES.select { |entry| entry.symbol == symbol && entry.until && venue_fits?(entry, exchange) }
                      .map(&:until).select { |cut| cut >= from && cut < to }.sort
        starts = [from] + cuts.map { |cut| cut + 1 }
        ends = cuts + [to]
        starts.zip(ends).filter_map do |first, last|
          coin = coin_id(symbol, exchange: exchange, at: first)
          [first..last, coin] if coin
        end
      end

      # What a symbol is at a venue: [:stock, the security], [:coin, the one coin named, if any], or nil
      # for a name that is still both. On a crypto venue a symbol is a coin. A broker can list a
      # security and a coin under one ticker, so there the assets recorded under the name decide
      # (`recorded`: the row's own, or what the venue's rows recorded); with none, what the venue
      # lists under it, then the catalogue.
      def identity(symbol, exchange:, recorded: [])
        return [:coin] unless exchange&.stock_venue?

        candidates = recorded.compact.presence ||
                     exchange.tickers.where(base: symbol).order(:id).includes(:base_asset).filter_map(&:base_asset).presence ||
                     Asset.where(symbol: symbol).order(Arel.sql('market_cap_rank IS NULL, market_cap_rank'), :id).to_a
        stocks, coins = candidates.uniq.partition { |asset| STOCK_CATEGORIES.include?(asset.category) }
        return [:coin, (coins.first if coins.one?)] if stocks.empty?

        [:stock, stocks.first] if coins.empty?
      end

      # The `historical_prices` key a symbol's prices are stored under at a venue on a day, and the
      # security when it is one; nil for a name that is still both. A coin's prices are its symbol's.
      # A security's are too, unless a coin has that symbol anywhere — in the catalogue or in what
      # `recorded_coins` holds — when they are `stock:SYM`: the bare key is every venue's, and a coin
      # elsewhere must never read a security's closes, nor a security a coin's.
      def price_key(symbol, exchange:, at: nil, recorded: [], recorded_coins: false)
        return [symbol] if alias_coin(symbol, exchange: exchange, at: at)

        kind, asset = identity(symbol, exchange: exchange, recorded: recorded)
        return unless kind
        return [symbol] if kind == :coin

        shared = recorded_coins || Asset.exists?(symbol: symbol, category: CRYPTO)
        [shared ? "stock:#{symbol}" : symbol, asset]
      end

      # Whether a dated alias cuts this symbol's history on this venue.
      def dated_alias?(symbol, exchange: nil)
        ALIASES.any? { |entry| entry.symbol == symbol && entry.until && venue_fits?(entry, exchange) }
      end

      private

      def venue_fits?(entry, exchange)
        entry.exchange.nil? || entry.exchange == exchange&.name_id
      end

      def holds_on?(entry, at)
        entry.until.nil? || (at && at.to_date <= entry.until)
      end

      def listed(symbol, exchange)
        exchange&.tickers&.joins(:base_asset)&.where(base: symbol, assets: { category: CRYPTO })&.order(:id)&.first&.base_asset
      end

      def catalogued(symbol, _exchange)
        Asset.where(symbol: symbol, category: CRYPTO).order(Arel.sql('market_cap_rank IS NULL, market_cap_rank'), :id).first
      end
    end
  end
end
