module ApplicationHelper
  EXCHANGE_SVGS = Hash.new do |hash, name_id|
    path = Rails.root.join("app/views/svg/_exchange-#{name_id}.html.erb")
    hash[name_id] = File.read(path).html_safe.freeze
  end

  def exchange_icon_svg(exchange_name_id)
    EXCHANGE_SVGS[exchange_name_id]
  end

  def ticker_class(asset)
    ticker_class_for(category: asset&.category, color: asset&.color)
  end

  # Data attributes for a .ticker pill that renders a KNOWN asset, so the hover-card resolves to
  # that exact row instead of guessing by symbol (symbols collide across categories — e.g. the
  # stock XYZ vs the crypto "Xyzverse" XYZ). Merge into the pill's existing `data:` hash. Emits
  # BOTH keys on purpose: the explicit symbol keeps ticker_tooltips_controller's eligibility/
  # resolution robust even when the pill carries no text node (e.g. the readonly-input wizard chip).
  def ticker_data(asset)
    ticker_data_for(id: asset&.id, symbol: asset&.symbol)
  end

  # Same, for pills rendered from primitives (search rows, index preview) where no Asset object
  # is in scope — only its id/symbol.
  def ticker_data_for(id:, symbol:)
    { ticker_symbol: symbol, ticker_asset_id: id }.compact
  end

  # A stock with a real (data-API) color renders like any colored ticker; only a colorless
  # stock keeps the distinct open-source fallback styling (.ticker--stock). Takes the RAW
  # persisted color (nullable) — never a fallback — so "has a real color" stays distinguishable.
  def ticker_class_for(category:, color:)
    return 'ticker' if color.present?

    category == 'Stock' ? 'ticker ticker--stock' : 'ticker'
  end

  # Friendly asset-type label for the tooltip info line. Returns nil for unknown/blank
  # categories so the info line is omitted rather than mislabeled. Single swappable field:
  # a real per-asset description can replace this later without touching the frontend.
  TICKER_TYPE_LABELS = {
    'Cryptocurrency' => 'Crypto',
    'Stock' => 'Stock',
    'Common Stock' => 'Stock',
    'ETF' => 'ETF',
    'Fund' => 'Fund',
    'Fiat' => 'Cash',
    'Currency' => 'Cash'
  }.freeze

  def asset_type_label(category)
    TICKER_TYPE_LABELS[category.to_s]
  end
end
