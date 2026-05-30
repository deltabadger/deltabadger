class AssetsController < ApplicationController
  before_action :authenticate_user!

  # Hover-card for any .ticker pill. The pill only carries its symbol, so we resolve that to
  # an asset (best match by market cap) and render the card. Inert unless the data API is
  # connected; fail-soft on price — a non-crypto asset or a flaky upstream yields no price,
  # never a 500.
  def tooltip
    return head :not_found unless ticker_tooltips_enabled?

    symbol = params[:symbol].to_s.strip
    return head :not_found if symbol.blank?

    asset = Asset.where('UPPER(symbol) = ?', symbol.upcase)
                 .order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank, :id)
                 .first
    return head :not_found unless asset

    render partial: 'assets/tooltip_card', layout: false,
           locals: { asset: asset, price: fetch_price(asset, 'usd') }
  end

  private

  # Returns the formatted price string, or nil when unavailable. Rescues only the price
  # lookup so unrelated bugs still surface; unexpected errors are logged, not swallowed.
  def fetch_price(asset, currency)
    result = asset.get_price(currency: currency)
    return nil if result.failure?

    format_price(result.data, currency)
  rescue Client::TransientNetworkError => e
    Rails.logger.info("[ticker tooltip] transient price failure for asset #{asset.id}: #{e.message}")
    nil
  rescue StandardError => e
    Rails.logger.error("[ticker tooltip] unexpected price failure for asset #{asset.id}: #{e.class}: #{e.message}")
    nil
  end

  CURRENCY_SYMBOLS = {
    'usd' => '$', 'eur' => '€', 'gbp' => '£', 'jpy' => '¥',
    'cad' => 'CA$', 'aud' => 'A$', 'chf' => 'CHF ', 'btc' => '₿'
  }.freeze

  def format_price(value, currency)
    return nil unless value.is_a?(Numeric)

    "#{CURRENCY_SYMBOLS[currency]}#{helpers.number_with_delimiter(helpers.format_value(value))}"
  end
end
