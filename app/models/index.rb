class Index < ApplicationRecord
  include Index::ExchangeAvailability

  SOURCE_COINGECKO = 'coingecko'.freeze
  SOURCE_INTERNAL = 'internal'.freeze
  TOP_COINS_EXTERNAL_ID = 'top-coins'.freeze

  validates :external_id, presence: true
  validates :source, presence: true
  validates :name, presence: true

  scope :coingecko, -> { where(source: SOURCE_COINGECKO) }
  scope :with_description, -> { where.not(description: [nil, '']) }

  # Filter to indices available on a specific exchange
  scope :available_on_exchange, ->(exchange) {
    exchange_type = exchange.is_a?(Exchange) ? exchange.type : exchange.to_s
    where("json_extract(available_exchanges, ?) IS NOT NULL", "$.\"#{exchange_type}\"")
  }

  # Filter to indices available on at least one exchange
  scope :available_on_any_exchange, -> {
    where("json_extract(available_exchanges, '$') != '{}' AND available_exchanges IS NOT NULL")
  }

  # Returns assets for the top_coins array (for displaying tickers with colors)
  def top_assets
    return [] if top_coins.blank?

    Asset.where(external_id: top_coins)
  end
end
