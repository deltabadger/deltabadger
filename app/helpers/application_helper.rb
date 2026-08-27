module ApplicationHelper
  EXCHANGE_SVGS = Hash.new do |hash, name_id|
    path = Rails.root.join("app/views/svg/_exchange-#{name_id}.html.erb")
    hash[name_id] = File.read(path).html_safe.freeze
  end

  def exchange_icon_svg(exchange_name_id)
    EXCHANGE_SVGS[exchange_name_id]
  end

  # The dash a cell shows when there is nothing to show. It is the absence of a figure, not a
  # figure, so it never carries the ink of the numbers beside it — one helper so every table
  # reads the same, and so the colour is decided once.
  def no_value
    tag.span('—', class: 'no-value')
  end

  # One date shape for every table on the app: year first, in the reader's own zone. A reader
  # moving between the bot log and the tracker is not re-learning the format on the way.
  #
  # The zone is passed in rather than read off `current_user`: these rows are also rendered from a
  # model broadcast, where there is no request and the user arrives as a partial LOCAL — which a
  # helper method cannot see. Making it an argument is what keeps one formatter for both paths.
  def table_date(time, zone)
    time.in_time_zone(zone).strftime('%Y/%m/%d')
  end

  # When a row happened: the date, with the clock time behind it in <small>. The date is what a
  # row is found by and the time is the detail, so they are not set at the same weight.
  def table_when(time, zone)
    safe_join([table_date(time, zone), ' ', tag.small(time.in_time_zone(zone).strftime('%H:%M'))])
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
