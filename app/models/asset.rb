class Asset < ApplicationRecord
  has_many :exchange_assets
  has_many :exchanges, through: :exchange_assets

  validates :external_id, presence: true, uniqueness: true
  validate :can_be_destroyed, on: :destroy

  include Undeletable

  # https://docs.coingecko.com/reference/simple-supported-currencies
  VS_CURRENCIES = %w[usd eur jpy gbp cad aud chf btc].freeze
  COINGECKO_BLACKLISTED_IDS = [
    'covalent', # TODO: remove this once covalent is supported in coingecko
    'assister-ai' # TODO: remove this once assister-ai is supported in coingecko
  ].freeze

  def sync_data_with_coingecko
    return Result::Success.new(self) if COINGECKO_BLACKLISTED_IDS.include?(external_id)

    result = coingecko_client.coin_data_by_id(
      id: external_id,
      localization: false,
      tickers: false,
      market_data: true,
      community_data: false,
      developer_data: false,
      sparkline: false
    )
    return Result::Failure.new("Failed to get #{external_id} data from coingecko") if result.failure?

    update!(
      symbol: Utilities::Hash.dig_or_raise(result.data, 'symbol').upcase,
      name: Utilities::Hash.dig_or_raise(result.data, 'name'),
      url: "https://www.coingecko.com/coins/#{Utilities::Hash.dig_or_raise(result.data, 'web_slug')}",
      image_url: Utilities::Hash.dig_or_raise(result.data, 'image', 'large'),
      market_cap_rank: result.data['market_cap_rank']
    )
    Result::Success.new(self)
  end

  def infer_color_from_image
    return if image_url.blank?

    # some images have single quotes in the url that ImageMagick doesn't like
    parsed_image_url = image_url.gsub("'", '%27')

    colors = Utilities::Image.extract_dominant_colors(parsed_image_url)
    update!(color: Utilities::Image.most_vivid_color(colors))
  end

  def get_price(currency: 'usd')
    return Result::Failure.new('Asset is not a cryptocurrency') if category != 'Cryptocurrency'

    currency = currency.downcase
    price = Rails.cache.fetch("asset_price_#{external_id}_#{currency}", expires_in: 20.seconds) do
      result = coingecko_client.coin_price_by_ids(coin_ids: [external_id], vs_currencies: [currency])
      return result if result.failure?

      Utilities::Hash.dig_or_raise(result.data, external_id, currency)
    end
    Result::Success.new(price)
  end

  def get_market_cap(currency: 'usd')
    return Result::Failure.new('Asset is not a cryptocurrency') if category != 'Cryptocurrency'

    currency = currency.downcase
    market_cap = Rails.cache.fetch("asset_market_cap_#{external_id}_#{currency}", expires_in: 6.hours) do
      result = coingecko_client.coin_price_by_ids(coin_ids: [external_id], vs_currencies: [currency], include_market_cap: true)
      return result if result.failure?

      Utilities::Hash.dig_or_raise(result.data, external_id, "#{currency}_market_cap").to_i
    end
    Result::Success.new(market_cap)
  end

  private

  def coingecko_client
    @coingecko_client ||= Clients::Coingecko.new
  end
end
