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
  # coin of that symbol by market rank. Never a stock for a crypto venue — and a stock first for a
  # stock venue, which trades coins as well.
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

      # What the venue lists, then the catalogue by rank.
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

      private

      def venue_fits?(entry, exchange)
        entry.exchange.nil? || entry.exchange == exchange&.name_id
      end

      def holds_on?(entry, at)
        entry.until.nil? || (at && at.to_date <= entry.until)
      end

      def listed(symbol, exchange)
        asset = exchange&.tickers&.find_by(base: symbol)&.base_asset
        asset if asset && (exchange.stock_venue? || asset.category == CRYPTO)
      end

      def catalogued(symbol, exchange)
        assets = Asset.where(symbol: symbol).order(Arel.sql('market_cap_rank IS NULL, market_cap_rank'), :id).to_a
        stock = assets.find { |asset| asset.category == STOCK } if exchange&.stock_venue?
        stock || assets.find { |asset| asset.category == CRYPTO }
      end
    end
  end
end
