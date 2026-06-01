class AssetsController < ApplicationController
  before_action :authenticate_user!

  # Hover-card for any .ticker pill. A pill that renders a KNOWN asset carries its `asset_id`, so
  # we resolve that exact row — symbols collide across categories (e.g. the stock XYZ / Block Inc
  # vs the crypto "Xyzverse" XYZ), so a bare symbol can't be disambiguated. A present-but-invalid
  # `asset_id` 404s rather than falling back, so a failed id lookup never serves the wrong asset.
  # Context-free pills (bare text) still resolve by symbol, best match by market cap.
  def tooltip
    asset =
      if (id = params[:asset_id]).present?
        Asset.find_by(id: id) if id.match?(/\A\d+\z/)
      elsif (symbol = params[:symbol].to_s.strip).present?
        Asset.where('UPPER(symbol) = ?', symbol.upcase)
             .order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank, :id)
             .first
      end
    return head :not_found unless asset

    render partial: 'assets/tooltip_card', layout: false, locals: { asset: asset }
  end
end
