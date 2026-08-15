module Tax
  # Which report a ticker belongs to. Getting it wrong drops a security from the broker report
  # entirely — no summary, no worksheet, no symbol row — so it is the one decision the broker
  # report's refusal-first design cannot make safe. `symbol` is NOT unique on
  # assets (only `external_id` is), and a user with any crypto exchange connected carries thousands
  # of CoinGecko rows, so a stock ticker colliding with a coin would silently understate a signed
  # form. Hence three ways out before excluding anything. Both scopes ask this one predicate, so no
  # security is crypto to one report and not the other. The job asks it about `base_currency`, while
  # the broker report asks it about the resolved instrument symbol, so a dividend-family row booked
  # against a crypto holding would be claimed by neither, which no Alpaca activity type produces today.
  class CryptoScope
    def initialize(user:)
      @user = user
      @asset_cache = {}
    end

    # Yields the ambiguous category list (once per call) when the ticker resolves to more than one.
    def crypto?(symbol)
      return false if symbol.blank?

      assets = assets_for(symbol)
      return false if assets.empty?

      categories = assets.filter_map(&:category).uniq.sort
      # One ticker resolving to two asset categories is worth the user's eye even when the tie-breaks
      # below settle it, so warn before them, not after — a mis-resolved ticker is not a small error.
      yield categories if block_given? && categories.size > 1

      # The user telling us this is a share or a fund outranks any catalogue guess.
      return false if classified_symbols.include?(symbol)
      # A stock or ETF row for the same ticker means the crypto row is the collision, not the holding.
      return false if assets.any? { |asset| asset.instrument_type.in?(%w[stock etf]) }

      categories == ['Cryptocurrency']
    end

    def assets_for(symbol)
      @asset_cache[symbol] ||= Asset.where(symbol: symbol).to_a
    end

    private

    def classified_symbols
      @classified_symbols ||= @user.fund_classifications.pluck(:symbol).to_set
    end
  end
end
