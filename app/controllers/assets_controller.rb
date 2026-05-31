class AssetsController < ApplicationController
  before_action :authenticate_user!

  # Hover-card for any .ticker pill. The pill only carries its symbol, so we resolve that to
  # an asset (best match by market cap) and render the card.
  def tooltip
    symbol = params[:symbol].to_s.strip
    return head :not_found if symbol.blank?

    asset = Asset.where('UPPER(symbol) = ?', symbol.upcase)
                 .order(Arel.sql('market_cap_rank IS NULL'), :market_cap_rank, :id)
                 .first
    return head :not_found unless asset

    render partial: 'assets/tooltip_card', layout: false, locals: { asset: asset }
  end
end
