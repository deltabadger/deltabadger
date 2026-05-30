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

  # A stock with a real (data-API) color renders like any colored ticker; only a colorless
  # stock keeps the distinct open-source fallback styling (.ticker--stock). Takes the RAW
  # persisted color (nullable) — never a fallback — so "has a real color" stays distinguishable.
  def ticker_class_for(category:, color:)
    return 'ticker' if color.present?

    category == 'Stock' ? 'ticker ticker--stock' : 'ticker'
  end
end
